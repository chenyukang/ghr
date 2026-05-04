use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use crate::model::{SectionKind, SectionSnapshot, mark_notification_read_in_section};

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

    pub fn mark_notification_read(&self, thread_id: &str) -> Result<bool> {
        let mut changed = false;
        for mut section in self.load_all()?.into_values() {
            if mark_notification_read_in_section(&mut section, thread_id) {
                self.save_section(&section)?;
                changed = true;
            }
        }
        Ok(changed)
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use chrono::Utc;

    use super::*;
    use crate::model::{ItemKind, WorkItem};

    #[test]
    fn mark_notification_read_updates_cached_notification_sections() {
        let path = std::env::temp_dir().join(format!(
            "ghr-snapshot-notification-read-{}-{}.db",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        let store = SnapshotStore::new(path.clone());
        store.init().expect("init snapshot store");

        let unread_item = notification_item("thread-1", true);
        store
            .save_section(&SectionSnapshot {
                key: "notifications:unread".to_string(),
                kind: SectionKind::Notifications,
                title: "Unread".to_string(),
                filters: "is:unread".to_string(),
                items: vec![unread_item.clone(), notification_item("thread-2", true)],
                total_count: None,
                page: 1,
                page_size: 0,
                refreshed_at: Some(Utc::now()),
                error: None,
            })
            .expect("save unread snapshot");
        store
            .save_section(&SectionSnapshot {
                key: "notifications:all".to_string(),
                kind: SectionKind::Notifications,
                title: "All".to_string(),
                filters: "is:all".to_string(),
                items: vec![unread_item],
                total_count: None,
                page: 1,
                page_size: 0,
                refreshed_at: Some(Utc::now()),
                error: None,
            })
            .expect("save all snapshot");

        assert!(
            store
                .mark_notification_read("thread-1")
                .expect("mark notification read")
        );

        let cached = store.load_all().expect("load cached snapshots");
        let unread = cached.get("notifications:unread").expect("unread section");
        assert_eq!(unread.items.len(), 1);
        assert_eq!(unread.items[0].id, "thread-2");
        let all = cached.get("notifications:all").expect("all section");
        assert_eq!(all.items[0].unread, Some(false));

        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("db-wal"));
        let _ = fs::remove_file(path.with_extension("db-shm"));
    }

    fn notification_item(id: &str, unread: bool) -> WorkItem {
        WorkItem {
            id: id.to_string(),
            kind: ItemKind::Notification,
            repo: "rust-lang/rust".to_string(),
            number: Some(1),
            title: format!("Notification {id}"),
            body: None,
            author: None,
            state: None,
            url: "https://github.com/rust-lang/rust/pull/1".to_string(),
            created_at: None,
            updated_at: None,
            labels: Vec::new(),
            reactions: Default::default(),
            milestone: None,
            assignees: Vec::new(),
            comments: None,
            unread: Some(unread),
            reason: Some("mention".to_string()),
            extra: Some("PullRequest".to_string()),
        }
    }
}
