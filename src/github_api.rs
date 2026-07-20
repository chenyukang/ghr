use std::{env, sync::OnceLock};

use anyhow::{Context, Result, anyhow, bail};
use octocrab::Octocrab;
use serde_json::{Map, Value};
use tracing::{debug, error};

use crate::log::{GhLogRequest, fail_api_request, finish_api_request, start_gh_request};

const TOKEN_ENV_VARS: [&str; 3] = ["GHR_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"];

static CLIENT: OnceLock<std::result::Result<Octocrab, String>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitHubBackend {
    DirectApi,
    GitHubCli,
}

impl GitHubBackend {
    pub fn supports_cli_commands(self) -> bool {
        matches!(self, Self::GitHubCli)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApiRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    fields: Map<String, Value>,
    graphql: bool,
}

pub fn selected_backend() -> GitHubBackend {
    if token_from_env().is_some() {
        GitHubBackend::DirectApi
    } else {
        GitHubBackend::GitHubCli
    }
}

#[cfg(not(test))]
pub fn token_env_name() -> Option<&'static str> {
    TOKEN_ENV_VARS
        .into_iter()
        .find(|name| env::var(name).is_ok_and(|value| !value.trim().is_empty()))
}

pub fn parse_api_args(args: &[String]) -> Result<ApiRequest> {
    parse_api_args_impl(args)
}

pub async fn run_api(args: &[String]) -> Result<String> {
    let request = match parse_api_args(args) {
        Ok(request) => request,
        Err(request_error) => {
            let command = unparsed_request_display(args);
            let log_request = start_gh_request("api", &command, None);
            debug!(kind = "api", command, "GitHub API request started");
            log_request_failed(log_request, &command, &request_error);
            return Err(request_error);
        }
    };
    let command = request_display(&request);
    let log_request = start_gh_request("api", &command, None);
    debug!(kind = "api", command, "GitHub API request started");

    run_direct_request(&request, log_request, &command).await
}

async fn run_direct_request(
    request: &ApiRequest,
    log_request: GhLogRequest,
    command: &str,
) -> Result<String> {
    let graphql = request.graphql;
    let client = match client() {
        Ok(client) => client,
        Err(request_error) => {
            log_request_failed(log_request, command, &request_error);
            return Err(request_error);
        }
    };
    let path = normalized_path(&request.path);

    let response_result: Result<_> = async {
        if request.method == "GET" {
            let path = append_query_fields(&path, &request.fields)?;
            let headers = request_headers(&request.headers)?;
            Ok(client._get_with_headers(path, Some(headers)).await?)
        } else {
            let body = if request.graphql {
                graphql_body(request.fields.clone())?
            } else {
                Value::Object(request.fields.clone())
            };
            match request.method.as_str() {
                "POST" => Ok(client._post(path, Some(&body)).await?),
                "PATCH" => Ok(client._patch(path, Some(&body)).await?),
                "PUT" => Ok(client._put(path, Some(&body)).await?),
                "DELETE" => Ok(client._delete(path, Some(&body)).await?),
                method => bail!("direct GitHub API backend does not support method {method}"),
            }
        }
    }
    .await;
    let response = match response_result {
        Ok(response) => response,
        Err(request_error) => {
            log_request_failed(log_request, command, &request_error);
            return Err(request_error);
        }
    };

    let status = response.status();
    let body = match client
        .body_to_string(response)
        .await
        .context("failed to read GitHub API response")
    {
        Ok(body) => body,
        Err(request_error) => {
            log_request_finished(
                log_request,
                command,
                status.as_u16(),
                0,
                Some(&request_error.to_string()),
            );
            return Err(request_error);
        }
    };
    if !status.is_success() {
        let message = github_error_message(&body);
        let request_error = anyhow!(
            "GitHub API request failed: HTTP {}: {message}",
            status.as_u16()
        );
        log_request_finished(
            log_request,
            command,
            status.as_u16(),
            body.len(),
            Some(&message),
        );
        return Err(request_error);
    }
    if graphql && let Some(message) = graphql_error_message(&body) {
        let request_error = anyhow!("GitHub GraphQL request failed: {message}");
        log_request_finished(
            log_request,
            command,
            status.as_u16(),
            body.len(),
            Some(&message),
        );
        return Err(request_error);
    }
    log_request_succeeded(log_request, command, status.as_u16(), body.len());
    Ok(body)
}

