use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tokio::process::Command as TokioCommand;
use tracing::{debug, error};

use super::text::truncate_text;
use crate::config::{Config, github_repo_from_remote_url};
use crate::model::{PullRequestBranch, WorkItem};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PrCheckoutResult {
    pub(super) command: String,
    pub(super) directory: PathBuf,
    pub(super) output: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PrCheckoutPlan {
    pub(super) directory: PathBuf,
    pub(super) branch: Option<PullRequestBranch>,
}

pub(super) async fn run_pr_checkout(
    item: WorkItem,
    directory: PathBuf,
) -> std::result::Result<PrCheckoutResult, String> {
    let number = item
        .number
        .ok_or_else(|| "selected item has no pull request number".to_string())?;
    let args = pr_checkout_command_args(&item.repo, number);
    let command = pr_checkout_command_display(&args);
    debug!(
        command = %command,
        cwd = %directory.display(),
        "gh request started"
    );
    let output = TokioCommand::new("gh")
        .env("GH_PROMPT_DISABLED", "1")
        .current_dir(&directory)
        .args(&args)
        .output()
        .await
        .map_err(|error| {
            debug!(
                command = %command,
                cwd = %directory.display(),
                error = %error,
                "gh request failed to start"
            );
            error!(
                command = %command,
                cwd = %directory.display(),
                error = %error,
                "gh request failed to start"
            );
            if error.kind() == io::ErrorKind::NotFound {
                format!(
                    "GitHub CLI `gh` is required for local checkout. Install it, run `gh auth login`, then retry.\n\n{}\n\nTried: {command}",
                    checkout_directory_notice(&directory),
                )
            } else {
                format!(
                    "failed to run {command}: {error}\n\n{}",
                    checkout_directory_notice(&directory),
                )
            }
        })?;
    debug!(
        command = %command,
        cwd = %directory.display(),
        status = %output.status,
        success = output.status.success(),
        stdout_bytes = output.stdout.len(),
        stderr_bytes = output.stderr.len(),
        "gh request finished"
    );

    let output_text = command_output_text(&output.stdout, &output.stderr);
    if !output.status.success() {
        let detail = if output_text.is_empty() {
            "gh did not return any output".to_string()
        } else {
            output_text
        };
        error!(
            command = %command,
            cwd = %directory.display(),
            status = %output.status,
            message = %truncate_text(&detail, 900),
            stdout_bytes = output.stdout.len(),
            stderr_bytes = output.stderr.len(),
            "gh request returned failure"
        );
        return Err(format!(
            "{} failed.\n\n{}\n\n{}",
            command,
            checkout_directory_notice(&directory),
            truncate_text(&detail, 900),
        ));
    }

    let output = if output_text.is_empty() {
        "gh pr checkout completed successfully.".to_string()
    } else {
        truncate_text(&output_text, 900)
    };
    Ok(PrCheckoutResult {
        command,
        directory,
        output,
    })
}

pub(super) fn pr_checkout_command_args(repository: &str, number: u64) -> Vec<String> {
    vec![
        "pr".to_string(),
        "checkout".to_string(),
        number.to_string(),
        "--repo".to_string(),
        repository.to_string(),
    ]
}

pub(super) fn pr_checkout_command_display(args: &[String]) -> String {
    format!("gh {}", args.join(" "))
}

pub(super) fn command_output_text(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout,
        (true, false) => stderr,
        (false, false) => format!("{stdout}\n{stderr}"),
    }
}

pub(super) fn checkout_directory_notice(directory: &Path) -> String {
    format!("Checkout runs from {}.", directory.display())
}

pub(super) fn resolve_pr_checkout_directory(
    config: &Config,
    repository: &str,
) -> std::result::Result<PathBuf, String> {
    if let Some(repo) = config
        .repos
        .iter()
        .find(|repo| repo.repo.eq_ignore_ascii_case(repository))
        && let Some(local_dir) = repo.local_dir.as_deref().map(str::trim)
        && !local_dir.is_empty()
    {
        let directory = expand_user_path(local_dir);
        ensure_directory_tracks_configured_repo(&directory, repository, repo.remote.as_deref()).map_err(|error| {
            format!(
                "Configured local_dir for {repository} cannot be used.\n\n{error}\n\nSet [[repos]].local_dir and [[repos]].remote to a checkout remote that points at {repository}."
            )
        })?;
        return Ok(directory);
    }

    let cwd = std::env::current_dir().map_err(|error| {
        format!(
            "Could not inspect the current working directory for {repository}: {error}\n\nSet [[repos]].local_dir for this repository."
        )
    })?;
    ensure_directory_tracks_repo(&cwd, repository).map_err(|error| {
        format!(
            "No local checkout found for {repository}.\n\n{error}\n\nLaunch ghr inside a checkout whose git remote points at {repository}, or set [[repos]].local_dir for this repository."
        )
    })?;
    Ok(cwd)
}

