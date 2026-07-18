use std::io::{self, ErrorKind};
use std::process::{Command as StdCommand, Output};

use anyhow::{Context, Result, anyhow, bail};
use tokio::process::Command as TokioCommand;
use tokio::sync::OnceCell;
use tracing::{debug, error};

use crate::log::{GhLogRequest, fail_gh_request_to_start, finish_gh_request, start_gh_request};

static API_SLURP_SUPPORTED: OnceCell<bool> = OnceCell::const_new();

pub async fn run_api(args: &[String]) -> Result<String> {
    if args.first().map(String::as_str) != Some("api") {
        bail!("GitHub CLI backend expected a `gh api` request");
    }

    let request = log_request_started(args);
    let output = TokioCommand::new("gh")
        .env("GH_PROMPT_DISABLED", "1")
        .args(args)
        .output()
        .await
        .map_err(|error| {
            log_request_failed_to_start(request.clone(), args, &error);
            if error.kind() == ErrorKind::NotFound {
                anyhow!("{}", missing_message(args))
            } else {
                anyhow!("failed to run {}: {error}", command_display(args))
            }
        })?;
    log_request_finished(request, args, &output);

    if !output.status.success() {
        log_request_failed_result(args, &output);
        bail!("{}", failure_message(args, &output_message(&output)));
    }

    String::from_utf8(output.stdout).context("gh output was not UTF-8")
}

pub async fn api_slurp_supported() -> bool {
    *API_SLURP_SUPPORTED
        .get_or_init(detect_api_slurp_support)
        .await
}

pub fn version_output() -> io::Result<Output> {
    let args = vec!["--version".to_string()];
    let request = log_request_started(&args);
    let result = StdCommand::new("gh")
        .env("GH_PROMPT_DISABLED", "1")
        .arg("--version")
        .output();

    match &result {
        Ok(output) => {
            log_request_finished(request, &args, output);
            if !output.status.success() {
                log_request_failed_result(&args, output);
            }
        }
        Err(error) => log_request_failed_to_start(request, &args, error),
    }

    result
}

pub fn command_display(args: &[String]) -> String {
    format!("gh {}", args.join(" "))
}

async fn detect_api_slurp_support() -> bool {
    let args = vec!["api".to_string(), "--help".to_string()];
    let request = log_request_started(&args);
    let output = TokioCommand::new("gh")
        .env("GH_PROMPT_DISABLED", "1")
        .args(&args)
        .output()
        .await;

    match output {
        Ok(output) if output.status.success() => {
            log_request_finished(request, &args, &output);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            api_help_has_flag(&stdout, "--slurp") || api_help_has_flag(&stderr, "--slurp")
        }
        Ok(output) => {
            log_request_finished(request, &args, &output);
            debug!(
                status = %output.status,
                "failed to inspect gh api help for --slurp support; assuming supported"
            );
            true
        }
        Err(error) => {
            log_request_failed_to_start(request, &args, &error);
            debug!(
                error = %error,
                "failed to inspect gh api help for --slurp support; assuming supported"
            );
            true
        }
    }
}

fn api_help_has_flag(help: &str, flag: &str) -> bool {
    help.split(|ch: char| ch.is_whitespace() || matches!(ch, ',' | '[' | ']' | '(' | ')' | '`'))
        .any(|token| token == flag)
}

fn log_request_started(args: &[String]) -> GhLogRequest {
    let kind = request_kind(args);
    let command = command_display(args);
    debug!(kind, command, "gh request started");
    start_gh_request(kind, command, None)
}

fn log_request_finished(request: GhLogRequest, args: &[String], output: &std::process::Output) {
    let kind = request_kind(args);
    debug!(
        kind,
        command = %command_display(args),
        status = %output.status,
        success = output.status.success(),
        stdout_bytes = output.stdout.len(),
        stderr_bytes = output.stderr.len(),
        "gh request finished"
    );
    finish_gh_request(request, output);
}

