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
    pub updated_at: Option<DateTime<Utc>>,
    pub labels: Vec<String>,
    pub comments: Option<u64>,
    pub unread: Option<bool>,
    pub reason: Option<String>,
    pub extra: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentPreview {
    pub author: String,
    pub body: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionSnapshot {
    pub key: String,
    pub kind: SectionKind,
    pub title: String,
    pub filters: String,
    pub items: Vec<WorkItem>,
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

    pub fn next(self) -> Self {
        match self {
            Self::PullRequests => Self::Issues,
            Self::Issues => Self::Notifications,
            Self::Notifications => Self::PullRequests,
        }
    }

    pub fn previous(self) -> Self {
        match self {
            Self::PullRequests => Self::Notifications,
            Self::Issues => Self::PullRequests,
            Self::Notifications => Self::Issues,
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
        let filters = filters.into();
        Self {
            key: section_key(kind, &title),
            kind,
            title,
            filters,
            items: Vec::new(),
            refreshed_at: None,
            error: None,
        }
    }
}

pub fn section_key(kind: SectionKind, title: &str) -> String {
    format!("{}:{title}", kind.as_str())
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

    sections
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
    let total = section.items.len();
    let unread = section
        .items
        .iter()
        .filter(|item| item.unread.unwrap_or(false))
        .count();
    (total, unread)
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
}
