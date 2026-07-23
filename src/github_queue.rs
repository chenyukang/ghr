use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use http::HeaderMap;
use tokio::sync::Notify;
use tokio::time::sleep;
use tracing::warn;

const RATE_LIMIT_FALLBACK_DELAY: Duration = Duration::from_secs(60);
const RATE_LIMIT_RESET_GRACE: Duration = Duration::from_secs(1);
const MAX_USER_REQUESTS_IN_FLIGHT: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GitHubQueueBackend {
    DirectApi,
    GitHubCli,
}

impl GitHubQueueBackend {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::DirectApi => "direct API",
            Self::GitHubCli => "gh",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GitHubRequestPriority {
    User,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GitHubRateResource {
    Core,
    Search,
    Graphql,
}

impl GitHubRateResource {
    const COUNT: usize = 3;

    fn index(self) -> usize {
        match self {
            Self::Core => 0,
            Self::Search => 1,
            Self::Graphql => 2,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Search => "search",
            Self::Graphql => "graphql",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitHubQueueResourceSnapshot {
    pub(crate) resource: GitHubRateResource,
    pub(crate) user_waiting: usize,
    pub(crate) background_waiting: usize,
    pub(crate) cooldown_remaining: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitHubQueueSnapshot {
    pub(crate) backend: GitHubQueueBackend,
    pub(crate) active: usize,
    pub(crate) max_active: usize,
    pub(crate) resources: Vec<GitHubQueueResourceSnapshot>,
}

static DIRECT_API_QUEUE: OnceLock<RequestQueue> = OnceLock::new();
static GH_CLI_QUEUE: OnceLock<RequestQueue> = OnceLock::new();

pub(crate) async fn acquire(
    backend: GitHubQueueBackend,
    priority: GitHubRequestPriority,
    resource: GitHubRateResource,
) -> RequestPermit<'static> {
    queue(backend).acquire(priority, resource).await
}

pub(crate) fn snapshot(backend: GitHubQueueBackend) -> GitHubQueueSnapshot {
    queue(backend).snapshot(backend)
}

pub(crate) fn observe_response(
    backend: GitHubQueueBackend,
    request_resource: GitHubRateResource,
    status: u16,
    headers: &HeaderMap,
    message: Option<&str>,
) -> Option<Duration> {
    let delay = rate_limit_delay(status, headers, message)?;
    let resource = response_rate_resource(headers, request_resource);
    queue(backend).defer_for(resource, delay);
    warn!(
        backend = backend.label(),
        status,
        resource = resource.label(),
        retry_in_seconds = delay.as_secs(),
        "GitHub request queue cooling down"
    );
    Some(delay)
}

pub(crate) fn message_looks_rate_limited(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("api rate limit exceeded")
        || message.contains("rate limit exceeded")
        || message.contains("secondary rate limit")
        || message.contains("abuse detection")
        || message.contains("http 429")
}

fn queue(backend: GitHubQueueBackend) -> &'static RequestQueue {
    match backend {
        GitHubQueueBackend::DirectApi => DIRECT_API_QUEUE.get_or_init(RequestQueue::default),
        GitHubQueueBackend::GitHubCli => GH_CLI_QUEUE.get_or_init(RequestQueue::default),
    }
}

#[derive(Default)]
struct RequestQueue {
    state: Mutex<RequestQueueState>,
    changed: Notify,
}

#[derive(Default)]
struct RequestQueueState {
    active: usize,
    next_ticket: u64,
    user_waiters: [VecDeque<u64>; GitHubRateResource::COUNT],
    background_waiters: [VecDeque<u64>; GitHubRateResource::COUNT],
    cooldown_until: [Option<Instant>; GitHubRateResource::COUNT],
}

enum AcquireState {
    Ready,
    Waiting,
    CoolingDown(Duration),
}

impl RequestQueue {
    async fn acquire(
        &self,
        priority: GitHubRequestPriority,
        resource: GitHubRateResource,
    ) -> RequestPermit<'_> {
        let ticket = {
            let mut state = self.lock_state();
            let ticket = state.next_ticket;
            state.next_ticket = state.next_ticket.wrapping_add(1);
            state.waiters_mut(priority, resource).push_back(ticket);
            ticket
        };
        let mut pending = PendingRequest {
            queue: self,
            priority,
            resource,
            ticket,
            queued: true,
        };

        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            match self.try_acquire(priority, resource, ticket) {
                AcquireState::Ready => {
                    pending.queued = false;
                    return RequestPermit { queue: self };
                }
                AcquireState::Waiting => changed.await,
                AcquireState::CoolingDown(delay) => {
                    tokio::select! {
                        _ = changed => {}
                        _ = sleep(delay) => {}
                    }
                }
            }
        }
    }

    fn try_acquire(
        &self,
        priority: GitHubRequestPriority,
        resource: GitHubRateResource,
        ticket: u64,
    ) -> AcquireState {
        let mut state = self.lock_state();
        let now = Instant::now();
        state.clear_expired_cooldowns(now);
        if let Some(until) = state.cooldown_until[resource.index()] {
            return AcquireState::CoolingDown(until.duration_since(now));
        }

        let is_next = match priority {
            GitHubRequestPriority::User => {
                state.active < MAX_USER_REQUESTS_IN_FLIGHT
                    && state.next_ready_ticket(GitHubRequestPriority::User) == Some(ticket)
            }
            GitHubRequestPriority::Background => {
                state.active == 0
                    && state
                        .next_ready_ticket(GitHubRequestPriority::User)
                        .is_none()
                    && state.next_ready_ticket(GitHubRequestPriority::Background) == Some(ticket)
            }
        };
        if !is_next {
            return AcquireState::Waiting;
        }

        state.waiters_mut(priority, resource).pop_front();
        state.active += 1;
        AcquireState::Ready
    }

    fn defer_for(&self, resource: GitHubRateResource, delay: Duration) {
        let until = Instant::now() + delay;
        let mut state = self.lock_state();
        let cooldown = &mut state.cooldown_until[resource.index()];
        if cooldown.is_none_or(|current| until > current) {
            *cooldown = Some(until);
        }
        drop(state);
        self.changed.notify_waiters();
    }

    fn snapshot(&self, backend: GitHubQueueBackend) -> GitHubQueueSnapshot {
        let mut state = self.lock_state();
        let now = Instant::now();
        state.clear_expired_cooldowns(now);
        let resources = [
            GitHubRateResource::Core,
            GitHubRateResource::Search,
            GitHubRateResource::Graphql,
        ]
        .into_iter()
        .map(|resource| GitHubQueueResourceSnapshot {
            resource,
            user_waiting: state.user_waiters[resource.index()].len(),
            background_waiting: state.background_waiters[resource.index()].len(),
            cooldown_remaining: state.cooldown_until[resource.index()]
                .map(|until| until.duration_since(now)),
        })
        .collect();
        GitHubQueueSnapshot {
            backend,
            active: state.active,
            max_active: MAX_USER_REQUESTS_IN_FLIGHT,
            resources,
        }
    }

    fn remove_waiter(
        &self,
        priority: GitHubRequestPriority,
        resource: GitHubRateResource,
        ticket: u64,
    ) {
        let mut state = self.lock_state();
        let waiters = state.waiters_mut(priority, resource);
        if let Some(position) = waiters.iter().position(|queued| *queued == ticket) {
            waiters.remove(position);
        }
        drop(state);
        self.changed.notify_waiters();
    }

    fn release(&self) {
        let mut state = self.lock_state();
        state.active = state.active.saturating_sub(1);
        drop(state);
        self.changed.notify_waiters();
    }

    fn lock_state(&self) -> MutexGuard<'_, RequestQueueState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl RequestQueueState {
    fn waiters_mut(
        &mut self,
        priority: GitHubRequestPriority,
        resource: GitHubRateResource,
    ) -> &mut VecDeque<u64> {
        match priority {
            GitHubRequestPriority::User => &mut self.user_waiters[resource.index()],
            GitHubRequestPriority::Background => &mut self.background_waiters[resource.index()],
        }
    }

    fn next_ready_ticket(&self, priority: GitHubRequestPriority) -> Option<u64> {
        let waiters = match priority {
            GitHubRequestPriority::User => &self.user_waiters,
            GitHubRequestPriority::Background => &self.background_waiters,
        };
        waiters
            .iter()
            .enumerate()
            .filter(|(index, _)| self.cooldown_until[*index].is_none())
            .filter_map(|(_, queue)| queue.front().copied())
            .min()
    }

    fn clear_expired_cooldowns(&mut self, now: Instant) {
        for cooldown in &mut self.cooldown_until {
            if cooldown.is_some_and(|until| until <= now) {
                *cooldown = None;
            }
        }
    }
}

struct PendingRequest<'a> {
    queue: &'a RequestQueue,
    priority: GitHubRequestPriority,
    resource: GitHubRateResource,
    ticket: u64,
    queued: bool,
}

impl Drop for PendingRequest<'_> {
    fn drop(&mut self) {
        if self.queued {
            self.queue
                .remove_waiter(self.priority, self.resource, self.ticket);
        }
    }
}

