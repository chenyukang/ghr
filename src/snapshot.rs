use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use crate::model::{
    SectionKind, SectionSnapshot, mark_all_notifications_read_in_section,
    mark_notification_read_in_section,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoCandidateCache {
    pub labels: HashMap<String, Vec<String>>,
    pub assignees: HashMap<String, Vec<String>>,
    pub reviewers: HashMap<String, Vec<String>>,
}

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
            CREATE TABLE IF NOT EXISTS repo_candidate_cache (
                repo TEXT NOT NULL,
                kind TEXT NOT NULL,
                values_json TEXT NOT NULL,
                refreshed_at TEXT NOT NULL,
                PRIMARY KEY (repo, kind)
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

    pub fn load_repo_candidate_cache(&self) -> Result<RepoCandidateCache> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare(
                r#"
                SELECT repo, kind, values_json
                FROM repo_candidate_cache
                "#,
            )
            .context("failed to prepare repo candidate cache load")?;

        let mut rows = stmt
            .query([])
            .context("failed to load repo candidate cache")?;
        let mut cache = RepoCandidateCache::default();
        while let Some(row) = rows.next().context("failed to read candidate cache row")? {
            let repo: String = row.get(0)?;
            let kind: String = row.get(1)?;
            let values_json: String = row.get(2)?;
            let values = serde_json::from_str(&values_json)
                .with_context(|| format!("failed to parse cached {kind} candidates for {repo}"))?;
            match kind.as_str() {
                "labels" => {
                    cache.labels.insert(repo, values);
                }
                "assignees" => {
                    cache.assignees.insert(repo, values);
                }
                "reviewers" => {
                    cache.reviewers.insert(repo, values);
                }
                _ => {}
            }
        }

        Ok(cache)
    }

    pub fn save_label_candidates(&self, repo: &str, labels: &[String]) -> Result<()> {
        self.save_repo_candidates(repo, "labels", labels)
    }

    pub fn save_assignee_candidates(&self, repo: &str, assignees: &[String]) -> Result<()> {
        self.save_repo_candidates(repo, "assignees", assignees)
    }

    pub fn save_reviewer_candidates(&self, repo: &str, reviewers: &[String]) -> Result<()> {
        self.save_repo_candidates(repo, "reviewers", reviewers)
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

    pub fn mark_all_notifications_read(&self) -> Result<bool> {
        let mut changed = false;
        let last_read_at = Utc::now();
        for mut section in self.load_all()?.into_values() {
            if mark_all_notifications_read_in_section(&mut section, last_read_at) {
                self.save_section(&section)?;
                changed = true;
            }
        }
        Ok(changed)
    }

    pub fn clear_snapshots(&self) -> Result<usize> {
        let conn = self.connect()?;
        conn.execute("DELETE FROM snapshots", [])
            .context("failed to clear snapshot cache")
    }

    pub fn clear_snapshots_by_keys(&self, keys: &[String]) -> Result<usize> {
        let conn = self.connect()?;
        let mut keys = keys.to_vec();
        keys.sort();
        keys.dedup();
        let mut changed = 0;
        for key in keys {
            changed += conn
                .execute("DELETE FROM snapshots WHERE key = ?1", params![key])
                .with_context(|| format!("failed to clear snapshot {key}"))?;
        }
        Ok(changed)
    }

    pub fn clear_repo_candidate_cache(&self) -> Result<usize> {
        let conn = self.connect()?;
        conn.execute("DELETE FROM repo_candidate_cache", [])
            .context("failed to clear repo candidate cache")
    }

    fn save_repo_candidates(&self, repo: &str, kind: &str, values: &[String]) -> Result<()> {
        let conn = self.connect()?;
        let values_json = serde_json::to_string(values)
            .with_context(|| format!("failed to encode cached {kind} candidates for {repo}"))?;
        conn.execute(
            r#"
            INSERT INTO repo_candidate_cache (repo, kind, values_json, refreshed_at)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(repo, kind) DO UPDATE SET
                values_json = excluded.values_json,
                refreshed_at = excluded.refreshed_at
            "#,
            params![repo, kind, values_json, Utc::now().to_rfc3339()],
        )
        .with_context(|| format!("failed to save cached {kind} candidates for {repo}"))?;

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

    #[test]
    fn mark_all_notifications_read_updates_cached_notification_sections() {
        let path = temp_db_path("snapshot-mark-all-notifications-read");
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
                .mark_all_notifications_read()
                .expect("mark all notifications read")
        );

        let cached = store.load_all().expect("load cached snapshots");
        let unread = cached.get("notifications:unread").expect("unread section");
        assert!(unread.items.is_empty());
        let all = cached.get("notifications:all").expect("all section");
        assert_eq!(all.items[0].unread, Some(false));
        assert!(all.items[0].last_read_at.is_some());

        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("db-wal"));
        let _ = fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn repo_candidate_cache_round_trips_and_updates() {
        let path = temp_db_path("repo-candidate-cache");
        let store = SnapshotStore::new(path.clone());
        store.init().expect("init snapshot store");

        store
            .save_label_candidates(
                "rust-lang/rust",
                &["T-compiler".to_string(), "A-diagnostics".to_string()],
            )
            .expect("save label candidates");
        store
            .save_assignee_candidates(
                "rust-lang/rust",
                &["rustbot".to_string(), "compiler-errors".to_string()],
            )
            .expect("save assignee candidates");
        store
            .save_reviewer_candidates(
                "rust-lang/rust",
                &["reviewer-a".to_string(), "reviewer-b".to_string()],
            )
            .expect("save reviewer candidates");

        let cache = store
            .load_repo_candidate_cache()
            .expect("load repo candidate cache");
        assert_eq!(
            cache.labels.get("rust-lang/rust"),
            Some(&vec!["T-compiler".to_string(), "A-diagnostics".to_string()])
        );
        assert_eq!(
            cache.assignees.get("rust-lang/rust"),
            Some(&vec!["rustbot".to_string(), "compiler-errors".to_string()])
        );
        assert_eq!(
            cache.reviewers.get("rust-lang/rust"),
            Some(&vec!["reviewer-a".to_string(), "reviewer-b".to_string()])
        );

        store
            .save_label_candidates("rust-lang/rust", &["T-rustdoc".to_string()])
            .expect("update label candidates");
        let cache = store
            .load_repo_candidate_cache()
            .expect("reload repo candidate cache");
        assert_eq!(
            cache.labels.get("rust-lang/rust"),
            Some(&vec!["T-rustdoc".to_string()])
        );

        remove_db_files(&path);
    }

    #[test]
    fn clear_cache_methods_remove_selected_snapshot_and_candidate_rows() {
        let path = temp_db_path("clear-cache");
        let store = SnapshotStore::new(path.clone());
        store.init().expect("init snapshot store");

        store
            .save_section(&SectionSnapshot {
                key: "pull_requests:mine".to_string(),
                kind: SectionKind::PullRequests,
                title: "Mine".to_string(),
                filters: "is:open author:@me".to_string(),
                items: vec![notification_item("item-1", false)],
                total_count: None,
                page: 1,
                page_size: 0,
                refreshed_at: Some(Utc::now()),
                error: None,
            })
            .expect("save first snapshot");
        store
            .save_section(&SectionSnapshot {
                key: "issues:mine".to_string(),
                kind: SectionKind::Issues,
                title: "Mine".to_string(),
                filters: "is:open assignee:@me".to_string(),
                items: vec![notification_item("item-2", false)],
                total_count: None,
                page: 1,
                page_size: 0,
                refreshed_at: Some(Utc::now()),
                error: None,
            })
            .expect("save second snapshot");
        store
            .save_label_candidates("rust-lang/rust", &["T-compiler".to_string()])
            .expect("save candidate cache");

        assert_eq!(
            store
                .clear_snapshots_by_keys(&["pull_requests:mine".to_string()])
                .expect("clear selected snapshot"),
            1
        );
        let snapshots = store.load_all().expect("load snapshots");
        assert!(!snapshots.contains_key("pull_requests:mine"));
        assert!(snapshots.contains_key("issues:mine"));

        assert_eq!(
            store
                .clear_repo_candidate_cache()
                .expect("clear candidate cache"),
            1
        );
        assert!(
            store
                .load_repo_candidate_cache()
                .expect("load candidate cache")
                .labels
                .is_empty()
        );
        assert_eq!(store.clear_snapshots().expect("clear all snapshots"), 1);
        assert!(store.load_all().expect("reload snapshots").is_empty());

        remove_db_files(&path);
    }

    fn temp_db_path(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "ghr-{prefix}-{}-{}.db",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ))
    }

    fn remove_db_files(path: &PathBuf) {
        let _ = fs::remove_file(path);
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
            last_read_at: None,
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
