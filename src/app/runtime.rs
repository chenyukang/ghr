use super::*;

pub(super) fn runtime_info_body(app: &AppState, config: &Config, paths: &Paths) -> String {
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

    [
        format!("version: {}", env!("CARGO_PKG_VERSION")),
        format!("pid: {}", std::process::id()),
        format!("ghr memory: {}", process_memory_summary()),
        format!("gh: {}", github_cli_version_summary()),
        String::new(),
        format!("config: {}", paths.config_path.display()),
        format!("db: {}", paths.db_path.display()),
        format!("log: {}", paths.log_path.display()),
        format!("state: {}", paths.state_path.display()),
        String::new(),
        format!("cwd: {cwd}"),
        format!("log_level: {}", config.defaults.log_level),
        format!("view: {}", app.active_view),
        format!("focus: {}", app.focus.as_state_str()),
        format!("details_mode: {}", app.details_mode.as_state_str()),
        format!("section: {section}"),
        format!("repo: {repo}"),
        format!("selected: {selected}"),
        format!(
            "mouse: {}",
            if app.mouse_capture_enabled {
                "tui"
            } else {
                "text selection"
            }
        ),
        format!(
            "sections: {}, cached items: {item_count}",
            app.sections.len()
        ),
        format!("ignored items: {}", app.ignored_items.len()),
        format!("recent items: {}", app.recent_items.len()),
    ]
    .join("\n")
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
fn github_cli_version_summary() -> String {
    "not checked in tests".to_string()
}

#[cfg(not(test))]
fn github_cli_version_summary() -> String {
    debug!(command = "gh --version", "gh request started");
    let output = Command::new("gh")
        .env("GH_PROMPT_DISABLED", "1")
        .arg("--version")
        .output();
    match output {
        Ok(output) => {
            debug!(
                command = "gh --version",
                status = %output.status,
                success = output.status.success(),
                stdout_bytes = output.stdout.len(),
                stderr_bytes = output.stderr.len(),
                "gh request finished"
            );
            if output.status.success() {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .next()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .unwrap_or("installed")
                    .to_string()
            } else {
                error!(
                    command = "gh --version",
                    status = %output.status,
                    message = %gh_version_output_message(&output),
                    stdout_bytes = output.stdout.len(),
                    stderr_bytes = output.stderr.len(),
                    "gh request returned failure"
                );
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
        Err(error) => {
            debug!(
                command = "gh --version",
                error = %error,
                "gh request failed to start"
            );
            error!(
                command = "gh --version",
                error = %error,
                "gh request failed to start"
            );
            format!("unavailable ({error})")
        }
    }
}

#[cfg(not(test))]
fn gh_version_output_message(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stderr.is_empty() { stdout } else { stderr }
}