pub(super) fn configured_local_dir_for_repo(config: &Config, repository: &str) -> Option<PathBuf> {
    config
        .repos
        .iter()
        .find(|repo| repo.repo.eq_ignore_ascii_case(repository))
        .and_then(|repo| repo.local_dir.as_deref())
        .map(str::trim)
        .filter(|local_dir| !local_dir.is_empty())
        .map(expand_user_path)
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

pub(super) fn ensure_directory_tracks_repo(
    directory: &Path,
    repository: &str,
) -> std::result::Result<(), String> {
    if !directory.is_dir() {
        return Err(format!("{} is not a directory.", directory.display()));
    }
    let remotes = git_remotes_for_directory(directory)?;
    if remotes
        .iter()
        .any(|(_, repo)| repo.eq_ignore_ascii_case(repository))
    {
        return Ok(());
    }

    let remote_list = if remotes.is_empty() {
        "no GitHub remotes found".to_string()
    } else {
        remotes
            .iter()
            .map(|(remote, repo)| format!("{remote} -> {repo}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    Err(format!(
        "{} does not track {repository}; found {remote_list}.",
        directory.display()
    ))
}

pub(super) fn ensure_directory_tracks_configured_repo(
    directory: &Path,
    repository: &str,
    remote: Option<&str>,
) -> std::result::Result<(), String> {
    let Some(remote) = remote.map(str::trim).filter(|remote| !remote.is_empty()) else {
        return ensure_directory_tracks_repo(directory, repository);
    };
    if !directory.is_dir() {
        return Err(format!("{} is not a directory.", directory.display()));
    }
    match git_remote_repo(directory, remote) {
        Some(repo) if repo.eq_ignore_ascii_case(repository) => Ok(()),
        Some(repo) => Err(format!(
            "{} remote {remote} points at {repo}, expected {repository}.",
            directory.display()
        )),
        None => Err(format!(
            "{} has no GitHub remote named {remote}.",
            directory.display()
        )),
    }
}

pub(super) fn validate_pr_create_preflight(
    directory: &Path,
    repository: &str,
    branch: &str,
    head_ref: &str,
    title: &str,
) -> std::result::Result<(), String> {
    let mut issues = Vec::new();
    let branch = branch.trim();
    let head_ref = head_ref.trim();

    if title.trim().is_empty() {
        issues.push("Title is empty.".to_string());
    }
    if branch.is_empty() {
        issues.push("Current branch is empty.".to_string());
    }
    if head_ref.is_empty() {
        issues.push("Head ref is empty.".to_string());
    }
    if !directory.is_dir() {
        issues.push(format!("{} is not a directory.", directory.display()));
        return Err(pr_create_preflight_body(issues));
    }

    if let Err(error) = ensure_directory_tracks_repo(directory, repository) {
        issues.push(error);
    }

    match current_git_branch_for_directory(directory) {
        Ok(current) if current != branch => issues.push(format!(
            "Dialog was opened for branch {branch}, but the checkout is now on {current}."
        )),
        Ok(_) => {}
        Err(error) => issues.push(error),
    }

    let head_branch = pull_request_head_branch_for_preflight(head_ref);
    if !head_branch.is_empty() && head_branch != branch {
        issues.push(format!(
            "Head ref {head_ref} points at branch {head_branch}, but the checkout branch is {branch}."
        ));
    }

    let resolved_head_ref = resolve_pull_request_head_ref(directory, repository, branch);
    if !head_ref.is_empty() && resolved_head_ref != head_ref {
        issues.push(format!(
            "Head ref changed from {head_ref} to {resolved_head_ref}. Reopen the dialog and retry."
        ));
    }

    if let Err(error) = ensure_clean_worktree(directory) {
        issues.push(error);
    }

    if let Err(error) = ensure_head_commit_exists(directory) {
        issues.push(error);
    }

    match matching_push_remote_for_head_ref(directory, repository, head_ref) {
        Ok(Some(_)) => {}
        Ok(None) => issues.push(format!(
            "No GitHub push remote matches head {head_ref} for {repository}."
        )),
        Err(error) => issues.push(error),
    }

    match local_base_ref_for_repository(directory, repository) {
        Ok(Some(base)) => match commits_ahead_of(directory, &base.rev) {
            Ok(0) => issues.push(format!(
                "No commits between {} and {branch}. Commit changes on {branch} before creating a PR.",
                base.display
            )),
            Ok(_) => {}
            Err(error) => issues.push(error),
        },
        Ok(None) => issues.push(format!(
            "Could not find a local base branch for {repository}. Run `git fetch` for the base remote and retry."
        )),
        Err(error) => issues.push(error),
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(pr_create_preflight_body(issues))
    }
}

fn pr_create_preflight_body(issues: Vec<String>) -> String {
    let mut body = String::from("Fix these before creating the pull request:\n");
    for issue in issues {
        body.push_str("\n- ");
        body.push_str(&issue);
    }
    body
}

fn pull_request_head_branch_for_preflight(head: &str) -> &str {
    head.rsplit_once(':')
        .map(|(_, branch)| branch)
        .unwrap_or(head)
        .trim()
}

fn pull_request_head_owner_for_preflight(head: &str) -> Option<&str> {
    head.split_once(':')
        .map(|(owner, _)| owner.trim())
        .filter(|owner| !owner.is_empty())
}

fn ensure_clean_worktree(directory: &Path) -> std::result::Result<(), String> {
    let status = git_text(directory, &["status", "--porcelain"])?;
    if status.trim().is_empty() {
        return Ok(());
    }
    Err(format!(
        "Working tree has uncommitted changes. Commit, stash, or discard them first:\n{}",
        git_status_preview(&status)
    ))
}

fn git_status_preview(status: &str) -> String {
    let mut lines = status
        .lines()
        .take(8)
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>();
    if status.lines().count() > lines.len() {
        lines.push("  ...".to_string());
    }
    lines.join("\n")
}

fn ensure_head_commit_exists(directory: &Path) -> std::result::Result<(), String> {
    if git_ref_exists(directory, "HEAD") {
        Ok(())
    } else {
        Err("HEAD has no commit yet. Commit changes before creating a PR.".to_string())
    }
}

fn matching_push_remote_for_head_ref(
    directory: &Path,
    repository: &str,
    head_ref: &str,
) -> std::result::Result<Option<String>, String> {
    let Some((base_owner, base_name)) = repository.split_once('/') else {
        return Ok(None);
    };
    let expected_owner = pull_request_head_owner_for_preflight(head_ref).unwrap_or(base_owner);
    Ok(git_push_remotes_for_directory(directory)?
        .into_iter()
        .find(|(_, repo)| {
            repo.split_once('/').is_some_and(|(owner, name)| {
                owner.eq_ignore_ascii_case(expected_owner) && name.eq_ignore_ascii_case(base_name)
            })
        })
        .map(|(remote, _)| remote))
}

fn git_push_remotes_for_directory(
    directory: &Path,
) -> std::result::Result<Vec<(String, String)>, String> {
    let output = git_text(directory, &["remote", "-v"])?;
    Ok(output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let name = parts.next()?.trim();
            let url = parts.next()?.trim();
            let kind = parts.next()?.trim();
            if kind != "(push)" {
                return None;
            }
            Some((name.to_string(), github_repo_from_remote_url(url)?))
        })
        .collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalBaseRef {
    display: String,
    rev: String,
}

fn local_base_ref_for_repository(
    directory: &Path,
    repository: &str,
) -> std::result::Result<Option<LocalBaseRef>, String> {
    let remotes = git_remotes_for_directory(directory)?;
    let Some((remote, _)) = remotes
        .into_iter()
        .find(|(_, repo)| repo.eq_ignore_ascii_case(repository))
    else {
        return Ok(None);
    };

    if let Some(remote_head) = remote_head_ref(directory, &remote)? {
        let display = remote_head
            .strip_prefix(&format!("{remote}/"))
            .unwrap_or(remote_head.as_str())
            .to_string();
        return Ok(Some(LocalBaseRef {
            display,
            rev: remote_head,
        }));
    }

    for branch in ["main", "master", "develop"] {
        let rev = format!("refs/remotes/{remote}/{branch}");
        if git_ref_exists(directory, &rev) {
            return Ok(Some(LocalBaseRef {
                display: branch.to_string(),
                rev,
            }));
        }
    }

    for branch in ["main", "master", "develop"] {
        if git_ref_exists(directory, branch) {
            return Ok(Some(LocalBaseRef {
                display: branch.to_string(),
                rev: branch.to_string(),
            }));
        }
    }

    Ok(None)
}

fn remote_head_ref(directory: &Path, remote: &str) -> std::result::Result<Option<String>, String> {
    let output = git_output(
        directory,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            &format!("refs/remotes/{remote}/HEAD"),
        ],
    )?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|remote_head| !remote_head.is_empty()),
    )
}

