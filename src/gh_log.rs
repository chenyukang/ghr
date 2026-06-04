use std::collections::VecDeque;
use std::path::Path;
use std::process::Output;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use chrono::{DateTime, Utc};

const MAX_GH_LOG_ENTRIES: usize = 200;
const MAX_GH_LOG_COMMAND_CHARS: usize = 1200;
const MAX_GH_LOG_MESSAGE_CHARS: usize = 280;

static GH_LOG: OnceLock<Mutex<VecDeque<GhLogEntry>>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhLogEntry {
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub duration_ms: u128,
    pub kind: String,
    pub command: String,
    pub cwd: Option<String>,
    pub status: String,
    pub success: bool,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub message: Option<String>,
    pub rate_limited: bool,
}

#[derive(Debug, Clone)]
pub struct GhLogRequest {
    started_at: DateTime<Utc>,
    started_instant: Instant,
    kind: String,
    command: String,
    cwd: Option<String>,
}

impl GhLogRequest {
    pub fn new(kind: impl Into<String>, command: impl Into<String>, cwd: Option<&Path>) -> Self {
        let command = command.into();
        Self {
            started_at: Utc::now(),
            started_instant: Instant::now(),
            kind: kind.into(),
            command: truncate_gh_log_text(&command, MAX_GH_LOG_COMMAND_CHARS),
            cwd: cwd.map(|path| path.display().to_string()),
        }
    }
}

pub fn start_gh_request(
    kind: impl Into<String>,
    command: impl Into<String>,
    cwd: Option<&Path>,
) -> GhLogRequest {
    GhLogRequest::new(kind, command, cwd)
}

pub fn finish_gh_request(request: GhLogRequest, output: &Output) {
    let message = (!output.status.success()).then(|| gh_output_message(output));
    let rate_limited = message.as_deref().is_some_and(looks_like_github_rate_limit);
    push_gh_log_entry(GhLogEntry {
        started_at: request.started_at,
        finished_at: Utc::now(),
        duration_ms: request.started_instant.elapsed().as_millis(),
        kind: request.kind,
        command: request.command,
        cwd: request.cwd,
        status: output.status.to_string(),
        success: output.status.success(),
        stdout_bytes: output.stdout.len(),
        stderr_bytes: output.stderr.len(),
        message,
        rate_limited,
    });
}

pub fn fail_gh_request_to_start(request: GhLogRequest, error: &std::io::Error) {
    let message = error.to_string();
    push_gh_log_entry(GhLogEntry {
        started_at: request.started_at,
        finished_at: Utc::now(),
        duration_ms: request.started_instant.elapsed().as_millis(),
        kind: request.kind,
        command: request.command,
        cwd: request.cwd,
        status: "failed to start".to_string(),
        success: false,
        stdout_bytes: 0,
        stderr_bytes: 0,
        rate_limited: looks_like_github_rate_limit(&message),
        message: Some(truncate_gh_log_text(&message, MAX_GH_LOG_MESSAGE_CHARS)),
    });
}

pub fn recent_gh_log_entries() -> Vec<GhLogEntry> {
    gh_log()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .rev()
        .cloned()
        .collect()
}

#[cfg(test)]
pub fn clear_gh_log_entries() {
    gh_log()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

fn push_gh_log_entry(entry: GhLogEntry) {
    let mut entries = gh_log()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    entries.push_back(entry);
    while entries.len() > MAX_GH_LOG_ENTRIES {
        entries.pop_front();
    }
}

fn gh_log() -> &'static Mutex<VecDeque<GhLogEntry>> {
    GH_LOG.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn gh_output_message(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let message = if stderr.is_empty() { stdout } else { stderr };
    truncate_gh_log_text(&message, MAX_GH_LOG_MESSAGE_CHARS)
}

fn truncate_gh_log_text(message: &str, max_chars: usize) -> String {
    let mut truncated = String::new();
    for ch in message.chars().take(max_chars) {
        truncated.push(ch);
    }
    if message.chars().count() > max_chars {
        truncated.push_str("...");
    }
    truncated
}

fn looks_like_github_rate_limit(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("api rate limit exceeded")
        || message.contains("rate limit exceeded")
        || message.contains("secondary rate limit")
        || message.contains("too many requests")
        || message.contains("http 429")
        || (message.contains("http 403") && message.contains("rate limit"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    #[test]
    fn recent_entries_are_newest_first_and_flag_rate_limits() {
        clear_gh_log_entries();
        let request = start_gh_request("gh api", "gh api /rate_limit", None);
        fail_gh_request_to_start(
            request,
            &Error::new(ErrorKind::Other, "HTTP 403: API rate limit exceeded"),
        );

        let entries = recent_gh_log_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].command, "gh api /rate_limit");
        assert!(!entries[0].success);
        assert!(entries[0].rate_limited);
        assert!(
            entries[0]
                .message
                .as_deref()
                .is_some_and(|message| message.contains("API rate limit exceeded"))
        );
    }
}