pub(crate) struct RequestPermit<'a> {
    queue: &'a RequestQueue,
}

impl Drop for RequestPermit<'_> {
    fn drop(&mut self) {
        self.queue.release();
    }
}

fn rate_limit_delay(status: u16, headers: &HeaderMap, message: Option<&str>) -> Option<Duration> {
    if let Some(seconds) = header_u64(headers, "retry-after") {
        return Some(Duration::from_secs(seconds.max(1)));
    }

    let remaining = header_u64(headers, "x-ratelimit-remaining");
    if remaining == Some(0) {
        return Some(
            header_u64(headers, "x-ratelimit-reset")
                .map(delay_until_epoch)
                .unwrap_or(RATE_LIMIT_FALLBACK_DELAY),
        );
    }

    if status == 429 || message.is_some_and(message_looks_rate_limited) {
        return Some(RATE_LIMIT_FALLBACK_DELAY);
    }

    None
}

fn delay_until_epoch(reset_epoch: u64) -> Duration {
    let now_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Duration::from_secs(reset_epoch.saturating_sub(now_epoch)) + RATE_LIMIT_RESET_GRACE
}

fn header_u64(headers: &HeaderMap, name: &str) -> Option<u64> {
    header_value(headers, name)?.parse().ok()
}

fn header_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

