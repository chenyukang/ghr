use std::collections::HashMap;
use std::future::Future;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio::process::Command;
use tokio::sync::OnceCell;
use tokio::time::sleep;
use tracing::{debug, info, warn};

use crate::config::{Config, SearchSection};
use crate::model::{
    ActionHints, CheckSummary, CommentPreview, FailedCheckRunSummary, ItemKind, Milestone,
    PullRequestBranch, ReactionSummary, ReviewCommentPreview, SectionKind, SectionSnapshot,
    WorkItem, builtin_view_key, global_search_view_key, repo_section_filters_with_labels,
    repo_view_key,
};

static VIEWER_LOGIN: OnceCell<String> = OnceCell::const_new();
static USER_GH_REQUESTS_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

const SEARCH_API_MAX_RESULTS: usize = 1000;
const SEARCH_API_MAX_PAGE_SIZE: usize = 100;
const SEARCH_REFRESH_SPACING: Duration = Duration::from_millis(350);
const BACKGROUND_GH_YIELD_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MergeMethod {
    #[default]
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    pub fn label(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Squash => "squash",
            Self::Rebase => "rebase",
        }
    }

    pub fn gh_flag(self) -> &'static str {
        match self {
            Self::Merge => "--merge",
            Self::Squash => "--squash",
            Self::Rebase => "--rebase",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Merge => Self::Squash,
            Self::Squash => Self::Rebase,
            Self::Rebase => Self::Merge,
        }
    }
}

pub struct CommentFetchResult {
    pub item_reactions: ReactionSummary,
    pub item_milestone: Option<Milestone>,
    pub comments: Vec<CommentPreview>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GhRequestPriority {
    User,
    Background,
}

tokio::task_local! {
    static GH_REQUEST_PRIORITY: GhRequestPriority;
}

pub async fn with_background_github_priority<F>(future: F) -> F::Output
where
    F: Future,
{
    GH_REQUEST_PRIORITY
        .scope(GhRequestPriority::Background, future)
        .await
}

fn current_gh_request_priority() -> GhRequestPriority {
    GH_REQUEST_PRIORITY
        .try_with(|priority| *priority)
        .unwrap_or(GhRequestPriority::User)
}

struct UserGhRequestGuard;

impl UserGhRequestGuard {
    fn new() -> Self {
        USER_GH_REQUESTS_IN_FLIGHT.fetch_add(1, Ordering::AcqRel);
        Self
    }
}

impl Drop for UserGhRequestGuard {
    fn drop(&mut self) {
        USER_GH_REQUESTS_IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
    }
}

async fn wait_for_user_gh_requests() {
    while USER_GH_REQUESTS_IN_FLIGHT.load(Ordering::Acquire) > 0 {
        sleep(BACKGROUND_GH_YIELD_INTERVAL).await;
    }
}

fn gh_request_kind(args: &[String]) -> &'static str {
    if args.first().is_some_and(|arg| arg == "api") {
        "gh api"
    } else {
        "gh"
    }
}

fn gh_command_display(args: &[String]) -> String {
    format!("gh {}", args.join(" "))
}

fn debug_log_gh_request_started(args: &[String], directory: Option<&Path>) {
    let kind = gh_request_kind(args);
    let command = gh_command_display(args);
    if let Some(directory) = directory {
        debug!(
            kind,
            command = %command,
            cwd = %directory.display(),
            "gh request started"
        );
    } else {
        debug!(kind, command = %command, "gh request started");
    }
}

fn debug_log_gh_request_finished(
    args: &[String],
    directory: Option<&Path>,
    output: &std::process::Output,
) {
    let kind = gh_request_kind(args);
    let command = gh_command_display(args);
    if let Some(directory) = directory {
        debug!(
            kind,
            command = %command,
            cwd = %directory.display(),
            status = %output.status,
            success = output.status.success(),
            stdout_bytes = output.stdout.len(),
            stderr_bytes = output.stderr.len(),
            "gh request finished"
        );
    } else {
        debug!(
            kind,
            command = %command,
            status = %output.status,
            success = output.status.success(),
            stdout_bytes = output.stdout.len(),
            stderr_bytes = output.stderr.len(),
            "gh request finished"
        );
    }
}

