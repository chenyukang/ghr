use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::SectionKind;
use crate::state::{GlobalSearchSavedState, GlobalSearchState};
use crate::theme::{ThemeName, ThemePreference};

pub const DEFAULT_COMMAND_PALETTE_KEY: &str = ":";
pub const DEFAULT_LOG_LEVEL: &str = "info";
pub const DEFAULT_REPO_REMOTE: &str = "origin";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub defaults: Defaults,
    pub exclude_repos: Vec<String>,
    pub repos: Vec<RepoConfig>,
    pub pr_sections: Vec<SearchSection>,
    pub issue_sections: Vec<SearchSection>,
    pub notification_sections: Vec<SearchSection>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub saved_search_filters: Vec<SavedSearchFilterConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Defaults {
    pub view: SectionKind,
    pub command_palette_key: String,
    pub theme: ThemePreference,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme_name: Option<ThemeName>,
    pub log_level: String,
    pub pr_per_page: usize,
    pub issue_per_page: usize,
    pub notification_limit: usize,
    pub refetch_interval_seconds: u64,
    pub include_read_notifications: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchSection {
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub filters: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub queries: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RepoConfig {
    pub name: String,
    pub repo: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_dir: Option<String>,
    pub show_prs: bool,
    pub show_issues: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pr_labels: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub issue_labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubRemoteCandidate {
    pub remote: String,
    pub repo: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentRepoRemotePrompt {
    pub directory: PathBuf,
    pub candidates: Vec<GitHubRemoteCandidate>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct SavedSearchFilterConfig {
    pub name: String,
    pub repo: String,
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub status: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub author: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub assignee: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sort: String,
}

impl SavedSearchFilterConfig {
    pub fn from_saved_state(saved: GlobalSearchSavedState) -> Option<Self> {
        let saved = saved.normalized()?;
        Some(Self {
            name: saved.name,
            repo: saved.repo,
            kind: saved.kind,
            title: saved.search.title,
            status: saved.search.status,
            label: saved.search.label,
            author: saved.search.author,
            assignee: saved.search.assignee,
            sort: saved.search.sort,
        })
    }

    pub fn into_saved_state(self) -> Option<GlobalSearchSavedState> {
        let search = GlobalSearchState {
            title: self.title,
            status: self.status,
            label: self.label,
            author: self.author,
            assignee: self.assignee,
            sort: self.sort,
            field: String::new(),
        };
        GlobalSearchSavedState {
            name: self.name,
            repo: self.repo,
            kind: self.kind,
            search,
        }
        .normalized()
    }
}

impl SearchSection {
    pub fn search_filters(&self) -> Vec<String> {
        if self.queries.is_empty() {
            vec![self.filters.clone()]
        } else {
            self.queries.clone()
        }
        .into_iter()
        .filter(|query| !query.trim().is_empty())
        .collect()
    }

    pub fn display_filters(&self) -> String {
        if self.queries.is_empty() {
            self.filters.clone()
        } else {
            self.queries.join(" | ")
        }
    }
}

impl Config {
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if path.exists() {
            let content = fs::read_to_string(path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            return toml::from_str(&content)
                .with_context(|| format!("failed to parse {}", path.display()));
        }

        let config = Self::default();
        let content = toml::to_string_pretty(&config).context("failed to encode default config")?;
        fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))?;
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let content = toml::to_string_pretty(self).context("failed to encode config")?;
        fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))
    }

    pub fn include_current_git_repo_and_save(self, path: &Path) -> (Self, Option<Result<String>>) {
        let Some(directory) = std::env::current_dir().ok() else {
            return (self, None);
        };
        self.include_git_repo_at_and_save(path, &directory)
    }

    pub fn current_repo_remote_prompt(&self) -> Option<CurrentRepoRemotePrompt> {
        let directory = std::env::current_dir().ok()?;
        self.current_repo_remote_prompt_for_directory(&directory)
    }

    pub fn current_repo_remote_prompt_for_directory(
        &self,
        directory: &Path,
    ) -> Option<CurrentRepoRemotePrompt> {
        let candidates = current_github_remote_candidates(directory);
        if candidates.len() <= 1
            || self.current_directory_remote_is_configured(directory, &candidates)
        {
            return None;
        }
        Some(CurrentRepoRemotePrompt {
            directory: directory.to_path_buf(),
            candidates,
        })
    }

    pub fn remove_repo_at(&mut self, index: usize) -> Option<RepoConfig> {
        (index < self.repos.len()).then(|| self.repos.remove(index))
    }

    fn include_git_repo_at_and_save(
        mut self,
        path: &Path,
        directory: &Path,
    ) -> (Self, Option<Result<String>>) {
        let Some(candidate) = current_github_repo_in(directory) else {
            return (self, None);
        };
        let local_dir = Some(directory.display().to_string());
        let remote = repo_remote_config_value(&candidate.remote);
        if !self.add_runtime_repo_with_local_dir_and_remote(
            candidate.repo.clone(),
            local_dir,
            remote,
        ) {
            return (self, None);
        }

        let save_result = self.save(path).map(|()| candidate.repo);
        (self, Some(save_result))
    }

    #[cfg(test)]
    fn add_runtime_repo_with_local_dir(&mut self, repo: String, local_dir: Option<String>) -> bool {
        self.add_runtime_repo_with_local_dir_and_remote(repo, local_dir, None)
    }

    pub fn add_runtime_repo_with_local_dir_and_remote(
        &mut self,
        repo: String,
        local_dir: Option<String>,
        remote: Option<String>,
    ) -> bool {
        if let Some(configured) = self
            .repos
            .iter_mut()
            .find(|configured| configured.repo.eq_ignore_ascii_case(&repo))
        {
            let has_local_dir = configured
                .local_dir
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty());
            let has_remote = configured
                .remote
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty());
            let should_set_local_dir = !has_local_dir && local_dir.is_some();
            let should_set_remote = !has_remote && remote.is_some();
            if !should_set_local_dir && !should_set_remote {
                return false;
            }
            if should_set_local_dir {
                configured.local_dir = local_dir;
            }
            if should_set_remote {
                configured.remote = remote;
            }
            return true;
        }

        let name = runtime_repo_name(&self.repos, &repo);
        self.repos.insert(
            0,
            RepoConfig {
                name,
                repo,
                remote,
                local_dir,
                show_prs: true,
                show_issues: true,
                labels: Vec::new(),
                pr_labels: Vec::new(),
                issue_labels: Vec::new(),
            },
        );
        true
    }

    pub fn repo_name_for_repo(&self, repo: &str) -> Option<&str> {
        self.repos
            .iter()
            .find(|configured| configured.repo.eq_ignore_ascii_case(repo))
            .map(|configured| configured.name.as_str())
    }

    fn current_directory_remote_is_configured(
        &self,
        directory: &Path,
        candidates: &[GitHubRemoteCandidate],
    ) -> bool {
        self.repos.iter().any(|repo| {
            let Some(local_dir) = repo.local_dir.as_deref() else {
                return false;
            };
            if !same_local_dir(local_dir, directory) {
                return false;
            }

            let matches = candidates
                .iter()
                .filter(|candidate| candidate.repo.eq_ignore_ascii_case(&repo.repo))
                .collect::<Vec<_>>();
            if matches.is_empty() {
                return true;
            }

            let configured_remote = repo.remote.as_deref().unwrap_or(DEFAULT_REPO_REMOTE);
            matches
                .iter()
                .any(|candidate| candidate.remote == configured_remote)
        })
    }
}

