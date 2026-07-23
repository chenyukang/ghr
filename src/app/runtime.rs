use super::*;
use crate::log::{GhLogEntry, recent_gh_log_entries};

pub(super) fn info_lines(app: &AppState, config: &Config, paths: &Paths) -> Vec<String> {
    let cwd = std::env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|error| format!("unavailable ({error})"));
    let repo = app
        .current_repo_scope()
        .unwrap_or_else(|| "(none)".to_string());
    let section = app
        .current_section()
        .map(|section| section.title.as_str())
        .unwrap_or("(none)");
    let selected = selected_item_summary(app.current_item());
    let item_count = app
        .sections
        .iter()
        .map(|section| section.items.len())
        .sum::<usize>();
    let github_auth = github_auth_summary();
    let gh_log_entries = recent_gh_log_entries();

    vec![
        "ghr".to_string(),
        format!("version: {}", env!("CARGO_PKG_VERSION")),
        format!(
            "build target: {} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
        format!("pid: {}", std::process::id()),
        format!("process memory: {}", process_memory_summary()),
        format!("GitHub auth: {github_auth}"),
        String::new(),
        "terminal".to_string(),
        format!("TERM: {}", env_summary("TERM")),
        format!("TERM_PROGRAM: {}", env_summary("TERM_PROGRAM")),
        format!(
            "TERM_PROGRAM_VERSION: {}",
            env_summary("TERM_PROGRAM_VERSION")
        ),
        format!("COLORTERM: {}", env_summary("COLORTERM")),
        format!("SHELL: {}", env_summary("SHELL")),
        String::new(),
        "paths".to_string(),
        format!("root: {}", paths.root.display()),
        format!("config: {}", paths.config_path.display()),
        format!("db: {}", paths.db_path.display()),
        format!("log: {}", paths.log_path.display()),
        format!("state: {}", paths.state_path.display()),
        format!("cwd: {cwd}"),
        String::new(),
        "config".to_string(),
        format!("log_level: {}", config.defaults.log_level),
        format!("command_palette_key: {}", app.command_palette_key),
        format!("theme_preference: {:?}", config.defaults.theme),
        format!("theme_name: {:?}", config.defaults.theme_name),
        format!("active_theme: {:?}", app.theme_name),
        format!(
            "page_limits: pr {}, issue {}, notifications {}",
            config.defaults.pr_per_page,
            config.defaults.issue_per_page,
            config.defaults.notification_limit
        ),
        format!(
            "refetch_interval_seconds: {}",
            config.defaults.refetch_interval_seconds
        ),
        format!(
            "include_read_notifications: {}",
            config.defaults.include_read_notifications
        ),
        String::new(),
        "runtime".to_string(),
        format!("view: {}", app.active_view),
        format!("focus: {}", app.focus.as_state_str()),
        format!("details_mode: {}", app.details_mode.as_state_str()),
        format!("section: {section}"),
        format!("repo: {repo}"),
        format!("selected: {selected}"),
        format!("status: {}", empty_summary(&app.status)),
        format!("refreshing: {}", app.refreshing),
        format!("refresh_scope: {:?}", app.current_refresh_scope),
        format!("idle_sweep_refreshing: {}", app.idle_sweep_refreshing),
        format!("inbox_idle_refreshing: {}", app.inbox_idle_refreshing),
        format!(
            "mouse: {}",
            if app.mouse_capture_enabled {
                "tui"
            } else {
                "text selection"
            }
        ),
        String::new(),
        "cache".to_string(),
        format!(
            "sections: {}, cached items: {item_count}",
            app.sections.len()
        ),
        format!("details entries: {}", app.details.len()),
        format!("diff entries: {}", app.diffs.len()),
        format!("ignored items: {}", app.ignored_items.len()),
        format!("recent items: {}", app.recent_items.len()),
        format!("recent commands: {}", app.recent_commands.len()),
        format!("GitHub request log entries: {}", gh_log_entries.len()),
    ]
}

pub(super) fn gh_log_lines_with_details() -> (Vec<String>, Vec<Vec<String>>) {
    let entries = recent_gh_log_entries();
    if entries.is_empty() {
        return (
            vec!["No GitHub requests logged yet".to_string()],
            Vec::new(),
        );
    }

    let lines = entries.iter().map(gh_log_entry_line).collect();
    let details = entries.iter().map(gh_log_detail_lines).collect();
    (lines, details)
}

