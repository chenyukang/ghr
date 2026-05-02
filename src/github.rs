use std::collections::HashMap;
use std::io::ErrorKind;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use futures::future::{BoxFuture, FutureExt};
use futures::stream::{FuturesUnordered, StreamExt};
use serde::Deserialize;
use tokio::process::Command;
use tracing::{info, warn};

use crate::config::{Config, SearchSection};
use crate::model::{CommentPreview, ItemKind, SectionKind, SectionSnapshot, WorkItem};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchItemRaw {
    author: Option<SearchAuthorRaw>,
    body: Option<String>,
    comments_count: Option<u64>,
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
#[serde(rename_all = "camelCase")]
struct SearchRepositoryRaw {
    name_with_owner: String,
}

#[derive(Debug, Clone, Deserialize)]
struct NotificationRaw {
    id: String,
    unread: bool,
    reason: String,
    updated_at: Option<DateTime<Utc>>,
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
    body: Option<String>,
    html_url: Option<String>,
    created_at: Option<DateTime<Utc>>,
    updated_at: Option<DateTime<Utc>>,
    user: Option<SearchAuthorRaw>,
}

pub async fn refresh_dashboard(config: &Config) -> Vec<SectionSnapshot> {
    let started = Instant::now();
    let notifications = refresh_notification_sections(config);
    let searches = async {
        let viewer_login = match fetch_viewer_login().await {
            Ok(login) => Some(login),
            Err(error) => {
                warn!(error = %error, "failed to resolve @me for search filters");
                None
            }
        };
        refresh_search_sections(config, viewer_login.as_deref()).await
    };
    let (mut searches, mut notifications) = tokio::join!(searches, notifications);
    searches.append(&mut notifications);
    info!(
        sections = searches.len(),
        elapsed_ms = started.elapsed().as_millis(),
        "dashboard refresh finished"
    );
    searches
}

async fn refresh_search_sections(
    config: &Config,
    viewer_login: Option<&str>,
) -> Vec<SectionSnapshot> {
    let excludes = Arc::new(config.exclude_repos.clone());
    let viewer_login = Arc::new(viewer_login.map(str::to_string));
    let mut tasks: FuturesUnordered<BoxFuture<'static, (usize, SectionSnapshot)>> =
        FuturesUnordered::new();
    let mut order = 0;

    for section in config.pr_sections.clone() {
        let excludes = excludes.clone();
        let viewer_login = viewer_login.clone();
        let limit = section.limit.unwrap_or(config.defaults.pr_limit);
        let index = order;
        order += 1;
        tasks.push(
            async move {
                let section = resolve_me_section(section, viewer_login.as_ref().as_deref());
                (
                    index,
                    refresh_search_section(
                        SectionKind::PullRequests,
                        section,
                        limit,
                        excludes.as_slice(),
                    )
                    .await,
                )
            }
            .boxed(),
        );
    }

    for section in config.issue_sections.clone() {
        let excludes = excludes.clone();
        let viewer_login = viewer_login.clone();
        let limit = section.limit.unwrap_or(config.defaults.issue_limit);
        let index = order;
        order += 1;
        tasks.push(
            async move {
                let section = resolve_me_section(section, viewer_login.as_ref().as_deref());
                (
                    index,
                    refresh_search_section(
                        SectionKind::Issues,
                        section,
                        limit,
                        excludes.as_slice(),
                    )
                    .await,
                )
            }
            .boxed(),
        );
    }

    let mut sections = Vec::new();
    while let Some(section) = tasks.next().await {
        sections.push(section);
    }
    sections.sort_by_key(|(index, _)| *index);
    sections
        .into_iter()
        .map(|(_, section)| section)
        .collect::<Vec<_>>()
}

async fn refresh_search_section(
    kind: SectionKind,
    section: SearchSection,
    limit: usize,
    exclude_repos: &[String],
) -> SectionSnapshot {
    let queries = section.search_filters();
    let display_filters = section.display_filters();
    let mut snapshot = SectionSnapshot::empty(kind, section.title, display_filters);
    let started = Instant::now();

    match fetch_search_items(kind, queries, limit, exclude_repos).await {
        Ok(items) => {
            snapshot.items = items;
            snapshot.refreshed_at = Some(Utc::now());
            info!(
                kind = %kind,
                title = %snapshot.title,
                items = snapshot.items.len(),
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

async fn fetch_search_items(
    kind: SectionKind,
    queries: Vec<String>,
    limit: usize,
    exclude_repos: &[String],
) -> Result<Vec<WorkItem>> {
    let queries = if queries.is_empty() {
        vec![String::new()]
    } else {
        queries
    };
    let exclude_repos = Arc::new(exclude_repos.to_vec());
    let mut tasks = FuturesUnordered::new();

    for query in queries {
        let exclude_repos = exclude_repos.clone();
        tasks.push(async move {
            fetch_search_items_for_query(kind, query, limit, exclude_repos.as_slice()).await
        });
    }

    let mut deduped = HashMap::<String, WorkItem>::new();
    while let Some(result) = tasks.next().await {
        for item in result? {
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
    Ok(items)
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

fn search_fields(kind: SectionKind) -> &'static str {
    match kind {
        SectionKind::PullRequests => {
            "number,title,body,repository,author,updatedAt,url,state,isDraft,labels,commentsCount"
        }
        SectionKind::Issues => {
            "number,title,body,repository,author,updatedAt,url,state,labels,commentsCount"
        }
        SectionKind::Notifications => unreachable!("notifications are not fetched via search"),
    }
}

pub async fn fetch_issue_comments(repository: &str, number: u64) -> Result<Vec<CommentPreview>> {
    let path = format!("repos/{repository}/issues/{number}/comments?per_page=100");
    let output = run_gh_json(&[
        "api".to_string(),
        "--paginate".to_string(),
        "--slurp".to_string(),
        path,
    ])
    .await?;
    parse_issue_comments_output(&output, repository, number)
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

fn parse_issue_comments_output(
    output: &str,
    repository: &str,
    number: u64,
) -> Result<Vec<CommentPreview>> {
    let pages = serde_json::from_str::<Vec<Vec<IssueCommentRaw>>>(output)
        .with_context(|| format!("failed to parse comments for {repository}#{number}"))?;
    let mut comments = pages
        .into_iter()
        .flatten()
        .map(|comment| CommentPreview {
            author: comment
                .user
                .map(|user| user.login)
                .unwrap_or_else(|| "unknown".to_string()),
            body: comment.body.unwrap_or_default(),
            created_at: comment.created_at,
            updated_at: comment.updated_at,
            url: comment.html_url,
        })
        .collect::<Vec<_>>();

    comments.sort_by_key(|comment| comment.created_at);
    Ok(comments)
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

async fn fetch_viewer_login() -> Result<String> {
    let output = run_gh_json(&["api".to_string(), "user".to_string()]).await?;
    let viewer =
        serde_json::from_str::<ViewerRaw>(&output).context("failed to parse gh user output")?;
    Ok(viewer.login)
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
    let output = Command::new("gh")
        .args(args)
        .output()
        .await
        .map_err(|error| {
            if error.kind() == ErrorKind::NotFound {
                anyhow!("{}", gh_missing_message(args))
            } else {
                anyhow!("failed to run gh {}: {error}", args.join(" "))
            }
        })?;

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
        updated_at: item.updated_at,
        labels,
        comments: item.comments_count,
        unread: None,
        reason: None,
        extra: item
            .is_draft
            .filter(|is_draft| *is_draft)
            .map(|_| "draft".to_string()),
    }
}

fn notification_to_work_item(notification: &NotificationRaw) -> WorkItem {
    let number = notification
        .subject
        .url
        .as_deref()
        .and_then(extract_trailing_number);
    let url = notification_url(notification, number);

    WorkItem {
        id: notification.id.clone(),
        kind: ItemKind::Notification,
        repo: notification.repository.full_name.clone(),
        number,
        title: notification.subject.title.clone(),
        body: None,
        author: None,
        state: None,
        url,
        updated_at: notification.updated_at,
        labels: Vec::new(),
        comments: None,
        unread: Some(notification.unread),
        reason: Some(normalize_reason_for_display(&notification.reason)),
        extra: Some(notification.subject.subject_type.clone()),
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
            "reviewed-by:chenyukang -author:chenyukang sort:updated-desc",
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
                "sort:updated-desc"
            ]
        );
    }

    #[test]
    fn search_fields_include_body_for_preview() {
        assert!(search_fields(SectionKind::PullRequests).contains("body"));
        assert!(search_fields(SectionKind::Issues).contains("body"));
    }

    #[test]
    fn paginated_comments_are_flattened_sorted_oldest_first_and_not_truncated() {
        let output = r##"
        [
          [
            {
              "body": "old",
              "html_url": "https://github.com/owner/repo/issues/1#issuecomment-1",
              "created_at": "2026-01-01T00:00:00Z",
              "updated_at": "2026-01-04T00:00:00Z",
              "user": { "login": "alice" }
            },
            {
              "body": "new",
              "html_url": "https://github.com/owner/repo/issues/1#issuecomment-2",
              "created_at": "2026-01-03T00:00:00Z",
              "updated_at": "2026-01-03T00:00:00Z",
              "user": { "login": "bob" }
            }
          ],
          [
            {
              "body": "middle",
              "html_url": "https://github.com/owner/repo/issues/1#issuecomment-3",
              "created_at": "2026-01-02T00:00:00Z",
              "updated_at": "2026-01-02T00:00:00Z",
              "user": { "login": "carol" }
            }
          ]
        ]
        "##;

        let comments = parse_issue_comments_output(output, "owner/repo", 1).unwrap();

        assert_eq!(comments.len(), 3);
        assert_eq!(
            comments
                .iter()
                .map(|comment| comment.body.as_str())
                .collect::<Vec<_>>(),
            vec!["old", "middle", "new"]
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
    fn non_auth_gh_errors_keep_original_command_context() {
        let message = gh_failure_message(&["search".to_string(), "issues".to_string()], "HTTP 500");

        assert_eq!(message, "gh search issues failed: HTTP 500");
    }
}