fn current_github_repo_in(directory: &Path) -> Option<GitHubRemoteCandidate> {
    current_github_remote_candidates(directory)
        .into_iter()
        .next()
}

pub fn current_github_remote_candidates(directory: &Path) -> Vec<GitHubRemoteCandidate> {
    if git_output(directory, ["rev-parse", "--is-inside-work-tree"])
        .is_none_or(|output| output.trim() != "true")
    {
        return Vec::new();
    }

    git_remote_candidates(directory)
        .into_iter()
        .filter_map(|remote| {
            let url = git_output(directory, ["remote", "get-url", remote.as_str()])?;
            let repo = github_repo_from_remote_url(url.trim())?;
            Some(GitHubRemoteCandidate { remote, repo })
        })
        .collect()
}

fn git_remote_candidates(directory: &Path) -> Vec<String> {
    let mut remotes = Vec::new();

    push_unique_remote(&mut remotes, DEFAULT_REPO_REMOTE);

    if let Some(branch) = git_output(directory, ["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        let key = format!("branch.{}.remote", branch.trim());
        if let Some(remote) = git_output(directory, ["config", "--get", key.as_str()]) {
            push_unique_remote(&mut remotes, remote.trim());
        }
    }

    if let Some(output) = git_output(directory, ["remote"]) {
        for remote in output.lines() {
            push_unique_remote(&mut remotes, remote.trim());
        }
    }

    remotes
}

fn push_unique_remote(remotes: &mut Vec<String>, remote: &str) {
    if remote.is_empty() || remotes.iter().any(|existing| existing == remote) {
        return;
    }
    remotes.push(remote.to_string());
}

fn git_output<const N: usize>(directory: &Path, args: [&str; N]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    String::from_utf8(output.stdout).ok()
}

pub(crate) fn github_repo_from_remote_url(url: &str) -> Option<String> {
    let path = if let Some(path) = url.strip_prefix("git@github.com:") {
        path
    } else if let Some(path) = url.strip_prefix("ssh://git@github.com/") {
        path
    } else if let Some((_, path)) = url.split_once("github.com/") {
        path
    } else {
        return None;
    };

    let path = path
        .trim()
        .trim_end_matches('/')
        .strip_suffix(".git")
        .unwrap_or(path.trim().trim_end_matches('/'));
    let mut parts = path.split('/');
    let owner = parts.next()?.trim();
    let name = parts.next()?.trim();
    if owner.is_empty() || name.is_empty() || parts.next().is_some() {
        return None;
    }

    Some(format!("{owner}/{name}"))
}

pub fn repo_remote_config_value(remote: &str) -> Option<String> {
    let remote = remote.trim();
    if remote.is_empty() || remote == DEFAULT_REPO_REMOTE {
        None
    } else {
        Some(remote.to_string())
    }
}

fn same_local_dir(configured: &str, directory: &Path) -> bool {
    let configured = configured.trim();
    if configured.is_empty() {
        return false;
    }
    let configured = expand_user_path(configured);
    let directory = directory.to_path_buf();
    paths_equal(&configured, &directory)
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn expand_user_path(value: &str) -> PathBuf {
    if value == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from(value));
    }
    if let Some(rest) = value.strip_prefix("~/")
        && let Some(home) = home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(value)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(::dirs::home_dir)
}

