use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::Config;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SectionKind {
    #[default]
    #[serde(rename = "notifications")]
    Notifications,
    #[serde(rename = "pull_requests", alias = "pullrequests", alias = "prs")]
    PullRequests,
    #[serde(rename = "issues")]
    Issues,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Notification,
    PullRequest,
    Issue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItem {
    pub id: String,
    pub kind: ItemKind,
    pub repo: String,
    pub number: Option<u64>,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    pub author: Option<String>,
    pub state: Option<String>,
    pub url: String,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub labels: Vec<String>,
    #[serde(default)]
    pub reactions: ReactionSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone: Option<Milestone>,
    #[serde(default)]
    pub assignees: Vec<String>,
    pub comments: Option<u64>,
    pub unread: Option<bool>,
    pub reason: Option<String>,
    pub extra: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactionSummary {
    #[serde(default)]
    pub plus_one: u64,
    #[serde(default)]
    pub minus_one: u64,
    #[serde(default)]
    pub laugh: u64,
    #[serde(default)]
    pub hooray: u64,
    #[serde(default)]
    pub confused: u64,
    #[serde(default)]
    pub heart: u64,
    #[serde(default)]
    pub rocket: u64,
    #[serde(default)]
    pub eyes: u64,
}

impl ReactionSummary {
    pub fn is_empty(&self) -> bool {
        self.plus_one == 0
            && self.minus_one == 0
            && self.laugh == 0
            && self.hooray == 0
            && self.confused == 0
            && self.heart == 0
            && self.rocket == 0
            && self.eyes == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Milestone {
    pub number: u64,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentPreview {
    #[serde(default)]
    pub id: Option<u64>,
    pub author: String,
    pub body: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<u64>,
    #[serde(default)]
    pub is_mine: bool,
    #[serde(default)]
    pub reactions: ReactionSummary,
    #[serde(default)]
    pub review: Option<ReviewCommentPreview>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewCommentPreview {
    pub path: String,
    #[serde(default)]
    pub line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_start_line: Option<u64>,
    #[serde(default)]
    pub side: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_side: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_hunk: Option<String>,
    #[serde(default)]
    pub is_resolved: bool,
    #[serde(default)]
    pub is_outdated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActionHints {
    pub labels: Vec<String>,
    pub checks: Option<CheckSummary>,
    pub commits: Option<usize>,
    pub failed_check_runs: Vec<FailedCheckRunSummary>,
    pub note: Option<String>,
    pub head: Option<PullRequestBranch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedCheckRunSummary {
    pub run_id: u64,
    pub workflow: Option<String>,
    pub checks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestBranch {
    pub repository: String,
    pub branch: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckSummary {
    pub passed: usize,
    pub failed: usize,
    pub pending: usize,
    pub skipped: usize,
    pub total: usize,
    pub incomplete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionSnapshot {
    pub key: String,
    pub kind: SectionKind,
    pub title: String,
    pub filters: String,
    pub items: Vec<WorkItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_count: Option<usize>,
    #[serde(default = "default_section_page")]
    pub page: usize,
    #[serde(default)]
    pub page_size: usize,
    pub refreshed_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

impl SectionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Notifications => "notifications",
            Self::PullRequests => "pull_requests",
            Self::Issues => "issues",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Notifications => "Notification",
            Self::PullRequests => "Pull Requests",
            Self::Issues => "Issues",
        }
    }
}

impl fmt::Display for SectionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SectionKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "notifications" => Ok(Self::Notifications),
            "pull_requests" | "prs" => Ok(Self::PullRequests),
            "issues" => Ok(Self::Issues),
            other => Err(format!("unknown section kind: {other}")),
        }
    }
}

impl SectionSnapshot {
    pub fn empty(kind: SectionKind, title: impl Into<String>, filters: impl Into<String>) -> Self {
        let title = title.into();
        Self::empty_for_view(kind.as_str(), kind, title, filters)
    }

    pub fn empty_for_view(
        view: impl AsRef<str>,
        kind: SectionKind,
        title: impl Into<String>,
        filters: impl Into<String>,
    ) -> Self {
        let view = view.as_ref();
        let title = title.into();
        let filters = filters.into();
        Self {
            key: section_key_for_view(view, kind, &title),
            kind,
            title,
            filters,
            items: Vec::new(),
            total_count: None,
            page: 1,
            page_size: 0,
            refreshed_at: None,
            error: None,
        }
    }
}

fn default_section_page() -> usize {
    1
}

pub fn section_key(kind: SectionKind, title: &str) -> String {
    format!("{}:{title}", kind.as_str())
}

pub fn builtin_view_key(kind: SectionKind) -> String {
    kind.as_str().to_string()
}

pub fn repo_view_key(name: &str) -> String {
    format!("repo:{name}")
}

pub fn global_search_view_key() -> String {
    "search".to_string()
}

pub fn section_view_key(section: &SectionSnapshot) -> String {
    if let Some((prefix, rest)) = section.key.split_once(':') {
        if prefix == "repo" {
            if let Some((name, _)) = rest.split_once(':') {
                return repo_view_key(name);
            }
        } else if prefix == "search" {
            return global_search_view_key();
        }
    }

    builtin_view_key(section.kind)
}

pub fn section_key_for_view(view: &str, kind: SectionKind, title: &str) -> String {
    if view == kind.as_str() {
        section_key(kind, title)
    } else {
        format!("{view}:{}:{title}", kind.as_str())
    }
}

pub fn configured_sections(config: &Config) -> Vec<SectionSnapshot> {
    let mut sections = Vec::new();

    for section in &config.pr_sections {
        sections.push(SectionSnapshot::empty(
            SectionKind::PullRequests,
            section.title.clone(),
            section.filters.clone(),
        ));
    }

    for section in &config.issue_sections {
        sections.push(SectionSnapshot::empty(
            SectionKind::Issues,
            section.title.clone(),
            section.filters.clone(),
        ));
    }

    for section in &config.notification_sections {
        sections.push(SectionSnapshot::empty(
            SectionKind::Notifications,
            section.title.clone(),
            section.filters.clone(),
        ));
    }

    for repo in &config.repos {
        if repo.name.trim().is_empty() || repo.repo.trim().is_empty() {
            continue;
        }

        let view = repo_view_key(&repo.name);
        if repo.show_issues {
            let labels = repo.label_filters(SectionKind::Issues);
            sections.push(SectionSnapshot::empty_for_view(
                &view,
                SectionKind::Issues,
                "Issues",
                repo_section_filters_with_labels(&repo.repo, &labels),
            ));
        }
        if repo.show_prs {
            let labels = repo.label_filters(SectionKind::PullRequests);
            sections.push(SectionSnapshot::empty_for_view(
                &view,
                SectionKind::PullRequests,
                "Pull Requests",
                repo_section_filters_with_labels(&repo.repo, &labels),
            ));
        }
    }

    sections
}

pub fn repo_section_filters_with_labels(repo: &str, labels: &[String]) -> String {
    let mut tokens = vec![
        format!("repo:{repo}"),
        "is:open".to_string(),
        "archived:false".to_string(),
    ];
    tokens.extend(labels.iter().filter_map(|label| label_filter(label)));
    tokens.push("sort:created-desc".to_string());
    tokens.join(" ")
}

fn label_filter(label: &str) -> Option<String> {
    let label = label.trim();
    if label.is_empty() {
        return None;
    }

    let escaped = label.replace('\\', "\\\\").replace('"', "\\\"");
    Some(format!("label:\"{escaped}\""))
}

pub fn merge_cached_sections(
    configured: Vec<SectionSnapshot>,
    cached: HashMap<String, SectionSnapshot>,
) -> Vec<SectionSnapshot> {
    configured
        .into_iter()
        .map(|section| match cached.get(&section.key).cloned() {
            Some(mut cached) => {
                cached.kind = section.kind;
                cached.title = section.title;
                cached.filters = section.filters;
                cached
            }
            None => section,
        })
        .collect()
}

pub fn merge_refreshed_sections(
    current: Vec<SectionSnapshot>,
    refreshed: Vec<SectionSnapshot>,
) -> Vec<SectionSnapshot> {
    let mut refreshed_by_key = refreshed
        .into_iter()
        .map(|section| (section.key.clone(), section))
        .collect::<HashMap<_, _>>();

    current
        .into_iter()
        .map(|mut section| match refreshed_by_key.remove(&section.key) {
            Some(refreshed) if refreshed.error.is_none() => refreshed,
            Some(refreshed) => {
                section.error = refreshed.error;
                section
            }
            None => section,
        })
        .collect()
}

pub fn section_counts(section: &SectionSnapshot) -> (usize, usize) {
    let total = section.total_count.unwrap_or(section.items.len());
    let unread = section
        .items
        .iter()
        .filter(|item| item.unread.unwrap_or(false))
        .count();
    (total, unread)
}

pub fn notification_section_requires_unread(section: &SectionSnapshot) -> bool {
    matches!(section.kind, SectionKind::Notifications)
        && section
            .filters
            .split_whitespace()
            .any(|token| token.eq_ignore_ascii_case("is:unread"))
}

pub fn mark_notification_read_in_section(section: &mut SectionSnapshot, thread_id: &str) -> bool {
    if !matches!(section.kind, SectionKind::Notifications) {
        return false;
    }

    if notification_section_requires_unread(section) {
        let previous_len = section.items.len();
        section.items.retain(|item| item.id != thread_id);
        return section.items.len() != previous_len;
    }

    let mut changed = false;
    for item in &mut section.items {
        if item.id == thread_id && item.unread.unwrap_or(false) {
            item.unread = Some(false);
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_item_deserializes_old_snapshots_without_body() {
        let item = serde_json::from_str::<WorkItem>(
            r#"
            {
              "id": "rust-lang/rust#1",
              "kind": "pull_request",
              "repo": "rust-lang/rust",
              "number": 1,
              "title": "Example",
              "author": "alice",
              "state": "open",
              "url": "https://github.com/rust-lang/rust/pull/1",
              "updated_at": null,
              "labels": [],
              "comments": 0,
              "unread": null,
              "reason": null,
              "extra": null
            }
            "#,
        )
        .expect("old cached work item should still parse");

        assert_eq!(item.body, None);
    }

    #[test]
    fn configured_repo_sections_use_repo_view_and_generic_titles() {
        let mut config = Config::default();
        config.repos.push(crate::config::RepoConfig {
            name: "fiber".to_string(),
            repo: "nervosnetwork/fiber".to_string(),
            local_dir: None,
            show_prs: true,
            show_issues: true,
            labels: Vec::new(),
            pr_labels: Vec::new(),
            issue_labels: Vec::new(),
        });

        let sections = configured_sections(&config);
        let repo_sections = sections
            .iter()
            .filter(|section| section_view_key(section) == "repo:fiber")
            .collect::<Vec<_>>();

        assert_eq!(repo_sections.len(), 2);
        assert_eq!(repo_sections[0].title, "Issues");
        assert_eq!(repo_sections[1].title, "Pull Requests");
        assert!(repo_sections[0].key.starts_with("repo:fiber:"));
        assert_eq!(
            repo_sections[0].filters,
            "repo:nervosnetwork/fiber is:open archived:false sort:created-desc"
        );
    }

    #[test]
    fn repo_sections_apply_common_and_kind_specific_label_filters() {
        let mut config = Config::default();
        config.repos.push(crate::config::RepoConfig {
            name: "rust".to_string(),
            repo: "rust-lang/rust".to_string(),
            local_dir: None,
            show_prs: true,
            show_issues: true,
            labels: vec!["T-compiler".to_string()],
            pr_labels: vec!["S-waiting-on-review".to_string()],
            issue_labels: vec!["E-easy".to_string()],
        });

        let sections = configured_sections(&config);
        let repo_sections = sections
            .iter()
            .filter(|section| section_view_key(section) == "repo:rust")
            .collect::<Vec<_>>();

        assert_eq!(
            repo_sections[0].filters,
            "repo:rust-lang/rust is:open archived:false label:\"T-compiler\" label:\"E-easy\" sort:created-desc"
        );
        assert_eq!(
            repo_sections[1].filters,
            "repo:rust-lang/rust is:open archived:false label:\"T-compiler\" label:\"S-waiting-on-review\" sort:created-desc"
        );
    }

    #[test]
    fn repo_section_label_filters_trim_skip_and_quote_labels() {
        let filters = repo_section_filters_with_labels(
            "rust-lang/rust",
            &[
                " T-compiler ".to_string(),
                "good first issue".to_string(),
                String::new(),
            ],
        );

        assert_eq!(
            filters,
            "repo:rust-lang/rust is:open archived:false label:\"T-compiler\" label:\"good first issue\" sort:created-desc"
        );
    }

    #[test]
    fn search_sections_share_search_view() {
        let section = SectionSnapshot::empty_for_view(
            global_search_view_key(),
            SectionKind::PullRequests,
            "Pull Requests",
            "fiber",
        );

        assert_eq!(section_view_key(&section), "search");
    }
}
