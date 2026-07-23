use std::io::{self, ErrorKind};
use std::process::{Command as StdCommand, Output};

use anyhow::{Context, Result, anyhow, bail};
use http::{HeaderMap, HeaderName, HeaderValue};
use tokio::process::Command as TokioCommand;
use tracing::{debug, error};

use crate::github_queue::{GitHubQueueBackend, GitHubRateResource, observe_response};
use crate::log::{GhLogRequest, fail_gh_request_to_start, finish_gh_request, start_gh_request};

pub async fn run_api(args: &[String], resource: GitHubRateResource) -> Result<String> {
    if args.first().map(String::as_str) != Some("api") {
        bail!("GitHub CLI backend expected a `gh api` request");
    }

    let request = log_request_started(args);
    let command_args = included_headers_args(args);
    let mut output = TokioCommand::new("gh")
        .env("GH_PROMPT_DISABLED", "1")
        .args(&command_args)
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
    let response = match parse_included_response(&output.stdout) {
        Ok(response) => response,
        Err(parse_error) => {
            log_request_finished(request, args, &output);
            if !output.status.success() {
                bail!("{}", failure_message(args, &output_message(&output)));
            }
            return Err(anyhow!(
                "failed to parse response headers from {}: {parse_error}",
                command_display(args)
            ));
        }
    };
    let message = (!output.status.success()).then(|| output_message(&output));
    observe_response(
        GitHubQueueBackend::GitHubCli,
        resource,
        response.status,
        &response.headers,
        message.as_deref(),
    );
    output.stdout = response.body;
    log_request_finished(request, args, &output);

    if !output.status.success() {
        log_request_failed_result(args, &output);
        bail!("{}", failure_message(args, &output_message(&output)));
    }

    String::from_utf8(output.stdout).context("gh output was not UTF-8")
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

struct IncludedResponse {
    status: u16,
    headers: HeaderMap,
    body: Vec<u8>,
}

fn included_headers_args(args: &[String]) -> Vec<String> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-i" | "--include"))
    {
        return args.to_vec();
    }
    let mut command_args = args.to_vec();
    command_args.insert(1, "--include".to_string());
    command_args
}

fn parse_included_response(output: &[u8]) -> Result<IncludedResponse> {
    let output = std::str::from_utf8(output).context("gh api response was not UTF-8")?;
    let (header_text, body) = split_headers_and_body(output)
        .ok_or_else(|| anyhow!("gh api response did not include an HTTP header block"))?;
    let mut lines = header_text.lines();
    let status_line = lines
        .next()
        .ok_or_else(|| anyhow!("gh api response status line is missing"))?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| anyhow!("invalid gh api response status line `{status_line}`"))?
        .parse::<u16>()
        .with_context(|| format!("invalid gh api response status line `{status_line}`"))?;
    let mut headers = HeaderMap::new();
    for line in lines.filter(|line| !line.trim().is_empty()) {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| anyhow!("invalid gh api response header `{line}`"))?;
        let name = HeaderName::from_bytes(name.trim().to_ascii_lowercase().as_bytes())
            .with_context(|| format!("invalid gh api response header name `{name}`"))?;
        let value = HeaderValue::from_str(value.trim())
            .with_context(|| format!("invalid gh api response header value for `{name}`"))?;
        headers.append(name, value);
    }

    Ok(IncludedResponse {
        status,
        headers,
        body: body.as_bytes().to_vec(),
    })
}

fn split_headers_and_body(output: &str) -> Option<(&str, &str)> {
    let crlf = output.find("\r\n\r\n").map(|index| (index, 4));
    let lf = output.find("\n\n").map(|index| (index, 2));
    let (index, delimiter_len) = match (crlf, lf) {
        (Some(crlf), Some(lf)) => Some(crlf.min(lf)),
        (Some(crlf), None) => Some(crlf),
        (None, Some(lf)) => Some(lf),
        (None, None) => None,
    }?;
    Some((&output[..index], &output[index + delimiter_len..]))
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
    fn gh_api_requests_include_response_headers() {
        assert_eq!(
            included_headers_args(&["api".to_string(), "user".to_string()]),
            vec!["api", "--include", "user"]
        );
    }

    #[test]
    fn included_response_parser_returns_headers_and_clean_body() {
        let output = b"HTTP/2.0 200 OK\nX-RateLimit-Remaining: 42\r\nX-RateLimit-Resource: core\r\n\r\n{\"login\":\"octocat\"}";
        let response = parse_included_response(output).expect("response should parse");

        assert_eq!(response.status, 200);
        assert_eq!(response.headers["x-ratelimit-remaining"], "42");
        assert_eq!(response.headers["x-ratelimit-resource"], "core");
        assert_eq!(response.body, br#"{"login":"octocat"}"#);
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
