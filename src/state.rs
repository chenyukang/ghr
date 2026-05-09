use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::warn;

pub const DEFAULT_LIST_WIDTH_PERCENT: u16 = 50;
pub const MIN_LIST_WIDTH_PERCENT: u16 = 30;
pub const MAX_LIST_WIDTH_PERCENT: u16 = 85;
pub const MAX_RECENT_ITEMS: usize = 200;
pub const MAX_RECENT_COMMANDS: usize = 100;
pub const MAX_GLOBAL_SAVED_SEARCHES_PER_REPO: usize = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    pub list_width_percent: u16,
    pub active_view: String,
    pub section_index: HashMap<String, usize>,
    pub selected_index: HashMap<String, usize>,
    pub view_snapshots: HashMap<String, ViewSnapshot>,
    pub focus: String,
    pub details_mode: String,
    pub details_scroll: u16,
    pub selected_comment_index: usize,
    pub expanded_comments: Vec<String>,
    pub details_scroll_by_item: HashMap<String, u16>,
    pub selected_comment_index_by_item: HashMap<String, usize>,
    pub viewed_item_at: HashMap<String, DateTime<Utc>>,
    pub selected_diff_file: HashMap<String, usize>,
    pub selected_diff_line: HashMap<String, usize>,
    pub diff_file_details_scroll: HashMap<String, u16>,
    pub ignored_items: Vec<String>,
    pub recent_items: Vec<RecentItemState>,
    pub recent_commands: Vec<RecentCommandState>,
    pub global_search_by_repo: HashMap<String, GlobalSearchState>,
    pub global_search_saved_by_repo: HashMap<String, Vec<GlobalSearchSavedState>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ViewSnapshot {
    pub focus: String,
    pub section_key: Option<String>,
    pub item_id: Option<String>,
    pub selected_index: usize,
    pub list_scroll_offset: usize,
    pub details_mode: String,
    pub details_scroll: u16,
    pub selected_comment_index: usize,
}