fn debug_log_gh_request_failed_to_start(
    args: &[String],
    directory: Option<&Path>,
    error: &std::io::Error,
) {
    let kind = gh_request_kind(args);
    let command = gh_command_display(args);
    if let Some(directory) = directory {
        debug!(
            kind,
            command = %command,
            cwd = %directory.display(),
            error = %error,
            "gh request failed to start"
        );
    } else {
        debug!(
            kind,
            command = %command,
            error = %error,
            "gh request failed to start"
        );
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchItemRaw {
    author: Option<SearchAuthorRaw>,
    assignees: Option<Vec<SearchAuthorRaw>>,
    body: Option<String>,
    comments_count: Option<u64>,
    created_at: Option<DateTime<Utc>>,
    is_draft: Option<bool>,
    labels: Option<Vec<SearchLabelRaw>>,
    number: u64,
    repository: SearchRepositoryRaw,
    state: Option<String>,
    title: String,
    updated_at: Option<DateTime<Utc>>,
    url: String,
}

#[derive(Debug, Deserialize)]
struct SearchAuthorRaw {
    login: String,
}

#[derive(Debug, Deserialize)]
struct SearchLabelRaw {
    name: String,
}

#[derive(Debug, Deserialize)]
struct RepositoryLabelRaw {
    name: String,
}

#[derive(Debug, Deserialize)]
struct RepositoryAssigneeRaw {
    login: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchRepositoryRaw {
    name_with_owner: String,
}

#[derive(Debug, Deserialize)]
struct SearchPageRaw {
    total_count: usize,
    items: Vec<SearchApiIssueRaw>,
}

#[derive(Debug, Deserialize)]
struct SearchApiIssueRaw {
    assignees: Option<Vec<SearchAuthorRaw>>,
    body: Option<String>,
    comments: Option<u64>,
    created_at: Option<DateTime<Utc>>,
    draft: Option<bool>,
    html_url: String,
    labels: Option<Vec<SearchLabelRaw>>,
    milestone: Option<SearchMilestoneRaw>,
    number: u64,
    reactions: Option<ReactionSummaryRaw>,
    repository_url: String,
    state: Option<String>,
    title: String,
    updated_at: Option<DateTime<Utc>>,
    user: Option<SearchAuthorRaw>,
}

#[derive(Debug, Deserialize)]
struct IssueDetailsRaw {
    reactions: Option<ReactionSummaryRaw>,
    milestone: Option<MilestoneRaw>,
}

struct IssueDetails {
    reactions: ReactionSummary,
    milestone: Option<Milestone>,
}

#[derive(Debug, Default, Deserialize)]
struct ReactionSummaryRaw {
    #[serde(rename = "+1")]
    plus_one: Option<u64>,
    #[serde(rename = "-1")]
    minus_one: Option<u64>,
    laugh: Option<u64>,
    hooray: Option<u64>,
    confused: Option<u64>,
    heart: Option<u64>,
    rocket: Option<u64>,
    eyes: Option<u64>,
}

impl From<ReactionSummaryRaw> for ReactionSummary {
    fn from(raw: ReactionSummaryRaw) -> Self {
        Self {
            plus_one: raw.plus_one.unwrap_or(0),
            minus_one: raw.minus_one.unwrap_or(0),
            laugh: raw.laugh.unwrap_or(0),
            hooray: raw.hooray.unwrap_or(0),
            confused: raw.confused.unwrap_or(0),
            heart: raw.heart.unwrap_or(0),
            rocket: raw.rocket.unwrap_or(0),
            eyes: raw.eyes.unwrap_or(0),
        }
    }
}

#[derive(Debug, Deserialize)]
struct SearchMilestoneRaw {
    number: u64,
    title: String,
}

#[derive(Debug, Deserialize)]
struct MilestoneRaw {
    number: u64,
    title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemMetadataUpdate {
    pub title: String,
    pub body: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct IssueItemRaw {
    assignees: Option<Vec<SearchAuthorRaw>>,
    body: Option<String>,
    comments: Option<u64>,
    html_url: String,
    labels: Option<Vec<SearchLabelRaw>>,
    number: u64,
    repository_url: String,
    state: Option<String>,
    title: String,
    updated_at: Option<DateTime<Utc>>,
    user: Option<SearchAuthorRaw>,
}

#[derive(Debug, Clone, Deserialize)]
struct NotificationRaw {
    id: String,
    unread: bool,
    reason: String,
    updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    last_read_at: Option<DateTime<Utc>>,
    subject: NotificationSubjectRaw,
    repository: NotificationRepositoryRaw,
}

#[derive(Debug, Clone, Deserialize)]
struct NotificationSubjectRaw {
    title: String,
    url: Option<String>,
    #[serde(rename = "type")]
    subject_type: String,
}

#[derive(Debug, Clone, Deserialize)]
struct NotificationRepositoryRaw {
    full_name: String,
    html_url: String,
}

#[derive(Debug, Deserialize)]
struct ViewerRaw {
    login: String,
}

#[derive(Debug, Deserialize)]
struct IssueCommentRaw {
    id: Option<u64>,
    body: Option<String>,
    html_url: Option<String>,
    created_at: Option<DateTime<Utc>>,
    updated_at: Option<DateTime<Utc>>,
    reactions: Option<ReactionSummaryRaw>,
    user: Option<SearchAuthorRaw>,
}

#[derive(Debug, Deserialize)]
struct PullRequestReviewCommentRaw {
    id: Option<u64>,
    in_reply_to_id: Option<u64>,
    body: Option<String>,
    diff_hunk: Option<String>,
    html_url: Option<String>,
    created_at: Option<DateTime<Utc>>,
    updated_at: Option<DateTime<Utc>>,
    reactions: Option<ReactionSummaryRaw>,
    user: Option<SearchAuthorRaw>,
    path: Option<String>,
    line: Option<u64>,
    original_line: Option<u64>,
    start_line: Option<u64>,
    original_start_line: Option<u64>,
    side: Option<String>,
    start_side: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ReviewThreadState {
    is_resolved: bool,
    is_outdated: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestReviewThreadsGraphQlRaw {
    data: PullRequestReviewThreadsDataRaw,
}

#[derive(Debug, Deserialize)]
struct PullRequestReviewThreadsDataRaw {
    repository: Option<PullRequestReviewThreadsRepositoryRaw>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestReviewThreadsRepositoryRaw {
    pull_request: Option<PullRequestReviewThreadsPullRequestRaw>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestReviewThreadsPullRequestRaw {
    review_threads: PullRequestReviewThreadConnectionRaw,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestReviewThreadConnectionRaw {
    nodes: Option<Vec<PullRequestReviewThreadRaw>>,
    page_info: GraphQlPageInfoRaw,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestReviewThreadRaw {
    is_resolved: bool,
    is_outdated: bool,
    comments: PullRequestReviewThreadCommentConnectionRaw,
}

#[derive(Debug, Deserialize)]
struct PullRequestReviewThreadCommentConnectionRaw {
    nodes: Option<Vec<PullRequestReviewThreadCommentRaw>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestReviewThreadCommentRaw {
    database_id: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlPageInfoRaw {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestMergeStatusRaw {
    is_draft: Option<bool>,
    merge_state_status: Option<String>,
    review_decision: Option<String>,
    state: Option<String>,
    status_check_rollup: Option<Vec<PullRequestCheckRaw>>,
}

#[derive(Debug, Deserialize)]
struct PullRequestCheckRaw {
    conclusion: Option<String>,
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestAutoMergeStatusRaw {
    auto_merge_request: Option<serde_json::Value>,
    is_draft: Option<bool>,
    state: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RepositoryMergeMethodsRaw {
    allow_merge_commit: Option<bool>,
    allow_rebase_merge: Option<bool>,
    allow_squash_merge: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestActionGraphQlRaw {
    data: PullRequestActionDataRaw,
}

#[derive(Debug, Deserialize)]
struct PullRequestActionDataRaw {
    repository: Option<PullRequestActionRepositoryRaw>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestActionRepositoryRaw {
    pull_request: Option<PullRequestActionRaw>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestActionRaw {
    auto_merge_request: Option<serde_json::Value>,
    commits: Option<PullRequestCommitConnectionRaw>,
    head_ref_name: Option<String>,
    head_repository: Option<PullRequestHeadRepositoryRaw>,
    is_draft: Option<bool>,
    mergeable: Option<String>,
    merge_state_status: Option<String>,
    review_decision: Option<String>,
    state: Option<String>,
    status_check_rollup: Option<PullRequestStatusRollupRaw>,
    viewer_can_enable_auto_merge: Option<bool>,
    viewer_can_merge_as_admin: Option<bool>,
    viewer_can_update: Option<bool>,
    viewer_can_update_branch: Option<bool>,
    viewer_did_author: Option<bool>,
    viewer_latest_review: Option<PullRequestViewerReviewRaw>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestCommitConnectionRaw {
    total_count: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestHeadRepositoryRaw {
    name_with_owner: String,
}

#[derive(Debug, Deserialize)]
struct PullRequestStatusRollupRaw {
    state: Option<String>,
    contexts: Option<PullRequestCheckConnectionRaw>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestCheckConnectionRaw {
    total_count: usize,
    nodes: Option<Vec<PullRequestCheckContextRaw>>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "__typename")]
enum PullRequestCheckContextRaw {
    CheckRun {
        conclusion: Option<String>,
        #[serde(rename = "detailsUrl")]
        details_url: Option<String>,
        name: Option<String>,
        status: Option<String>,
    },
    StatusContext {
        state: Option<String>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct PullRequestViewerReviewRaw {
    state: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PullRequestCheckStatusRaw {
    bucket: Option<String>,
    link: Option<String>,
    name: Option<String>,
    state: Option<String>,
    workflow: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct FailedCheckRunDiscovery {
    runs: Vec<FailedCheckRunSummary>,
    unmapped_failed_checks: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PullRequestHeadRaw {
    head: PullRequestHeadRefRaw,
}

#[derive(Debug, Deserialize)]
struct PullRequestHeadRefRaw {
    sha: String,
}

pub async fn refresh_dashboard(config: &Config) -> Vec<SectionSnapshot> {
    refresh_dashboard_with_progress(config, |_| {}).await
}

pub async fn refresh_dashboard_with_progress<F>(
    config: &Config,
    mut on_section: F,
) -> Vec<SectionSnapshot>
where
    F: FnMut(&SectionSnapshot),
{
    let started = Instant::now();
    let viewer_login = match cached_viewer_login().await {
        Ok(login) => Some(login),
        Err(error) => {
            warn!(error = %error, "failed to resolve @me for search filters");
            None
        }
    };
    let mut searches =
        refresh_search_sections(config, viewer_login.as_deref(), &mut on_section).await;
    pace_search_refresh().await;
    let mut notifications = refresh_notification_sections(config).await;
    for section in &notifications {
        on_section(section);
    }
    searches.append(&mut notifications);
    info!(
        sections = searches.len(),
        elapsed_ms = started.elapsed().as_millis(),
        "dashboard refresh finished"
    );
    searches
}

pub async fn search_global(
    query: &str,
    repo_scope: Option<&str>,
    config: &Config,
) -> Vec<SectionSnapshot> {
    let viewer_login = match cached_viewer_login().await {
        Ok(login) => Some(login),
        Err(error) => {
            warn!(error = %error, "failed to resolve @me for global search filters");
            None
        }
    };
    let filters = global_search_filters(query, repo_scope);
    let pr_section = resolve_me_section(
        SearchSection {
            title: "Pull Requests".to_string(),
            filters: filters.clone(),
            queries: Vec::new(),
            limit: None,
        },
        viewer_login.as_deref(),
    );
    let issue_section = resolve_me_section(
        SearchSection {
            title: "Issues".to_string(),
            filters,
            queries: Vec::new(),
            limit: None,
        },
        viewer_login.as_deref(),
    );
    let view = global_search_view_key();
    let excludes = config.exclude_repos.clone();
    let pr_per_page = config.defaults.pr_per_page;
    let issue_per_page = config.defaults.issue_per_page;
    let pull_requests = refresh_search_section(
        view.clone(),
        SectionKind::PullRequests,
        pr_section,
        pr_per_page,
        1,
        excludes.as_slice(),
    )
    .await;
    pace_search_refresh().await;
    let issues = refresh_search_section(
        view,
        SectionKind::Issues,
        issue_section,
        issue_per_page,
        1,
        excludes.as_slice(),
    )
    .await;

    vec![pull_requests, issues]
}

async fn refresh_search_sections(
    config: &Config,
    viewer_login: Option<&str>,
    on_section: &mut impl FnMut(&SectionSnapshot),
) -> Vec<SectionSnapshot> {
    let excludes = config.exclude_repos.clone();
    let mut jobs = Vec::new();

    for section in config.pr_sections.clone() {
        let limit = section.limit.unwrap_or(config.defaults.pr_per_page);
        jobs.push(SearchRefreshJob {
            view: builtin_view_key(SectionKind::PullRequests),
            kind: SectionKind::PullRequests,
            section: resolve_me_section(section, viewer_login),
            limit,
        });
    }

    for section in config.issue_sections.clone() {
        let limit = section.limit.unwrap_or(config.defaults.issue_per_page);
        jobs.push(SearchRefreshJob {
            view: builtin_view_key(SectionKind::Issues),
            kind: SectionKind::Issues,
            section: resolve_me_section(section, viewer_login),
            limit,
        });
    }

    for repo in config.repos.clone() {
        if repo.name.trim().is_empty() || repo.repo.trim().is_empty() {
            continue;
        }

        let view = repo_view_key(&repo.name);
        if repo.show_prs {
            let limit = config.defaults.pr_per_page;
            let labels = repo.label_filters(SectionKind::PullRequests);
            let section = SearchSection {
                title: "Pull Requests".to_string(),
                filters: repo_section_filters_with_labels(&repo.repo, &labels),
                queries: Vec::new(),
                limit: None,
            };
            jobs.push(SearchRefreshJob {
                view: view.clone(),
                kind: SectionKind::PullRequests,
                section,
                limit,
            });
        }

        if repo.show_issues {
            let limit = config.defaults.issue_per_page;
            let labels = repo.label_filters(SectionKind::Issues);
            let section = SearchSection {
                title: "Issues".to_string(),
                filters: repo_section_filters_with_labels(&repo.repo, &labels),
                queries: Vec::new(),
                limit: None,
            };
            jobs.push(SearchRefreshJob {
                view: view.clone(),
                kind: SectionKind::Issues,
                section,
                limit,
            });
        }
    }

    let mut sections = Vec::new();
    for job in jobs {
        if !sections.is_empty() {
            pace_search_refresh().await;
        }
        let section = refresh_search_section(
            job.view,
            job.kind,
            job.section,
            job.limit,
            1,
            excludes.as_slice(),
        )
        .await;
        on_section(&section);
        sections.push(section);
    }
    sections
}

struct SearchRefreshJob {
    view: String,
    kind: SectionKind,
    section: SearchSection,
    limit: usize,
}

async fn pace_search_refresh() {
    sleep(SEARCH_REFRESH_SPACING).await;
}

pub async fn refresh_section_page(
    view: String,
    kind: SectionKind,
    title: String,
    filters: String,
    page: usize,
    page_size: usize,
    config: &Config,
) -> SectionSnapshot {
    let viewer_login = match cached_viewer_login().await {
        Ok(login) => Some(login),
        Err(error) => {
            warn!(error = %error, "failed to resolve @me for section page filters");
            None
        }
    };
    let queries = filters
        .split(" | ")
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let section = resolve_me_section(
        SearchSection {
            title,
            filters: if queries.len() > 1 {
                String::new()
            } else {
                filters
            },
            queries: if queries.len() > 1 {
                queries
            } else {
                Vec::new()
            },
            limit: Some(page_size),
        },
        viewer_login.as_deref(),
    );
    refresh_search_section(
        view,
        kind,
        section,
        page_size,
        page,
        config.exclude_repos.as_slice(),
    )
    .await
}

async fn refresh_search_section(
    view: String,
    kind: SectionKind,
    section: SearchSection,
    limit: usize,
    page: usize,
    exclude_repos: &[String],
) -> SectionSnapshot {
    let queries = section.search_filters();
    let display_filters = section.display_filters();
    let mut snapshot = SectionSnapshot::empty_for_view(view, kind, section.title, display_filters);
    let started = Instant::now();

    match fetch_search_items(kind, queries, limit, page, exclude_repos).await {
        Ok(result) => {
            snapshot.items = result.items;
            snapshot.total_count = result.total_count;
            snapshot.page = result.page;
            snapshot.page_size = result.page_size;
            snapshot.refreshed_at = Some(Utc::now());
            info!(
                kind = %kind,
                title = %snapshot.title,
                items = snapshot.items.len(),
                total_count = ?snapshot.total_count,
                elapsed_ms = started.elapsed().as_millis(),
                "search section refreshed"
            );
        }
        Err(error) => {
            let message = error.to_string();
            warn!(
                kind = %kind,
                title = %snapshot.title,
                error = %message,
                "search section refresh failed"
            );
            snapshot.error = Some(message);
        }
    }

    snapshot
}

#[derive(Debug)]
struct SearchFetchResult {
    items: Vec<WorkItem>,
    total_count: Option<usize>,
    page: usize,
    page_size: usize,
}

async fn fetch_search_items(
    kind: SectionKind,
    queries: Vec<String>,
    limit: usize,
    page: usize,
    exclude_repos: &[String],
) -> Result<SearchFetchResult> {
    let queries = if queries.is_empty() {
        vec![String::new()]
    } else {
        queries
    };
    if queries.len() == 1 {
        let query = queries.first().expect("single query should exist");
        return fetch_search_page(kind, query, page, limit, exclude_repos).await;
    }

    let mut deduped = HashMap::<String, WorkItem>::new();
    for (index, query) in queries.into_iter().enumerate() {
        if index > 0 {
            pace_search_refresh().await;
        }
        for item in fetch_search_items_for_query(kind, query, limit, exclude_repos).await? {
            match deduped.get(&item.id) {
                Some(existing) if existing.updated_at >= item.updated_at => {}
                _ => {
                    deduped.insert(item.id.clone(), item);
                }
            }
        }
    }

    let mut items = deduped.into_values().collect::<Vec<_>>();
    items.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.repo.cmp(&right.repo))
            .then_with(|| left.number.cmp(&right.number))
    });
    items.truncate(limit);
    Ok(SearchFetchResult {
        items,
        total_count: None,
        page: 1,
        page_size: search_command_limit(limit),
    })
}

async fn fetch_search_items_for_query(
    kind: SectionKind,
    filters: String,
    limit: usize,
    exclude_repos: &[String],
) -> Result<Vec<WorkItem>> {
    let subcommand = match kind {
        SectionKind::PullRequests => "prs",
        SectionKind::Issues => "issues",
        SectionKind::Notifications => bail!("notifications are not fetched via search"),
    };

    let fields = search_fields(kind);

    let args = search_args(subcommand, fields, &filters, limit);

    let output = run_gh_json(&args).await?;
    let items = serde_json::from_str::<Vec<SearchItemRaw>>(&output)
        .with_context(|| format!("failed to parse gh search {subcommand} output"))?;

    Ok(items
        .into_iter()
        .filter(|item| !is_excluded_repo(&item.repository.name_with_owner, exclude_repos))
        .map(|item| search_item_to_work_item(kind, item))
        .collect())
}

fn search_args(subcommand: &str, fields: &str, filters: &str, limit: usize) -> Vec<String> {
    let limit = search_command_limit(limit);
    let mut args = vec![
        "search".to_string(),
        subcommand.to_string(),
        "--json".to_string(),
        fields.to_string(),
        "--limit".to_string(),
        limit.to_string(),
    ];
    let query_tokens = filters
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if !query_tokens.is_empty() {
        args.push("--".to_string());
        args.extend(query_tokens);
    }
    args
}

fn search_command_limit(limit: usize) -> usize {
    limit.clamp(1, SEARCH_API_MAX_RESULTS)
}

fn search_api_page_size(limit: usize) -> usize {
    limit.clamp(1, SEARCH_API_MAX_PAGE_SIZE)
}

async fn fetch_search_page(
    kind: SectionKind,
    filters: &str,
    page: usize,
    page_size: usize,
    exclude_repos: &[String],
) -> Result<SearchFetchResult> {
    let page = page.max(1);
    let page_size = search_api_page_size(page_size);
    let args = search_page_args(kind, filters, page, page_size);
    let output = run_gh_json(&args).await?;
    let raw = serde_json::from_str::<SearchPageRaw>(&output)
        .context("failed to parse GitHub search page")?;
    let items = raw
        .items
        .into_iter()
        .map(|item| search_api_item_to_work_item(kind, item))
        .filter(|item| !is_excluded_repo(&item.repo, exclude_repos))
        .collect::<Vec<_>>();

    Ok(SearchFetchResult {
        items,
        total_count: Some(raw.total_count),
        page,
        page_size,
    })
}

fn search_page_args(kind: SectionKind, filters: &str, page: usize, per_page: usize) -> Vec<String> {
    let query = search_query(kind, filters);
    let mut args = vec![
        "api".to_string(),
        "--method".to_string(),
        "GET".to_string(),
        "search/issues".to_string(),
        "-f".to_string(),
        format!("q={query}"),
        "-f".to_string(),
        format!("per_page={}", search_api_page_size(per_page)),
        "-f".to_string(),
        format!("page={}", page.max(1)),
    ];
    if let Some((sort, order)) = search_sort(filters) {
        args.push("-f".to_string());
        args.push(format!("sort={sort}"));
        args.push("-f".to_string());
        args.push(format!("order={order}"));
    }
    args
}

fn search_query(kind: SectionKind, filters: &str) -> String {
    let mut query_tokens = vec![match kind {
        SectionKind::PullRequests => "is:pr".to_string(),
        SectionKind::Issues => "is:issue".to_string(),
        SectionKind::Notifications => unreachable!("notifications are not fetched via search"),
    }];
    query_tokens.extend(
        filters
            .split_whitespace()
            .filter(|token| !token.starts_with("sort:"))
            .map(str::to_string),
    );
    query_tokens.join(" ")
}

fn search_sort(filters: &str) -> Option<(String, String)> {
    filters.split_whitespace().find_map(|token| {
        let value = token.strip_prefix("sort:")?;
        match value.rsplit_once('-') {
            Some((field, order)) if matches!(order, "asc" | "desc") && !field.is_empty() => {
                Some((field.to_string(), order.to_string()))
            }
            _ if !value.is_empty() => Some((value.to_string(), "desc".to_string())),
            _ => None,
        }
    })
}

fn global_search_filters(query: &str, repo_scope: Option<&str>) -> String {
    let mut tokens = query
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if should_default_global_search_to_title(&tokens) {
        tokens.push("in:title".to_string());
    }
    if !tokens.iter().any(|token| token.starts_with("repo:"))
        && let Some(repo) = repo_scope.map(str::trim).filter(|repo| !repo.is_empty())
    {
        tokens.push(format!("repo:{repo}"));
    }
    if !tokens.iter().any(|token| token.starts_with("archived:")) {
        tokens.push("archived:false".to_string());
    }
    if !tokens.iter().any(|token| token.starts_with("sort:")) {
        tokens.push("sort:created-desc".to_string());
    }
    tokens.join(" ")
}

fn should_default_global_search_to_title(tokens: &[String]) -> bool {
    tokens.iter().any(|token| is_plain_search_term(token))
        && !tokens
            .iter()
            .any(|token| token.starts_with("in:") || token.starts_with("-in:"))
}

fn is_plain_search_term(token: &str) -> bool {
    let token = token.trim_matches('"');
    if token.is_empty() {
        return false;
    }
    let token = token.strip_prefix('-').unwrap_or(token);
    !token.contains(':')
}

fn search_fields(kind: SectionKind) -> &'static str {
    match kind {
        SectionKind::PullRequests => {
            "number,title,body,repository,author,assignees,createdAt,updatedAt,url,state,isDraft,labels,commentsCount"
        }
        SectionKind::Issues => {
            "number,title,body,repository,author,assignees,createdAt,updatedAt,url,state,labels,commentsCount"
        }
        SectionKind::Notifications => unreachable!("notifications are not fetched via search"),
    }
}

pub async fn fetch_comments(
    repository: &str,
    number: u64,
    kind: ItemKind,
) -> Result<CommentFetchResult> {
    match kind {
        ItemKind::PullRequest => fetch_pull_request_comments(repository, number).await,
        ItemKind::Issue => fetch_issue_comments(repository, number).await,
        ItemKind::Notification => Ok(CommentFetchResult {
            item_reactions: ReactionSummary::default(),
            item_milestone: None,
            comments: Vec::new(),
        }),
    }
}

pub async fn fetch_issue_comments(repository: &str, number: u64) -> Result<CommentFetchResult> {
    let issue_output = fetch_issue_details_output(repository, number);
    let comments_output = fetch_issue_comments_output(repository, number);
    let viewer_login = comment_viewer_login("comment ownership");
    let (issue_output, comments_output, viewer_login) =
        tokio::join!(issue_output, comments_output, viewer_login);
    let issue_details = parse_issue_details_output(&issue_output?, repository, number)?;
    Ok(CommentFetchResult {
        item_reactions: issue_details.reactions,
        item_milestone: issue_details.milestone,
        comments: parse_issue_comments_output(
            &comments_output?,
            repository,
            number,
            viewer_login.as_deref(),
        )?,
    })
}

async fn fetch_issue_details_output(repository: &str, number: u64) -> Result<String> {
    let path = format!("repos/{repository}/issues/{number}");
    run_gh_json(&[
        "api".to_string(),
        "-H".to_string(),
        "Accept: application/vnd.github.squirrel-girl-preview+json".to_string(),
        path,
    ])
    .await
}

async fn fetch_issue_comments_output(repository: &str, number: u64) -> Result<String> {
    let path = format!("repos/{repository}/issues/{number}/comments?per_page=100");
    run_gh_json(&[
        "api".to_string(),
        "-H".to_string(),
        "Accept: application/vnd.github.squirrel-girl-preview+json".to_string(),
        "--paginate".to_string(),
        "--slurp".to_string(),
        path,
    ])
    .await
}

pub async fn fetch_pull_request_comments(
    repository: &str,
    number: u64,
) -> Result<CommentFetchResult> {
    let issue_details = fetch_issue_details_output(repository, number);
    let issue_comments = fetch_issue_comments_output(repository, number);
    let review_comments = fetch_pull_request_review_comments_output(repository, number);
    let review_thread_states = fetch_pull_request_review_thread_states(repository, number);
    let viewer_login = comment_viewer_login("pull request comment ownership");
    let (issue_details, issue_output, review_output, review_thread_states, viewer_login) = tokio::join!(
        issue_details,
        issue_comments,
        review_comments,
        review_thread_states,
        viewer_login
    );
    let viewer_login = viewer_login.as_deref();
    let mut comments =
        parse_issue_comments_output(&issue_output?, repository, number, viewer_login)?;
    let review_thread_states = match review_thread_states {
        Ok(states) => states,
        Err(error) => {
            warn!(
                error = %error,
                repository,
                number,
                "failed to load pull request review thread states"
            );
            HashMap::new()
        }
    };
    comments.append(&mut parse_pull_request_review_comments_output(
        &review_output?,
        repository,
        number,
        viewer_login,
        &review_thread_states,
    )?);
    comments.sort_by_key(|comment| comment.created_at);
    let issue_details = parse_issue_details_output(&issue_details?, repository, number)?;
    Ok(CommentFetchResult {
        item_reactions: issue_details.reactions,
        item_milestone: issue_details.milestone,
        comments,
    })
}

async fn fetch_pull_request_review_comments_output(
    repository: &str,
    number: u64,
) -> Result<String> {
    let path = format!("repos/{repository}/pulls/{number}/comments?per_page=100");
    run_gh_json(&[
        "api".to_string(),
        "-H".to_string(),
        "Accept: application/vnd.github.squirrel-girl-preview+json".to_string(),
        "--paginate".to_string(),
        "--slurp".to_string(),
        path,
    ])
    .await
}

async fn fetch_pull_request_review_thread_states(
    repository: &str,
    number: u64,
) -> Result<HashMap<u64, ReviewThreadState>> {
    let (owner, name) = split_repository(repository)?;
    let query = r#"
query($owner: String!, $name: String!, $number: Int!, $cursor: String) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewThreads(first: 100, after: $cursor) {
        pageInfo {
          hasNextPage
          endCursor
        }
        nodes {
          isResolved
          isOutdated
          comments(first: 100) {
            nodes {
              databaseId
            }
          }
        }
      }
    }
  }
}
"#;
    let mut cursor = None;
    let mut states = HashMap::new();

    loop {
        let mut args = vec![
            "api".to_string(),
            "graphql".to_string(),
            "-f".to_string(),
            format!("query={query}"),
            "-F".to_string(),
            format!("owner={owner}"),
            "-F".to_string(),
            format!("name={name}"),
            "-F".to_string(),
            format!("number={number}"),
        ];
        if let Some(cursor) = &cursor {
            args.push("-F".to_string());
            args.push(format!("cursor={cursor}"));
        }

        let output = run_gh_json(&args).await?;
        let (page_states, has_next_page, end_cursor) =
            parse_pull_request_review_thread_states_page(&output, repository, number)?;
        states.extend(page_states);
        if !has_next_page {
            break;
        }
        let Some(next_cursor) = end_cursor else {
            break;
        };
        cursor = Some(next_cursor);
    }

    Ok(states)
}

async fn comment_viewer_login(context: &'static str) -> Option<String> {
    match cached_viewer_login().await {
        Ok(login) => Some(login),
        Err(error) => {
            warn!(error = %error, "failed to resolve current GitHub user for {context}");
            None
        }
    }
}

pub async fn fetch_pull_request_action_hints(repository: &str, number: u64) -> Result<ActionHints> {
    let (owner, name) = split_repository(repository)?;
    let query = r#"
query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      state
      autoMergeRequest {
        enabledAt
      }
      headRefName
      headRepository {
        nameWithOwner
      }
      isDraft
      mergeable
      mergeStateStatus
      reviewDecision
      viewerCanUpdate
      viewerCanUpdateBranch
      viewerCanEnableAutoMerge
      viewerCanMergeAsAdmin
      viewerDidAuthor
      commits {
        totalCount
      }
      viewerLatestReview {
        state
      }
      statusCheckRollup {
        state
        contexts(first: 100) {
          totalCount
          nodes {
            __typename
            ... on CheckRun {
              name
              status
              conclusion
              detailsUrl
            }
            ... on StatusContext {
              state
            }
          }
        }
      }
    }
  }
}
"#;
    let output = run_gh_json(&[
        "api".to_string(),
        "graphql".to_string(),
        "-f".to_string(),
        format!("query={query}"),
        "-F".to_string(),
        format!("owner={owner}"),
        "-F".to_string(),
        format!("name={name}"),
        "-F".to_string(),
        format!("number={number}"),
    ])
    .await?;
    let raw = serde_json::from_str::<PullRequestActionGraphQlRaw>(&output)
        .with_context(|| format!("failed to parse action hints for {repository}#{number}"))?;
    let pr = raw
        .data
        .repository
        .and_then(|repository| repository.pull_request)
        .ok_or_else(|| anyhow!("pull request {repository}#{number} was not found"))?;
    Ok(pull_request_action_hints(&pr))
}

pub async fn rerun_failed_pull_request_checks(repository: &str, number: u64) -> Result<()> {
    let discovery = fetch_failed_check_runs_from_pr_checks(repository, number).await?;
    if discovery.runs.is_empty() {
        if discovery.unmapped_failed_checks.is_empty() {
            bail!("no failed checks found for {repository}#{number}");
        }
        bail!(
            "failed checks were found, but none linked to GitHub Actions workflow runs: {}",
            discovery.unmapped_failed_checks.join(", ")
        );
    }

    for run in &discovery.runs {
        run_gh_json(&rerun_failed_check_run_args(repository, run.run_id)).await?;
    }

    Ok(())
}

async fn fetch_failed_check_runs_from_pr_checks(
    repository: &str,
    number: u64,
) -> Result<FailedCheckRunDiscovery> {
    let output = run_gh_pr_checks_json(&pr_checks_args(repository, number)).await?;
    failed_check_runs_from_pr_checks_json(&output)
        .with_context(|| format!("failed to parse PR checks for {repository}#{number}"))
}

fn pr_checks_args(repository: &str, number: u64) -> Vec<String> {
    vec![
        "pr".to_string(),
        "checks".to_string(),
        number.to_string(),
        "--repo".to_string(),
        repository.to_string(),
        "--json".to_string(),
        "name,state,bucket,workflow,link".to_string(),
    ]
}

fn rerun_failed_check_run_args(repository: &str, run_id: u64) -> Vec<String> {
    vec![
        "run".to_string(),
        "rerun".to_string(),
        run_id.to_string(),
        "--failed".to_string(),
        "--repo".to_string(),
        repository.to_string(),
    ]
}

async fn run_gh_pr_checks_json(args: &[String]) -> Result<String> {
    debug_log_gh_request_started(args, None);
    let output = Command::new("gh")
        .env("GH_PROMPT_DISABLED", "1")
        .args(args)
        .output()
        .await
        .map_err(|error| {
            debug_log_gh_request_failed_to_start(args, None, &error);
            if error.kind() == ErrorKind::NotFound {
                anyhow!("{}", gh_missing_message(args))
            } else {
                anyhow!("failed to run gh {}: {error}", args.join(" "))
            }
        })?;
    debug_log_gh_request_finished(args, None, &output);

    if !output.status.success() && output.stdout.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let message = if stderr.is_empty() { stdout } else { stderr };
        bail!("{}", gh_failure_message(args, &message));
    }

    String::from_utf8(output.stdout).context("gh output was not UTF-8")
}

pub async fn fetch_pull_request_diff(repository: &str, number: u64) -> Result<String> {
    let path = format!("repos/{repository}/pulls/{number}");
    run_gh_json(&[
        "api".to_string(),
        "-H".to_string(),
        "Accept: application/vnd.github.v3.diff".to_string(),
        path,
    ])
    .await
    .with_context(|| format!("failed to fetch diff for {repository}#{number}"))
}

pub async fn post_issue_comment(repository: &str, number: u64, body: &str) -> Result<()> {
    let path = format!("repos/{repository}/issues/{number}/comments");
    run_gh_json(&[
        "api".to_string(),
        "-X".to_string(),
        "POST".to_string(),
        path,
        "-f".to_string(),
        format!("body={body}"),
    ])
    .await?;
    Ok(())
}

pub async fn fetch_repository_labels(repository: &str) -> Result<Vec<String>> {
    let path = format!("repos/{repository}/labels");
    let output = run_gh_json(&[
        "api".to_string(),
        "--method".to_string(),
        "GET".to_string(),
        "--paginate".to_string(),
        "--slurp".to_string(),
        path,
        "-f".to_string(),
        "per_page=100".to_string(),
    ])
    .await
    .with_context(|| format!("failed to fetch labels for {repository}"))?;
    parse_repository_labels_output(&output)
        .with_context(|| format!("failed to parse labels for {repository}"))
}

pub async fn fetch_repository_assignees(repository: &str) -> Result<Vec<String>> {
    let path = format!("repos/{repository}/assignees");
    let output = run_gh_json(&[
        "api".to_string(),
        "--method".to_string(),
        "GET".to_string(),
        "--paginate".to_string(),
        "--slurp".to_string(),
        path,
        "-f".to_string(),
        "per_page=100".to_string(),
    ])
    .await
    .with_context(|| format!("failed to fetch assignees for {repository}"))?;
    parse_repository_assignees_output(&output)
        .with_context(|| format!("failed to parse assignees for {repository}"))
}

pub async fn add_issue_label(repository: &str, number: u64, label: &str) -> Result<()> {
    let path = format!("repos/{repository}/issues/{number}/labels");
    run_gh_json(&[
        "api".to_string(),
        "-X".to_string(),
        "POST".to_string(),
        path,
        "-f".to_string(),
        format!("labels[]={label}"),
    ])
    .await
    .with_context(|| format!("failed to add label {label} to {repository}#{number}"))?;
    Ok(())
}

pub async fn remove_issue_label(repository: &str, number: u64, label: &str) -> Result<()> {
    let encoded_label = percent_encode_path_segment(label);
    let path = format!("repos/{repository}/issues/{number}/labels/{encoded_label}");
    run_gh_json(&[
        "api".to_string(),
        "-X".to_string(),
        "DELETE".to_string(),
        path,
    ])
    .await
    .with_context(|| format!("failed to remove label {label} from {repository}#{number}"))?;
    Ok(())
}

pub async fn create_issue(
    repository: &str,
    title: &str,
    body: &str,
    labels: &[String],
) -> Result<WorkItem> {
    let path = format!("repos/{repository}/issues");
    let mut args = vec![
        "api".to_string(),
        "-X".to_string(),
        "POST".to_string(),
        path,
        "-f".to_string(),
        format!("title={title}"),
        "-f".to_string(),
        format!("body={body}"),
    ];
    for label in labels {
        args.push("-f".to_string());
        args.push(format!("labels[]={label}"));
    }

    let output = run_gh_json(&args)
        .await
        .with_context(|| format!("failed to create issue in {repository}"))?;
    let raw = serde_json::from_str::<SearchApiIssueRaw>(&output)
        .with_context(|| format!("failed to parse created issue in {repository}"))?;
    Ok(search_api_item_to_work_item(SectionKind::Issues, raw))
}

pub async fn create_pull_request(
    repository: &str,
    local_dir: &Path,
    head: &str,
    title: &str,
    body: &str,
) -> Result<WorkItem> {
    let args = create_pull_request_args(repository, head, title, body);
    let output = run_gh_text_in_dir(&args, local_dir)
        .await
        .with_context(|| format!("failed to create pull request in {repository}"))?;
    let number = pull_request_number_from_create_output(&output)
        .ok_or_else(|| anyhow!("gh pr create did not return a pull request URL"))?;
    let issue_path = format!("repos/{repository}/issues/{number}");
    let issue_output = run_gh_json(&["api".to_string(), issue_path])
        .await
        .with_context(|| format!("failed to fetch created pull request {repository}#{number}"))?;
    let raw = serde_json::from_str::<SearchApiIssueRaw>(&issue_output)
        .with_context(|| format!("failed to parse created pull request {repository}#{number}"))?;
    Ok(search_api_item_to_work_item(SectionKind::PullRequests, raw))
}

fn create_pull_request_args(repository: &str, head: &str, title: &str, body: &str) -> Vec<String> {
    vec![
        "pr".to_string(),
        "create".to_string(),
        "--repo".to_string(),
        repository.to_string(),
        "--head".to_string(),
        head.to_string(),
        "--title".to_string(),
        title.to_string(),
        "--body".to_string(),
        body.to_string(),
    ]
}

fn pull_request_number_from_create_output(output: &str) -> Option<u64> {
    output.split_whitespace().find_map(|token| {
        let token = token.trim_matches(|ch: char| matches!(ch, '"' | '\'' | ')' | '(' | ',' | '.'));
        let marker = "/pull/";
        let start = token.find(marker)?.saturating_add(marker.len());
        let number = token[start..]
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect::<String>();
        (!number.is_empty())
            .then(|| number.parse::<u64>().ok())
            .flatten()
    })
}

pub async fn fetch_open_milestones(repository: &str) -> Result<Vec<Milestone>> {
    let output = run_gh_json(&open_milestones_args(repository)).await?;
    parse_open_milestones_output(&output, repository)
}

pub async fn create_milestone(repository: &str, title: &str) -> Result<Milestone> {
    let output = run_gh_json(&create_milestone_args(repository, title))
        .await
        .with_context(|| format!("failed to create milestone in {repository}"))?;
    parse_milestone_output(&output, repository)
}

pub async fn change_issue_milestone(
    repository: &str,
    issue_number: u64,
    milestone: Option<u64>,
) -> Result<()> {
    run_gh_json(&change_issue_milestone_args(
        repository,
        issue_number,
        milestone,
    ))
    .await?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PullRequestReviewCommentTarget<'a> {
    pub path: &'a str,
    pub line: usize,
    pub side: &'a str,
    pub start_line: Option<usize>,
    pub start_side: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRequestReviewEvent {
    Comment,
    RequestChanges,
    Approve,
}

impl PullRequestReviewEvent {
    pub fn as_api_value(self) -> &'static str {
        match self {
            Self::Comment => "COMMENT",
            Self::RequestChanges => "REQUEST_CHANGES",
            Self::Approve => "APPROVE",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Comment => "comment",
            Self::RequestChanges => "request changes",
            Self::Approve => "approve",
        }
    }

    pub fn success_label(self) -> &'static str {
        match self {
            Self::Comment => "review comment submitted",
            Self::RequestChanges => "changes requested",
            Self::Approve => "pull request approved",
        }
    }

    pub fn requires_body(self) -> bool {
        matches!(self, Self::Comment | Self::RequestChanges)
    }

    pub fn next(self) -> Self {
        match self {
            Self::Comment => Self::RequestChanges,
            Self::RequestChanges => Self::Approve,
            Self::Approve => Self::Comment,
        }
    }
}

#[derive(Debug, Deserialize)]
struct PullRequestReviewRaw {
    id: u64,
}

pub async fn post_pull_request_review_comment(
    repository: &str,
    number: u64,
    target: PullRequestReviewCommentTarget<'_>,
    body: &str,
) -> Result<()> {
    let pr_path = format!("repos/{repository}/pulls/{number}");
    let pr_output = run_gh_json(&["api".to_string(), pr_path]).await?;
    let pr = serde_json::from_str::<PullRequestHeadRaw>(&pr_output)
        .with_context(|| format!("failed to parse pull request head for {repository}#{number}"))?;
    let comments_path = format!("repos/{repository}/pulls/{number}/comments");
    let mut args = vec![
        "api".to_string(),
        "-X".to_string(),
        "POST".to_string(),
        comments_path,
        "-f".to_string(),
        format!("body={body}"),
        "-f".to_string(),
        format!("commit_id={}", pr.head.sha),
        "-f".to_string(),
        format!("path={}", target.path),
        "-F".to_string(),
        format!("line={}", target.line),
        "-f".to_string(),
        format!("side={}", target.side),
    ];
    if let Some(start_line) = target.start_line {
        args.push("-F".to_string());
        args.push(format!("start_line={start_line}"));
    }
    if let Some(start_side) = target.start_side {
        args.push("-f".to_string());
        args.push(format!("start_side={start_side}"));
    }

    run_gh_json(&args)
        .await
        .with_context(|| format!("failed to post review comment for {repository}#{number}"))?;
    Ok(())
}

pub async fn create_pending_pull_request_review(
    repository: &str,
    number: u64,
    body: &str,
) -> Result<u64> {
    let args = create_pull_request_review_args(repository, number, None, body);
    let output = run_gh_json(&args)
        .await
        .with_context(|| format!("failed to create pending review for {repository}#{number}"))?;
    let review = serde_json::from_str::<PullRequestReviewRaw>(&output)
        .with_context(|| format!("failed to parse pending review for {repository}#{number}"))?;
    Ok(review.id)
}

pub async fn submit_pull_request_review(
    repository: &str,
    number: u64,
    event: PullRequestReviewEvent,
    body: &str,
) -> Result<()> {
    let args = create_pull_request_review_args(repository, number, Some(event), body);
    run_gh_json(&args)
        .await
        .with_context(|| format!("failed to submit review for {repository}#{number}"))?;
    Ok(())
}

pub async fn submit_pending_pull_request_review(
    repository: &str,
    number: u64,
    review_id: u64,
    event: PullRequestReviewEvent,
    body: &str,
) -> Result<()> {
    let args = submit_pending_pull_request_review_args(repository, number, review_id, event, body);
    run_gh_json(&args)
        .await
        .with_context(|| format!("failed to submit pending review for {repository}#{number}"))?;
    Ok(())
}

pub async fn discard_pending_pull_request_review(
    repository: &str,
    number: u64,
    review_id: u64,
) -> Result<()> {
    let args = discard_pending_pull_request_review_args(repository, number, review_id);
    run_gh_json(&args)
        .await
        .with_context(|| format!("failed to discard pending review for {repository}#{number}"))?;
    Ok(())
}

fn create_pull_request_review_args(
    repository: &str,
    number: u64,
    event: Option<PullRequestReviewEvent>,
    body: &str,
) -> Vec<String> {
    let path = format!("repos/{repository}/pulls/{number}/reviews");
    let mut args = vec![
        "api".to_string(),
        "-X".to_string(),
        "POST".to_string(),
        path,
    ];
    if let Some(event) = event {
        args.push("-f".to_string());
        args.push(format!("event={}", event.as_api_value()));
    }
    push_review_body_arg(&mut args, body);
    args
}

fn submit_pending_pull_request_review_args(
    repository: &str,
    number: u64,
    review_id: u64,
    event: PullRequestReviewEvent,
    body: &str,
) -> Vec<String> {
    let path = format!("repos/{repository}/pulls/{number}/reviews/{review_id}/events");
    let mut args = vec![
        "api".to_string(),
        "-X".to_string(),
        "POST".to_string(),
        path,
        "-f".to_string(),
        format!("event={}", event.as_api_value()),
    ];
    push_review_body_arg(&mut args, body);
    args
}

fn discard_pending_pull_request_review_args(
    repository: &str,
    number: u64,
    review_id: u64,
) -> Vec<String> {
    vec![
        "api".to_string(),
        "-X".to_string(),
        "DELETE".to_string(),
        format!("repos/{repository}/pulls/{number}/reviews/{review_id}"),
    ]
}

fn push_review_body_arg(args: &mut Vec<String>, body: &str) {
    if !body.trim().is_empty() {
        args.push("-f".to_string());
        args.push(format!("body={}", body.trim()));
    }
}

pub async fn post_pull_request_review_reply(
    repository: &str,
    number: u64,
    comment_id: u64,
    body: &str,
) -> Result<()> {
    let path = format!("repos/{repository}/pulls/{number}/comments/{comment_id}/replies");
    run_gh_json(&[
        "api".to_string(),
        "-X".to_string(),
        "POST".to_string(),
        path,
        "-f".to_string(),
        format!("body={body}"),
    ])
    .await
    .with_context(|| format!("failed to reply to review comment for {repository}#{number}"))?;
    Ok(())
}

pub async fn add_issue_reaction(repository: &str, number: u64, content: &str) -> Result<()> {
    let path = format!("repos/{repository}/issues/{number}/reactions");
    post_reaction(path, content)
        .await
        .with_context(|| format!("failed to add reaction to {repository}#{number}"))
}

pub async fn add_issue_comment_reaction(
    repository: &str,
    comment_id: u64,
    content: &str,
) -> Result<()> {
    let path = format!("repos/{repository}/issues/comments/{comment_id}/reactions");
    post_reaction(path, content)
        .await
        .with_context(|| format!("failed to add reaction to issue comment {comment_id}"))
}

pub async fn add_pull_request_review_comment_reaction(
    repository: &str,
    comment_id: u64,
    content: &str,
) -> Result<()> {
    let path = format!("repos/{repository}/pulls/comments/{comment_id}/reactions");
    post_reaction(path, content)
        .await
        .with_context(|| format!("failed to add reaction to review comment {comment_id}"))
}

async fn post_reaction(path: String, content: &str) -> Result<()> {
    run_gh_json(&[
        "api".to_string(),
        "-H".to_string(),
        "Accept: application/vnd.github.squirrel-girl-preview+json".to_string(),
        "-X".to_string(),
        "POST".to_string(),
        path,
        "-f".to_string(),
        format!("content={content}"),
    ])
    .await?;
    Ok(())
}

pub async fn edit_issue_comment(repository: &str, comment_id: u64, body: &str) -> Result<()> {
    let path = format!("repos/{repository}/issues/comments/{comment_id}");
    run_gh_json(&[
        "api".to_string(),
        "-X".to_string(),
        "PATCH".to_string(),
        path,
        "-f".to_string(),
        format!("body={body}"),
    ])
    .await?;
    Ok(())
}

pub async fn edit_pull_request_review_comment(
    repository: &str,
    comment_id: u64,
    body: &str,
) -> Result<()> {
    let path = format!("repos/{repository}/pulls/comments/{comment_id}");
    run_gh_json(&[
        "api".to_string(),
        "-X".to_string(),
        "PATCH".to_string(),
        path,
        "-f".to_string(),
        format!("body={body}"),
    ])
    .await
    .with_context(|| format!("failed to edit review comment {comment_id} in {repository}"))?;
    Ok(())
}

pub async fn edit_item_metadata(
    repository: &str,
    number: u64,
    title: Option<&str>,
    body: Option<&str>,
) -> Result<ItemMetadataUpdate> {
    if title.is_none() && body.is_none() {
        bail!("no metadata field selected");
    }

    let path = format!("repos/{repository}/issues/{number}");
    let mut args = vec![
        "api".to_string(),
        "-X".to_string(),
        "PATCH".to_string(),
        path,
    ];
    if let Some(title) = title {
        args.push("-f".to_string());
        args.push(format!("title={title}"));
    }
    if let Some(body) = body {
        args.push("-f".to_string());
        args.push(format!("body={body}"));
    }

    let output = run_gh_json(&args)
        .await
        .with_context(|| format!("failed to edit item metadata for {repository}#{number}"))?;
    let raw = serde_json::from_str::<SearchApiIssueRaw>(&output)
        .with_context(|| format!("failed to parse updated item for {repository}#{number}"))?;
    Ok(ItemMetadataUpdate {
        title: raw.title,
        body: raw.body.filter(|body| !body.trim().is_empty()),
        updated_at: raw.updated_at,
    })
}

pub async fn merge_pull_request(repository: &str, number: u64, method: MergeMethod) -> Result<()> {
    ensure_pull_request_can_merge(repository, number).await?;
    run_gh_json(&merge_pull_request_args(repository, number, method)).await?;
    Ok(())
}

fn merge_pull_request_args(repository: &str, number: u64, method: MergeMethod) -> Vec<String> {
    vec![
        "pr".to_string(),
        "merge".to_string(),
        number.to_string(),
        "--repo".to_string(),
        repository.to_string(),
        method.gh_flag().to_string(),
    ]
}

async fn ensure_pull_request_can_merge(repository: &str, number: u64) -> Result<()> {
    let output = run_gh_json(&[
        "pr".to_string(),
        "view".to_string(),
        number.to_string(),
        "--repo".to_string(),
        repository.to_string(),
        "--json".to_string(),
        "state,isDraft,mergeStateStatus,reviewDecision,statusCheckRollup".to_string(),
    ])
    .await?;
    let status = serde_json::from_str::<PullRequestMergeStatusRaw>(&output)
        .with_context(|| format!("failed to parse merge status for {repository}#{number}"))?;
    if let Some(message) = pull_request_merge_blocker_message(repository, number, &status) {
        bail!("{message}");
    }
    Ok(())
}

pub async fn close_pull_request(repository: &str, number: u64) -> Result<()> {
    run_gh_json(&[
        "pr".to_string(),
        "close".to_string(),
        number.to_string(),
        "--repo".to_string(),
        repository.to_string(),
    ])
    .await?;
    Ok(())
}

pub async fn reopen_pull_request(repository: &str, number: u64) -> Result<()> {
    run_gh_json(&[
        "pr".to_string(),
        "reopen".to_string(),
        number.to_string(),
        "--repo".to_string(),
        repository.to_string(),
    ])
    .await?;
    Ok(())
}

pub async fn close_issue(repository: &str, number: u64) -> Result<()> {
    run_gh_json(&[
        "issue".to_string(),
        "close".to_string(),
        number.to_string(),
        "--repo".to_string(),
        repository.to_string(),
    ])
    .await?;
    Ok(())
}

pub async fn reopen_issue(repository: &str, number: u64) -> Result<()> {
    run_gh_json(&[
        "issue".to_string(),
        "reopen".to_string(),
        number.to_string(),
        "--repo".to_string(),
        repository.to_string(),
    ])
    .await?;
    Ok(())
}

pub async fn update_pull_request_branch(repository: &str, number: u64) -> Result<()> {
    run_gh_json(&update_pull_request_branch_args(repository, number)).await?;
    Ok(())
}

fn update_pull_request_branch_args(repository: &str, number: u64) -> Vec<String> {
    vec![
        "pr".to_string(),
        "update-branch".to_string(),
        number.to_string(),
        "--repo".to_string(),
        repository.to_string(),
    ]
}

pub async fn enable_pull_request_auto_merge(repository: &str, number: u64) -> Result<()> {
    ensure_pull_request_auto_merge_can_enable(repository, number).await?;
    let method = auto_merge_method_flag(repository).await?;
    run_gh_json(&enable_pull_request_auto_merge_args(
        repository, number, method,
    ))
    .await?;
    Ok(())
}

fn enable_pull_request_auto_merge_args(
    repository: &str,
    number: u64,
    method: &'static str,
) -> Vec<String> {
    vec![
        "pr".to_string(),
        "merge".to_string(),
        number.to_string(),
        "--repo".to_string(),
        repository.to_string(),
        method.to_string(),
        "--auto".to_string(),
    ]
}

async fn auto_merge_method_flag(repository: &str) -> Result<&'static str> {
    let output = run_gh_json(&repository_merge_methods_args(repository)).await?;
    let methods = serde_json::from_str::<RepositoryMergeMethodsRaw>(&output)
        .with_context(|| format!("failed to parse merge methods for {repository}"))?;
    repository_merge_method_flag(repository, &methods)
}

fn repository_merge_methods_args(repository: &str) -> Vec<String> {
    vec![
        "api".to_string(),
        format!("repos/{repository}"),
        "--jq".to_string(),
        "{allow_merge_commit,allow_squash_merge,allow_rebase_merge}".to_string(),
    ]
}

fn repository_merge_method_flag(
    repository: &str,
    methods: &RepositoryMergeMethodsRaw,
) -> Result<&'static str> {
    if methods.allow_merge_commit.unwrap_or(false) {
        Ok("--merge")
    } else if methods.allow_squash_merge.unwrap_or(false) {
        Ok("--squash")
    } else if methods.allow_rebase_merge.unwrap_or(false) {
        Ok("--rebase")
    } else {
        bail!(
            "cannot enable auto-merge for {repository}: repository does not allow merge, squash, or rebase merges"
        );
    }
}

pub async fn disable_pull_request_auto_merge(repository: &str, number: u64) -> Result<()> {
    ensure_pull_request_auto_merge_can_disable(repository, number).await?;
    run_gh_json(&disable_pull_request_auto_merge_args(repository, number)).await?;
    Ok(())
}

fn disable_pull_request_auto_merge_args(repository: &str, number: u64) -> Vec<String> {
    vec![
        "pr".to_string(),
        "merge".to_string(),
        number.to_string(),
        "--repo".to_string(),
        repository.to_string(),
        "--disable-auto".to_string(),
    ]
}

async fn ensure_pull_request_auto_merge_can_enable(repository: &str, number: u64) -> Result<()> {
    let status = fetch_pull_request_auto_merge_status(repository, number).await?;
    if let Some(message) = pull_request_auto_merge_enable_blocker(repository, number, &status) {
        bail!("{message}");
    }
    Ok(())
}

async fn ensure_pull_request_auto_merge_can_disable(repository: &str, number: u64) -> Result<()> {
    let status = fetch_pull_request_auto_merge_status(repository, number).await?;
    if let Some(message) = pull_request_auto_merge_disable_blocker(repository, number, &status) {
        bail!("{message}");
    }
    Ok(())
}

async fn fetch_pull_request_auto_merge_status(
    repository: &str,
    number: u64,
) -> Result<PullRequestAutoMergeStatusRaw> {
    let output = run_gh_json(&pull_request_auto_merge_status_args(repository, number)).await?;
    serde_json::from_str::<PullRequestAutoMergeStatusRaw>(&output)
        .with_context(|| format!("failed to parse auto-merge status for {repository}#{number}"))
}

fn pull_request_auto_merge_status_args(repository: &str, number: u64) -> Vec<String> {
    vec![
        "pr".to_string(),
        "view".to_string(),
        number.to_string(),
        "--repo".to_string(),
        repository.to_string(),
        "--json".to_string(),
        "state,isDraft,autoMergeRequest".to_string(),
    ]
}

fn pull_request_auto_merge_enable_blocker(
    repository: &str,
    number: u64,
    status: &PullRequestAutoMergeStatusRaw,
) -> Option<String> {
    if let Some(message) =
        pull_request_auto_merge_open_blocker(repository, number, status, "enable")
    {
        return Some(message);
    }
    if status.is_draft.unwrap_or(false) {
        return Some(format!(
            "cannot enable auto-merge for {repository}#{number}: pull request is a draft"
        ));
    }
    if status.auto_merge_request.is_some() {
        return Some(format!(
            "auto-merge is already enabled for {repository}#{number}"
        ));
    }
    None
}

fn pull_request_auto_merge_disable_blocker(
    repository: &str,
    number: u64,
    status: &PullRequestAutoMergeStatusRaw,
) -> Option<String> {
    if let Some(message) =
        pull_request_auto_merge_open_blocker(repository, number, status, "disable")
    {
        return Some(message);
    }
    if status.auto_merge_request.is_none() {
        return Some(format!(
            "auto-merge is already disabled for {repository}#{number}"
        ));
    }
    None
}

fn pull_request_auto_merge_open_blocker(
    repository: &str,
    number: u64,
    status: &PullRequestAutoMergeStatusRaw,
    action: &str,
) -> Option<String> {
    let state = status.state.as_deref().unwrap_or("UNKNOWN");
    (!state.eq_ignore_ascii_case("OPEN")).then(|| {
        format!("cannot {action} auto-merge for {repository}#{number}: pull request is {state}")
    })
}

pub async fn approve_pull_request(repository: &str, number: u64) -> Result<()> {
    submit_pull_request_review(repository, number, PullRequestReviewEvent::Approve, "").await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssigneeAction {
    Assign,
    Unassign,
}

impl AssigneeAction {
    fn method(self) -> &'static str {
        match self {
            Self::Assign => "POST",
            Self::Unassign => "DELETE",
        }
    }
}

pub async fn update_issue_assignees(
    repository: &str,
    number: u64,
    kind: ItemKind,
    action: AssigneeAction,
    assignees: &[String],
) -> Result<WorkItem> {
    if assignees.is_empty() {
        bail!("assignee login is required");
    }
    let args = assignee_api_args(repository, number, action, assignees);
    let output = run_gh_json(&args).await?;
    parse_issue_item_output(&output, repository, number, kind)
}

fn assignee_api_args(
    repository: &str,
    number: u64,
    action: AssigneeAction,
    assignees: &[String],
) -> Vec<String> {
    let path = format!("repos/{repository}/issues/{number}/assignees");
    let mut args = vec![
        "api".to_string(),
        "-X".to_string(),
        action.method().to_string(),
        path,
    ];
    for assignee in assignees {
        args.push("-f".to_string());
        args.push(format!("assignees[]={assignee}"));
    }
    args
}

pub async fn convert_pull_request_to_draft(repository: &str, number: u64) -> Result<()> {
    run_gh_json(&pull_request_ready_args(repository, number, true)).await?;
    Ok(())
}

pub async fn mark_pull_request_ready_for_review(repository: &str, number: u64) -> Result<()> {
    run_gh_json(&pull_request_ready_args(repository, number, false)).await?;
    Ok(())
}

fn pull_request_ready_args(repository: &str, number: u64, undo: bool) -> Vec<String> {
    let mut args = vec![
        "pr".to_string(),
        "ready".to_string(),
        number.to_string(),
        "--repo".to_string(),
        repository.to_string(),
    ];
    if undo {
        args.push("--undo".to_string());
    }
    args
}

fn parse_issue_item_output(
    output: &str,
    repository: &str,
    number: u64,
    kind: ItemKind,
) -> Result<WorkItem> {
    let item = serde_json::from_str::<IssueItemRaw>(output).with_context(|| {
        format!("failed to parse assignee update response for {repository}#{number}")
    })?;
    Ok(issue_api_item_to_work_item(kind, item))
}

pub async fn request_pull_request_reviewers(
    repository: &str,
    number: u64,
    reviewers: &[String],
) -> Result<()> {
    let args = pull_request_reviewers_api_args("POST", repository, number, reviewers)?;
    run_gh_json(&args)
        .await
        .with_context(|| format!("failed to request reviewers for {repository}#{number}"))?;
    Ok(())
}

pub async fn remove_pull_request_reviewers(
    repository: &str,
    number: u64,
    reviewers: &[String],
) -> Result<()> {
    let args = pull_request_reviewers_api_args("DELETE", repository, number, reviewers)?;
    run_gh_json(&args).await.with_context(|| {
        format!("failed to remove requested reviewers from {repository}#{number}")
    })?;
    Ok(())
}

fn pull_request_reviewers_api_args(
    method: &str,
    repository: &str,
    number: u64,
    reviewers: &[String],
) -> Result<Vec<String>> {
    if reviewers.is_empty() {
        bail!("at least one reviewer login is required");
    }

    // GitHub uses the same review-request endpoint for both first-time requests
    // and re-requesting review from a user who already reviewed the PR.
    let path = format!("repos/{repository}/pulls/{number}/requested_reviewers");
    let mut args = vec![
        "api".to_string(),
        "-X".to_string(),
        method.to_string(),
        path,
    ];
    for reviewer in reviewers {
        args.push("-f".to_string());
        args.push(format!("reviewers[]={reviewer}"));
    }
    Ok(args)
}

fn parse_issue_comments_output(
    output: &str,
    repository: &str,
    number: u64,
    viewer_login: Option<&str>,
) -> Result<Vec<CommentPreview>> {
    let pages = serde_json::from_str::<Vec<Vec<IssueCommentRaw>>>(output)
        .with_context(|| format!("failed to parse comments for {repository}#{number}"))?;
    let mut comments = pages
        .into_iter()
        .flatten()
        .map(|comment| {
            let author = comment
                .user
                .as_ref()
                .map(|user| user.login.as_str())
                .unwrap_or("unknown")
                .to_string();
            let is_mine = viewer_login.is_some_and(|viewer| author.eq_ignore_ascii_case(viewer));
            CommentPreview {
                id: comment.id,
                author,
                body: comment.body.unwrap_or_default(),
                created_at: comment.created_at,
                updated_at: comment.updated_at,
                url: comment.html_url,
                parent_id: None,
                is_mine,
                reactions: comment
                    .reactions
                    .map(ReactionSummary::from)
                    .unwrap_or_default(),
                review: None,
            }
        })
        .collect::<Vec<_>>();

    comments.sort_by_key(|comment| comment.created_at);
    Ok(comments)
}

fn parse_issue_details_output(output: &str, repository: &str, number: u64) -> Result<IssueDetails> {
    let issue = serde_json::from_str::<IssueDetailsRaw>(output)
        .with_context(|| format!("failed to parse issue details for {repository}#{number}"))?;
    Ok(IssueDetails {
        reactions: issue
            .reactions
            .map(ReactionSummary::from)
            .unwrap_or_default(),
        milestone: issue.milestone.map(|milestone| Milestone {
            number: milestone.number,
            title: milestone.title,
        }),
    })
}

fn parse_pull_request_review_comments_output(
    output: &str,
    repository: &str,
    number: u64,
    viewer_login: Option<&str>,
    thread_states: &HashMap<u64, ReviewThreadState>,
) -> Result<Vec<CommentPreview>> {
    let pages = serde_json::from_str::<Vec<Vec<PullRequestReviewCommentRaw>>>(output)
        .with_context(|| format!("failed to parse review comments for {repository}#{number}"))?;
    let mut comments = pages
        .into_iter()
        .flatten()
        .map(|comment| {
            let author = comment
                .user
                .as_ref()
                .map(|user| user.login.as_str())
                .unwrap_or("unknown")
                .to_string();
            let is_mine = viewer_login.is_some_and(|viewer| author.eq_ignore_ascii_case(viewer));
            let thread_state = comment
                .id
                .and_then(|id| thread_states.get(&id).copied())
                .unwrap_or_default();
            CommentPreview {
                id: comment.id,
                author,
                body: comment.body.unwrap_or_default(),
                created_at: comment.created_at,
                updated_at: comment.updated_at,
                url: comment.html_url,
                parent_id: comment.in_reply_to_id,
                is_mine,
                reactions: comment
                    .reactions
                    .map(ReactionSummary::from)
                    .unwrap_or_default(),
                review: Some(ReviewCommentPreview {
                    path: comment.path.unwrap_or_else(|| "-".to_string()),
                    line: comment.line,
                    original_line: comment.original_line,
                    start_line: comment.start_line,
                    original_start_line: comment.original_start_line,
                    side: comment.side,
                    start_side: comment.start_side,
                    diff_hunk: comment.diff_hunk,
                    is_resolved: thread_state.is_resolved,
                    is_outdated: thread_state.is_outdated,
                }),
            }
        })
        .collect::<Vec<_>>();

    comments.sort_by_key(|comment| comment.created_at);
    Ok(comments)
}

fn parse_pull_request_review_thread_states_page(
    output: &str,
    repository: &str,
    number: u64,
) -> Result<(HashMap<u64, ReviewThreadState>, bool, Option<String>)> {
    let raw = serde_json::from_str::<PullRequestReviewThreadsGraphQlRaw>(output)
        .with_context(|| format!("failed to parse review threads for {repository}#{number}"))?;
    let threads = raw
        .data
        .repository
        .and_then(|repository| repository.pull_request)
        .ok_or_else(|| anyhow!("pull request {repository}#{number} was not found"))?
        .review_threads;
    let mut states = HashMap::new();
    for thread in threads.nodes.unwrap_or_default() {
        let state = ReviewThreadState {
            is_resolved: thread.is_resolved,
            is_outdated: thread.is_outdated,
        };
        for comment in thread.comments.nodes.unwrap_or_default() {
            if let Some(id) = comment.database_id {
                states.insert(id, state);
            }
        }
    }

    Ok((
        states,
        threads.page_info.has_next_page,
        threads.page_info.end_cursor,
    ))
}

async fn refresh_notification_sections(config: &Config) -> Vec<SectionSnapshot> {
    let limit = notification_fetch_limit(config);
    let include_all = should_fetch_all_notifications(config);
    let fetched = fetch_notifications(limit, include_all).await;

    config
        .notification_sections
        .iter()
        .map(|section| {
            let mut snapshot = SectionSnapshot::empty(
                SectionKind::Notifications,
                section.title.clone(),
                section.filters.clone(),
            );

            match &fetched {
                Ok(notifications) => {
                    let section_limit = section.limit.unwrap_or(config.defaults.notification_limit);
                    snapshot.items = notifications
                        .iter()
                        .filter(|notification| {
                            !is_excluded_repo(
                                &notification.repository.full_name,
                                &config.exclude_repos,
                            )
                        })
                        .filter(|notification| {
                            notification_matches(
                                notification,
                                &snapshot.filters,
                                config.defaults.include_read_notifications,
                            )
                        })
                        .take(section_limit)
                        .map(notification_to_work_item)
                        .collect();
                    snapshot.refreshed_at = Some(Utc::now());
                }
                Err(error) => {
                    snapshot.error = Some(error.to_string());
                }
            }

            snapshot
        })
        .collect()
}

async fn fetch_notifications(limit: usize, include_all: bool) -> Result<Vec<NotificationRaw>> {
    let path = if include_all {
        format!("notifications?per_page={limit}&all=true")
    } else {
        format!("notifications?per_page={limit}")
    };

    let output = run_gh_json(&["api".to_string(), path]).await?;
    serde_json::from_str(&output).context("failed to parse gh notifications output")
}

pub async fn mark_notification_thread_read(thread_id: &str) -> Result<()> {
    let path = format!("notifications/threads/{thread_id}");
    run_gh_json(&[
        "api".to_string(),
        "-X".to_string(),
        "PATCH".to_string(),
        path,
    ])
    .await
    .with_context(|| format!("failed to mark notification {thread_id} as read"))?;
    Ok(())
}

pub async fn mark_all_notifications_read() -> Result<()> {
    run_gh_json(&[
        "api".to_string(),
        "-X".to_string(),
        "PUT".to_string(),
        "notifications".to_string(),
        "-F".to_string(),
        "read=true".to_string(),
    ])
    .await
    .context("failed to mark all notifications as read")?;
    Ok(())
}

pub async fn mute_notification_thread(thread_id: &str) -> Result<()> {
    set_notification_thread_subscription(thread_id, true).await
}

pub async fn subscribe_notification_thread(thread_id: &str) -> Result<()> {
    set_notification_thread_subscription(thread_id, false).await
}

pub async fn unsubscribe_notification_thread(thread_id: &str) -> Result<()> {
    let path = format!("notifications/threads/{thread_id}/subscription");
    run_gh_json(&[
        "api".to_string(),
        "-X".to_string(),
        "DELETE".to_string(),
        path,
    ])
    .await
    .with_context(|| format!("failed to unsubscribe from notification {thread_id}"))?;
    Ok(())
}

async fn set_notification_thread_subscription(thread_id: &str, ignored: bool) -> Result<()> {
    let path = format!("notifications/threads/{thread_id}/subscription");
    run_gh_json(&[
        "api".to_string(),
        "-X".to_string(),
        "PUT".to_string(),
        path,
        "-F".to_string(),
        format!("ignored={ignored}"),
    ])
    .await
    .with_context(|| {
        if ignored {
            format!("failed to mute notification {thread_id}")
        } else {
            format!("failed to subscribe to notification {thread_id}")
        }
    })?;
    Ok(())
}

async fn fetch_viewer_login() -> Result<String> {
    let output = run_gh_json(&["api".to_string(), "user".to_string()]).await?;
    let viewer =
        serde_json::from_str::<ViewerRaw>(&output).context("failed to parse gh user output")?;
    Ok(viewer.login)
}

async fn cached_viewer_login() -> Result<String> {
    if let Some(login) = VIEWER_LOGIN.get() {
        return Ok(login.clone());
    }

    let login = fetch_viewer_login().await?;
    let _ = VIEWER_LOGIN.set(login.clone());
    Ok(login)
}

fn resolve_me_section(mut section: SearchSection, viewer_login: Option<&str>) -> SearchSection {
    if let Some(viewer_login) = viewer_login {
        section.filters = section.filters.replace("@me", viewer_login);
        section.queries = section
            .queries
            .into_iter()
            .map(|query| query.replace("@me", viewer_login))
            .collect();
    }
    section
}

fn notification_fetch_limit(config: &Config) -> usize {
    let requested = config
        .notification_sections
        .iter()
        .map(|section| section.limit.unwrap_or(config.defaults.notification_limit))
        .max()
        .unwrap_or(config.defaults.notification_limit);
    requested.saturating_mul(3).clamp(1, 100)
}

fn should_fetch_all_notifications(config: &Config) -> bool {
    config.defaults.include_read_notifications
        || config.notification_sections.iter().any(|section| {
            section
                .filters
                .split_whitespace()
                .any(|token| matches!(token, "is:read" | "is:all"))
        })
}

async fn run_gh_json(args: &[String]) -> Result<String> {
    match current_gh_request_priority() {
        GhRequestPriority::User => {
            let _guard = UserGhRequestGuard::new();
            run_gh_json_raw(args).await
        }
        GhRequestPriority::Background => {
            wait_for_user_gh_requests().await;
            run_gh_json_raw(args).await
        }
    }
}

async fn run_gh_json_raw(args: &[String]) -> Result<String> {
    debug_log_gh_request_started(args, None);
    let output = Command::new("gh")
        .env("GH_PROMPT_DISABLED", "1")
        .args(args)
        .output()
        .await
        .map_err(|error| {
            debug_log_gh_request_failed_to_start(args, None, &error);
            if error.kind() == ErrorKind::NotFound {
                anyhow!("{}", gh_missing_message(args))
            } else {
                anyhow!("failed to run gh {}: {error}", args.join(" "))
            }
        })?;
    debug_log_gh_request_finished(args, None, &output);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let message = if stderr.is_empty() { stdout } else { stderr };
        bail!("{}", gh_failure_message(args, &message));
    }

    String::from_utf8(output.stdout).context("gh output was not UTF-8")
}

async fn run_gh_text_in_dir(args: &[String], directory: &Path) -> Result<String> {
    let _guard = UserGhRequestGuard::new();
    debug_log_gh_request_started(args, Some(directory));
    let output = Command::new("gh")
        .env("GH_PROMPT_DISABLED", "1")
        .current_dir(directory)
        .args(args)
        .output()
        .await
        .map_err(|error| {
            debug_log_gh_request_failed_to_start(args, Some(directory), &error);
            if error.kind() == ErrorKind::NotFound {
                anyhow!("{}", gh_missing_message(args))
            } else {
                anyhow!(
                    "failed to run gh {} in {}: {error}",
                    args.join(" "),
                    directory.display()
                )
            }
        })?;
    debug_log_gh_request_finished(args, Some(directory), &output);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let message = if stderr.is_empty() { stdout } else { stderr };
        bail!("{}", gh_failure_message(args, &message));
    }

    String::from_utf8(output.stdout).context("gh output was not UTF-8")
}

fn gh_missing_message(args: &[String]) -> String {
    format!(
        "GitHub CLI `gh` is required but was not found. Install it for your OS from https://cli.github.com/: macOS `brew install gh`, Fedora `sudo dnf install gh`, Arch `sudo pacman -S github-cli`, Debian/Ubuntu official apt setup at https://github.com/cli/cli/blob/trunk/docs/install_linux.md. Then run `gh auth login`. Tried: gh {}",
        args.join(" ")
    )
}

fn gh_failure_message(args: &[String], message: &str) -> String {
    if is_gh_auth_error(message) {
        return format!(
            "GitHub CLI is installed but not authenticated. Run `gh auth login`, then restart ghr. Original error from `gh {}`: {}",
            args.join(" "),
            message
        );
    }

    format!("gh {} failed: {message}", args.join(" "))
}

fn pull_request_merge_blocker_message(
    repository: &str,
    number: u64,
    status: &PullRequestMergeStatusRaw,
) -> Option<String> {
    let mut blockers = Vec::new();
    let state = status.state.as_deref().unwrap_or("UNKNOWN");
    if !state.eq_ignore_ascii_case("OPEN") {
        push_unique_blocker(&mut blockers, format!("pull request is {state}"));
    }

    if status.is_draft.unwrap_or(false) {
        push_unique_blocker(&mut blockers, "pull request is draft".to_string());
    }

    match status.review_decision.as_deref() {
        Some("REVIEW_REQUIRED") => {
            push_unique_blocker(&mut blockers, "review approval required".to_string())
        }
        Some("CHANGES_REQUESTED") => {
            push_unique_blocker(&mut blockers, "changes requested".to_string())
        }
        _ => {}
    }

    let merge_state = status.merge_state_status.as_deref().unwrap_or("UNKNOWN");
    match merge_state {
        "CLEAN" | "HAS_HOOKS" | "UNSTABLE" => {}
        "BEHIND" => push_unique_blocker(&mut blockers, "branch must be updated".to_string()),
        "BLOCKED" => push_unique_blocker(
            &mut blockers,
            "branch protection is blocking merge".to_string(),
        ),
        "DIRTY" => push_unique_blocker(
            &mut blockers,
            "merge conflicts must be resolved".to_string(),
        ),
        "DRAFT" => push_unique_blocker(&mut blockers, "pull request is draft".to_string()),
        "UNKNOWN" => push_unique_blocker(
            &mut blockers,
            "GitHub is still computing mergeability".to_string(),
        ),
        other => push_unique_blocker(&mut blockers, format!("merge state is {other}")),
    }

    if blockers.is_empty() {
        return None;
    }

    let (failing, pending) = check_rollup_counts(status);
    if failing > 0 {
        push_unique_blocker(&mut blockers, format!("{failing} check(s) failing"));
    }
    if pending > 0 {
        push_unique_blocker(&mut blockers, format!("{pending} check(s) pending"));
    }

    Some(format!(
        "merge blocked for {repository}#{number}: {}. Resolve the blockers, then press M again. To queue it after requirements pass, use `gh pr merge {number} --repo {repository} --merge --auto`.",
        blockers.join("; ")
    ))
}

fn check_rollup_counts(status: &PullRequestMergeStatusRaw) -> (usize, usize) {
    let mut failing = 0;
    let mut pending = 0;
    for check in status.status_check_rollup.as_deref().unwrap_or(&[]) {
        match check.conclusion.as_deref() {
            Some("FAILURE" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED" | "STARTUP_FAILURE") => {
                failing += 1
            }
            Some(_) => {}
            None => {
                if !matches!(check.status.as_deref(), Some("COMPLETED")) {
                    pending += 1;
                }
            }
        }
    }
    (failing, pending)
}

fn push_unique_blocker(blockers: &mut Vec<String>, blocker: String) {
    if !blockers.iter().any(|existing| existing == &blocker) {
        blockers.push(blocker);
    }
}

fn pull_request_action_hints(pr: &PullRequestActionRaw) -> ActionHints {
    let mut labels = Vec::new();
    let blockers = pull_request_action_blockers(pr);
    let checks = pr
        .status_check_rollup
        .as_ref()
        .map(check_summary_from_rollup);
    let failed_check_runs = failed_check_runs_from_rollup(pr.status_check_rollup.as_ref());
    let can_update = pr.viewer_can_update.unwrap_or(false);
    let can_auto_merge = pr.viewer_can_enable_auto_merge.unwrap_or(false);
    let can_admin_merge = pr.viewer_can_merge_as_admin.unwrap_or(false);
    let can_merge = can_update || can_auto_merge || can_admin_merge;
    let open = pr
        .state
        .as_deref()
        .is_none_or(|state| state.eq_ignore_ascii_case("OPEN"));
    let draft = pr.is_draft.unwrap_or(false);
    let auto_merge_enabled = pr.auto_merge_request.is_some();
    let did_author = pr.viewer_did_author.unwrap_or(false);
    let latest_review = pr
        .viewer_latest_review
        .as_ref()
        .and_then(|review| review.state.as_deref());

    if open && auto_merge_enabled {
        labels.push("Auto-merge on".to_string());
    }

    if open
        && !draft
        && !did_author
        && can_update
        && latest_review != Some("APPROVED")
        && !matches!(pr.review_decision.as_deref(), Some("APPROVED"))
    {
        labels.push("Approvable".to_string());
    }

    if blockers.is_empty() && can_merge {
        labels.push("Mergeable".to_string());
    } else if open && !draft && can_auto_merge && !auto_merge_enabled {
        labels.push("Auto-mergeable".to_string());
    }

    if open && pr.viewer_can_update_branch.unwrap_or(false) && merge_state(pr) == "BEHIND" {
        labels.push("Update branch".to_string());
    }

    let note = if blockers.is_empty() {
        None
    } else {
        Some(format!("Merge blocked: {}", blockers.join("; ")))
    };
    let head = pull_request_branch(pr);

    ActionHints {
        labels,
        checks,
        commits: pr.commits.as_ref().map(|commits| commits.total_count),
        failed_check_runs,
        note,
        head,
    }
}

fn pull_request_branch(pr: &PullRequestActionRaw) -> Option<PullRequestBranch> {
    let branch = pr
        .head_ref_name
        .as_deref()
        .map(str::trim)
        .filter(|branch| !branch.is_empty())?;
    let repository = pr
        .head_repository
        .as_ref()
        .map(|repository| repository.name_with_owner.trim())
        .filter(|repository| !repository.is_empty())?;

    Some(PullRequestBranch {
        repository: repository.to_string(),
        branch: branch.to_string(),
    })
}

fn pull_request_action_blockers(pr: &PullRequestActionRaw) -> Vec<String> {
    let mut blockers = Vec::new();
    let state = pr.state.as_deref().unwrap_or("UNKNOWN");
    if !state.eq_ignore_ascii_case("OPEN") {
        push_unique_blocker(&mut blockers, format!("pull request is {state}"));
    }

    if pr.is_draft.unwrap_or(false) {
        push_unique_blocker(&mut blockers, "draft".to_string());
    }

    match pr.review_decision.as_deref() {
        Some("REVIEW_REQUIRED") => {
            push_unique_blocker(&mut blockers, "review approval required".to_string())
        }
        Some("CHANGES_REQUESTED") => {
            push_unique_blocker(&mut blockers, "changes requested".to_string())
        }
        _ => {}
    }

    match (merge_state(pr), pr.mergeable.as_deref()) {
        (_, Some("CONFLICTING")) => push_unique_blocker(
            &mut blockers,
            "merge conflicts must be resolved".to_string(),
        ),
        ("CLEAN" | "HAS_HOOKS" | "UNSTABLE", _) => {}
        ("BEHIND", _) => push_unique_blocker(&mut blockers, "branch must be updated".to_string()),
        ("BLOCKED", _) => push_unique_blocker(
            &mut blockers,
            "branch protection is blocking merge".to_string(),
        ),
        ("DIRTY", _) => push_unique_blocker(
            &mut blockers,
            "merge conflicts must be resolved".to_string(),
        ),
        ("DRAFT", _) => push_unique_blocker(&mut blockers, "draft".to_string()),
        ("UNKNOWN", _) => push_unique_blocker(
            &mut blockers,
            "GitHub is still computing mergeability".to_string(),
        ),
        (other, _) => push_unique_blocker(&mut blockers, format!("merge state is {other}")),
    }

    match pr
        .status_check_rollup
        .as_ref()
        .and_then(|rollup| rollup.state.as_deref())
    {
        Some("SUCCESS") | None => {}
        Some("PENDING" | "EXPECTED") => {
            push_unique_blocker(&mut blockers, "checks pending".to_string())
        }
        Some("FAILURE" | "ERROR") => {
            push_unique_blocker(&mut blockers, "checks failing".to_string())
        }
        Some(other) => push_unique_blocker(&mut blockers, format!("checks are {other}")),
    }

    blockers
}

fn merge_state(pr: &PullRequestActionRaw) -> &str {
    pr.merge_state_status.as_deref().unwrap_or("UNKNOWN")
}

fn check_summary_from_rollup(rollup: &PullRequestStatusRollupRaw) -> CheckSummary {
    let mut summary = CheckSummary::default();
    let Some(contexts) = &rollup.contexts else {
        return summary;
    };

    let nodes = contexts.nodes.as_deref().unwrap_or(&[]);
    for node in nodes {
        match node {
            PullRequestCheckContextRaw::CheckRun {
                conclusion, status, ..
            } => add_check_run_to_summary(&mut summary, conclusion.as_deref(), status.as_deref()),
            PullRequestCheckContextRaw::StatusContext { state } => {
                add_status_context_to_summary(&mut summary, state.as_deref())
            }
            PullRequestCheckContextRaw::Other => {}
        }
    }
    let counted = summary.passed + summary.failed + summary.pending + summary.skipped;
    summary.total = contexts.total_count.max(counted);
    summary.incomplete = summary.total > counted;
    summary
}

fn add_check_run_to_summary(
    summary: &mut CheckSummary,
    conclusion: Option<&str>,
    status: Option<&str>,
) {
    match conclusion {
        Some("SUCCESS") => summary.passed += 1,
        Some("FAILURE" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED" | "STARTUP_FAILURE") => {
            summary.failed += 1
        }
        Some("SKIPPED" | "NEUTRAL") => summary.skipped += 1,
        Some(_) => summary.pending += 1,
        None if matches!(status, Some("COMPLETED")) => summary.pending += 1,
        None => summary.pending += 1,
    }
}

fn add_status_context_to_summary(summary: &mut CheckSummary, state: Option<&str>) {
    match state {
        Some("SUCCESS") => summary.passed += 1,
        Some("FAILURE" | "ERROR") => summary.failed += 1,
        Some("PENDING" | "EXPECTED") | None => summary.pending += 1,
        Some(_) => summary.pending += 1,
    }
}

fn failed_check_runs_from_rollup(
    rollup: Option<&PullRequestStatusRollupRaw>,
) -> Vec<FailedCheckRunSummary> {
    let mut runs = Vec::new();
    let Some(contexts) = rollup.and_then(|rollup| rollup.contexts.as_ref()) else {
        return runs;
    };

    for node in contexts.nodes.as_deref().unwrap_or(&[]) {
        let PullRequestCheckContextRaw::CheckRun {
            conclusion,
            details_url,
            name,
            status,
        } = node
        else {
            continue;
        };
        if !check_run_failed(conclusion.as_deref(), status.as_deref()) {
            continue;
        }
        let Some(run_id) = details_url.as_deref().and_then(actions_run_id_from_url) else {
            continue;
        };
        add_failed_check_run(
            &mut runs,
            run_id,
            None,
            name.clone().unwrap_or_else(|| "unknown check".to_string()),
        );
    }

    runs
}

fn failed_check_runs_from_pr_checks_json(output: &str) -> Result<FailedCheckRunDiscovery> {
    let checks = serde_json::from_str::<Vec<PullRequestCheckStatusRaw>>(output)?;
    let mut discovery = FailedCheckRunDiscovery::default();

    for check in checks {
        if !pr_check_failed(&check) {
            continue;
        }
        let name = check.name.unwrap_or_else(|| "unknown check".to_string());
        let Some(run_id) = check.link.as_deref().and_then(actions_run_id_from_url) else {
            push_unique_string(&mut discovery.unmapped_failed_checks, name);
            continue;
        };
        add_failed_check_run(&mut discovery.runs, run_id, check.workflow, name);
    }

    Ok(discovery)
}

fn pr_check_failed(check: &PullRequestCheckStatusRaw) -> bool {
    matches_normalized(check.bucket.as_deref(), &["fail", "cancel"])
        || matches_normalized(
            check.state.as_deref(),
            &[
                "fail",
                "failure",
                "error",
                "cancel",
                "cancelled",
                "canceled",
                "timed_out",
                "action_required",
                "startup_failure",
            ],
        )
}

fn check_run_failed(conclusion: Option<&str>, _status: Option<&str>) -> bool {
    matches_normalized(
        conclusion,
        &[
            "failure",
            "error",
            "cancelled",
            "canceled",
            "timed_out",
            "action_required",
            "startup_failure",
        ],
    )
}

fn matches_normalized(value: Option<&str>, expected: &[&str]) -> bool {
    let Some(value) = value else {
        return false;
    };
    expected
        .iter()
        .any(|expected| value.eq_ignore_ascii_case(expected))
}

fn actions_run_id_from_url(url: &str) -> Option<u64> {
    let rest = url.split_once("/actions/runs/")?.1;
    let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn add_failed_check_run(
    runs: &mut Vec<FailedCheckRunSummary>,
    run_id: u64,
    workflow: Option<String>,
    check: String,
) {
    if let Some(existing) = runs.iter_mut().find(|existing| existing.run_id == run_id) {
        if existing.workflow.is_none() {
            existing.workflow = workflow;
        }
        push_unique_string(&mut existing.checks, check);
        return;
    }

    runs.push(FailedCheckRunSummary {
        run_id,
        workflow,
        checks: vec![check],
    });
}

fn push_unique_string(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn split_repository(repository: &str) -> Result<(&str, &str)> {
    let Some((owner, name)) = repository.split_once('/') else {
        bail!("repository should be in owner/name form: {repository}");
    };
    if owner.trim().is_empty() || name.trim().is_empty() || name.contains('/') {
        bail!("repository should be in owner/name form: {repository}");
    }
    Ok((owner, name))
}

fn is_gh_auth_error(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    [
        "gh auth login",
        "not authenticated",
        "not logged in",
        "authentication required",
        "requires authentication",
        "must authenticate",
        "bad credentials",
        "no oauth token",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

fn search_item_to_work_item(kind: SectionKind, item: SearchItemRaw) -> WorkItem {
    let item_kind = match kind {
        SectionKind::PullRequests => ItemKind::PullRequest,
        SectionKind::Issues => ItemKind::Issue,
        SectionKind::Notifications => ItemKind::Notification,
    };
    let repo = item.repository.name_with_owner;
    let labels = item
        .labels
        .unwrap_or_default()
        .into_iter()
        .map(|label| label.name)
        .collect::<Vec<_>>();
    let assignees = assignees_from_raw(item.assignees);

    WorkItem {
        id: format!("{repo}#{}", item.number),
        kind: item_kind,
        repo,
        number: Some(item.number),
        title: item.title,
        body: item.body.filter(|body| !body.trim().is_empty()),
        author: item.author.map(|author| author.login),
        state: item.state,
        url: item.url,
        created_at: item.created_at,
        updated_at: item.updated_at,
        last_read_at: None,
        labels,
        reactions: ReactionSummary::default(),
        milestone: None,
        assignees,
        comments: item.comments_count,
        unread: None,
        reason: None,
        extra: item
            .is_draft
            .filter(|is_draft| *is_draft)
            .map(|_| "draft".to_string()),
    }
}

fn search_api_item_to_work_item(kind: SectionKind, item: SearchApiIssueRaw) -> WorkItem {
    let item_kind = match kind {
        SectionKind::PullRequests => ItemKind::PullRequest,
        SectionKind::Issues => ItemKind::Issue,
        SectionKind::Notifications => ItemKind::Notification,
    };
    let repo = repo_from_repository_url(&item.repository_url);
    let labels = item
        .labels
        .unwrap_or_default()
        .into_iter()
        .map(|label| label.name)
        .collect::<Vec<_>>();
    let assignees = assignees_from_raw(item.assignees);

    WorkItem {
        id: format!("{repo}#{}", item.number),
        kind: item_kind,
        repo,
        number: Some(item.number),
        title: item.title,
        body: item.body.filter(|body| !body.trim().is_empty()),
        author: item.user.map(|author| author.login),
        state: item.state,
        url: item.html_url,
        created_at: item.created_at,
        updated_at: item.updated_at,
        last_read_at: None,
        labels,
        reactions: item
            .reactions
            .map(ReactionSummary::from)
            .unwrap_or_default(),
        milestone: item.milestone.map(|milestone| Milestone {
            number: milestone.number,
            title: milestone.title,
        }),
        assignees,
        comments: item.comments,
        unread: None,
        reason: None,
        extra: item
            .draft
            .filter(|is_draft| *is_draft)
            .map(|_| "draft".to_string()),
    }
}

fn open_milestones_args(repository: &str) -> Vec<String> {
    vec![
        "api".to_string(),
        "--paginate".to_string(),
        "--slurp".to_string(),
        format!("repos/{repository}/milestones?state=open&per_page=100"),
    ]
}

fn create_milestone_args(repository: &str, title: &str) -> Vec<String> {
    vec![
        "api".to_string(),
        "-X".to_string(),
        "POST".to_string(),
        format!("repos/{repository}/milestones"),
        "-F".to_string(),
        format!("title={}", title.trim()),
    ]
}

fn change_issue_milestone_args(
    repository: &str,
    issue_number: u64,
    milestone: Option<u64>,
) -> Vec<String> {
    let mut args = vec![
        "api".to_string(),
        "-X".to_string(),
        "PATCH".to_string(),
        format!("repos/{repository}/issues/{issue_number}"),
        "-F".to_string(),
    ];
    args.push(match milestone {
        Some(number) => format!("milestone={number}"),
        None => "milestone=null".to_string(),
    });
    args
}

fn parse_open_milestones_output(output: &str, repository: &str) -> Result<Vec<Milestone>> {
    let pages = serde_json::from_str::<Vec<Vec<MilestoneRaw>>>(output)
        .with_context(|| format!("failed to parse milestones for {repository}"))?;
    Ok(pages
        .into_iter()
        .flatten()
        .map(|milestone| Milestone {
            number: milestone.number,
            title: milestone.title,
        })
        .collect())
}

fn parse_milestone_output(output: &str, repository: &str) -> Result<Milestone> {
    let milestone = serde_json::from_str::<MilestoneRaw>(output)
        .with_context(|| format!("failed to parse milestone for {repository}"))?;
    Ok(Milestone {
        number: milestone.number,
        title: milestone.title,
    })
}

fn issue_api_item_to_work_item(kind: ItemKind, item: IssueItemRaw) -> WorkItem {
    let repo = repo_from_repository_url(&item.repository_url);
    let labels = item
        .labels
        .unwrap_or_default()
        .into_iter()
        .map(|label| label.name)
        .collect::<Vec<_>>();
    let assignees = assignees_from_raw(item.assignees);

    WorkItem {
        id: format!("{repo}#{}", item.number),
        kind,
        repo,
        number: Some(item.number),
        title: item.title,
        body: item.body.filter(|body| !body.trim().is_empty()),
        author: item.user.map(|author| author.login),
        state: item.state,
        url: item.html_url,
        created_at: None,
        updated_at: item.updated_at,
        last_read_at: None,
        labels,
        reactions: ReactionSummary::default(),
        milestone: None,
        assignees,
        comments: item.comments,
        unread: None,
        reason: None,
        extra: None,
    }
}

fn assignees_from_raw(assignees: Option<Vec<SearchAuthorRaw>>) -> Vec<String> {
    assignees
        .unwrap_or_default()
        .into_iter()
        .map(|assignee| assignee.login)
        .collect()
}

fn repo_from_repository_url(url: &str) -> String {
    url.split("/repos/")
        .nth(1)
        .unwrap_or(url)
        .trim_matches('/')
        .to_string()
}

fn parse_repository_labels_output(output: &str) -> Result<Vec<String>> {
    let pages = serde_json::from_str::<Vec<Vec<RepositoryLabelRaw>>>(output)
        .context("failed to parse paginated repository labels")?;
    let mut labels = pages
        .into_iter()
        .flatten()
        .map(|label| label.name.trim().to_string())
        .filter(|label| !label.is_empty())
        .collect::<Vec<_>>();
    labels.sort_by_key(|label| label.to_ascii_lowercase());
    labels.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    Ok(labels)
}

fn parse_repository_assignees_output(output: &str) -> Result<Vec<String>> {
    let pages = serde_json::from_str::<Vec<Vec<RepositoryAssigneeRaw>>>(output)
        .context("failed to parse paginated repository assignees")?;
    let mut assignees = pages
        .into_iter()
        .flatten()
        .map(|assignee| assignee.login.trim().to_string())
        .filter(|assignee| !assignee.is_empty())
        .collect::<Vec<_>>();
    assignees.sort_by_key(|assignee| assignee.to_ascii_lowercase());
    assignees.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    Ok(assignees)
}

fn percent_encode_path_segment(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn notification_to_work_item(notification: &NotificationRaw) -> WorkItem {
    let number = notification
        .subject
        .url
        .as_deref()
        .and_then(extract_trailing_number);
    let url = notification_url(notification, number);
    let kind = notification_item_kind(&notification.subject.subject_type);

    WorkItem {
        id: notification.id.clone(),
        kind,
        repo: notification.repository.full_name.clone(),
        number,
        title: notification.subject.title.clone(),
        body: None,
        author: None,
        state: None,
        url,
        created_at: None,
        updated_at: notification.updated_at,
        last_read_at: notification.last_read_at,
        labels: Vec::new(),
        reactions: ReactionSummary::default(),
        milestone: None,
        assignees: Vec::new(),
        comments: None,
        unread: Some(notification.unread),
        reason: Some(normalize_reason_for_display(&notification.reason)),
        extra: Some(notification.subject.subject_type.clone()),
    }
}

fn notification_item_kind(subject_type: &str) -> ItemKind {
    match subject_type {
        "PullRequest" => ItemKind::PullRequest,
        "Issue" => ItemKind::Issue,
        _ => ItemKind::Notification,
    }
}

fn notification_url(notification: &NotificationRaw, number: Option<u64>) -> String {
    match (notification.subject.subject_type.as_str(), number) {
        ("PullRequest", Some(number)) => {
            format!(
                "https://github.com/{}/pull/{number}",
                notification.repository.full_name
            )
        }
        ("Issue", Some(number)) => {
            format!(
                "https://github.com/{}/issues/{number}",
                notification.repository.full_name
            )
        }
        _ => notification.repository.html_url.clone(),
    }
}

fn notification_matches(
    notification: &NotificationRaw,
    filters: &str,
    include_read_default: bool,
) -> bool {
    let mut read_filter = None;
    let mut reasons = Vec::new();
    let mut repos = Vec::new();

    for token in filters.split_whitespace() {
        if let Some(value) = token.strip_prefix("is:") {
            read_filter = Some(value);
        } else if let Some(value) = token.strip_prefix("reason:") {
            reasons.push(value);
        } else if let Some(value) = token.strip_prefix("repo:") {
            repos.push(value);
        }
    }

    match read_filter {
        Some("unread") if !notification.unread => return false,
        Some("read") if notification.unread => return false,
        Some("all") | None => {}
        Some(_) => {}
    }

    if read_filter.is_none() && !include_read_default && !notification.unread {
        return false;
    }

    if !reasons.is_empty()
        && !reasons
            .iter()
            .any(|reason| reason_matches(&notification.reason, reason))
    {
        return false;
    }

    if !repos.is_empty()
        && !repos
            .iter()
            .any(|repo| repo.eq_ignore_ascii_case(&notification.repository.full_name))
    {
        return false;
    }

    true
}

fn reason_matches(raw: &str, filter: &str) -> bool {
    if filter == "participating" {
        return matches!(
            raw,
            "author" | "comment" | "mention" | "review_requested" | "assign" | "state_change"
        );
    }

    raw == filter.replace('-', "_")
}

fn normalize_reason_for_display(reason: &str) -> String {
    reason.replace('_', "-")
}

fn extract_trailing_number(value: &str) -> Option<u64> {
    value.rsplit('/').next()?.parse().ok()
}

fn is_excluded_repo(repo: &str, patterns: &[String]) -> bool {
    patterns
        .iter()
        .any(|pattern| wildcard_match(&pattern.to_lowercase(), &repo.to_lowercase()))
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == value;
    }

    let mut remainder = value;
    let mut parts = pattern.split('*').peekable();
    let starts_with_wildcard = pattern.starts_with('*');
    let ends_with_wildcard = pattern.ends_with('*');

    if let Some(first) = parts.next() {
        if !first.is_empty() {
            if !remainder.starts_with(first) {
                return false;
            }
            remainder = &remainder[first.len()..];
        } else if !starts_with_wildcard {
            return false;
        }
    }

    let mut last_part = "";
    for part in parts {
        if part.is_empty() {
            continue;
        }
        last_part = part;
        let Some(index) = remainder.find(part) else {
            return false;
        };
        remainder = &remainder[index + part.len()..];
    }

    ends_with_wildcard || last_part.is_empty() || remainder.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gh_request_priority_defaults_to_user() {
        assert_eq!(current_gh_request_priority(), GhRequestPriority::User);
    }

    #[tokio::test]
    async fn background_github_priority_marks_current_task() {
        let priority =
            with_background_github_priority(async { current_gh_request_priority() }).await;

        assert_eq!(priority, GhRequestPriority::Background);
        assert_eq!(current_gh_request_priority(), GhRequestPriority::User);
    }

    #[test]
    fn wildcard_excludes_repositories() {
        assert!(wildcard_match("nervosnetwork/*", "nervosnetwork/ckb"));
        assert!(wildcard_match("*/sandbox", "me/sandbox"));
        assert!(wildcard_match("*play*", "me/playground"));
        assert!(!wildcard_match("nervosnetwork/*", "rust-lang/rust"));
    }

    #[test]
    fn search_args_put_flags_before_query_separator() {
        let args = search_args(
            "prs",
            "number,title",
            "reviewed-by:chenyukang -author:chenyukang sort:created-desc",
            50,
        );

        assert_eq!(
            args,
            vec![
                "search",
                "prs",
                "--json",
                "number,title",
                "--limit",
                "50",
                "--",
                "reviewed-by:chenyukang",
                "-author:chenyukang",
                "sort:created-desc"
            ]
        );
    }

    #[test]
    fn search_args_cap_limit_to_github_search_result_window() {
        let args = search_args("issues", "number,title", "repo:rust-lang/rust is:open", 500);

        assert_eq!(
            args,
            vec![
                "search",
                "issues",
                "--json",
                "number,title",
                "--limit",
                "500",
                "--",
                "repo:rust-lang/rust",
                "is:open"
            ]
        );
        assert_eq!(search_command_limit(0), 1);
        assert_eq!(search_command_limit(50), 50);
        assert_eq!(search_command_limit(101), 101);
        assert_eq!(search_command_limit(5_000), 1_000);
    }

    #[test]
    fn pull_request_ready_args_toggle_ready_and_draft() {
        assert_eq!(
            pull_request_ready_args("owner/repo", 42, false),
            vec!["pr", "ready", "42", "--repo", "owner/repo"]
        );
        assert_eq!(
            pull_request_ready_args("owner/repo", 42, true),
            vec!["pr", "ready", "42", "--repo", "owner/repo", "--undo"]
        );
    }

    #[test]
    fn search_page_args_use_rest_search_pagination_and_sort() {
        let args = search_page_args(
            SectionKind::Issues,
            "repo:rust-lang/rust is:open archived:false sort:created-desc",
            3,
            250,
        );

        assert_eq!(
            args,
            vec![
                "api",
                "--method",
                "GET",
                "search/issues",
                "-f",
                "q=is:issue repo:rust-lang/rust is:open archived:false",
                "-f",
                "per_page=100",
                "-f",
                "page=3",
                "-f",
                "sort=created",
                "-f",
                "order=desc"
            ]
        );
    }

    #[test]
    fn assignee_api_args_use_issue_assignee_endpoint_for_assign_and_unassign() {
        let assign = assignee_api_args(
            "owner/repo",
            42,
            AssigneeAction::Assign,
            &["alice".to_string(), "bob".to_string()],
        );
        assert_eq!(
            assign,
            vec![
                "api",
                "-X",
                "POST",
                "repos/owner/repo/issues/42/assignees",
                "-f",
                "assignees[]=alice",
                "-f",
                "assignees[]=bob",
            ]
        );

        let unassign = assignee_api_args(
            "owner/repo",
            42,
            AssigneeAction::Unassign,
            &["alice".to_string()],
        );
        assert_eq!(
            unassign,
            vec![
                "api",
                "-X",
                "DELETE",
                "repos/owner/repo/issues/42/assignees",
                "-f",
                "assignees[]=alice",
            ]
        );
    }

    #[test]
    fn search_page_args_preserve_label_filters() {
        let args = search_page_args(
            SectionKind::PullRequests,
            "repo:rust-lang/rust is:open label:\"good first issue\" sort:updated-desc",
            1,
            50,
        );

        assert_eq!(
            args,
            vec![
                "api",
                "--method",
                "GET",
                "search/issues",
                "-f",
                "q=is:pr repo:rust-lang/rust is:open label:\"good first issue\"",
                "-f",
                "per_page=50",
                "-f",
                "page=1",
                "-f",
                "sort=updated",
                "-f",
                "order=desc"
            ]
        );
    }

    #[test]
    fn search_api_item_maps_repository_url() {
        let item = SearchApiIssueRaw {
            assignees: Some(vec![SearchAuthorRaw {
                login: "triager".to_string(),
            }]),
            body: Some("hello".to_string()),
            comments: Some(7),
            created_at: DateTime::parse_from_rfc3339("2026-01-01T08:30:00Z")
                .ok()
                .map(|value| value.with_timezone(&Utc)),
            draft: Some(false),
            html_url: "https://github.com/rust-lang/rust/issues/1".to_string(),
            labels: Some(vec![SearchLabelRaw {
                name: "T-compiler".to_string(),
            }]),
            milestone: Some(SearchMilestoneRaw {
                number: 3,
                title: "1.90".to_string(),
            }),
            number: 1,
            reactions: None,
            repository_url: "https://api.github.com/repos/rust-lang/rust".to_string(),
            state: Some("open".to_string()),
            title: "ICE".to_string(),
            updated_at: None,
            user: Some(SearchAuthorRaw {
                login: "chenyukang".to_string(),
            }),
        };

        let mapped = search_api_item_to_work_item(SectionKind::Issues, item);

        assert_eq!(mapped.id, "rust-lang/rust#1");
        assert_eq!(mapped.repo, "rust-lang/rust");
        assert_eq!(mapped.author.as_deref(), Some("chenyukang"));
        assert_eq!(mapped.assignees, vec!["triager"]);
        assert_eq!(mapped.comments, Some(7));
        assert!(mapped.created_at.is_some());
        assert_eq!(mapped.labels, vec!["T-compiler"]);
        assert_eq!(
            mapped.milestone,
            Some(Milestone {
                number: 3,
                title: "1.90".to_string(),
            })
        );
    }

    #[test]
    fn open_milestones_args_use_paginated_issue_metadata_api() {
        assert_eq!(
            open_milestones_args("owner/repo"),
            vec![
                "api",
                "--paginate",
                "--slurp",
                "repos/owner/repo/milestones?state=open&per_page=100"
            ]
        );
    }

    #[test]
    fn create_milestone_args_post_repository_milestone() {
        assert_eq!(
            create_milestone_args("owner/repo", " next-release "),
            vec![
                "api",
                "-X",
                "POST",
                "repos/owner/repo/milestones",
                "-F",
                "title=next-release"
            ]
        );
    }

    #[test]
    fn change_issue_milestone_args_patch_issue_metadata() {
        assert_eq!(
            change_issue_milestone_args("owner/repo", 42, Some(7)),
            vec![
                "api",
                "-X",
                "PATCH",
                "repos/owner/repo/issues/42",
                "-F",
                "milestone=7"
            ]
        );
        assert_eq!(
            change_issue_milestone_args("owner/repo", 42, None),
            vec![
                "api",
                "-X",
                "PATCH",
                "repos/owner/repo/issues/42",
                "-F",
                "milestone=null"
            ]
        );
    }

    #[test]
    fn parse_open_milestones_output_flattens_paginated_response() {
        let output = r#"
[
  [{"number": 1, "title": "alpha"}],
  [{"number": 2, "title": "beta"}]
]
"#;

        let milestones = parse_open_milestones_output(output, "owner/repo").unwrap();

        assert_eq!(
            milestones,
            vec![
                Milestone {
                    number: 1,
                    title: "alpha".to_string(),
                },
                Milestone {
                    number: 2,
                    title: "beta".to_string(),
                },
            ]
        );
    }

    #[test]
    fn parse_milestone_output_reads_created_milestone() {
        let milestone =
            parse_milestone_output(r#"{"number": 9, "title": "next-release"}"#, "owner/repo")
                .unwrap();

        assert_eq!(
            milestone,
            Milestone {
                number: 9,
                title: "next-release".to_string(),
            }
        );
    }

    #[test]
    fn label_path_segments_are_percent_encoded() {
        assert_eq!(percent_encode_path_segment("T-compiler"), "T-compiler");
        assert_eq!(
            percent_encode_path_segment("needs review/triage"),
            "needs%20review%2Ftriage"
        );
    }

    #[test]
    fn paginated_repository_labels_are_sorted_and_deduped() {
        let output = r#"[
          [{"name": "bug"}, {"name": "T-compiler"}],
          [{"name": "enhancement"}, {"name": "Bug"}]
        ]"#;

        assert_eq!(
            parse_repository_labels_output(output).unwrap(),
            vec!["bug", "enhancement", "T-compiler"]
        );
    }

    #[test]
    fn paginated_repository_assignees_are_sorted_and_deduped() {
        let output = r#"[
          [{"login": "Bob"}, {"login": "alice"}],
          [{"login": "ALICE"}, {"login": "chenyukang"}]
        ]"#;

        assert_eq!(
            parse_repository_assignees_output(output).unwrap(),
            vec!["alice", "Bob", "chenyukang"]
        );
    }

    #[test]
    fn issue_details_and_comments_parse_reactions() {
        let issue = r#"{"reactions": {"+1": 0, "-1": 0, "laugh": 0, "hooray": 0, "confused": 0, "heart": 1, "rocket": 0, "eyes": 0}, "milestone": {"number": 9, "title": "next-release"}}"#;
        let details = parse_issue_details_output(issue, "owner/repo", 1).unwrap();
        assert_eq!(details.reactions.heart, 1);
        assert_eq!(
            details.milestone,
            Some(Milestone {
                number: 9,
                title: "next-release".to_string(),
            })
        );

        let output = r##"[
          [{
            "id": 1,
            "body": "reacted",
            "html_url": "https://github.com/owner/repo/issues/1#issuecomment-1",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "reactions": {"+1": 2, "-1": 0, "laugh": 0, "hooray": 0, "confused": 0, "heart": 0, "rocket": 1, "eyes": 0},
            "user": { "login": "alice" }
          }]
        ]"##;
        let comments = parse_issue_comments_output(output, "owner/repo", 1, None).unwrap();
        assert_eq!(comments[0].reactions.plus_one, 2);
        assert_eq!(comments[0].reactions.rocket, 1);
    }

    #[test]
    fn pull_request_notification_maps_to_pull_request_item() {
        let item = notification_to_work_item(&NotificationRaw {
            id: "thread-1".to_string(),
            unread: true,
            reason: "review_requested".to_string(),
            updated_at: None,
            last_read_at: None,
            subject: NotificationSubjectRaw {
                title: "Add fee support".to_string(),
                url: Some("https://api.github.com/repos/owner/repo/pulls/42".to_string()),
                subject_type: "PullRequest".to_string(),
            },
            repository: NotificationRepositoryRaw {
                full_name: "owner/repo".to_string(),
                html_url: "https://github.com/owner/repo".to_string(),
            },
        });

        assert_eq!(item.kind, ItemKind::PullRequest);
        assert_eq!(item.number, Some(42));
        assert_eq!(item.url, "https://github.com/owner/repo/pull/42");
        assert!(item.created_at.is_none());
        assert_eq!(item.unread, Some(true));
        assert_eq!(item.reason.as_deref(), Some("review-requested"));
        assert_eq!(item.extra.as_deref(), Some("PullRequest"));
    }

    #[test]
    fn global_search_filters_add_default_sort_and_archive_filter() {
        assert_eq!(
            global_search_filters("fiber rpc", None),
            "fiber rpc in:title archived:false sort:created-desc"
        );
        assert_eq!(
            global_search_filters("fiber archived:true sort:created-desc", None),
            "fiber archived:true sort:created-desc in:title"
        );
        assert_eq!(
            global_search_filters("fiber in:body", None),
            "fiber in:body archived:false sort:created-desc"
        );
        assert_eq!(
            global_search_filters("author:chenyukang", None),
            "author:chenyukang archived:false sort:created-desc"
        );
    }

    #[test]
    fn global_search_filters_scope_to_current_repo_when_available() {
        assert_eq!(
            global_search_filters("payment channel", Some("nervosnetwork/fiber")),
            "payment channel in:title repo:nervosnetwork/fiber archived:false sort:created-desc"
        );
        assert_eq!(
            global_search_filters("rpc repo:rust-lang/rust", Some("nervosnetwork/fiber")),
            "rpc repo:rust-lang/rust in:title archived:false sort:created-desc"
        );
    }

    #[test]
    fn search_fields_include_body_for_preview() {
        assert!(search_fields(SectionKind::PullRequests).contains("body"));
        assert!(search_fields(SectionKind::Issues).contains("body"));
        assert!(search_fields(SectionKind::PullRequests).contains("createdAt"));
        assert!(search_fields(SectionKind::Issues).contains("createdAt"));
    }

    #[test]
    fn paginated_comments_are_flattened_sorted_oldest_first_and_not_truncated() {
        let output = r##"
        [
          [
            {
              "id": 1,
              "body": "old",
              "html_url": "https://github.com/owner/repo/issues/1#issuecomment-1",
              "created_at": "2026-01-01T00:00:00Z",
              "updated_at": "2026-01-04T00:00:00Z",
              "user": { "login": "alice" }
            },
            {
              "id": 2,
              "body": "new",
              "html_url": "https://github.com/owner/repo/issues/1#issuecomment-2",
              "created_at": "2026-01-03T00:00:00Z",
              "updated_at": "2026-01-03T00:00:00Z",
              "user": { "login": "bob" }
            }
          ],
          [
            {
              "id": 3,
              "body": "middle",
              "html_url": "https://github.com/owner/repo/issues/1#issuecomment-3",
              "created_at": "2026-01-02T00:00:00Z",
              "updated_at": "2026-01-02T00:00:00Z",
              "user": { "login": "carol" }
            }
          ]
        ]
        "##;

        let comments = parse_issue_comments_output(output, "owner/repo", 1, Some("bob")).unwrap();

        assert_eq!(comments.len(), 3);
        assert_eq!(
            comments
                .iter()
                .map(|comment| comment.body.as_str())
                .collect::<Vec<_>>(),
            vec!["old", "middle", "new"]
        );
        assert_eq!(
            comments
                .iter()
                .map(|comment| comment.id)
                .collect::<Vec<_>>(),
            vec![Some(1), Some(3), Some(2)]
        );
        assert_eq!(
            comments
                .iter()
                .map(|comment| comment.is_mine)
                .collect::<Vec<_>>(),
            vec![false, false, true]
        );
        assert!(comments.iter().all(|comment| comment.review.is_none()));
    }

    #[test]
    fn review_comments_parse_inline_location_and_mark_mine() {
        let output = r##"
        [
          [
            {
              "id": 10,
              "in_reply_to_id": 8,
              "body": "inline",
              "html_url": "https://github.com/owner/repo/pull/1#discussion_r10",
              "created_at": "2026-01-02T00:00:00Z",
              "updated_at": "2026-01-02T00:00:00Z",
              "user": { "login": "alice" },
              "path": "src/app.rs",
              "line": 57,
              "original_line": 50,
              "start_line": 44,
              "original_start_line": 42,
              "side": "RIGHT",
              "start_side": "RIGHT",
              "diff_hunk": "@@ -55,6 +55,7 @@ fn main() {\n line 55\n+line 57\n line 58"
            }
          ]
        ]
        "##;

        let comments = parse_pull_request_review_comments_output(
            output,
            "owner/repo",
            1,
            Some("alice"),
            &HashMap::from([(
                10,
                ReviewThreadState {
                    is_resolved: true,
                    is_outdated: false,
                },
            )]),
        )
        .unwrap();

        assert_eq!(comments.len(), 1);
        let comment = &comments[0];
        assert_eq!(comment.id, Some(10));
        assert_eq!(comment.parent_id, Some(8));
        assert_eq!(comment.author, "alice");
        assert!(comment.is_mine);
        let review = comment.review.as_ref().expect("review metadata");
        assert_eq!(review.path, "src/app.rs");
        assert_eq!(review.line, Some(57));
        assert_eq!(review.original_line, Some(50));
        assert_eq!(review.start_line, Some(44));
        assert_eq!(review.original_start_line, Some(42));
        assert_eq!(review.side.as_deref(), Some("RIGHT"));
        assert_eq!(review.start_side.as_deref(), Some("RIGHT"));
        assert_eq!(
            review.diff_hunk.as_deref(),
            Some("@@ -55,6 +55,7 @@ fn main() {\n line 55\n+line 57\n line 58")
        );
        assert!(review.is_resolved);
        assert!(!review.is_outdated);
    }

    #[test]
    fn review_thread_states_parse_resolved_and_outdated_by_comment_id() {
        let output = r#"
        {
          "data": {
            "repository": {
              "pullRequest": {
                "reviewThreads": {
                  "pageInfo": {
                    "hasNextPage": true,
                    "endCursor": "cursor-2"
                  },
                  "nodes": [
                    {
                      "isResolved": true,
                      "isOutdated": false,
                      "comments": {
                        "nodes": [
                          { "databaseId": 10 },
                          { "databaseId": 11 }
                        ]
                      }
                    },
                    {
                      "isResolved": false,
                      "isOutdated": true,
                      "comments": {
                        "nodes": [
                          { "databaseId": 20 }
                        ]
                      }
                    }
                  ]
                }
              }
            }
          }
        }
        "#;

        let (states, has_next, cursor) =
            parse_pull_request_review_thread_states_page(output, "owner/repo", 1).unwrap();

        assert!(has_next);
        assert_eq!(cursor.as_deref(), Some("cursor-2"));
        assert_eq!(
            states.get(&10),
            Some(&ReviewThreadState {
                is_resolved: true,
                is_outdated: false,
            })
        );
        assert_eq!(
            states.get(&20),
            Some(&ReviewThreadState {
                is_resolved: false,
                is_outdated: true,
            })
        );
    }

    #[test]
    fn missing_gh_message_explains_install_and_login() {
        let message = gh_missing_message(&["api".to_string(), "user".to_string()]);

        assert!(message.contains("GitHub CLI `gh` is required"));
        assert!(message.contains("brew install gh"));
        assert!(message.contains("sudo dnf install gh"));
        assert!(message.contains("sudo pacman -S github-cli"));
        assert!(message.contains("official apt setup"));
        assert!(message.contains("gh auth login"));
    }

    #[test]
    fn auth_errors_are_rewritten_with_login_hint() {
        let message = gh_failure_message(
            &["search".to_string(), "prs".to_string()],
            "To get started with GitHub CLI, please run: gh auth login",
        );

        assert!(message.contains("not authenticated"));
        assert!(message.contains("Run `gh auth login`"));
    }

    #[test]
    fn merge_pull_request_args_default_to_merge_flag() {
        assert_eq!(
            merge_pull_request_args("owner/repo", 42, MergeMethod::default()),
            vec![
                "pr".to_string(),
                "merge".to_string(),
                "42".to_string(),
                "--repo".to_string(),
                "owner/repo".to_string(),
                "--merge".to_string(),
            ]
        );
    }

    #[test]
    fn merge_pull_request_args_use_selected_method_flag() {
        assert_eq!(
            merge_pull_request_args("owner/repo", 42, MergeMethod::Squash).last(),
            Some(&"--squash".to_string())
        );
        assert_eq!(
            merge_pull_request_args("owner/repo", 42, MergeMethod::Rebase).last(),
            Some(&"--rebase".to_string())
        );
    }

    #[test]
    fn auto_merge_commands_use_non_interactive_gh_pr_merge_flags() {
        assert_eq!(
            enable_pull_request_auto_merge_args("owner/repo", 42, "--merge"),
            vec![
                "pr",
                "merge",
                "42",
                "--repo",
                "owner/repo",
                "--merge",
                "--auto"
            ]
        );
        assert_eq!(
            disable_pull_request_auto_merge_args("owner/repo", 42),
            vec![
                "pr",
                "merge",
                "42",
                "--repo",
                "owner/repo",
                "--disable-auto"
            ]
        );
        assert_eq!(
            pull_request_auto_merge_status_args("owner/repo", 42),
            vec![
                "pr",
                "view",
                "42",
                "--repo",
                "owner/repo",
                "--json",
                "state,isDraft,autoMergeRequest"
            ]
        );
        assert_eq!(
            repository_merge_methods_args("owner/repo"),
            vec![
                "api",
                "repos/owner/repo",
                "--jq",
                "{allow_merge_commit,allow_squash_merge,allow_rebase_merge}"
            ]
        );
    }

    #[test]
    fn auto_merge_method_uses_allowed_repo_merge_policy() {
        let merge = RepositoryMergeMethodsRaw {
            allow_merge_commit: Some(true),
            allow_squash_merge: Some(true),
            allow_rebase_merge: Some(true),
        };
        assert_eq!(
            repository_merge_method_flag("owner/repo", &merge).expect("merge allowed"),
            "--merge"
        );

        let squash = RepositoryMergeMethodsRaw {
            allow_merge_commit: Some(false),
            allow_squash_merge: Some(true),
            allow_rebase_merge: Some(true),
        };
        assert_eq!(
            repository_merge_method_flag("owner/repo", &squash).expect("squash allowed"),
            "--squash"
        );

        let none = RepositoryMergeMethodsRaw {
            allow_merge_commit: Some(false),
            allow_squash_merge: Some(false),
            allow_rebase_merge: Some(false),
        };
        let error = repository_merge_method_flag("owner/repo", &none)
            .expect_err("no merge methods should be rejected")
            .to_string();
        assert!(error.contains("repository does not allow merge"));
    }

    #[test]
    fn auto_merge_preflight_rejects_closed_and_already_matching_state() {
        let closed = PullRequestAutoMergeStatusRaw {
            auto_merge_request: None,
            is_draft: Some(false),
            state: Some("MERGED".to_string()),
        };
        let error = pull_request_auto_merge_enable_blocker("owner/repo", 42, &closed)
            .expect("merged PR should be rejected");
        assert!(error.contains("pull request is MERGED"));

        let enabled = PullRequestAutoMergeStatusRaw {
            auto_merge_request: Some(serde_json::json!({ "enabledAt": "2026-05-04T00:00:00Z" })),
            is_draft: Some(false),
            state: Some("OPEN".to_string()),
        };
        let error = pull_request_auto_merge_enable_blocker("owner/repo", 42, &enabled)
            .expect("already enabled PR should be rejected");
        assert_eq!(error, "auto-merge is already enabled for owner/repo#42");

        let disabled = PullRequestAutoMergeStatusRaw {
            auto_merge_request: None,
            is_draft: Some(false),
            state: Some("OPEN".to_string()),
        };
        let error = pull_request_auto_merge_disable_blocker("owner/repo", 42, &disabled)
            .expect("already disabled PR should be rejected");
        assert_eq!(error, "auto-merge is already disabled for owner/repo#42");
    }

    #[test]
    fn pull_request_reviewers_api_args_builds_request_and_remove_commands() {
        let reviewers = vec!["alice".to_string(), "bob".to_string()];

        let request = pull_request_reviewers_api_args("POST", "owner/repo", 42, &reviewers)
            .expect("request args");
        assert_eq!(
            request,
            vec![
                "api",
                "-X",
                "POST",
                "repos/owner/repo/pulls/42/requested_reviewers",
                "-f",
                "reviewers[]=alice",
                "-f",
                "reviewers[]=bob",
            ]
        );

        let remove = pull_request_reviewers_api_args("DELETE", "owner/repo", 42, &reviewers)
            .expect("remove args");
        assert_eq!(remove[2], "DELETE");
        assert_eq!(remove[3], "repos/owner/repo/pulls/42/requested_reviewers");
    }

    #[test]
    fn pull_request_reviewers_api_args_rejects_empty_reviewers() {
        let error = pull_request_reviewers_api_args("POST", "owner/repo", 42, &[])
            .expect_err("empty reviewers should fail");

        assert!(error.to_string().contains("at least one reviewer"));
    }

    #[test]
    fn merge_blocker_message_explains_review_and_checks() {
        let status = PullRequestMergeStatusRaw {
            is_draft: Some(false),
            merge_state_status: Some("BLOCKED".to_string()),
            review_decision: Some("REVIEW_REQUIRED".to_string()),
            state: Some("OPEN".to_string()),
            status_check_rollup: Some(vec![
                PullRequestCheckRaw {
                    conclusion: Some("FAILURE".to_string()),
                    status: Some("COMPLETED".to_string()),
                },
                PullRequestCheckRaw {
                    conclusion: Some("SUCCESS".to_string()),
                    status: Some("COMPLETED".to_string()),
                },
            ]),
        };

        let message = pull_request_merge_blocker_message("owner/repo", 42, &status)
            .expect("blocked PR should produce message");

        assert!(message.contains("review approval required"));
        assert!(message.contains("branch protection is blocking merge"));
        assert!(message.contains("1 check(s) failing"));
        assert!(message.contains("gh pr merge 42 --repo owner/repo --merge --auto"));
    }

    #[test]
    fn clean_approved_pr_has_no_merge_blocker_message() {
        let status = PullRequestMergeStatusRaw {
            is_draft: Some(false),
            merge_state_status: Some("CLEAN".to_string()),
            review_decision: Some("APPROVED".to_string()),
            state: Some("OPEN".to_string()),
            status_check_rollup: Some(vec![PullRequestCheckRaw {
                conclusion: Some("SUCCESS".to_string()),
                status: Some("COMPLETED".to_string()),
            }]),
        };

        assert!(pull_request_merge_blocker_message("owner/repo", 42, &status).is_none());
    }

    #[test]
    fn action_hints_show_approvable_when_review_is_needed() {
        let hints = pull_request_action_hints(&PullRequestActionRaw {
            auto_merge_request: None,
            commits: Some(PullRequestCommitConnectionRaw { total_count: 5 }),
            head_ref_name: Some("feature/diagnostics".to_string()),
            head_repository: Some(PullRequestHeadRepositoryRaw {
                name_with_owner: "rust-lang/rust".to_string(),
            }),
            is_draft: Some(false),
            mergeable: Some("MERGEABLE".to_string()),
            merge_state_status: Some("BLOCKED".to_string()),
            review_decision: Some("REVIEW_REQUIRED".to_string()),
            state: Some("OPEN".to_string()),
            status_check_rollup: Some(PullRequestStatusRollupRaw {
                state: Some("FAILURE".to_string()),
                contexts: Some(PullRequestCheckConnectionRaw {
                    total_count: 3,
                    nodes: Some(vec![
                        PullRequestCheckContextRaw::CheckRun {
                            conclusion: Some("SUCCESS".to_string()),
                            details_url: None,
                            name: None,
                            status: Some("COMPLETED".to_string()),
                        },
                        PullRequestCheckContextRaw::CheckRun {
                            conclusion: Some("FAILURE".to_string()),
                            details_url: None,
                            name: None,
                            status: Some("COMPLETED".to_string()),
                        },
                        PullRequestCheckContextRaw::StatusContext {
                            state: Some("PENDING".to_string()),
                        },
                    ]),
                }),
            }),
            viewer_can_enable_auto_merge: Some(false),
            viewer_can_merge_as_admin: Some(false),
            viewer_can_update: Some(true),
            viewer_can_update_branch: Some(false),
            viewer_did_author: Some(false),
            viewer_latest_review: None,
        });

        assert_eq!(hints.labels, vec!["Approvable"]);
        assert_eq!(hints.commits, Some(5));
        assert_eq!(
            hints.checks,
            Some(CheckSummary {
                passed: 1,
                failed: 1,
                pending: 1,
                skipped: 0,
                total: 3,
                incomplete: false,
            })
        );
        let note = hints.note.expect("blocked PR should explain why");
        assert!(note.contains("review approval required"));
        assert!(note.contains("checks failing"));
        assert_eq!(
            hints.head,
            Some(PullRequestBranch {
                repository: "rust-lang/rust".to_string(),
                branch: "feature/diagnostics".to_string(),
            })
        );
    }

    #[test]
    fn action_hints_show_mergeable_when_pr_is_ready_and_viewer_can_merge() {
        let hints = pull_request_action_hints(&PullRequestActionRaw {
            auto_merge_request: None,
            commits: Some(PullRequestCommitConnectionRaw { total_count: 2 }),
            head_ref_name: None,
            head_repository: None,
            is_draft: Some(false),
            mergeable: Some("MERGEABLE".to_string()),
            merge_state_status: Some("CLEAN".to_string()),
            review_decision: Some("APPROVED".to_string()),
            state: Some("OPEN".to_string()),
            status_check_rollup: Some(PullRequestStatusRollupRaw {
                state: Some("SUCCESS".to_string()),
                contexts: Some(PullRequestCheckConnectionRaw {
                    total_count: 1,
                    nodes: Some(vec![PullRequestCheckContextRaw::CheckRun {
                        conclusion: Some("SUCCESS".to_string()),
                        details_url: None,
                        name: None,
                        status: Some("COMPLETED".to_string()),
                    }]),
                }),
            }),
            viewer_can_enable_auto_merge: Some(false),
            viewer_can_merge_as_admin: Some(false),
            viewer_can_update: Some(true),
            viewer_can_update_branch: Some(false),
            viewer_did_author: Some(false),
            viewer_latest_review: Some(PullRequestViewerReviewRaw {
                state: Some("APPROVED".to_string()),
            }),
        });

        assert_eq!(hints.labels, vec!["Mergeable"]);
        assert_eq!(hints.commits, Some(2));
        assert_eq!(
            hints.checks,
            Some(CheckSummary {
                passed: 1,
                failed: 0,
                pending: 0,
                skipped: 0,
                total: 1,
                incomplete: false,
            })
        );
        assert!(hints.note.is_none());
    }

    #[test]
    fn action_hints_show_conflicts_when_mergeable_is_conflicting() {
        let hints = pull_request_action_hints(&PullRequestActionRaw {
            auto_merge_request: None,
            commits: Some(PullRequestCommitConnectionRaw { total_count: 2 }),
            head_ref_name: None,
            head_repository: None,
            is_draft: Some(true),
            mergeable: Some("CONFLICTING".to_string()),
            merge_state_status: Some("UNKNOWN".to_string()),
            review_decision: None,
            state: Some("OPEN".to_string()),
            status_check_rollup: None,
            viewer_can_enable_auto_merge: Some(false),
            viewer_can_merge_as_admin: Some(false),
            viewer_can_update: Some(true),
            viewer_can_update_branch: Some(false),
            viewer_did_author: Some(true),
            viewer_latest_review: None,
        });

        let note = hints.note.expect("conflicting PR should explain why");
        assert!(note.contains("draft"));
        assert!(note.contains("merge conflicts must be resolved"));
        assert!(!note.contains("GitHub is still computing mergeability"));
    }

    #[test]
    fn action_hints_collect_failed_actions_run_ids_from_rollup() {
        let hints = pull_request_action_hints(&PullRequestActionRaw {
            auto_merge_request: None,
            commits: None,
            head_ref_name: None,
            head_repository: None,
            is_draft: Some(false),
            mergeable: None,
            merge_state_status: Some("BLOCKED".to_string()),
            review_decision: None,
            state: Some("OPEN".to_string()),
            status_check_rollup: Some(PullRequestStatusRollupRaw {
                state: Some("FAILURE".to_string()),
                contexts: Some(PullRequestCheckConnectionRaw {
                    total_count: 2,
                    nodes: Some(vec![
                        PullRequestCheckContextRaw::CheckRun {
                            conclusion: Some("FAILURE".to_string()),
                            details_url: Some(
                                "https://github.com/owner/repo/actions/runs/987/job/654"
                                    .to_string(),
                            ),
                            name: Some("test".to_string()),
                            status: Some("COMPLETED".to_string()),
                        },
                        PullRequestCheckContextRaw::CheckRun {
                            conclusion: Some("SUCCESS".to_string()),
                            details_url: Some(
                                "https://github.com/owner/repo/actions/runs/111/job/222"
                                    .to_string(),
                            ),
                            name: Some("lint".to_string()),
                            status: Some("COMPLETED".to_string()),
                        },
                    ]),
                }),
            }),
            viewer_can_enable_auto_merge: Some(false),
            viewer_can_merge_as_admin: Some(false),
            viewer_can_update: Some(true),
            viewer_can_update_branch: Some(false),
            viewer_did_author: Some(false),
            viewer_latest_review: None,
        });

        assert_eq!(
            hints.failed_check_runs,
            vec![FailedCheckRunSummary {
                run_id: 987,
                workflow: None,
                checks: vec!["test".to_string()],
            }]
        );
    }

    #[test]
    fn pr_checks_json_groups_failed_checks_by_actions_run() {
        let output = r#"
[
  {
    "bucket": "fail",
    "link": "https://github.com/owner/repo/actions/runs/1001/job/1?pr=2",
    "name": "test",
    "state": "FAILURE",
    "workflow": "CI"
  },
  {
    "bucket": "fail",
    "link": "https://github.com/owner/repo/actions/runs/1001/job/2?pr=2",
    "name": "lint",
    "state": "FAILURE",
    "workflow": "CI"
  },
  {
    "bucket": "pass",
    "link": "https://github.com/owner/repo/actions/runs/1002/job/3?pr=2",
    "name": "fmt",
    "state": "SUCCESS",
    "workflow": "CI"
  },
  {
    "bucket": "fail",
    "link": "https://example.com/checks/9",
    "name": "external",
    "state": "ERROR",
    "workflow": null
  }
]
"#;

        let discovery = failed_check_runs_from_pr_checks_json(output).expect("valid checks");

        assert_eq!(
            discovery.runs,
            vec![FailedCheckRunSummary {
                run_id: 1001,
                workflow: Some("CI".to_string()),
                checks: vec!["test".to_string(), "lint".to_string()],
            }]
        );
        assert_eq!(discovery.unmapped_failed_checks, vec!["external"]);
    }

    #[test]
    fn rerun_failed_checks_commands_are_constructed_for_pr_and_run() {
        assert_eq!(
            pr_checks_args("owner/repo", 42),
            vec![
                "pr",
                "checks",
                "42",
                "--repo",
                "owner/repo",
                "--json",
                "name,state,bucket,workflow,link",
            ]
        );
        assert_eq!(
            rerun_failed_check_run_args("owner/repo", 1001),
            vec!["run", "rerun", "1001", "--failed", "--repo", "owner/repo"]
        );
    }

    #[test]
    fn update_pull_request_branch_uses_gh_update_branch_command() {
        assert_eq!(
            update_pull_request_branch_args("owner/repo", 42),
            vec![
                "pr".to_string(),
                "update-branch".to_string(),
                "42".to_string(),
                "--repo".to_string(),
                "owner/repo".to_string(),
            ]
        );
    }

    #[test]
    fn create_pull_request_args_use_repo_head_title_and_body() {
        assert_eq!(
            create_pull_request_args("owner/repo", "feature/new-pr", "Add feature", "Body text"),
            vec![
                "pr".to_string(),
                "create".to_string(),
                "--repo".to_string(),
                "owner/repo".to_string(),
                "--head".to_string(),
                "feature/new-pr".to_string(),
                "--title".to_string(),
                "Add feature".to_string(),
                "--body".to_string(),
                "Body text".to_string(),
            ]
        );
    }

    #[test]
    fn pull_request_number_parses_from_create_output_url() {
        assert_eq!(
            pull_request_number_from_create_output("https://github.com/owner/repo/pull/42\n"),
            Some(42)
        );
        assert_eq!(
            pull_request_number_from_create_output(
                "created https://github.com/owner/repo/pull/77."
            ),
            Some(77)
        );
    }

    #[test]
    fn non_auth_gh_errors_keep_original_command_context() {
        let message = gh_failure_message(&["search".to_string(), "issues".to_string()], "HTTP 500");

        assert_eq!(message, "gh search issues failed: HTTP 500");
    }

    #[test]
    fn review_submit_args_include_event_and_body() {
        let args = create_pull_request_review_args(
            "owner/repo",
            42,
            Some(PullRequestReviewEvent::RequestChanges),
            "please fix",
        );

        assert_eq!(
            args,
            vec![
                "api",
                "-X",
                "POST",
                "repos/owner/repo/pulls/42/reviews",
                "-f",
                "event=REQUEST_CHANGES",
                "-f",
                "body=please fix",
            ]
        );
    }

    #[test]
    fn pending_review_create_args_omit_event() {
        let args = create_pull_request_review_args("owner/repo", 42, None, "draft notes");

        assert_eq!(
            args,
            vec![
                "api",
                "-X",
                "POST",
                "repos/owner/repo/pulls/42/reviews",
                "-f",
                "body=draft notes",
            ]
        );
    }

    #[test]
    fn pending_review_submit_args_target_events_endpoint() {
        let args = submit_pending_pull_request_review_args(
            "owner/repo",
            42,
            77,
            PullRequestReviewEvent::Approve,
            "",
        );

        assert_eq!(
            args,
            vec![
                "api",
                "-X",
                "POST",
                "repos/owner/repo/pulls/42/reviews/77/events",
                "-f",
                "event=APPROVE",
            ]
        );
    }

    #[test]
    fn pending_review_discard_args_target_review_endpoint() {
        let args = discard_pending_pull_request_review_args("owner/repo", 42, 77);

        assert_eq!(
            args,
            vec![
                "api",
                "-X",
                "DELETE",
                "repos/owner/repo/pulls/42/reviews/77",
            ]
        );
    }
}
