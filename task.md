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
| Edit title / body | `codex/edit-title-body` | Nietzsche | In review | [#20](https://github.com/chenyukang/ghr/pull/20) | Uses existing editor dialog: `E`, then `t` or `b`, then `Ctrl+Enter`. |
| Change milestone | `codex/milestone-actions` | Hegel | In review | [#22](https://github.com/chenyukang/ghr/pull/22) | Prefix milestone selection plus clear/remove; current milestone display depends on fetched metadata. |
| Quick state/search filters in TUI | `codex/search-filter-shortcuts` | Hooke | In review | [#21](https://github.com/chenyukang/ghr/pull/21) | open/closed/merged/draft/all plus assignee/author/label filters; labels with spaces are not grouped. |

## PR-only backlog

| Feature | Branch | Owner | Status | PR | Notes |
| --- | --- | --- | --- | --- | --- |
| Mark draft / ready for review | `codex/pr-draft-ready-actions` | Chandrasekhar | In review | [#15](https://github.com/chenyukang/ghr/pull/15) | Add PR lifecycle actions and refresh details afterward. |
| Update branch | `codex/pr-update-branch` | Boyle | In review | [#18](https://github.com/chenyukang/ghr/pull/18) | Use GitHub branch update API/CLI where available. |
| Enable / disable auto-merge | `codex/pr-auto-merge-actions` | Later wave | Todo | - | Requires merge method choice and repository capability checks. |
| Merge method selection: merge / squash / rebase | `codex/pr-merge-methods` | Later wave | Todo | - | Existing merge confirmation should expose method selection. |
| Full review submit flow: comment / request changes / approve, pending submit/discard | `codex/pr-review-submit-flow` | Later wave | Todo | - | Existing approve and inline comment flows are a starting point. |
| Rerun failed checks | `codex/pr-rerun-checks` | Later wave | Todo | - | Needs check-suite/job discovery and a safe rerun action. |
| Checkout PR locally | `codex/pr-checkout-local` | Cicero | In review | [#19](https://github.com/chenyukang/ghr/pull/19) | Should run a local checkout command with a clear confirmation/status. |

## Agent queue

| Agent | Scope | Worktree | Branch | Status |
| --- | --- | --- | --- | --- |
| Harvey | Assign / unassign assignees | `~/.codex/worktrees/ghr-assignee-actions` | `codex/assignee-actions` | In review: [#17](https://github.com/chenyukang/ghr/pull/17) |
| Huygens | Request / remove / re-request reviewers | `~/.codex/worktrees/ghr-reviewer-actions` | `codex/reviewer-actions` | In review: [#16](https://github.com/chenyukang/ghr/pull/16) |
| Russell | Close / reopen issue and PR reopen | `~/.codex/worktrees/ghr-state-actions` | `codex/state-actions` | In review: [#14](https://github.com/chenyukang/ghr/pull/14) |
| Chandrasekhar | Mark draft / ready for review | `~/.codex/worktrees/ghr-pr-draft-ready-actions` | `codex/pr-draft-ready-actions` | In review: [#15](https://github.com/chenyukang/ghr/pull/15) |
| Nietzsche | Edit title / body | `~/.codex/worktrees/ghr-edit-title-body` | `codex/edit-title-body` | In review: [#20](https://github.com/chenyukang/ghr/pull/20) |
| Hegel | Change milestone | `~/.codex/worktrees/ghr-milestone-actions` | `codex/milestone-actions` | In review: [#22](https://github.com/chenyukang/ghr/pull/22) |
| Hooke | Quick state/search filters | `~/.codex/worktrees/ghr-search-filter-shortcuts` | `codex/search-filter-shortcuts` | In review: [#21](https://github.com/chenyukang/ghr/pull/21) |
| Boyle | Update PR branch | `~/.codex/worktrees/ghr-pr-update-branch` | `codex/pr-update-branch` | In review: [#18](https://github.com/chenyukang/ghr/pull/18) |
| Cicero | Checkout PR locally | `~/.codex/worktrees/ghr-pr-checkout-local` | `codex/pr-checkout-local` | In review: [#19](https://github.com/chenyukang/ghr/pull/19) |