fn request_display(request: &ApiRequest) -> String {
    let mut details = Vec::new();
    if request.graphql
        && let Some(query) = request.fields.get("query").and_then(Value::as_str)
    {
        details.push(format!("document=<{} chars>", query.chars().count()));
    }
    details.extend(
        request
            .fields
            .iter()
            .filter(|(name, _)| !request.graphql || name.as_str() != "query")
            .map(|(name, value)| format!("{name}={}", request_value_display(value))),
    );

    let request_line = format!("{} {}", request.method, normalized_path(&request.path));
    if details.is_empty() {
        request_line
    } else {
        format!("{request_line}  {}", details.join("  "))
    }
}

fn request_value_display(value: &Value) -> String {
    let value = value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string());
    let char_count = value.chars().count();
    if char_count > 80 || value.contains(['\r', '\n']) {
        format!("<{char_count} chars>")
    } else if value.is_empty() || value.chars().any(char::is_whitespace) {
        format!("{value:?}")
    } else {
        value
    }
}

fn unparsed_request_display(args: &[String]) -> String {
    let args = args
        .iter()
        .skip(1)
        .map(|arg| {
            if let Some(query) = arg.strip_prefix("query=") {
                format!("query=<{} chars>", query.chars().count())
            } else {
                request_value_display(&Value::String(arg.clone()))
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!("api {args}").trim_end().to_string()
}

fn log_request_succeeded(request: GhLogRequest, command: &str, status: u16, response_bytes: usize) {
    debug!(
        kind = "api",
        command, status, response_bytes, "GitHub API request finished"
    );
    finish_api_request(request, status, true, response_bytes, None);
}

fn log_request_finished(
    request: GhLogRequest,
    command: &str,
    status: u16,
    response_bytes: usize,
    message: Option<&str>,
) {
    error!(
        kind = "api",
        command,
        status,
        response_bytes,
        message = message.unwrap_or("unknown error"),
        "GitHub API request returned failure"
    );
    finish_api_request(request, status, false, response_bytes, message);
}

fn log_request_failed(request: GhLogRequest, command: &str, request_error: &anyhow::Error) {
    error!(
        kind = "api",
        command,
        error = %request_error,
        "GitHub API request failed"
    );
    fail_api_request(request, request_error);
}

fn client() -> Result<&'static Octocrab> {
    CLIENT
        .get_or_init(|| {
            let token = token_from_env().ok_or_else(|| {
                "set GHR_GITHUB_TOKEN, GH_TOKEN, or GITHUB_TOKEN to use direct GitHub API access"
                    .to_string()
            })?;
            let mut builder = Octocrab::builder().personal_token(token);
            if let Some(api_url) = github_api_url() {
                builder = builder
                    .base_uri(api_url)
                    .map_err(|error| error.to_string())?;
            }
            builder.build().map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|message| anyhow!(message.clone()))
}

fn token_from_env() -> Option<String> {
    TOKEN_ENV_VARS
        .into_iter()
        .find_map(|name| env::var(name).ok().filter(|value| !value.trim().is_empty()))
}

fn github_api_url() -> Option<String> {
    ["GHR_GITHUB_API_URL", "GITHUB_API_URL"]
        .into_iter()
        .find_map(|name| env::var(name).ok().filter(|value| !value.trim().is_empty()))
}

fn parse_api_args_impl(args: &[String]) -> Result<ApiRequest> {
    if args.first().map(String::as_str) != Some("api") {
        bail!("direct GitHub API backend expected a `gh api` request");
    }

    let mut method = None;
    let mut path = None;
    let mut headers = Vec::new();
    let mut fields = Map::new();
    let mut graphql = false;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "graphql" => {
                path = Some("graphql".to_string());
                graphql = true;
            }
            "-X" | "--method" | "--request" => {
                index += 1;
                method = Some(required_arg(args, index, "HTTP method")?.to_ascii_uppercase());
            }
            "-H" | "--header" => {
                index += 1;
                headers.push(parse_header(required_arg(args, index, "header")?)?);
            }
            "-f" | "--raw-field" => {
                index += 1;
                insert_field(&mut fields, required_arg(args, index, "raw field")?, false)?;
            }
            "-F" | "--field" => {
                index += 1;
                insert_field(&mut fields, required_arg(args, index, "field")?, true)?;
            }
            "--jq" | "-q" | "--template" | "-t" => {
                index += 1;
                let _ = required_arg(args, index, "output selector")?;
            }
            "--paginate" | "--slurp" => {
                bail!("direct GitHub API pagination must be handled by the caller");
            }
            arg if arg.starts_with('-') => {
                bail!("direct GitHub API backend does not support gh api argument `{arg}`");
            }
            arg => {
                if path.replace(arg.to_string()).is_some() {
                    bail!("direct GitHub API backend received more than one API path");
                }
            }
        }
        index += 1;
    }

    let path = path.ok_or_else(|| anyhow!("direct GitHub API path is missing"))?;
    let method = method.unwrap_or_else(|| {
        if graphql || !fields.is_empty() {
            "POST".to_string()
        } else {
            "GET".to_string()
        }
    });
    Ok(ApiRequest {
        method,
        path,
        headers,
        fields,
        graphql,
    })
}

fn required_arg<'a>(args: &'a [String], index: usize, name: &str) -> Result<&'a str> {
    args.get(index)
        .map(String::as_str)
        .ok_or_else(|| anyhow!("direct GitHub API {name} is missing"))
}