fn commits_ahead_of(directory: &Path, base: &str) -> std::result::Result<u64, String> {
    let output = git_text(
        directory,
        &["rev-list", "--count", &format!("{base}..HEAD")],
    )?;
    output.trim().parse::<u64>().map_err(|error| {
        format!(
            "failed to parse commit count for {base}..HEAD in {}: {error}",
            directory.display()
        )
    })
}

fn git_ref_exists(directory: &Path, rev: &str) -> bool {
    git_output(directory, &["rev-parse", "--verify", "--quiet", rev])
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn git_text(directory: &Path, args: &[&str]) -> std::result::Result<String, String> {
    let output = git_output(directory, args)?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    Err(format!(
        "git {} failed in {}: {}",
        args.join(" "),
        directory.display(),
        command_output_text(&output.stdout, &output.stderr)
    ))
}

fn git_output(directory: &Path, args: &[&str]) -> std::result::Result<Output, String> {
    Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .map_err(|error| {
            format!(
                "failed to run git {} in {}: {error}",
                args.join(" "),
                directory.display()
            )
        })
}

pub(super) fn current_git_branch_for_directory(
    directory: &Path,
) -> std::result::Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .output()
        .map_err(|error| {
            format!(
                "failed to inspect current git branch in {}: {error}",
                directory.display()
            )
        })?;
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() && !branch.is_empty() {
        return Ok(branch);
    }

    let detail = command_output_text(&output.stdout, &output.stderr);
    let detail = if detail.is_empty() {
        "detached HEAD or no branch is checked out".to_string()
    } else {
        detail
    };
    Err(format!(
        "cannot create PR from {}: {detail}",
        directory.display()
    ))
}