pub(super) fn rate_limit_lines(snapshot: &GitHubRateLimitSnapshot) -> Vec<String> {
    let mut lines = vec![
        "GitHub quotas".to_string(),
        format!("backend: {}", snapshot.scheduler.backend.label()),
        format!(
            "checked: {}",
            snapshot
                .fetched_at
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S %:z")
        ),
        String::new(),
    ];

    for resource in &snapshot.resources {
        lines.push(format!(
            "{}: {:>5} remaining / {:>5} limit | {:>5} used | reset {}",
            resource.name,
            resource.remaining,
            resource.limit,
            resource.used,
            rate_limit_reset_summary(resource.reset, snapshot.fetched_at),
        ));
    }
    if snapshot.resources.is_empty() {
        lines.push("No core, search, or graphql quota returned".to_string());
    }

    lines.extend([
        String::new(),
        "Local scheduler".to_string(),
        format!(
            "active: {} / {} foreground slots",
            snapshot.scheduler.active, snapshot.scheduler.max_active
        ),
    ]);
    for resource in &snapshot.scheduler.resources {
        let cooldown = resource
            .cooldown_remaining
            .map(|duration| format!("{} remaining", compact_duration(duration)))
            .unwrap_or_else(|| "none".to_string());
        lines.push(format!(
            "{} queue: {} foreground, {} background waiting | cooldown {cooldown}",
            resource.resource.label(),
            resource.user_waiting,
            resource.background_waiting,
        ));
    }
    lines
}

fn rate_limit_reset_summary(reset_epoch: i64, checked_at: DateTime<Utc>) -> String {
    let Some(reset) = DateTime::<Utc>::from_timestamp(reset_epoch, 0) else {
        return "unknown".to_string();
    };
    let until_reset = reset.signed_duration_since(checked_at).num_seconds().max(0) as u64;
    format!(
        "{} (in {})",
        reset.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S"),
        compact_duration(Duration::from_secs(until_reset))
    )
}

fn compact_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {}m", seconds / 3_600, (seconds % 3_600) / 60)
    }
}

fn gh_log_detail_lines(entry: &GhLogEntry) -> Vec<String> {
    let direct_api = entry.kind == "api";
    let mut lines = vec![
        "Result".to_string(),
        format!("  {}", gh_log_result_label(entry)),
        format!("  Status      {}", entry.status),
        format!("  Duration    {} ms", entry.duration_ms),
        format!("  Backend     {}", gh_log_backend_label(entry)),
        String::new(),
        if direct_api { "Request" } else { "Command" }.to_string(),
    ];
    push_wrapped_detail_block(&mut lines, &entry.command);
    if !direct_api {
        lines.extend([String::new(), "Working Directory".to_string()]);
        push_wrapped_detail_block(&mut lines, entry.cwd.as_deref().unwrap_or("(none)"));
    }
    lines.extend([
        String::new(),
        "Timing".to_string(),
        format!(
            "  Started     {}",
            entry
                .started_at
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S")
        ),
        format!(
            "  Finished    {}",
            entry
                .finished_at
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S")
        ),
    ]);
    if entry.kind == "api" {
        lines.extend([
            String::new(),
            "Response".to_string(),
            format!("  body        {}", format_log_bytes(entry.stdout_bytes)),
        ]);
    } else {
        lines.extend([
            String::new(),
            "Output".to_string(),
            format!("  stdout      {}", format_log_bytes(entry.stdout_bytes)),
            format!("  stderr      {}", format_log_bytes(entry.stderr_bytes)),
        ]);
    }
    if let Some(message) = &entry.message {
        lines.extend([String::new(), "Message".to_string()]);
        push_wrapped_detail_block(&mut lines, message);
    }
    lines
}

fn gh_log_entry_line(entry: &GhLogEntry) -> String {
    let timestamp = entry
        .started_at
        .with_timezone(&Local)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let result = gh_log_entry_result(entry);
    let cwd = entry
        .cwd
        .as_ref()
        .map(|cwd| format!(" cwd={cwd}"))
        .unwrap_or_default();
    let message = entry
        .message
        .as_ref()
        .map(|message| format!(" message={message}"))
        .unwrap_or_default();

    format!(
        "{timestamp} {result:<12} {} {}ms status={} stdout={} stderr={} {}{}{}",
        entry.kind,
        entry.duration_ms,
        entry.status,
        entry.stdout_bytes,
        entry.stderr_bytes,
        entry.command,
        cwd,
        message
    )
}

fn gh_log_entry_result(entry: &GhLogEntry) -> &'static str {
    if entry.rate_limited {
        "rate-limited"
    } else if entry.success {
        "ok"
    } else {
        "failed"
    }
}

fn gh_log_result_label(entry: &GhLogEntry) -> &'static str {
    if entry.rate_limited {
        "Rate limited"
    } else if entry.success {
        "Success"
    } else {
        "Failed"
    }
}

fn gh_log_backend_label(entry: &GhLogEntry) -> &str {
    match entry.kind.as_str() {
        "api" => "Direct API",
        "gh" | "gh api" => "GitHub CLI",
        _ => entry.kind.as_str(),
    }
}

fn format_log_bytes(bytes: usize) -> String {
    if bytes == 1 {
        "1 byte".to_string()
    } else {
        format!("{bytes} bytes")
    }
}

