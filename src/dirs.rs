use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct Paths {
    pub root: PathBuf,
    pub config_path: PathBuf,
    pub db_path: PathBuf,
    pub log_path: PathBuf,
    pub state_path: PathBuf,
}

impl Paths {
    pub fn resolve(config_path: Option<PathBuf>) -> Result<Self> {
        let home = dirs::home_dir().context("could not locate home directory")?;
        let root = home.join(".ghr");
        let config_path = config_path.unwrap_or_else(|| root.join("config.toml"));
        let db_path = root.join("ghr.db");
        let log_path = root.join("ghr.log");
        let state_path = root.join("state.toml");

        Ok(Self {
            root,
            config_path,
            db_path,
            log_path,
            state_path,
        })
    }

    pub fn ensure(&self) -> Result<()> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("failed to create {}", self.root.display()))?;
        if let Some(parent) = self.config_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        Ok(())
    }
}