fn response_rate_resource(
    headers: &HeaderMap,
    request_resource: GitHubRateResource,
) -> GitHubRateResource {
    match header_value(headers, "x-ratelimit-resource") {
        Some("search" | "code_search") => GitHubRateResource::Search,
        Some("graphql") => GitHubRateResource::Graphql,
        Some(_) => GitHubRateResource::Core,
        None => request_resource,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::mpsc;
    use tokio::time::{Duration, timeout};

    use super::*;

    const CORE: GitHubRateResource = GitHubRateResource::Core;

    #[tokio::test]
    async fn background_queue_runs_one_request_at_a_time() {
        let queue = Arc::new(RequestQueue::default());
        let first = queue.acquire(GitHubRequestPriority::Background, CORE).await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let waiting_queue = queue.clone();
        let task = tokio::spawn(async move {
            let _permit = waiting_queue
                .acquire(GitHubRequestPriority::Background, CORE)
                .await;
            let _ = tx.send(());
        });

        assert!(timeout(Duration::from_millis(20), rx.recv()).await.is_err());
        drop(first);
        timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("second request should start")
            .expect("second request should report start");
        task.await.expect("queued task should finish");
    }

    #[tokio::test]
    async fn queue_snapshot_reports_active_waiters_and_cooldown() {
        let queue = Arc::new(RequestQueue::default());
        let permit = queue.acquire(GitHubRequestPriority::User, CORE).await;
        queue.defer_for(GitHubRateResource::Search, Duration::from_secs(30));
        let waiting_queue = queue.clone();
        let waiter = tokio::spawn(async move {
            let _permit = waiting_queue
                .acquire(
                    GitHubRequestPriority::Background,
                    GitHubRateResource::Graphql,
                )
                .await;
        });

        while queue.lock_state().background_waiters[GitHubRateResource::Graphql.index()].is_empty()
        {
            tokio::task::yield_now().await;
        }
        let snapshot = queue.snapshot(GitHubQueueBackend::DirectApi);
        assert_eq!(snapshot.active, 1);
        assert_eq!(snapshot.max_active, MAX_USER_REQUESTS_IN_FLIGHT);
        let search = &snapshot.resources[GitHubRateResource::Search.index()];
        assert!(search.cooldown_remaining.is_some());
        let graphql = &snapshot.resources[GitHubRateResource::Graphql.index()];
        assert_eq!(graphql.background_waiting, 1);

        waiter.abort();
        drop(permit);
    }

    #[tokio::test]
    async fn user_request_overtakes_waiting_background_request() {
        let queue = Arc::new(RequestQueue::default());
        let mut active = Vec::new();
        for _ in 0..MAX_USER_REQUESTS_IN_FLIGHT {
            active.push(queue.acquire(GitHubRequestPriority::User, CORE).await);
        }
        let (tx, mut rx) = mpsc::unbounded_channel();

        let background_queue = queue.clone();
        let background_tx = tx.clone();
        let background = tokio::spawn(async move {
            let _permit = background_queue
                .acquire(GitHubRequestPriority::Background, CORE)
                .await;
            let _ = background_tx.send("background");
        });
        while queue.lock_state().background_waiters[CORE.index()].is_empty() {
            tokio::task::yield_now().await;
        }

        let user_queue = queue.clone();
        let user = tokio::spawn(async move {
            let _permit = user_queue.acquire(GitHubRequestPriority::User, CORE).await;
            let _ = tx.send("user");
        });
        while queue.lock_state().user_waiters[CORE.index()].is_empty() {
            tokio::task::yield_now().await;
        }

        drop(active.pop());
        assert_eq!(rx.recv().await, Some("user"));
        drop(active);
        assert_eq!(rx.recv().await, Some("background"));
        user.await.expect("user task should finish");
        background.await.expect("background task should finish");
    }

    #[tokio::test]
    async fn cancelled_waiter_does_not_block_the_queue() {
        let queue = Arc::new(RequestQueue::default());
        let mut active = Vec::new();
        for _ in 0..MAX_USER_REQUESTS_IN_FLIGHT {
            active.push(queue.acquire(GitHubRequestPriority::User, CORE).await);
        }
        let waiting_queue = queue.clone();
        let waiting = tokio::spawn(async move {
            let _permit = waiting_queue
                .acquire(GitHubRequestPriority::User, CORE)
                .await;
        });
        while queue.lock_state().user_waiters[CORE.index()].is_empty() {
            tokio::task::yield_now().await;
        }

        waiting.abort();
        let _ = waiting.await;
        drop(active);
        timeout(
            Duration::from_secs(1),
            queue.acquire(GitHubRequestPriority::Background, CORE),
        )
        .await
        .expect("cancelled waiter should be removed");
    }

    #[tokio::test]
    async fn user_requests_use_bounded_concurrency() {
        let queue = Arc::new(RequestQueue::default());
        let mut active = Vec::new();
        let resources = [
            GitHubRateResource::Core,
            GitHubRateResource::Search,
            GitHubRateResource::Graphql,
        ];
        for index in 0..MAX_USER_REQUESTS_IN_FLIGHT {
            active.push(
                queue
                    .acquire(
                        GitHubRequestPriority::User,
                        resources[index % resources.len()],
                    )
                    .await,
            );
        }
        let (tx, mut rx) = mpsc::unbounded_channel();
        let waiting_queue = queue.clone();
        let task = tokio::spawn(async move {
            let _permit = waiting_queue
                .acquire(GitHubRequestPriority::User, CORE)
                .await;
            let _ = tx.send(());
        });

        assert!(timeout(Duration::from_millis(20), rx.recv()).await.is_err());
        drop(active.pop());
        timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("request should start when capacity is available")
            .expect("request should report start");
        drop(active);
        task.await.expect("queued task should finish");
    }

    #[tokio::test]
    async fn cooldown_only_blocks_matching_resource() {
        let queue = Arc::new(RequestQueue::default());
        queue.defer_for(GitHubRateResource::Search, Duration::from_millis(100));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let search_queue = queue.clone();
        let search = tokio::spawn(async move {
            let _permit = search_queue
                .acquire(GitHubRequestPriority::User, GitHubRateResource::Search)
                .await;
            let _ = tx.send(());
        });
        while queue.lock_state().user_waiters[GitHubRateResource::Search.index()].is_empty() {
            tokio::task::yield_now().await;
        }

        let core = timeout(
            Duration::from_secs(1),
            queue.acquire(GitHubRequestPriority::User, GitHubRateResource::Core),
        )
        .await
        .expect("search cooldown should not block core requests");
        let graphql = timeout(
            Duration::from_secs(1),
            queue.acquire(GitHubRequestPriority::User, GitHubRateResource::Graphql),
        )
        .await
        .expect("search cooldown should not block GraphQL requests");
        assert!(timeout(Duration::from_millis(20), rx.recv()).await.is_err());

        drop(core);
        drop(graphql);
        timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("search request should resume after its cooldown")
            .expect("search request should report start");
        search.await.expect("search task should finish");
    }

    #[test]
    fn retry_after_header_controls_cooldown() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", "12".parse().unwrap());

        assert_eq!(
            rate_limit_delay(403, &headers, Some("secondary rate limit")),
            Some(Duration::from_secs(12))
        );
    }

    #[test]
    fn exhausted_primary_limit_waits_until_reset() {
        let reset = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 5;
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-remaining", "0".parse().unwrap());
        headers.insert("x-ratelimit-reset", reset.to_string().parse().unwrap());

        let delay = rate_limit_delay(200, &headers, None).expect("cooldown should be set");
        assert!(delay >= Duration::from_secs(5));
        assert!(delay <= Duration::from_secs(6));
    }

    #[test]
    fn permission_error_without_rate_limit_headers_does_not_cool_down() {
        assert_eq!(
            rate_limit_delay(403, &HeaderMap::new(), Some("Resource not accessible")),
            None
        );
    }

    #[test]
    fn response_headers_select_the_rate_limit_resource() {
        for (header, expected) in [
            ("search", GitHubRateResource::Search),
            ("code_search", GitHubRateResource::Search),
            ("graphql", GitHubRateResource::Graphql),
            ("core", GitHubRateResource::Core),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert("x-ratelimit-resource", header.parse().unwrap());
            assert_eq!(
                response_rate_resource(&headers, GitHubRateResource::Core),
                expected
            );
        }

        assert_eq!(
            response_rate_resource(&HeaderMap::new(), GitHubRateResource::Search),
            GitHubRateResource::Search
        );
    }
}