fn log_request_failed_to_start(
    request: GhLogRequest,
    args: &[String],
    error_value: &std::io::Error,
) {
    let kind = request_kind(args);
    let command = command_display(args);
    debug!(kind, command, error = %error_value, "gh request failed to start");
    error!(kind, command, error = %error_value, "gh request failed to start");
    fail_gh_request_to_start(request, error_value);
}

fn log_request_failed_result(args: &[String], output: &std::process::Output) {
    let kind = request_kind(args);
    error!(
        kind,
        command = %command_display(args),
        status = %output.status,
        message = %output_message(output),
        stdout_bytes = output.stdout.len(),
        stderr_bytes = output.stderr.len(),
        "gh request returned failure"
    );
}

fn request_kind(args: &[String]) -> &'static str {
    if args.first().is_some_and(|arg| arg == "api") {
        "gh api"
    } else {
        "gh"
    }
}

fn output_message(output: &std::process::Output) -> String {
    output_message_from_parts(&output.stdout, &output.stderr)
}

fn output_message_from_parts(stdout: &[u8], stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(stdout).trim().to_string();
    let message = if stderr.is_empty() { stdout } else { stderr };
    truncate_output_message(&message)
}

fn truncate_output_message(message: &str) -> String {
    const MAX_CHARS: usize = 1200;
    if message.chars().count() <= MAX_CHARS {
        return message.to_string();
    }
    let mut truncated = message.chars().take(MAX_CHARS).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn missing_message(args: &[String]) -> String {
    format!(
        "No GitHub authentication backend is available: set GHR_GITHUB_TOKEN (GH_TOKEN and GITHUB_TOKEN are also supported), or install GitHub CLI from https://cli.github.com/ and run `gh auth login`. Tried: {}",
        command_display(args)
    )
}

fn failure_message(args: &[String], message: &str) -> String {
    if is_auth_error(message) {
        return format!(
            "GitHub CLI is installed but not authenticated. Run `gh auth login`, then restart ghr. Original error from `{}`: {}",
            command_display(args),
            message
        );
    }

    format!("{} failed: {message}", command_display(args))
}

fn is_auth_error(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    [
        "gh auth login",
        "not authenticated",
        "not logged in",
        "authentication required",
        "requires authentication",
        "must authenticate",
        "bad credentials",
        "no oauth token",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_message_prefers_stderr() {
        assert_eq!(
            output_message_from_parts(b"{\"state\":\"failure\"}", b"HTTP 403\n"),
            "HTTP 403"
        );
    }

    #[test]
    fn output_message_uses_stdout_when_stderr_is_empty() {
        assert_eq!(
            output_message_from_parts(b"{\"state\":\"failure\"}\n", b""),
            "{\"state\":\"failure\"}"
        );
    }

    #[test]
    fn api_help_flag_detection_matches_complete_flags() {
        assert!(api_help_has_flag(
            "      --slurp               Use an array of arrays for paginated responses",
            "--slurp"
        ));
        assert!(!api_help_has_flag(
            "      --paginate            Fetch all pages",
            "--slurp"
        ));
    }

    #[test]
    fn missing_backend_message_explains_token_and_cli_options() {
        let message = missing_message(&["api".to_string(), "user".to_string()]);

        assert!(message.contains("No GitHub authentication backend is available"));
        assert!(message.contains("GHR_GITHUB_TOKEN"));
        assert!(message.contains("https://cli.github.com/"));
        assert!(message.contains("gh auth login"));
    }

    #[test]
    fn auth_errors_are_rewritten_with_login_hint() {
        let message = failure_message(
            &["api".to_string(), "user".to_string()],
            "To get started with GitHub CLI, please run: gh auth login",
        );

        assert!(message.contains("not authenticated"));
        assert!(message.contains("Run `gh auth login`"));
    }

    #[test]
    fn non_auth_errors_keep_original_command_context() {
        let message = failure_message(
            &["api".to_string(), "search/issues".to_string()],
            "HTTP 500",
        );

        assert_eq!(message, "gh api search/issues failed: HTTP 500");
    }
}
