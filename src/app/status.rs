use std::time::Instant;

use super::{
    MessageDialog, MessageDialogKind, PendingCommentMode, PrAction, ReviewerAction,
    SUCCESS_DIALOG_AUTO_CLOSE, SetupDialog, item_kind_label, text::truncate_inline,
};
use crate::model::ItemKind;

pub(super) fn refresh_error_status(count: usize, first_error: Option<&str>) -> String {
    let Some(first_error) = first_error else {
        return format!("refresh complete with {count} failed section(s)");
    };

    if first_error.contains("GitHub CLI `gh` is required") {
        return "GitHub CLI missing: install `gh`, then run `gh auth login`".to_string();
    }

    if first_error.contains("Run `gh auth login`") {
        return "GitHub CLI auth required: run `gh auth login`".to_string();
    }

    if is_github_search_rate_limit(first_error) {
        return format!(
            "GitHub search rate limited; using cached data ({count} failed section(s))"
        );
    }

    format!("refresh complete with {count} failed section(s)")
}

pub(super) fn compact_error_label(error: &str) -> String {
    if error.contains("GitHub CLI `gh` is required") {
        return "GitHub CLI missing".to_string();
    }

    if error.contains("Run `gh auth login`") {
        return "GitHub CLI auth required".to_string();
    }

    if is_github_search_rate_limit(error) {
        return "GitHub search rate limited".to_string();
    }

    let summary = error
        .split_once(" failed: ")
        .map(|(_, message)| message)
        .unwrap_or(error)
        .trim();
    truncate_inline(summary, 80)
}

pub(super) fn setup_dialog_from_error(error: &str) -> Option<SetupDialog> {
    if error.contains("GitHub CLI `gh` is required") {
        return Some(SetupDialog::MissingGh);
    }

    if error.contains("Run `gh auth login`") {
        return Some(SetupDialog::AuthRequired);
    }

    None
}

pub(super) fn message_dialog(title: impl Into<String>, body: impl Into<String>) -> MessageDialog {
    MessageDialog {
        title: title.into(),
        body: body.into(),
        kind: MessageDialogKind::Error,
        auto_close_at: None,
    }
}

pub(super) fn retryable_message_dialog(
    title: impl Into<String>,
    body: impl Into<String>,
) -> MessageDialog {
    MessageDialog {
        title: title.into(),
        body: body.into(),
        kind: MessageDialogKind::RetryableError,
        auto_close_at: None,
    }
}

pub(super) fn success_message_dialog(
    title: impl Into<String>,
    body: impl Into<String>,
) -> MessageDialog {
    MessageDialog {
        title: title.into(),
        body: body.into(),
        kind: MessageDialogKind::Success,
        auto_close_at: Some(Instant::now() + SUCCESS_DIALOG_AUTO_CLOSE),
    }
}

pub(super) fn persistent_success_message_dialog(
    title: impl Into<String>,
    body: impl Into<String>,
) -> MessageDialog {
    MessageDialog {
        title: title.into(),
        body: body.into(),
        kind: MessageDialogKind::Success,
        auto_close_at: None,
    }
}

pub(super) fn info_message_dialog(
    title: impl Into<String>,
    body: impl Into<String>,
) -> MessageDialog {
    MessageDialog {
        title: title.into(),
        body: body.into(),
        kind: MessageDialogKind::Info,
        auto_close_at: None,
    }
}

pub(super) fn pr_action_success_title(action: PrAction, item_kind: ItemKind) -> String {
    match action {
        PrAction::Merge => "Pull Request Merged",
        PrAction::Close if item_kind == ItemKind::Issue => "Issue Closed",
        PrAction::Close => "Pull Request Closed",
        PrAction::Reopen if item_kind == ItemKind::Issue => "Issue Reopened",
        PrAction::Reopen => "Pull Request Reopened",
        PrAction::Approve => "Pull Request Approved",
        PrAction::EnableAutoMerge => "Auto-Merge Enabled",
        PrAction::DisableAutoMerge => "Auto-Merge Disabled",
        PrAction::Checkout => "Pull Request Checked Out",
        PrAction::RerunFailedChecks => "Failed Checks Rerun",
        PrAction::UpdateBranch => "Pull Request Branch Updated",
        PrAction::ConvertToDraft => "Pull Request Converted to Draft",
        PrAction::MarkReadyForReview => "Pull Request Ready for Review",
    }
    .to_string()
}

