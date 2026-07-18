use std::{env, sync::OnceLock};

use anyhow::{Context, Result, anyhow, bail};
use octocrab::Octocrab;
use serde_json::{Map, Value};

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

pub async fn run_direct_request(request: &ApiRequest) -> Result<String> {
    let graphql = request.graphql;
    let client = client()?;
    let path = normalized_path(&request.path);

    let response = if request.method == "GET" {
        let path = append_query_fields(&path, &request.fields)?;
        let headers = request_headers(&request.headers)?;
        client._get_with_headers(path, Some(headers)).await?
    } else {
        let body = if request.graphql {
            graphql_body(request.fields.clone())?
        } else {
            Value::Object(request.fields.clone())
        };
        match request.method.as_str() {
            "POST" => client._post(path, Some(&body)).await?,
            "PATCH" => client._patch(path, Some(&body)).await?,
            "PUT" => client._put(path, Some(&body)).await?,
            "DELETE" => client._delete(path, Some(&body)).await?,
            method => bail!("direct GitHub API backend does not support method {method}"),
        }
    };

    let status = response.status();
    let body = client
        .body_to_string(response)
        .await
        .context("failed to read GitHub API response")?;
    if !status.is_success() {
        let message = github_error_message(&body);
        bail!(
            "GitHub API request failed: HTTP {}: {message}",
            status.as_u16()
        );
    }
    if graphql && let Some(message) = graphql_error_message(&body) {
        bail!("GitHub GraphQL request failed: {message}");
    }
    Ok(body)
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
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| {
            let body = body.trim();
            if body.is_empty() {
                "empty response".to_string()
            } else {
                body.to_string()
            }
        })
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

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }
}
