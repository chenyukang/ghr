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
                total_count INTEGER,
                page INTEGER NOT NULL DEFAULT 1,
                page_size INTEGER NOT NULL DEFAULT 0,
                refreshed_at TEXT NOT NULL
            );
            "#,
        )
        .context("failed to initialize snapshot database")?;
        ensure_snapshot_column(&conn, "total_count", "INTEGER")?;
        ensure_snapshot_column(&conn, "page", "INTEGER NOT NULL DEFAULT 1")?;
        ensure_snapshot_column(&conn, "page_size", "INTEGER NOT NULL DEFAULT 0")?;
        Ok(())
    }

    pub fn load_all(&self) -> Result<HashMap<String, SectionSnapshot>> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                r#"
                SELECT key, kind, title, filters, items_json, total_count, page, page_size, refreshed_at
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
            let total_count_raw: Option<i64> = row.get(5)?;
            let total_count = total_count_raw.and_then(|value| usize::try_from(value).ok());
            let page_raw: i64 = row.get(6)?;
            let page = usize::try_from(page_raw)
                .ok()
                .filter(|page| *page > 0)
                .unwrap_or(1);
            let page_size_raw: i64 = row.get(7)?;
            let page_size = usize::try_from(page_size_raw).unwrap_or(0);
            let refreshed_at_raw: String = row.get(8)?;

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
                    total_count,
                    page,
                    page_size,
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
            INSERT INTO snapshots (key, kind, title, filters, items_json, total_count, page, page_size, refreshed_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(key) DO UPDATE SET
                kind = excluded.kind,
                title = excluded.title,
                filters = excluded.filters,
                items_json = excluded.items_json,
                total_count = excluded.total_count,
                page = excluded.page,
                page_size = excluded.page_size,
                refreshed_at = excluded.refreshed_at
            "#,
            params![
                &section.key,
                section.kind.as_str(),
                &section.title,
                &section.filters,
                items_json,
                section.total_count.map(|value| value as i64),
                section.page as i64,
                section.page_size as i64,
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

fn ensure_snapshot_column(conn: &Connection, name: &str, definition: &str) -> Result<()> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(snapshots)")
        .context("failed to inspect snapshot schema")?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .context("failed to read snapshot schema")?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to parse snapshot schema")?;
    if !columns.iter().any(|column| column == name) {
        conn.execute(
            &format!("ALTER TABLE snapshots ADD COLUMN {name} {definition}"),
            [],
        )
        .with_context(|| format!("failed to migrate snapshot {name} column"))?;
    }
    Ok(())
}
