use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::SectionKind;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub defaults: Defaults,
    pub exclude_repos: Vec<String>,
    pub pr_sections: Vec<SearchSection>,
    pub issue_sections: Vec<SearchSection>,
    pub notification_sections: Vec<SearchSection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Defaults {
    pub view: SectionKind,
    pub pr_limit: usize,
    pub issue_limit: usize,
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
            let config = toml::from_str(&content)
                .with_context(|| format!("failed to parse {}", path.display()))?;
            return Ok(config);
        }

        let config = Self::default();
        let content = toml::to_string_pretty(&config).context("failed to encode default config")?;
        fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))?;
        Ok(config)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            defaults: Defaults::default(),
            exclude_repos: vec![],
            pr_sections: vec![
                SearchSection {
                    title: "My Pull Requests".to_string(),
                    filters: "is:open author:@me archived:false sort:updated-desc".to_string(),
                    queries: Vec::new(),
                    limit: None,
                },
                SearchSection {
                    title: "Assigned to Me".to_string(),
                    filters: "is:open assignee:@me archived:false sort:updated-desc".to_string(),
                    queries: Vec::new(),
                    limit: None,
                },
                SearchSection {
                    title: "All Requests".to_string(),
                    filters: String::new(),
                    queries: vec![
                        "author:@me archived:false sort:updated-desc".to_string(),
                        "involves:@me -author:@me archived:false sort:updated-desc".to_string(),
                        "reviewed-by:@me -author:@me archived:false sort:updated-desc".to_string(),
                    ],
                    limit: None,
                },
            ],
            issue_sections: vec![
                SearchSection {
                    title: "Assigned to Me".to_string(),
                    filters: "is:open assignee:@me archived:false sort:updated-desc".to_string(),
                    queries: Vec::new(),
                    limit: None,
                },
                SearchSection {
                    title: "Mentioned".to_string(),
                    filters: "is:open mentions:@me archived:false sort:updated-desc".to_string(),
                    queries: Vec::new(),
                    limit: None,
                },
                SearchSection {
                    title: "Involved".to_string(),
                    filters: "is:open involves:@me archived:false sort:updated-desc".to_string(),
                    queries: Vec::new(),
                    limit: None,
                },
            ],
            notification_sections: vec![
                SearchSection {
                    title: "Unread".to_string(),
                    filters: "is:unread".to_string(),
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
                    title: "All".to_string(),
                    filters: "is:all".to_string(),
                    queries: Vec::new(),
                    limit: None,
                },
            ],
        }
    }
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            view: SectionKind::PullRequests,
            pr_limit: 50,
            issue_limit: 50,
            notification_limit: 50,
            refetch_interval_seconds: 120,
            include_read_notifications: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn default_config_round_trips_through_toml() {
        let encoded = toml::to_string_pretty(&Config::default()).expect("encode default config");

        assert!(
            encoded.contains(r#"view = "pull_requests""#),
            "default config should use the public snake_case view name"
        );

        let decoded = toml::from_str::<Config>(&encoded).expect("decode generated default config");
        assert_eq!(decoded.defaults.view, SectionKind::PullRequests);
        assert!(!decoded.pr_sections.is_empty());
        assert!(!decoded.issue_sections.is_empty());
        assert!(!decoded.notification_sections.is_empty());
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
            vec!["My Pull Requests", "Assigned to Me", "All Requests"]
        );
        assert_eq!(config.pr_sections[2].queries.len(), 3);
        assert!(
            config.pr_sections[2]
                .queries
                .iter()
                .any(|query| query.contains("reviewed-by:@me"))
        );
    }

    #[test]
    fn query_sections_use_queries_instead_of_filter() {
        let section = SearchSection {
            title: "All Requests".to_string(),
            filters: String::new(),
            queries: vec![
                "author:@me sort:updated-desc".to_string(),
                "reviewed-by:@me sort:updated-desc".to_string(),
            ],
            limit: None,
        };

        assert_eq!(
            section.search_filters(),
            vec![
                "author:@me sort:updated-desc".to_string(),
                "reviewed-by:@me sort:updated-desc".to_string()
            ]
        );
        assert_eq!(
            section.display_filters(),
            "author:@me sort:updated-desc | reviewed-by:@me sort:updated-desc"
        );
    }

    #[test]
    fn parses_existing_config_shape() {
        let config = toml::from_str::<Config>(
            r#"
            exclude_repos = ["nervosnetwork/archive-*"]

            [defaults]
            view = "pull_requests"
            pr_limit = 50
            issue_limit = 50
            notification_limit = 50
            refetch_interval_seconds = 120
            include_read_notifications = true

            [[pr_sections]]
            title = "Assigned to Me"
            filters = "is:open assignee:@me archived:false sort:updated-desc"

            [[issue_sections]]
            title = "Assigned to Me"
            filters = "is:open assignee:@me archived:false sort:updated-desc"

            [[notification_sections]]
            title = "Unread"
            filters = "is:unread"
            "#,
        )
        .expect("existing config should parse");

        assert_eq!(config.defaults.view, SectionKind::PullRequests);
        assert_eq!(config.exclude_repos, vec!["nervosnetwork/archive-*"]);
        assert_eq!(config.pr_sections[0].title, "Assigned to Me");
    }
}
