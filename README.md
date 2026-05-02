# ghr

`ghr` is a fast terminal dashboard for GitHub work queues. It focuses on the things you need to scan often: pull requests, issues, notifications, descriptions, and recent comments.

![ghr dashboard screenshot](assets/screenshot.png)

## Features

- Pull request, issue, and notification views.
- Snapshot-first startup: cached data is shown immediately, then refreshed in the background.
- Configurable sections, including multi-query sections such as `All Requests`.
- Fuzzy filtering in every list with `/`.
- Details pane with descriptions and recent comments.
- Local state under `~/.ghr`: config, SQLite snapshot database, and logs.
- Uses the GitHub CLI for authentication, API access, and browser opening behavior.

## Requirements

- Rust
- GitHub CLI (`gh`)
- An authenticated GitHub CLI session:

```bash
gh auth login
```

## Usage

Install from crates.io:

```bash
cargo install ghr-cli
ghr
```

Run from the repository:

```bash
cargo run
```

Refresh the snapshot without opening the TUI:

```bash
cargo run -- --refresh --no-tui
```

Show local paths:

```bash
cargo run -- --print-paths
```

Install locally:

```bash
cargo install --path .
ghr
```

The crates.io package is named `ghr-cli` because `ghr` is already taken, but the installed command is still `ghr`.

## Keybindings

| Key | Action |
| --- | --- |
| `1` / `2` / `3` | Switch to Pull Requests / Issues / Notification |
| `4` | Focus the primary list pane |
| `Enter` or `5` | Focus the details pane |
| `Esc` | Return from details to list, or clear search |
| `h` / `l` | Move between sections |
| `j` / `k` | Move selection in list, or scroll details when details is focused |
| `/` | Fuzzy filter the current list |
| `o` | Open the selected item in the browser |
| `r` | Refresh from GitHub |
| `q` | Quit |

## Default Sections

Pull Requests:

- `My Pull Requests`: open PRs authored by you.
- `Assigned to Me`: open PRs assigned to you.
- `All Requests`: recent PRs authored by you, involving you, or reviewed by you, including closed PRs.

Issues:

- `Assigned to Me`
- `Mentioned`
- `Involved`

Notifications:

- `Unread`
- `Review Requested`
- `Assigned`
- `Mentioned`
- `All`

## Configuration

The config file is created at:

```text
~/.ghr/config.toml
```

Example:

```toml
exclude_repos = ["some-org/archive-*"]

[defaults]
view = "pull_requests"
pr_limit = 50
issue_limit = 50
notification_limit = 50
refetch_interval_seconds = 120
include_read_notifications = true

[[pr_sections]]
title = "My Pull Requests"
filters = "is:open author:@me archived:false sort:updated-desc"

[[pr_sections]]
title = "All Requests"
queries = [
  "author:@me archived:false sort:updated-desc",
  "involves:@me -author:@me archived:false sort:updated-desc",
  "reviewed-by:@me -author:@me archived:false sort:updated-desc",
]
```

Use `filters` for a single GitHub search query. Use `queries` when a section should merge several GitHub searches into one deduplicated list.

## Local Data

`ghr` keeps all local files in `~/.ghr`:

- `config.toml`: user configuration
- `ghr.db`: SQLite snapshot cache
- `ghr.log`: log file

The snapshot cache is intentionally local and disposable. Delete `~/.ghr/ghr.db` if you want to rebuild it from GitHub.

## Design Notes

`ghr` is inspired by tools like `ghui` and `gh-dash`, but it is not a strict rewrite. The main goal is a responsive Rust TUI that opens instantly from cached state, then refreshes GitHub data in the background.