pub(super) fn pr_action_success_body(action: PrAction, _item_kind: ItemKind) -> String {
    match action {
        PrAction::Merge => "GitHub accepted the merge. Refreshing details.",
        PrAction::Close => "GitHub accepted the close action. Refreshing details.",
        PrAction::Reopen => "GitHub accepted the reopen action. Refreshing details.",
        PrAction::Approve => "GitHub accepted the approval. Refreshing details.",
        PrAction::EnableAutoMerge => "GitHub enabled auto-merge. Refreshing details.",
        PrAction::DisableAutoMerge => "GitHub disabled auto-merge. Refreshing details.",
        PrAction::Checkout => "GitHub CLI checked out the pull request locally.",
        PrAction::RerunFailedChecks => {
            "GitHub accepted the failed-check rerun request. Refreshing details."
        }
        PrAction::UpdateBranch => "GitHub accepted the branch update. Refreshing details.",
        PrAction::ConvertToDraft => {
            "GitHub converted the pull request to draft. Refreshing details."
        }
        PrAction::MarkReadyForReview => {
            "GitHub marked the pull request ready for review. Refreshing details."
        }
    }
    .to_string()
}

pub(super) fn pr_action_error_title(action: PrAction, _item_kind: ItemKind) -> String {
    match action {
        PrAction::Merge => "Merge Failed",
        PrAction::Close => "Close Failed",
        PrAction::Reopen => "Reopen Failed",
        PrAction::Approve => "Approve Failed",
        PrAction::EnableAutoMerge => "Enable Auto-Merge Failed",
        PrAction::DisableAutoMerge => "Disable Auto-Merge Failed",
        PrAction::Checkout => "Checkout Failed",
        PrAction::RerunFailedChecks => "Rerun Failed",
        PrAction::UpdateBranch => "Update Branch Failed",
        PrAction::ConvertToDraft => "Convert to Draft Failed",
        PrAction::MarkReadyForReview => "Ready for Review Failed",
    }
    .to_string()
}

pub(super) fn pr_action_error_status(action: PrAction, item_kind: ItemKind) -> String {
    let label = item_kind_label(item_kind);
    match action {
        PrAction::Merge => "pull request merge failed".to_string(),
        PrAction::Close => format!("{label} close failed"),
        PrAction::Reopen => format!("{label} reopen failed"),
        PrAction::Approve => "pull request approval failed".to_string(),
        PrAction::EnableAutoMerge => "pull request auto-merge enable failed".to_string(),
        PrAction::DisableAutoMerge => "pull request auto-merge disable failed".to_string(),
        PrAction::Checkout => "pull request checkout failed".to_string(),
        PrAction::RerunFailedChecks => "failed check rerun failed".to_string(),
        PrAction::UpdateBranch => "pull request branch update failed".to_string(),
        PrAction::ConvertToDraft => "pull request draft conversion failed".to_string(),
        PrAction::MarkReadyForReview => "pull request ready-for-review failed".to_string(),
    }
}

pub(super) fn pr_action_error_body(error: &str) -> String {
    operation_error_body(error)
}

pub(super) fn reviewer_action_success_title(action: ReviewerAction) -> &'static str {
    match action {
        ReviewerAction::Request => "Reviewers Requested",
        ReviewerAction::Remove => "Review Requests Removed",
    }
}

pub(super) fn reviewer_action_success_body(action: ReviewerAction) -> &'static str {
    match action {
        ReviewerAction::Request => "GitHub accepted the review request. Refreshing details.",
        ReviewerAction::Remove => "GitHub removed the requested reviewers. Refreshing details.",
    }
}

pub(super) fn reviewer_action_error_title(action: ReviewerAction) -> &'static str {
    match action {
        ReviewerAction::Request => "Reviewer Request Failed",
        ReviewerAction::Remove => "Reviewer Removal Failed",
    }
}

pub(super) fn reviewer_action_error_status(action: ReviewerAction) -> &'static str {
    match action {
        ReviewerAction::Request => "reviewer request failed",
        ReviewerAction::Remove => "reviewer removal failed",
    }
}

pub(super) fn operation_error_body(error: &str) -> String {
    let message = error
        .split_once(" failed: ")
        .map(|(_, message)| message)
        .unwrap_or(error)
        .trim();
    truncate_inline(message, 900)
}

pub(super) fn comment_pending_dialog(mode: &PendingCommentMode) -> MessageDialog {
    match mode {
        PendingCommentMode::Post => info_message_dialog(
            "Posting Comment",
            "Waiting for GitHub to accept the comment...",
        ),
        PendingCommentMode::ReviewReply { .. } => info_message_dialog(
            "Posting Review Reply",
            "Waiting for GitHub to accept the review reply...",
        ),
        PendingCommentMode::Edit { .. } => info_message_dialog(
            "Updating Comment",
            "Waiting for GitHub to accept the update...",
        ),
        PendingCommentMode::Review { .. } => info_message_dialog(
            "Posting Review Comment",
            "Waiting for GitHub to accept the review comment...",
        ),
    }
}

fn is_github_search_rate_limit(error: &str) -> bool {
    error
        .to_ascii_lowercase()
        .contains("api rate limit exceeded")
}