fn parse_header(input: &str) -> Result<(String, String)> {
    let (name, value) = input
        .split_once(':')
        .ok_or_else(|| anyhow!("invalid GitHub API header `{input}`"))?;
    Ok((name.trim().to_string(), value.trim().to_string()))
}

fn insert_field(fields: &mut Map<String, Value>, input: &str, typed: bool) -> Result<()> {
    let (name, value) = input
        .split_once('=')
        .ok_or_else(|| anyhow!("invalid GitHub API field `{input}`"))?;
    let value = if typed {
        typed_value(value)
    } else {
        Value::String(value.to_string())
    };
    if let Some(name) = name.strip_suffix("[]") {
        match fields
            .entry(name.to_string())
            .or_insert_with(|| Value::Array(Vec::new()))
        {
            Value::Array(values) => values.push(value),
            _ => bail!("GitHub API field `{name}` mixes scalar and array values"),
        }
    } else {
        fields.insert(name.to_string(), value);
    }
    Ok(())
}

fn typed_value(value: &str) -> Value {
    match value {
        "null" => Value::Null,
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => value
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or_else(|_| Value::String(value.to_string())),
    }
}

fn normalized_path(path: &str) -> String {
    if path.starts_with('/') || path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

fn append_query_fields(path: &str, fields: &Map<String, Value>) -> Result<String> {
    if fields.is_empty() {
        return Ok(path.to_string());
    }
    let pairs = fields
        .iter()
        .flat_map(|(name, value)| match value {
            Value::Array(values) => values
                .iter()
                .map(|value| (format!("{name}[]"), query_value(value)))
                .collect::<Vec<_>>(),
            value => vec![(name.clone(), query_value(value))],
        })
        .collect::<Vec<_>>();
    let query = serde_urlencoded::to_string(pairs).context("failed to encode GitHub API query")?;
    let separator = if path.contains('?') { '&' } else { '?' };
    Ok(format!("{path}{separator}{query}"))
}

fn query_value(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        value => value.to_string(),
    }
}

fn request_headers(values: &[(String, String)]) -> Result<http::HeaderMap> {
    let mut headers = http::HeaderMap::new();
    for (name, value) in values {
        let name = http::header::HeaderName::try_from(name.as_str())
            .with_context(|| format!("invalid GitHub API header name `{name}`"))?;
        let value = http::header::HeaderValue::try_from(value.as_str())
            .context("invalid GitHub API header value")?;
        headers.insert(name, value);
    }
    Ok(headers)
}

fn graphql_body(mut fields: Map<String, Value>) -> Result<Value> {
    let query = fields
        .remove("query")
        .and_then(|value| value.as_str().map(str::to_string))
        .ok_or_else(|| anyhow!("direct GitHub API GraphQL query is missing"))?;
    Ok(serde_json::json!({
        "query": query,
        "variables": fields,
    }))
}

