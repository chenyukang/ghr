mod app;
mod config;
mod dirs;
mod github;
mod model;
mod snapshot;
mod state;
mod theme;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crate::config::{Config, normalized_log_level};
use crate::dirs::Paths;
use crate::github::refresh_dashboard;
use crate::model::{merge_refreshed_sections, section_counts};
use crate::snapshot::SnapshotStore;

#[derive(Debug, Parser)]
#[command(version, about = "A fast Rust GitHub dashboard")]
struct Cli {
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    #[arg(long, help = "Create ~/.ghr/config.toml if it does not exist")]
    init_config: bool,

    #[arg(long, help = "Print ~/.ghr paths and exit")]
    print_paths: bool,

    #[arg(long, help = "Refresh the local snapshot before starting")]
    refresh: bool,

    #[arg(long, help = "Do not start the TUI")]
    no_tui: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::resolve(cli.config)?;
    paths.ensure()?;

    let mut config = Config::load_or_create(&paths.config_path)?;
    let _log_guard = init_logging(&paths, &config.defaults.log_level)?;
    let store = SnapshotStore::new(paths.db_path.clone());
    store.init()?;

    if cli.print_paths {
        println!("root:   {}", paths.root.display());
        println!("config: {}", paths.config_path.display());
        println!("db:     {}", paths.db_path.display());
        println!("log:    {}", paths.log_path.display());
        println!("state:  {}", paths.state_path.display());
        return Ok(());
    }

    if cli.init_config {
        println!("config ready: {}", paths.config_path.display());
        return Ok(());
    }

    let current_repo_remote_prompt = (!cli.no_tui)
        .then(|| config.current_repo_remote_prompt())
        .flatten();
    if current_repo_remote_prompt.is_none() {
        let (updated_config, repo_save_result) =
            config.include_current_git_repo_and_save(&paths.config_path);
        config = updated_config;
        if let Some(result) = repo_save_result {
            match result {
                Ok(repo) => info!(repo, "saved current git repo to config"),
                Err(error) => warn!(
                    error = %error,
                    "failed to save current git repo to config; using it for this run only"
                ),
            }
        }
    }

    if cli.refresh {
        let refreshed = refresh_dashboard(&config).await;
        for section in &refreshed {
            if section.error.is_none() {
                store.save_section(section)?;
            }
        }
        print_summary(&refreshed);

        if cli.no_tui {
            return Ok(());
        }
    }

    if cli.no_tui {
        let cached = store.load_all()?;
        let sections = merge_refreshed_sections(
            crate::model::configured_sections(&config),
            cached.into_values().collect(),
        );
        print_summary(&sections);
        return Ok(());
    }

    app::run(config, paths, store, current_repo_remote_prompt).await
}

fn init_logging(
    paths: &Paths,
    log_level: &str,
) -> Result<tracing_appender::non_blocking::WorkerGuard> {
    let file_appender = tracing_appender::rolling::never(&paths.root, "ghr.log");
    let (writer, guard) = tracing_appender::non_blocking(file_appender);
    let configured_log_level = normalized_log_level(log_level);
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("ghr={configured_log_level}")));

    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .finish();

    let _ = tracing::subscriber::set_global_default(subscriber);
    if !log_level.trim().eq_ignore_ascii_case(configured_log_level) {
        warn!(
            configured = log_level.trim(),
            effective = configured_log_level,
            "invalid config log_level; using default"
        );
    }
    info!(
        log_level = configured_log_level,
        log = %paths.log_path.display(),
        "logging initialized"
    );
    Ok(guard)
}

fn print_summary(sections: &[crate::model::SectionSnapshot]) {
    for section in sections {
        let (total, unread) = section_counts(section);
        let suffix = match (&section.error, section.refreshed_at) {
            (Some(error), _) => format!("error: {error}"),
            (None, Some(refreshed_at)) => format!("refreshed: {refreshed_at}"),
            (None, None) => "no snapshot".to_string(),
        };
        println!(
            "{} / {}: {} items, {} unread ({})",
            section.kind.as_str(),
            section.title,
            total,
            unread,
            suffix
        );
    }
}