impl ViewSnapshot {
    fn normalized(mut self) -> Self {
        if !matches!(self.focus.as_str(), "ghr" | "sections" | "list" | "details") {
            self.focus = "list".to_string();
        }
        if !matches!(self.details_mode.as_str(), "conversation" | "diff") {
            self.details_mode = "conversation".to_string();
        }
        self.section_key = self.section_key.filter(|value| !value.trim().is_empty());
        self.item_id = self.item_id.filter(|value| !value.trim().is_empty());
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct GlobalSearchState {
    pub title: String,
    pub status: String,
    pub label: String,
    pub author: String,
    pub assignee: String,
    pub sort: String,
    pub field: String,
}

impl GlobalSearchState {
    pub fn normalized(mut self) -> Option<Self> {
        self.title = self.title.trim().to_string();
        self.status = self.status.trim().to_string();
        self.label = self.label.trim().to_string();
        self.author = self.author.trim().trim_start_matches('@').to_string();
        self.assignee = self.assignee.trim().trim_start_matches('@').to_string();
        self.sort = self.sort.trim().to_string();
        if self.sort.is_empty() {
            self.sort = "created_at".to_string();
        }
        if !matches!(
            self.field.as_str(),
            "title" | "status" | "label" | "author" | "assignee" | "sort"
        ) {
            self.field = "title".to_string();
        }
        if self.title.is_empty()
            && self.status.is_empty()
            && self.label.is_empty()
            && self.author.is_empty()
            && self.assignee.is_empty()
            && self.sort == "created_at"
        {
            return None;
        }
        Some(self)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct GlobalSearchSavedState {
    pub name: String,
    pub repo: String,
    pub kind: String,
    pub search: GlobalSearchState,
}

impl GlobalSearchSavedState {
    pub fn normalized(mut self) -> Option<Self> {
        self.name = self.name.trim().to_string();
        self.repo = self.repo.trim().to_ascii_lowercase();
        self.kind = normalized_saved_search_kind(&self.kind)?;
        self.search = self.search.normalized()?;
        if self.name.is_empty() {
            return None;
        }
        Some(self)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RecentItemState {
    pub id: String,
    pub kind: String,
    pub repo: String,
    pub number: Option<u64>,
    pub title: String,
    pub url: String,
    pub visited_at: Option<DateTime<Utc>>,
}

impl RecentItemState {
    fn normalized(mut self) -> Option<Self> {
        self.id = self.id.trim().to_string();
        self.repo = self.repo.trim().to_string();
        self.title = self.title.trim().to_string();
        self.url = self.url.trim().to_string();
        self.kind = match self.kind.trim() {
            "pr" | "pull_request" | "pull_requests" => "pull_request".to_string(),
            "issue" | "issues" => "issue".to_string(),
            _ => return None,
        };
        if self.repo.is_empty() || self.number.is_none() || self.visited_at.is_none() {
            return None;
        }
        Some(self)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RecentCommandState {
    pub id: String,
    pub selected_at: Option<DateTime<Utc>>,
}

impl RecentCommandState {
    fn normalized(mut self) -> Option<Self> {
        self.id = self.id.trim().to_string();
        if self.id.is_empty() || self.selected_at.is_none() {
            return None;
        }
        Some(self)
    }
}

impl UiState {
    pub fn load_or_default(path: &Path) -> Self {
        let Ok(content) = fs::read_to_string(path) else {
            return Self::default();
        };

        match toml::from_str::<Self>(&content) {
            Ok(state) => state.normalized(),
            Err(error) => {
                warn!(error = %error, path = %path.display(), "failed to parse ui state");
                Self::default()
            }
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let content = toml::to_string_pretty(&self.clone().normalized())
            .context("failed to encode ui state")?;
        fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))
    }

    pub fn normalized(mut self) -> Self {
        self.list_width_percent = clamp_list_width_percent(self.list_width_percent);
        if !matches!(self.focus.as_str(), "ghr" | "sections" | "list" | "details") {
            self.focus = "list".to_string();
        }
        if !matches!(self.details_mode.as_str(), "conversation" | "diff") {
            self.details_mode = "conversation".to_string();
        }
        let mut seen = HashSet::new();
        self.expanded_comments
            .retain(|key| !key.trim().is_empty() && seen.insert(key.clone()));
        self.expanded_comments.sort();
        self.view_snapshots.retain(|key, snapshot| {
            if key.trim().is_empty() {
                return false;
            }
            *snapshot = snapshot.clone().normalized();
            true
        });
        self.details_scroll_by_item
            .retain(|key, _| !key.trim().is_empty());
        self.selected_comment_index_by_item
            .retain(|key, _| !key.trim().is_empty());
        self.viewed_item_at.retain(|key, _| !key.trim().is_empty());
        self.diff_file_details_scroll
            .retain(|key, _| !key.trim().is_empty());
        let mut seen_ignored = HashSet::new();
        self.ignored_items
            .retain(|key| !key.trim().is_empty() && seen_ignored.insert(key.clone()));
        self.ignored_items.sort();
        self.recent_items = normalized_recent_items(self.recent_items);
        self.recent_commands = normalized_recent_commands(self.recent_commands);
        self.global_search_by_repo = normalized_global_search_by_repo(self.global_search_by_repo);
        self.global_search_saved_by_repo =
            normalized_global_search_saved_by_repo(self.global_search_saved_by_repo);
        self
    }
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            list_width_percent: DEFAULT_LIST_WIDTH_PERCENT,
            active_view: String::new(),
            section_index: HashMap::new(),
            selected_index: HashMap::new(),
            view_snapshots: HashMap::new(),
            focus: "list".to_string(),
            details_mode: "conversation".to_string(),
            details_scroll: 0,
            selected_comment_index: 0,
            expanded_comments: Vec::new(),
            details_scroll_by_item: HashMap::new(),
            selected_comment_index_by_item: HashMap::new(),
            viewed_item_at: HashMap::new(),
            selected_diff_file: HashMap::new(),
            selected_diff_line: HashMap::new(),
            diff_file_details_scroll: HashMap::new(),
            ignored_items: Vec::new(),
            recent_items: Vec::new(),
            recent_commands: Vec::new(),
            global_search_by_repo: HashMap::new(),
            global_search_saved_by_repo: HashMap::new(),
        }
    }
}

pub fn clamp_list_width_percent(value: u16) -> u16 {
    value.clamp(MIN_LIST_WIDTH_PERCENT, MAX_LIST_WIDTH_PERCENT)
}

fn normalized_recent_items(items: Vec<RecentItemState>) -> Vec<RecentItemState> {
    let mut normalized = items
        .into_iter()
        .filter_map(RecentItemState::normalized)
        .collect::<Vec<_>>();
    normalized.sort_by_key(|item| std::cmp::Reverse(item.visited_at));

    let mut seen = HashSet::new();
    normalized.retain(|item| seen.insert(recent_item_key(item)));
    normalized.truncate(MAX_RECENT_ITEMS);
    normalized
}

fn recent_item_key(item: &RecentItemState) -> String {
    format!(
        "{}:{}:{}",
        item.kind,
        item.repo.to_ascii_lowercase(),
        item.number.unwrap_or_default()
    )
}

fn normalized_recent_commands(items: Vec<RecentCommandState>) -> Vec<RecentCommandState> {
    let mut normalized = items
        .into_iter()
        .filter_map(RecentCommandState::normalized)
        .collect::<Vec<_>>();
    normalized.sort_by_key(|item| std::cmp::Reverse(item.selected_at));

    let mut seen = HashSet::new();
    normalized.retain(|item| seen.insert(item.id.to_ascii_lowercase()));
    normalized.truncate(MAX_RECENT_COMMANDS);
    normalized
}

fn normalized_global_search_by_repo(
    items: HashMap<String, GlobalSearchState>,
) -> HashMap<String, GlobalSearchState> {
    items
        .into_iter()
        .filter_map(|(repo, state)| {
            let repo = normalized_global_search_repo_key(&repo)?;
            state.normalized().map(|state| (repo, state))
        })
        .collect()
}

fn normalized_global_search_saved_by_repo(
    items: HashMap<String, Vec<GlobalSearchSavedState>>,
) -> HashMap<String, Vec<GlobalSearchSavedState>> {
    items
        .into_iter()
        .filter_map(|(repo, searches)| {
            let repo = normalized_global_search_repo_key(&repo)?;
            let searches = normalized_global_search_saved_searches(&repo, searches);
            (!searches.is_empty()).then_some((repo, searches))
        })
        .collect()
}

fn normalized_global_search_saved_searches(
    repo: &str,
    items: Vec<GlobalSearchSavedState>,
) -> Vec<GlobalSearchSavedState> {
    let mut normalized = items
        .into_iter()
        .filter_map(|mut item| {
            if item.repo.trim().is_empty() {
                item.repo = repo.to_string();
            }
            item.normalized()
        })
        .collect::<Vec<_>>();
    normalized.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
    });

    let mut seen = HashSet::new();
    normalized.retain(|item| seen.insert(item.name.to_ascii_lowercase()));
    normalized.truncate(MAX_GLOBAL_SAVED_SEARCHES_PER_REPO);
    normalized
}

fn normalized_saved_search_kind(value: &str) -> Option<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "pr" | "prs" | "pull_request" | "pull_requests" | "pull request" | "pull requests" => {
            Some("pull_requests".to_string())
        }
        "issue" | "issues" => Some("issues".to_string()),
        _ => None,
    }
}

fn normalized_global_search_repo_key(repo: &str) -> Option<String> {
    let repo = repo.trim().to_ascii_lowercase();
    if repo.is_empty() { None } else { Some(repo) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_state_round_trips_and_clamps_split_ratio() {
        let path = std::env::temp_dir().join(format!(
            "ghr-ui-state-{}-{}.toml",
            std::process::id(),
            "roundtrip"
        ));
        let _ = fs::remove_file(&path);

        UiState {
            list_width_percent: 99,
            active_view: "issues".to_string(),
            section_index: HashMap::from([("issues".to_string(), 1)]),
            selected_index: HashMap::from([("issues".to_string(), 3)]),
            view_snapshots: HashMap::from([(
                "repo:Fiber".to_string(),
                ViewSnapshot {
                    focus: "details".to_string(),
                    section_key: Some("repo:Fiber:pull_requests:Pull Requests".to_string()),
                    item_id: Some("fiber-1".to_string()),
                    selected_index: 2,
                    list_scroll_offset: 7,
                    details_mode: "diff".to_string(),
                    details_scroll: 11,
                    selected_comment_index: 1,
                },
            )]),
            focus: "details".to_string(),
            details_mode: "diff".to_string(),
            details_scroll: 8,
            selected_comment_index: 2,
            expanded_comments: vec!["1:comment:42".to_string()],
            details_scroll_by_item: HashMap::from([("issue-3".to_string(), 12)]),
            selected_comment_index_by_item: HashMap::from([("issue-3".to_string(), 4)]),
            viewed_item_at: HashMap::from([(
                "issue:rust-lang/rust:3".to_string(),
                DateTime::from_timestamp(1_700_000_030, 0).unwrap(),
            )]),
            selected_diff_file: HashMap::from([("issue-3".to_string(), 4)]),
            selected_diff_line: HashMap::from([("issue-3".to_string(), 9)]),
            diff_file_details_scroll: HashMap::from([("issue-3::src/lib.rs".to_string(), 17)]),
            ignored_items: vec!["issue-2".to_string(), "issue-2".to_string(), String::new()],
            recent_items: vec![
                RecentItemState {
                    id: "pr-1".to_string(),
                    kind: "pull_request".to_string(),
                    repo: "rust-lang/rust".to_string(),
                    number: Some(1),
                    title: "Compiler diagnostics".to_string(),
                    url: "https://github.com/rust-lang/rust/pull/1".to_string(),
                    visited_at: Some(DateTime::from_timestamp(1_700_000_000, 0).unwrap()),
                },
                RecentItemState {
                    id: "pr-1-duplicate".to_string(),
                    kind: "pr".to_string(),
                    repo: "RUST-LANG/RUST".to_string(),
                    number: Some(1),
                    title: "Older duplicate".to_string(),
                    url: "https://github.com/rust-lang/rust/pull/1".to_string(),
                    visited_at: Some(DateTime::from_timestamp(1_600_000_000, 0).unwrap()),
                },
            ],
            recent_commands: vec![
                RecentCommandState {
                    id: "Refresh".to_string(),
                    selected_at: Some(DateTime::from_timestamp(1_700_000_010, 0).unwrap()),
                },
                RecentCommandState {
                    id: "refresh".to_string(),
                    selected_at: Some(DateTime::from_timestamp(1_600_000_000, 0).unwrap()),
                },
                RecentCommandState {
                    id: String::new(),
                    selected_at: Some(DateTime::from_timestamp(1_700_000_020, 0).unwrap()),
                },
            ],
            global_search_by_repo: HashMap::from([(
                "Rust-Lang/Rust".to_string(),
                GlobalSearchState {
                    title: "borrowck".to_string(),
                    status: "open".to_string(),
                    label: "T-compiler".to_string(),
                    author: "@alice".to_string(),
                    assignee: "bob".to_string(),
                    sort: String::new(),
                    field: "author".to_string(),
                },
            )]),
            global_search_saved_by_repo: HashMap::from([(
                "Rust-Lang/Rust".to_string(),
                vec![
                    GlobalSearchSavedState {
                        name: "open atomic issues".to_string(),
                        repo: String::new(),
                        kind: "issues".to_string(),
                        search: GlobalSearchState {
                            title: "atomic".to_string(),
                            status: "open".to_string(),
                            label: String::new(),
                            author: "chenyukang".to_string(),
                            assignee: String::new(),
                            sort: String::new(),
                            field: "author".to_string(),
                        },
                    },
                    GlobalSearchSavedState {
                        name: String::new(),
                        repo: "rust-lang/rust".to_string(),
                        kind: "issue".to_string(),
                        search: GlobalSearchState {
                            title: "atomic".to_string(),
                            status: "open".to_string(),
                            label: String::new(),
                            author: "@chenyukang".to_string(),
                            assignee: String::new(),
                            sort: String::new(),
                            field: "wat".to_string(),
                        },
                    },
                ],
            )]),
        }
        .save(&path)
        .expect("save state");

        let state = UiState::load_or_default(&path);

        assert_eq!(state.list_width_percent, MAX_LIST_WIDTH_PERCENT);
        assert_eq!(state.active_view, "issues");
        assert_eq!(state.section_index.get("issues"), Some(&1));
        assert_eq!(state.selected_index.get("issues"), Some(&3));
        assert_eq!(
            state.view_snapshots.get("repo:Fiber"),
            Some(&ViewSnapshot {
                focus: "details".to_string(),
                section_key: Some("repo:Fiber:pull_requests:Pull Requests".to_string()),
                item_id: Some("fiber-1".to_string()),
                selected_index: 2,
                list_scroll_offset: 7,
                details_mode: "diff".to_string(),
                details_scroll: 11,
                selected_comment_index: 1,
            })
        );
        assert_eq!(state.focus, "details");
        assert_eq!(state.details_mode, "diff");
        assert_eq!(state.details_scroll, 8);
        assert_eq!(state.selected_comment_index, 2);
        assert_eq!(state.expanded_comments, vec!["1:comment:42"]);
        assert_eq!(state.details_scroll_by_item.get("issue-3"), Some(&12));
        assert_eq!(
            state.selected_comment_index_by_item.get("issue-3"),
            Some(&4)
        );
        assert_eq!(
            state.viewed_item_at.get("issue:rust-lang/rust:3"),
            Some(&DateTime::from_timestamp(1_700_000_030, 0).unwrap())
        );
        assert_eq!(state.selected_diff_file.get("issue-3"), Some(&4));
        assert_eq!(state.selected_diff_line.get("issue-3"), Some(&9));
        assert_eq!(
            state.diff_file_details_scroll.get("issue-3::src/lib.rs"),
            Some(&17)
        );
        assert_eq!(state.ignored_items, vec!["issue-2"]);
        assert_eq!(
            state.recent_items,
            vec![RecentItemState {
                id: "pr-1".to_string(),
                kind: "pull_request".to_string(),
                repo: "rust-lang/rust".to_string(),
                number: Some(1),
                title: "Compiler diagnostics".to_string(),
                url: "https://github.com/rust-lang/rust/pull/1".to_string(),
                visited_at: Some(DateTime::from_timestamp(1_700_000_000, 0).unwrap()),
            }]
        );
        assert_eq!(
            state.recent_commands,
            vec![RecentCommandState {
                id: "Refresh".to_string(),
                selected_at: Some(DateTime::from_timestamp(1_700_000_010, 0).unwrap()),
            }]
        );
        assert_eq!(
            state.global_search_by_repo.get("rust-lang/rust"),
            Some(&GlobalSearchState {
                title: "borrowck".to_string(),
                status: "open".to_string(),
                label: "T-compiler".to_string(),
                author: "alice".to_string(),
                assignee: "bob".to_string(),
                sort: "created_at".to_string(),
                field: "author".to_string(),
            })
        );
        assert_eq!(
            state
                .global_search_saved_by_repo
                .get("rust-lang/rust")
                .and_then(|items| items.first()),
            Some(&GlobalSearchSavedState {
                name: "open atomic issues".to_string(),
                repo: "rust-lang/rust".to_string(),
                kind: "issues".to_string(),
                search: GlobalSearchState {
                    title: "atomic".to_string(),
                    status: "open".to_string(),
                    label: String::new(),
                    author: "chenyukang".to_string(),
                    assignee: String::new(),
                    sort: "created_at".to_string(),
                    field: "author".to_string(),
                },
            })
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn default_split_ratio_is_even() {
        assert_eq!(UiState::default().list_width_percent, 50);
    }

    #[test]
    fn recent_items_are_newest_first_deduped_and_limited() {
        let state = UiState {
            recent_items: (0..205)
                .map(|number| RecentItemState {
                    id: format!("pr-{number}"),
                    kind: "pr".to_string(),
                    repo: "chenyukang/ghr".to_string(),
                    number: Some(number),
                    title: format!("PR {number}"),
                    url: format!("https://github.com/chenyukang/ghr/pull/{number}"),
                    visited_at: Some(
                        DateTime::from_timestamp(1_700_000_000 + number as i64, 0).unwrap(),
                    ),
                })
                .chain([RecentItemState {
                    id: "duplicate".to_string(),
                    kind: "pull_request".to_string(),
                    repo: "CHENYUKANG/GHR".to_string(),
                    number: Some(204),
                    title: "older duplicate".to_string(),
                    url: "https://github.com/chenyukang/ghr/pull/204".to_string(),
                    visited_at: Some(DateTime::from_timestamp(1_600_000_000, 0).unwrap()),
                }])
                .collect(),
            ..UiState::default()
        }
        .normalized();

        assert_eq!(state.recent_items.len(), MAX_RECENT_ITEMS);
        assert_eq!(
            state.recent_items.first().and_then(|item| item.number),
            Some(204)
        );
        assert_eq!(
            state.recent_items.last().and_then(|item| item.number),
            Some(5)
        );
        assert_eq!(state.recent_items[0].kind, "pull_request");
    }

    #[test]
    fn recent_commands_are_newest_first_deduped_and_limited() {
        let state = UiState {
            recent_commands: (0..105)
                .map(|number| RecentCommandState {
                    id: format!("Command {number}"),
                    selected_at: Some(
                        DateTime::from_timestamp(1_700_000_000 + number as i64, 0).unwrap(),
                    ),
                })
                .chain([RecentCommandState {
                    id: "command 104".to_string(),
                    selected_at: Some(DateTime::from_timestamp(1_600_000_000, 0).unwrap()),
                }])
                .collect(),
            ..UiState::default()
        }
        .normalized();

        assert_eq!(state.recent_commands.len(), MAX_RECENT_COMMANDS);
        assert_eq!(
            state.recent_commands.first().map(|item| item.id.as_str()),
            Some("Command 104")
        );
        assert_eq!(
            state.recent_commands.last().map(|item| item.id.as_str()),
            Some("Command 5")
        );
    }

    #[test]
    fn invalid_focus_falls_back_to_list() {
        let state = UiState {
            focus: "somewhere".to_string(),
            details_mode: "elsewhere".to_string(),
            ..UiState::default()
        }
        .normalized();

        assert_eq!(state.focus, "list");
        assert_eq!(state.details_mode, "conversation");
    }
}
