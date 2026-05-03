use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::warn;

pub const DEFAULT_LIST_WIDTH_PERCENT: u16 = 50;
pub const MIN_LIST_WIDTH_PERCENT: u16 = 30;
pub const MAX_LIST_WIDTH_PERCENT: u16 = 85;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    pub list_width_percent: u16,
    pub active_view: String,
    pub section_index: HashMap<String, usize>,
    pub selected_index: HashMap<String, usize>,
    pub focus: String,
    pub details_mode: String,
    pub details_scroll: u16,
    pub selected_comment_index: usize,
    pub expanded_comments: Vec<String>,
    pub selected_diff_file: HashMap<String, usize>,
    pub selected_diff_line: HashMap<String, usize>,
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
            focus: "list".to_string(),
            details_mode: "conversation".to_string(),
            details_scroll: 0,
            selected_comment_index: 0,
            expanded_comments: Vec::new(),
            selected_diff_file: HashMap::new(),
            selected_diff_line: HashMap::new(),
        }
    }
}

pub fn clamp_list_width_percent(value: u16) -> u16 {
    value.clamp(MIN_LIST_WIDTH_PERCENT, MAX_LIST_WIDTH_PERCENT)
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
            focus: "details".to_string(),
            details_mode: "diff".to_string(),
            details_scroll: 8,
            selected_comment_index: 2,
            expanded_comments: vec!["1:comment:42".to_string()],
            selected_diff_file: HashMap::from([("issue-3".to_string(), 4)]),
            selected_diff_line: HashMap::from([("issue-3".to_string(), 9)]),
        }
        .save(&path)
        .expect("save state");

        let state = UiState::load_or_default(&path);

        assert_eq!(state.list_width_percent, MAX_LIST_WIDTH_PERCENT);
        assert_eq!(state.active_view, "issues");
        assert_eq!(state.section_index.get("issues"), Some(&1));
        assert_eq!(state.selected_index.get("issues"), Some(&3));
        assert_eq!(state.focus, "details");
        assert_eq!(state.details_mode, "diff");
        assert_eq!(state.details_scroll, 8);
        assert_eq!(state.selected_comment_index, 2);
        assert_eq!(state.expanded_comments, vec!["1:comment:42"]);
        assert_eq!(state.selected_diff_file.get("issue-3"), Some(&4));
        assert_eq!(state.selected_diff_line.get("issue-3"), Some(&9));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn default_split_ratio_is_even() {
        assert_eq!(UiState::default().list_width_percent, 50);
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