fn runtime_repo_name(configured_repos: &[RepoConfig], repo: &str) -> String {
    let short_name = repo.rsplit_once('/').map(|(_, name)| name).unwrap_or(repo);
    if configured_repos
        .iter()
        .any(|configured| configured.name.eq_ignore_ascii_case(short_name))
    {
        repo.to_string()
    } else {
        short_name.to_string()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            defaults: Defaults::default(),
            exclude_repos: vec![],
            repos: vec![],
            pr_sections: vec![
                SearchSection {
                    title: "Needs Attention".to_string(),
                    filters: String::new(),
                    queries: vec![
                        "is:open review-requested:@me archived:false sort:updated-desc".to_string(),
                        "is:open assignee:@me archived:false sort:updated-desc".to_string(),
                        "is:open mentions:@me archived:false sort:updated-desc".to_string(),
                    ],
                    limit: None,
                },
                SearchSection {
                    title: "My Pull Requests".to_string(),
                    filters: "is:open author:@me archived:false sort:updated-desc".to_string(),
                    queries: Vec::new(),
                    limit: None,
                },
                SearchSection {
                    title: "Reviewed".to_string(),
                    filters: "is:open reviewed-by:@me -author:@me archived:false sort:updated-desc"
                        .to_string(),
                    queries: Vec::new(),
                    limit: None,
                },
            ],
            issue_sections: vec![
                SearchSection {
                    title: "Assigned to Me".to_string(),
                    filters: "is:open assignee:@me archived:false sort:created-desc".to_string(),
                    queries: Vec::new(),
                    limit: None,
                },
                SearchSection {
                    title: "Mentioned".to_string(),
                    filters: "is:open mentions:@me archived:false sort:created-desc".to_string(),
                    queries: Vec::new(),
                    limit: None,
                },
                SearchSection {
                    title: "Involved".to_string(),
                    filters: "is:open involves:@me archived:false sort:created-desc".to_string(),
                    queries: Vec::new(),
                    limit: None,
                },
            ],
            notification_sections: default_notification_sections(),
            saved_search_filters: Vec::new(),
        }
    }
}

