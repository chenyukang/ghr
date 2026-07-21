use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use http::HeaderMap;
use tokio::sync::Notify;
use tokio::time::sleep;
use tracing::warn;

const RATE_LIMIT_FALLBACK_DELAY: Duration = Duration::from_secs(60);
const RATE_LIMIT_RESET_GRACE: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GitHubQueueBackend {
    DirectApi,
    GitHubCli,
}

impl GitHubQueueBackend {
    fn label(self) -> &'static str {
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

static DIRECT_API_QUEUE: OnceLock<RequestQueue> = OnceLock::new();
static GH_CLI_QUEUE: OnceLock<RequestQueue> = OnceLock::new();

pub(crate) async fn acquire(
    backend: GitHubQueueBackend,
    priority: GitHubRequestPriority,
) -> RequestPermit<'static> {
    queue(backend).acquire(priority).await
}

pub(crate) fn observe_response(
    backend: GitHubQueueBackend,
    status: u16,
    headers: &HeaderMap,
    message: Option<&str>,
) -> Option<Duration> {
    let delay = rate_limit_delay(status, headers, message)?;
    queue(backend).defer_for(delay);
    warn!(
        backend = backend.label(),
        status,
        resource = header_value(headers, "x-ratelimit-resource").unwrap_or("unknown"),
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
    active: bool,
    next_ticket: u64,
    user_waiters: VecDeque<u64>,
    background_waiters: VecDeque<u64>,
    cooldown_until: Option<Instant>,
}

enum AcquireState {
    Ready,
    Waiting,
    CoolingDown(Duration),
}

impl RequestQueue {
    async fn acquire(&self, priority: GitHubRequestPriority) -> RequestPermit<'_> {
        let ticket = {
            let mut state = self.lock_state();
            let ticket = state.next_ticket;
            state.next_ticket = state.next_ticket.wrapping_add(1);
            state.waiters_mut(priority).push_back(ticket);
            ticket
        };
        let mut pending = PendingRequest {
            queue: self,
            priority,
            ticket,
            queued: true,
        };

        loop {
            let changed = self.changed.notified();
            match self.try_acquire(priority, ticket) {
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

    fn try_acquire(&self, priority: GitHubRequestPriority, ticket: u64) -> AcquireState {
        let mut state = self.lock_state();
        if state.active {
            return AcquireState::Waiting;
        }

        if let Some(until) = state.cooldown_until {
            let now = Instant::now();
            if until > now {
                return AcquireState::CoolingDown(until.duration_since(now));
            }
            state.cooldown_until = None;
        }

        let is_next = match priority {
            GitHubRequestPriority::User => state.user_waiters.front() == Some(&ticket),
            GitHubRequestPriority::Background => {
                state.user_waiters.is_empty()
                    && state.background_waiters.front() == Some(&ticket)
            }
        };
        if !is_next {
            return AcquireState::Waiting;
        }

        state.waiters_mut(priority).pop_front();
        state.active = true;
        AcquireState::Ready
    }

    fn defer_for(&self, delay: Duration) {
        let until = Instant::now() + delay;
        let mut state = self.lock_state();
        if state.cooldown_until.is_none_or(|current| until > current) {
            state.cooldown_until = Some(until);
        }
        drop(state);
        self.changed.notify_waiters();
    }

    fn remove_waiter(&self, priority: GitHubRequestPriority, ticket: u64) {
        let mut state = self.lock_state();
        let waiters = state.waiters_mut(priority);
        if let Some(position) = waiters.iter().position(|queued| *queued == ticket) {
            waiters.remove(position);
        }
        drop(state);
        self.changed.notify_waiters();
    }

    fn release(&self) {
        let mut state = self.lock_state();
        state.active = false;
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
    fn waiters_mut(&mut self, priority: GitHubRequestPriority) -> &mut VecDeque<u64> {
        match priority {
            GitHubRequestPriority::User => &mut self.user_waiters,
            GitHubRequestPriority::Background => &mut self.background_waiters,
        }
    }
}

struct PendingRequest<'a> {
    queue: &'a RequestQueue,
    priority: GitHubRequestPriority,
    ticket: u64,
    queued: bool,
}

impl Drop for PendingRequest<'_> {
    fn drop(&mut self) {
        if self.queued {
            self.queue.remove_waiter(self.priority, self.ticket);
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::mpsc;
    use tokio::time::{Duration, timeout};

    use super::*;

    #[tokio::test]
    async fn queue_runs_one_request_at_a_time() {
        let queue = Arc::new(RequestQueue::default());
        let first = queue.acquire(GitHubRequestPriority::User).await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let waiting_queue = queue.clone();
        let task = tokio::spawn(async move {
            let _permit = waiting_queue.acquire(GitHubRequestPriority::User).await;
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
    async fn user_request_overtakes_waiting_background_request() {
        let queue = Arc::new(RequestQueue::default());
        let active = queue.acquire(GitHubRequestPriority::Background).await;
        let (tx, mut rx) = mpsc::unbounded_channel();

        let background_queue = queue.clone();
        let background_tx = tx.clone();
        let background = tokio::spawn(async move {
            let _permit = background_queue
                .acquire(GitHubRequestPriority::Background)
                .await;
            let _ = background_tx.send("background");
        });
        while queue.lock_state().background_waiters.is_empty() {
            tokio::task::yield_now().await;
        }

        let user_queue = queue.clone();
        let user = tokio::spawn(async move {
            let _permit = user_queue.acquire(GitHubRequestPriority::User).await;
            let _ = tx.send("user");
        });
        while queue.lock_state().user_waiters.is_empty() {
            tokio::task::yield_now().await;
        }

        drop(active);
        assert_eq!(rx.recv().await, Some("user"));
        assert_eq!(rx.recv().await, Some("background"));
        user.await.expect("user task should finish");
        background.await.expect("background task should finish");
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
    fn permission_error_without_rate_limit_headers_does_not_cool_down() {
        assert_eq!(
            rate_limit_delay(403, &HeaderMap::new(), Some("Resource not accessible")),
            None
        );
    }
}