fn push_wrapped_detail_block(lines: &mut Vec<String>, value: &str) {
    const DETAIL_CHUNK_CHARS: usize = 100;
    let mut chunks = detail_value_chunks(value, DETAIL_CHUNK_CHARS).into_iter();
    let Some(first) = chunks.next() else {
        lines.push("  (empty)".to_string());
        return;
    };

    lines.push(format!("  {first}"));
    for chunk in chunks {
        lines.push(format!("  {chunk}"));
    }
}

fn detail_value_chunks(value: &str, chunk_chars: usize) -> Vec<String> {
    if chunk_chars == 0 {
        return Vec::new();
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    for ch in value.chars() {
        current.push(ch);
        if current.chars().count() >= chunk_chars {
            chunks.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn env_summary(name: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "(unset)".to_string())
}

fn empty_summary(value: &str) -> String {
    if value.trim().is_empty() {
        "(empty)".to_string()
    } else {
        value.to_string()
    }
}

fn selected_item_summary(item: Option<&WorkItem>) -> String {
    let Some(item) = item else {
        return "(none)".to_string();
    };
    let number = item
        .number
        .map(|number| format!("#{number}"))
        .unwrap_or_else(|| "-".to_string());
    format!(
        "{} {} {}",
        item_kind_label(item.kind),
        number,
        truncate_inline(&item.title, 72)
    )
}

fn process_memory_summary() -> String {
    let Some((rss_kib, virtual_kib)) = process_memory_kib() else {
        return "unavailable".to_string();
    };

    match (rss_kib, virtual_kib) {
        (Some(rss), Some(virtual_size)) => {
            format!(
                "rss {}, virtual {}",
                format_kib(rss),
                format_kib(virtual_size)
            )
        }
        (Some(rss), None) => format!("rss {}", format_kib(rss)),
        (None, Some(virtual_size)) => format!("virtual {}", format_kib(virtual_size)),
        (None, None) => "unavailable".to_string(),
    }
}

fn process_memory_kib() -> Option<(Option<u64>, Option<u64>)> {
    process_memory_kib_from_proc_status().or_else(process_memory_kib_from_ps)
}

fn process_memory_kib_from_proc_status() -> Option<(Option<u64>, Option<u64>)> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let rss = proc_status_kib(&status, "VmRSS:");
    let virtual_size = proc_status_kib(&status, "VmSize:");
    (rss.is_some() || virtual_size.is_some()).then_some((rss, virtual_size))
}

fn proc_status_kib(status: &str, label: &str) -> Option<u64> {
    status.lines().find_map(|line| {
        let value = line.strip_prefix(label)?.trim();
        value.split_whitespace().next()?.parse::<u64>().ok()
    })
}

fn process_memory_kib_from_ps() -> Option<(Option<u64>, Option<u64>)> {
    let pid = std::process::id().to_string();
    let output = Command::new("ps")
        .args(["-o", "rss=", "-o", "vsz=", "-p", &pid])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut values = text
        .split_whitespace()
        .filter_map(|value| value.parse().ok());
    let rss = values.next();
    let virtual_size = values.next();
    (rss.is_some() || virtual_size.is_some()).then_some((rss, virtual_size))
}

fn format_kib(kib: u64) -> String {
    let mib = kib as f64 / 1024.0;
    if mib >= 1024.0 {
        format!("{:.2} GiB", mib / 1024.0)
    } else {
        format!("{mib:.1} MiB")
    }
}

#[cfg(test)]
fn github_auth_summary() -> String {
    "not checked in tests".to_string()
}

#[cfg(not(test))]
fn github_auth_summary() -> String {
    if let Some(name) = crate::github_api::token_env_name() {
        return format!("PAT via {name}");
    }

    match crate::github_gh::version_output() {
        Ok(output) => {
            if output.status.success() {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .next()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .unwrap_or("installed")
                    .to_string()
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                stderr
                    .lines()
                    .next()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .unwrap_or("unavailable")
                    .to_string()
            }
        }
        Err(error) => format!("unavailable ({error})"),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    #[test]
    fn direct_api_log_detail_uses_compact_request_layout() {
        let now = Utc::now();
        let entry = GhLogEntry {
            started_at: now,
            finished_at: now,
            duration_ms: 1022,
            kind: "api".to_string(),
            command: "POST /graphql  document=<1200 chars>  owner=rust-lang".to_string(),
            cwd: None,
            status: "HTTP 200".to_string(),
            success: true,
            stdout_bytes: 42,
            stderr_bytes: 0,
            message: None,
            rate_limited: false,
        };

        let lines = gh_log_detail_lines(&entry);
        assert!(lines.iter().any(|line| line == "  Backend     Direct API"));
        assert!(lines.iter().any(|line| line == "Request"));
        assert!(lines.iter().any(|line| line.contains("POST /graphql")));
        assert!(!lines.iter().any(|line| line == "Working Directory"));
        assert!(lines.iter().any(|line| line == "Response"));
    }
}