pub(super) fn resolve_pull_request_head_ref(
    directory: &Path,
    repository: &str,
    branch: &str,
) -> String {
    let Some(remote) = current_branch_push_remote(directory)
        .or_else(|| configured_branch_remote(directory, branch, "pushRemote"))
        .or_else(|| git_config_value(directory, "remote.pushDefault"))
        .or_else(|| configured_branch_remote(directory, branch, "remote"))
    else {
        return branch.to_string();
    };

    if remote == "." {
        return branch.to_string();
    }

    let Some(head_repo) = git_remote_repo(directory, &remote) else {
        return branch.to_string();
    };
    pull_request_head_ref(repository, &head_repo, branch)
}

fn pull_request_head_ref(base_repo: &str, head_repo: &str, branch: &str) -> String {
    if head_repo.eq_ignore_ascii_case(base_repo) {
        return branch.to_string();
    }

    head_repo
        .split_once('/')
        .map(|(owner, _)| format!("{owner}:{branch}"))
        .unwrap_or_else(|| branch.to_string())
}

fn current_branch_push_remote(directory: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args([
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{push}",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let push_ref = String::from_utf8(output.stdout).ok()?;
    push_ref
        .trim()
        .split_once('/')
        .map(|(remote, _)| remote.trim().to_string())
        .filter(|remote| !remote.is_empty())
}

fn configured_branch_remote(directory: &Path, branch: &str, key: &str) -> Option<String> {
    git_config_value(directory, &format!("branch.{branch}.{key}"))
}

fn git_config_value(directory: &Path, key: &str) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(["config", "--get", key])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn git_remotes_for_directory(
    directory: &Path,
) -> std::result::Result<Vec<(String, String)>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .arg("remote")
        .output()
        .map_err(|error| {
            format!(
                "failed to run git remote in {}: {error}",
                directory.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "{} is not a usable git checkout: {}",
            directory.display(),
            command_output_text(&output.stdout, &output.stderr)
        ));
    }

    let mut remotes = Vec::new();
    let names = String::from_utf8_lossy(&output.stdout);
    for remote in names
        .lines()
        .map(str::trim)
        .filter(|remote| !remote.is_empty())
    {
        if let Some(repo) = git_remote_repo(directory, remote) {
            remotes.push((remote.to_string(), repo));
        }
    }
    Ok(remotes)
}

fn git_remote_repo(directory: &Path, remote: &str) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(["remote", "get-url", remote])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8(output.stdout).ok()?;
    github_repo_from_remote_url(url.trim())
}
