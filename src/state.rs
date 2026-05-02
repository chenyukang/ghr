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
        self
    }
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            list_width_percent: DEFAULT_LIST_WIDTH_PERCENT,
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
        }
        .save(&path)
        .expect("save state");

        let state = UiState::load_or_default(&path);

        assert_eq!(state.list_width_percent, MAX_LIST_WIDTH_PERCENT);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn default_split_ratio_is_even() {
        assert_eq!(UiState::default().list_width_percent, 50);
    }
}
