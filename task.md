# ghr feature backlog

This tracker records the remaining high-value GitHub workflow gaps and the
branches/PRs used to implement them. Each implementation branch should stay
focused on one feature family and open a draft PR when the feature is ready for
review.

## Working model

- Tracker branch: `codex/ghr-feature-roadmap`
- Tracker PR: [#13](https://github.com/chenyukang/ghr/pull/13)
- Implementation branches start from `main`.
- One feature family per branch/worktree.
- Open one draft PR per completed feature family.
- Update this file after each feature PR is opened or blocked.

## Status legend

- `Todo`: not started
- `In progress`: branch/worktree assigned
- `In review`: draft PR opened
- `Blocked`: needs a product/API decision or external dependency
- `Done`: merged or otherwise complete

## Highest priority

| Feature | Branch | Owner | Status | PR | Notes |
| --- | --- | --- | --- | --- | --- |
| Assign / unassign assignees | `codex/assignee-actions` | Harvey | In review | [#17](https://github.com/chenyukang/ghr/pull/17) | Support issues and PRs through GitHub issue assignee APIs. |
| Request / remove reviewers, including re-request review | `codex/reviewer-actions` | Huygens | In review | [#16](https://github.com/chenyukang/ghr/pull/16) | PR-only reviewer management; user reviewers only. |
| Close / reopen issue and PR reopen | `codex/state-actions` | Russell | In review | [#14](https://github.com/chenyukang/ghr/pull/14) | Existing PR close flow expanded to issue close/reopen and PR reopen. |
| Edit title / body | `codex/edit-title-body` | Later wave | Todo | - | Needs a focused editor flow for current issue/PR metadata. |
| Change milestone | `codex/milestone-actions` | Later wave | Todo | - | Needs milestone list/prefix selection plus clear/remove. |
| Quick state/search filters in TUI | `codex/search-filter-shortcuts` | Later wave | Todo | - | open/closed/merged/draft/all plus assignee/author/label filters. |

## PR-only backlog

| Feature | Branch | Owner | Status | PR | Notes |
| --- | --- | --- | --- | --- | --- |
| Mark draft / ready for review | `codex/pr-draft-ready-actions` | Chandrasekhar | In review | [#15](https://github.com/chenyukang/ghr/pull/15) | Add PR lifecycle actions and refresh details afterward. |
| Update branch | `codex/pr-update-branch` | Later wave | Todo | - | Use GitHub branch update API/CLI where available. |
| Enable / disable auto-merge | `codex/pr-auto-merge-actions` | Later wave | Todo | - | Requires merge method choice and repository capability checks. |
| Merge method selection: merge / squash / rebase | `codex/pr-merge-methods` | Later wave | Todo | - | Existing merge confirmation should expose method selection. |
| Full review submit flow: comment / request changes / approve, pending submit/discard | `codex/pr-review-submit-flow` | Later wave | Todo | - | Existing approve and inline comment flows are a starting point. |
| Rerun failed checks | `codex/pr-rerun-checks` | Later wave | Todo | - | Needs check-suite/job discovery and a safe rerun action. |
| Checkout PR locally | `codex/pr-checkout-local` | Later wave | Todo | - | Should run a local checkout command with a clear confirmation/status. |

## Agent queue

| Agent | Scope | Worktree | Branch | Status |
| --- | --- | --- | --- | --- |
| Harvey | Assign / unassign assignees | `~/.codex/worktrees/ghr-assignee-actions` | `codex/assignee-actions` | In review: [#17](https://github.com/chenyukang/ghr/pull/17) |
| Huygens | Request / remove / re-request reviewers | `~/.codex/worktrees/ghr-reviewer-actions` | `codex/reviewer-actions` | In review: [#16](https://github.com/chenyukang/ghr/pull/16) |
| Russell | Close / reopen issue and PR reopen | `~/.codex/worktrees/ghr-state-actions` | `codex/state-actions` | In review: [#14](https://github.com/chenyukang/ghr/pull/14) |
| Chandrasekhar | Mark draft / ready for review | `~/.codex/worktrees/ghr-pr-draft-ready-actions` | `codex/pr-draft-ready-actions` | In review: [#15](https://github.com/chenyukang/ghr/pull/15) |