fn default_notification_sections() -> Vec<SearchSection> {
    vec![
        SearchSection {
            title: "All".to_string(),
            filters: "is:all".to_string(),
            queries: Vec::new(),
            limit: None,
        },
        SearchSection {
            title: "Review Requested".to_string(),
            filters: "reason:review-requested".to_string(),
            queries: Vec::new(),
            limit: None,
        },
        SearchSection {
            title: "Assigned".to_string(),
            filters: "reason:assign".to_string(),
            queries: Vec::new(),
            limit: None,
        },
        SearchSection {
            title: "Mentioned".to_string(),
            filters: "reason:mention".to_string(),
            queries: Vec::new(),
            limit: None,
        },
        SearchSection {
            title: "Subscribed".to_string(),
            filters: "reason:subscribed".to_string(),
            queries: Vec::new(),
            limit: None,
        },
        SearchSection {
            title: "Others".to_string(),
            filters: "reason:others".to_string(),
            queries: Vec::new(),
            limit: None,
        },
    ]
}

impl Default for RepoConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            repo: String::new(),
            remote: None,
            local_dir: None,
            show_prs: true,
            show_issues: true,
            labels: Vec::new(),
            pr_labels: Vec::new(),
            issue_labels: Vec::new(),
        }
    }
}

impl RepoConfig {
    pub fn label_filters(&self, kind: SectionKind) -> Vec<String> {
        let mut labels = self.labels.clone();
        match kind {
            SectionKind::PullRequests => labels.extend(self.pr_labels.clone()),
            SectionKind::Issues => labels.extend(self.issue_labels.clone()),
            SectionKind::Notifications => {}
        }
        labels
    }
}

