use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use crate::model::{SectionKind, SectionSnapshot};

#[derive(Debug, Clone)]
pub struct SnapshotStore {
    path: PathBuf,
}

impl SnapshotStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn init(&self) -> Result<()> {
        let conn = self.connect()?;
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS snapshots (
                key TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                title TEXT NOT NULL,
                filters TEXT NOT NULL,
                items_json TEXT NOT NULL,
                refreshed_at TEXT NOT NULL
            );
            "#,
        )
        .context("failed to initialize snapshot database")?;
        Ok(())
    }

    pub fn load_all(&self) -> Result<HashMap<String, SectionSnapshot>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                r#"
                SELECT key, kind, title, filters, items_json, refreshed_at
                FROM snapshots
                "#,
            )
            .context("failed to prepare snapshot load")?;

        let mut rows = stmt.query([]).context("failed to load snapshots")?;
        let mut sections = HashMap::new();
        while let Some(row) = rows.next().context("failed to read snapshot row")? {
            let key: String = row.get(0)?;
            let kind_raw: String = row.get(1)?;
            let title: String = row.get(2)?;
            let filters: String = row.get(3)?;
            let items_json: String = row.get(4)?;
            let refreshed_at_raw: String = row.get(5)?;

            let kind = SectionKind::from_str(&kind_raw).map_err(anyhow::Error::msg)?;
            let items = serde_json::from_str(&items_json)
                .with_context(|| format!("failed to parse cached items for {key}"))?;
            let refreshed_at = DateTime::parse_from_rfc3339(&refreshed_at_raw)
                .map(|value| value.with_timezone(&Utc))
                .with_context(|| format!("failed to parse refreshed_at for {key}"))?;

            sections.insert(
                key.clone(),
                SectionSnapshot {
                    key,
                    kind,
                    title,
                    filters,
                    items,
                    refreshed_at: Some(refreshed_at),
                    error: None,
                },
            );
        }

        Ok(sections)
    }

    pub fn save_section(&self, section: &SectionSnapshot) -> Result<()> {
        let Some(refreshed_at) = section.refreshed_at else {
            return Ok(());
        };

        let conn = self.connect()?;
        let items_json = serde_json::to_string(&section.items)
            .with_context(|| format!("failed to encode snapshot {}", section.key))?;

        conn.execute(
            r#"
            INSERT INTO snapshots (key, kind, title, filters, items_json, refreshed_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(key) DO UPDATE SET
                kind = excluded.kind,
                title = excluded.title,
                filters = excluded.filters,
                items_json = excluded.items_json,
                refreshed_at = excluded.refreshed_at
            "#,
            params![
                &section.key,
                section.kind.as_str(),
                &section.title,
                &section.filters,
                items_json,
                refreshed_at.to_rfc3339(),
            ],
        )
        .with_context(|| format!("failed to save snapshot {}", section.key))?;

        Ok(())
    }

    fn connect(&self) -> Result<Connection> {
        Connection::open(&self.path)
            .with_context(|| format!("failed to open {}", self.path.display()))
    }
}