fn github_error_message(body: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return non_empty_error_body(body);
    };

    let mut parts = value
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .into_iter()
        .collect::<Vec<_>>();
    if let Some(errors) = value.get("errors").and_then(Value::as_array) {
        let details = errors
            .iter()
            .map(|error| {
                error
                    .as_str()
                    .or_else(|| error.get("message").and_then(Value::as_str))
                    .map(str::to_string)
                    .unwrap_or_else(|| error.to_string())
            })
            .collect::<Vec<_>>();
        if !details.is_empty() {
            parts.push(format!("errors: {}", details.join("; ")));
        }
    }
    if let Some(documentation_url) = value.get("documentation_url").and_then(Value::as_str) {
        parts.push(format!("documentation: {documentation_url}"));
    }

    if parts.is_empty() {
        non_empty_error_body(body)
    } else {
        parts.join("; ")
    }
}

fn non_empty_error_body(body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        "empty response".to_string()
    } else {
        body.to_string()
    }
}

fn graphql_error_message(body: &str) -> Option<String> {
    serde_json::from_str::<Value>(body)
        .ok()?
        .get("errors")?
        .as_array()?
        .iter()
        .filter_map(|error| error.get("message").and_then(Value::as_str))
        .map(str::to_string)
        .reduce(|mut messages, message| {
            messages.push_str("; ");
            messages.push_str(&message);
            messages
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_args_parse_graphql_variables() {
        let request = parse_api_args(&strings(&[
            "api",
            "graphql",
            "-f",
            "query=query($number: Int!) { viewer { login } }",
            "-F",
            "number=57",
            "-F",
            "enabled=true",
        ]))
        .unwrap();

        assert_eq!(request.method, "POST");
        assert!(request.graphql);
        assert_eq!(request.fields["number"], 57);
        assert_eq!(request.fields["enabled"], true);
    }

    #[test]
    fn api_request_display_uses_http_method_and_path() {
        let request = parse_api_args(&strings(&["api", "user"])).unwrap();

        assert_eq!(request_display(&request), "GET /user");
    }

    #[test]
    fn graphql_request_display_summarizes_document_and_keeps_variables() {
        let query = "query($owner: String!) {\n  repository(owner: $owner) { name }\n}";
        let request = parse_api_args(&strings(&[
            "api",
            "graphql",
            "-f",
            &format!("query={query}"),
            "-F",
            "owner=rust-lang",
        ]))
        .unwrap();

        let display = request_display(&request);
        assert!(display.starts_with("POST /graphql  document=<"));
        assert!(display.contains("owner=rust-lang"));
        assert!(!display.contains("repository"));
        assert!(!display.contains('\n'));
    }

    #[test]
    fn api_args_collect_array_fields() {
        let request = parse_api_args(&strings(&[
            "api",
            "-X",
            "POST",
            "repos/owner/repo/issues",
            "-f",
            "labels[]=bug",
            "-f",
            "labels[]=help wanted",
        ]))
        .unwrap();

        assert_eq!(
            request.fields["labels"],
            serde_json::json!(["bug", "help wanted"])
        );
    }

    #[test]
    fn get_fields_are_encoded_as_query_parameters() {
        let request = parse_api_args(&strings(&[
            "api",
            "--method",
            "GET",
            "search/issues",
            "-f",
            "q=repo:owner/repo is:pr",
            "-f",
            "per_page=20",
        ]))
        .unwrap();
        let path = append_query_fields(&normalized_path(&request.path), &request.fields).unwrap();

        assert!(path.starts_with("/search/issues?"));
        assert!(path.contains("q=repo%3Aowner%2Frepo+is%3Apr"));
        assert!(path.contains("per_page=20"));
    }

    #[test]
    fn graphql_errors_are_not_treated_as_success() {
        let body = r#"{"data":null,"errors":[{"message":"Bad credentials"},{"message":"denied"}]}"#;
        assert_eq!(
            graphql_error_message(body).as_deref(),
            Some("Bad credentials; denied")
        );
    }

    #[test]
    fn rest_error_message_keeps_validation_details() {
        let body = r#"{
            "message":"Validation Failed",
            "errors":[{"resource":"Search","field":"q","code":"invalid"}],
            "documentation_url":"https://docs.github.com/rest/search/search"
        }"#;

        let message = github_error_message(body);
        assert!(message.contains("Validation Failed"));
        assert!(message.contains(r#""field":"q""#));
        assert!(message.contains("documentation: https://docs.github.com/rest/search/search"));
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }
}