pub fn normalized_log_level(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "trace" => "trace",
        "debug" => "debug",
        "info" => "info",
        "warn" => "warn",
        "error" => "error",
        _ => DEFAULT_LOG_LEVEL,
    }
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            view: SectionKind::PullRequests,
            command_palette_key: DEFAULT_COMMAND_PALETTE_KEY.to_string(),
            theme: ThemePreference::Auto,
            theme_name: None,
            log_level: DEFAULT_LOG_LEVEL.to_string(),
            pr_per_page: 50,
            issue_per_page: 50,
            notification_limit: 50,
            refetch_interval_seconds: 120,
            include_read_notifications: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_test_suffix() -> String {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let counter = TEST_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("{}-{unique}-{counter}", std::process::id())
    }

    #[test]
    fn parses_pull_requests_view_name() {
        let config = toml::from_str::<Config>(
            r#"
            [defaults]
            view = "pull_requests"
            "#,
        )
        .expect("pull_requests should be a valid default view");

        assert_eq!(config.defaults.view, SectionKind::PullRequests);
    }

    #[test]
    fn parses_inbox_view_name_as_notifications_for_compatibility() {
        let config = toml::from_str::<Config>(
            r#"
            [defaults]
            view = "inbox"
            "#,
        )
        .expect("inbox should be a valid default view alias");

        assert_eq!(config.defaults.view, SectionKind::Notifications);
    }

    #[test]
    fn default_config_round_trips_through_toml() {
        let encoded = toml::to_string_pretty(&Config::default()).expect("encode default config");

        assert!(
            encoded.contains(r#"view = "pull_requests""#),
            "default config should use the public snake_case view name"
        );
        assert!(
            encoded.contains(r#"log_level = "info""#),
            "default config should include a configurable log level"
        );

        let decoded = toml::from_str::<Config>(&encoded).expect("decode generated default config");
        assert_eq!(decoded.defaults.view, SectionKind::PullRequests);
        assert_eq!(decoded.defaults.log_level, "info");
        assert!(decoded.repos.is_empty());
        assert!(!decoded.pr_sections.is_empty());
        assert!(!decoded.issue_sections.is_empty());
        assert!(!decoded.notification_sections.is_empty());
    }

    #[test]
    fn saved_search_filters_round_trip_through_config_toml() {
        let config = toml::from_str::<Config>(
            r#"
            [[saved_search_filters]]
            name = "my rust prs"
            repo = "rust-lang/rust"
            kind = "pull_requests"
            status = "open"
            author = "chenyukang"
            sort = "created_at"
            "#,
        )
        .expect("saved search filters should parse from config");

        assert_eq!(config.saved_search_filters.len(), 1);
        assert_eq!(config.saved_search_filters[0].name, "my rust prs");
        assert_eq!(config.saved_search_filters[0].repo, "rust-lang/rust");
        assert_eq!(config.saved_search_filters[0].author, "chenyukang");

        let encoded = toml::to_string_pretty(&config).expect("encode config");
        assert!(encoded.contains("[[saved_search_filters]]"));
        assert!(!encoded.contains("[saved_search_filters.search]"));
        assert!(encoded.contains(r#"author = "chenyukang""#));
        assert!(!encoded.contains("field ="));
    }

    #[test]
    fn default_inbox_sections_start_with_all_without_unread_section() {
        let config = Config::default();
        let titles = config
            .notification_sections
            .iter()
            .map(|section| section.title.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            titles,
            vec![
                "All",
                "Review Requested",
                "Assigned",
                "Mentioned",
                "Subscribed",
                "Others"
            ]
        );
    }

    #[test]
    fn generated_default_config_includes_current_inbox_sections() {
        let dir = std::env::temp_dir().join(format!(
            "ghr-generated-config-test-{}",
            unique_test_suffix()
        ));
        fs::create_dir_all(&dir).expect("create generated config test dir");
        let config_path = dir.join("config.toml");

        let config = Config::load_or_create(&config_path).expect("generate default config");
        let written = fs::read_to_string(&config_path).expect("read generated default config");
        let titles = config
            .notification_sections
            .iter()
            .map(|section| section.title.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            titles,
            vec![
                "All",
                "Review Requested",
                "Assigned",
                "Mentioned",
                "Subscribed",
                "Others"
            ]
        );
        assert!(written.contains(r#"title = "Subscribed""#));
        assert!(written.contains(r#"filters = "reason:subscribed""#));
        assert!(written.contains(r#"title = "Others""#));
        assert!(written.contains(r#"filters = "reason:others""#));
    }

    #[test]
    fn parses_github_remote_urls() {
        for (url, expected) in [
            ("https://github.com/chenyukang/ghr.git", "chenyukang/ghr"),
            ("https://github.com/chenyukang/ghr", "chenyukang/ghr"),
            ("git@github.com:chenyukang/ghr.git", "chenyukang/ghr"),
            ("ssh://git@github.com/chenyukang/ghr.git", "chenyukang/ghr"),
        ] {
            assert_eq!(github_repo_from_remote_url(url), Some(expected.to_string()));
        }

        assert_eq!(
            github_repo_from_remote_url("git@example.com:chenyukang/ghr.git"),
            None
        );
    }

    #[test]
    fn runtime_repo_is_added_once() {
        let mut config = Config::default();
        assert!(config.add_runtime_repo_with_local_dir(
            "chenyukang/ghr".to_string(),
            Some("/tmp/ghr".to_string())
        ));
        assert_eq!(config.repos.len(), 1);
        assert_eq!(config.repos[0].name, "ghr");
        assert_eq!(config.repos[0].repo, "chenyukang/ghr");
        assert_eq!(config.repos[0].local_dir.as_deref(), Some("/tmp/ghr"));
        assert!(config.repos[0].show_prs);
        assert!(config.repos[0].show_issues);

        assert!(!config.add_runtime_repo_with_local_dir(
            "chenyukang/GHR".to_string(),
            Some("/tmp/other-ghr".to_string())
        ));
        assert_eq!(config.repos.len(), 1);
    }

    #[test]
    fn runtime_repo_uses_full_name_when_short_name_collides() {
        let mut config = Config::default();
        config.repos.push(RepoConfig {
            name: "ghr".to_string(),
            repo: "someone-else/ghr".to_string(),
            remote: None,
            local_dir: None,
            show_prs: true,
            show_issues: true,
            labels: Vec::new(),
            pr_labels: Vec::new(),
            issue_labels: Vec::new(),
        });

        assert!(config.add_runtime_repo_with_local_dir(
            "chenyukang/ghr".to_string(),
            Some("/tmp/ghr".to_string())
        ));
        assert_eq!(config.repos[0].name, "chenyukang/ghr");
        assert_eq!(config.repos[0].repo, "chenyukang/ghr");
    }

    #[test]
    fn runtime_repo_uses_remote_name_and_leads_configured_repos() {
        let mut config = Config::default();
        config.repos.push(RepoConfig {
            name: "Fiber".to_string(),
            repo: "nervosnetwork/fiber".to_string(),
            remote: None,
            local_dir: None,
            show_prs: true,
            show_issues: true,
            labels: Vec::new(),
            pr_labels: Vec::new(),
            issue_labels: Vec::new(),
        });

        assert!(config.add_runtime_repo_with_local_dir(
            "chenyukang/runnel".to_string(),
            Some("/tmp/runnel".to_string())
        ));
        assert_eq!(config.repos[0].name, "runnel");
        assert_eq!(config.repos[0].repo, "chenyukang/runnel");
        assert_eq!(config.repos[1].name, "Fiber");
    }

    #[test]
    fn runtime_repo_fills_missing_local_dir_without_duplicating_repo() {
        let mut config = Config::default();
        config.repos.push(RepoConfig {
            name: "ghr".to_string(),
            repo: "chenyukang/ghr".to_string(),
            remote: None,
            local_dir: None,
            show_prs: true,
            show_issues: true,
            labels: Vec::new(),
            pr_labels: Vec::new(),
            issue_labels: Vec::new(),
        });

        assert!(config.add_runtime_repo_with_local_dir(
            "chenyukang/GHR".to_string(),
            Some("/tmp/ghr".to_string())
        ));

        assert_eq!(config.repos.len(), 1);
        assert_eq!(config.repos[0].local_dir.as_deref(), Some("/tmp/ghr"));
    }

    #[test]
    fn runtime_repo_does_not_overwrite_existing_local_dir() {
        let mut config = Config::default();
        config.repos.push(RepoConfig {
            name: "ghr".to_string(),
            repo: "chenyukang/ghr".to_string(),
            remote: None,
            local_dir: Some("/tmp/original-ghr".to_string()),
            show_prs: true,
            show_issues: true,
            labels: Vec::new(),
            pr_labels: Vec::new(),
            issue_labels: Vec::new(),
        });

        assert!(!config.add_runtime_repo_with_local_dir(
            "chenyukang/ghr".to_string(),
            Some("/tmp/new-ghr".to_string())
        ));

        assert_eq!(
            config.repos[0].local_dir.as_deref(),
            Some("/tmp/original-ghr")
        );
    }

    #[test]
    fn current_git_repo_is_saved_to_config_with_local_dir() {
        let repo_dir = test_git_repo("https://github.com/chenyukang/ghr.git");
        let config_path = repo_dir.join("saved-config.toml");
        let local_dir = repo_dir.display().to_string();

        let (config, save_result) =
            Config::default().include_git_repo_at_and_save(&config_path, &repo_dir);

        assert_eq!(
            save_result
                .expect("repo should be detected")
                .expect("config should save"),
            "chenyukang/ghr"
        );
        assert_eq!(config.repos[0].repo, "chenyukang/ghr");
        assert_eq!(
            config.repos[0].local_dir.as_deref(),
            Some(local_dir.as_str())
        );

        let saved = Config::load_or_create(&config_path).expect("load saved config");
        assert_eq!(saved.repos[0].repo, "chenyukang/ghr");
        assert_eq!(
            saved.repos[0].local_dir.as_deref(),
            Some(local_dir.as_str())
        );
    }

    #[test]
    fn current_git_repo_with_multiple_github_remotes_prompts_for_remote() {
        let repo_dir = test_git_repo("https://github.com/Officeyutong/tentacle.git");
        add_git_remote(
            &repo_dir,
            "upstream",
            "https://github.com/nervosnetwork/tentacle.git",
        );

        let prompt = Config::default()
            .current_repo_remote_prompt_for_directory(&repo_dir)
            .expect("multiple remotes should prompt");

        assert_eq!(prompt.directory, repo_dir);
        assert_eq!(
            prompt
                .candidates
                .iter()
                .map(|candidate| (candidate.remote.as_str(), candidate.repo.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("origin", "Officeyutong/tentacle"),
                ("upstream", "nervosnetwork/tentacle")
            ]
        );
    }

    #[test]
    fn configured_current_repo_remote_does_not_prompt_again() {
        let repo_dir = test_git_repo("https://github.com/Officeyutong/tentacle.git");
        add_git_remote(
            &repo_dir,
            "upstream",
            "https://github.com/nervosnetwork/tentacle.git",
        );
        let mut config = Config::default();
        config.repos.push(RepoConfig {
            name: "tentacle".to_string(),
            repo: "nervosnetwork/tentacle".to_string(),
            remote: Some("upstream".to_string()),
            local_dir: Some(repo_dir.display().to_string()),
            show_prs: true,
            show_issues: true,
            labels: Vec::new(),
            pr_labels: Vec::new(),
            issue_labels: Vec::new(),
        });

        assert!(
            config
                .current_repo_remote_prompt_for_directory(&repo_dir)
                .is_none()
        );
    }

    #[test]
    fn runtime_repo_saves_non_origin_remote() {
        let mut config = Config::default();

        assert!(config.add_runtime_repo_with_local_dir_and_remote(
            "nervosnetwork/tentacle".to_string(),
            Some("/tmp/tentacle".to_string()),
            repo_remote_config_value("upstream"),
        ));

        assert_eq!(config.repos[0].repo, "nervosnetwork/tentacle");
        assert_eq!(config.repos[0].remote.as_deref(), Some("upstream"));
    }

    #[test]
    fn default_pr_sections_match_dashboard_order() {
        let config = Config::default();
        let titles = config
            .pr_sections
            .iter()
            .map(|section| section.title.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            titles,
            vec!["Needs Attention", "My Pull Requests", "Reviewed"]
        );
        assert_eq!(config.pr_sections[0].queries.len(), 3);
        assert!(
            config.pr_sections[0]
                .queries
                .iter()
                .all(|query| query.contains("is:open") && query.contains("sort:updated-desc"))
        );
        assert!(
            config.pr_sections[0]
                .queries
                .iter()
                .any(|query| query.contains("review-requested:@me"))
        );
        assert_eq!(
            config.pr_sections[1].filters,
            "is:open author:@me archived:false sort:updated-desc"
        );
        assert_eq!(
            config.pr_sections[2].filters,
            "is:open reviewed-by:@me -author:@me archived:false sort:updated-desc"
        );
    }

    #[test]
    fn query_sections_use_queries_instead_of_filter() {
        let section = SearchSection {
            title: "Needs Attention".to_string(),
            filters: String::new(),
            queries: vec![
                "review-requested:@me sort:updated-desc".to_string(),
                "mentions:@me sort:updated-desc".to_string(),
            ],
            limit: None,
        };

        assert_eq!(
            section.search_filters(),
            vec![
                "review-requested:@me sort:updated-desc".to_string(),
                "mentions:@me sort:updated-desc".to_string()
            ]
        );
        assert_eq!(
            section.display_filters(),
            "review-requested:@me sort:updated-desc | mentions:@me sort:updated-desc"
        );
    }

    #[test]
    fn parses_existing_config_shape() {
        let config = toml::from_str::<Config>(
            r#"
            exclude_repos = ["nervosnetwork/archive-*"]

            [[repos]]
            name = "fiber"
            repo = "nervosnetwork/fiber"
            remote = "upstream"
            local_dir = "~/code/fiber"
            show_prs = true
            show_issues = true
            labels = ["T-compiler"]
            pr_labels = ["S-waiting-on-review"]
            issue_labels = ["E-easy"]

            [defaults]
            view = "pull_requests"
            command_palette_key = ":"
            theme = "dark"
            log_level = "debug"
            pr_per_page = 50
            issue_per_page = 50
            notification_limit = 50
            refetch_interval_seconds = 120
            include_read_notifications = true

            [[pr_sections]]
            title = "Assigned to Me"
            filters = "is:open assignee:@me archived:false sort:created-desc"

            [[issue_sections]]
            title = "Assigned to Me"
            filters = "is:open assignee:@me archived:false sort:created-desc"

            [[notification_sections]]
            title = "Unread"
            filters = "is:unread"
            "#,
        )
        .expect("existing config should parse");

        assert_eq!(config.defaults.view, SectionKind::PullRequests);
        assert_eq!(config.defaults.command_palette_key, ":");
        assert_eq!(config.defaults.theme, ThemePreference::Dark);
        assert_eq!(config.defaults.log_level, "debug");
        assert_eq!(config.defaults.pr_per_page, 50);
        assert_eq!(config.defaults.issue_per_page, 50);
        assert_eq!(config.exclude_repos, vec!["nervosnetwork/archive-*"]);
        assert_eq!(config.repos[0].name, "fiber");
        assert_eq!(config.repos[0].repo, "nervosnetwork/fiber");
        assert_eq!(config.repos[0].remote.as_deref(), Some("upstream"));
        assert_eq!(config.repos[0].local_dir.as_deref(), Some("~/code/fiber"));
        assert!(config.repos[0].show_prs);
        assert!(config.repos[0].show_issues);
        assert_eq!(config.repos[0].labels, vec!["T-compiler"]);
        assert_eq!(config.repos[0].pr_labels, vec!["S-waiting-on-review"]);
        assert_eq!(config.repos[0].issue_labels, vec!["E-easy"]);
        assert_eq!(config.pr_sections[0].title, "Assigned to Me");
    }

    #[test]
    fn repo_label_filters_include_common_and_kind_specific_labels() {
        let repo = RepoConfig {
            name: "Rust".to_string(),
            repo: "rust-lang/rust".to_string(),
            remote: None,
            local_dir: None,
            show_prs: true,
            show_issues: true,
            labels: vec!["T-compiler".to_string()],
            pr_labels: vec!["S-waiting-on-review".to_string()],
            issue_labels: vec!["E-easy".to_string()],
        };

        assert_eq!(
            repo.label_filters(SectionKind::PullRequests),
            vec!["T-compiler", "S-waiting-on-review"]
        );
        assert_eq!(
            repo.label_filters(SectionKind::Issues),
            vec!["T-compiler", "E-easy"]
        );
    }

    #[test]
    fn parses_custom_command_palette_key() {
        let config = toml::from_str::<Config>(
            r#"
            [defaults]
            command_palette_key = "Ctrl+L"
            "#,
        )
        .expect("custom command palette key should parse");

        assert_eq!(config.defaults.command_palette_key, "Ctrl+L");
    }

    #[test]
    fn parses_light_theme() {
        let config = toml::from_str::<Config>(
            r#"
            [defaults]
            theme = "light"
            "#,
        )
        .expect("light theme should parse");

        assert_eq!(config.defaults.theme, ThemePreference::Light);
    }

    #[test]
    fn parses_auto_theme() {
        let config = toml::from_str::<Config>(
            r#"
            [defaults]
            theme = "auto"
            "#,
        )
        .expect("auto theme should parse");

        assert_eq!(config.defaults.theme, ThemePreference::Auto);
    }

    #[test]
    fn parses_theme_name() {
        let config = toml::from_str::<Config>(
            r#"
            [defaults]
            theme_name = "catppuccin_mocha"
            "#,
        )
        .expect("theme_name should parse");

        assert_eq!(config.defaults.theme_name, Some(ThemeName::CatppuccinMocha));
    }

    #[test]
    fn theme_name_is_none_by_default() {
        let config = Config::default();
        assert_eq!(config.defaults.theme_name, None);
    }

    #[test]
    fn normalizes_supported_log_levels() {
        assert_eq!(normalized_log_level("debug"), "debug");
        assert_eq!(normalized_log_level(" WARN "), "warn");
        assert_eq!(normalized_log_level("invalid"), "info");
    }

    #[test]
    fn defaults_reject_old_limit_names() {
        let error = toml::from_str::<Config>(
            r#"
            [defaults]
            pr_limit = 50
            issue_limit = 50
            "#,
        )
        .expect_err("old default limit names should not be accepted")
        .to_string();

        assert!(error.contains("unknown field"));
        assert!(error.contains("pr_limit"));
    }

    fn test_git_repo(remote_url: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ghr-config-test-{}", unique_test_suffix()));
        fs::create_dir_all(&dir).expect("create config test dir");

        let init = Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["init", "-q"])
            .output()
            .expect("run git init");
        assert!(
            init.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&init.stderr)
        );

        let remote = Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["remote", "add", "origin", remote_url])
            .output()
            .expect("run git remote add");
        assert!(
            remote.status.success(),
            "git remote add failed: {}",
            String::from_utf8_lossy(&remote.stderr)
        );

        dir
    }

    fn add_git_remote(directory: &Path, name: &str, remote_url: &str) {
        let remote = Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(["remote", "add", name, remote_url])
            .output()
            .expect("run git remote add");
        assert!(
            remote.status.success(),
            "git remote add failed: {}",
            String::from_utf8_lossy(&remote.stderr)
        );
    }
}
