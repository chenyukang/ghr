use std::time::Instant;

use super::{
    MessageDialog, MessageDialogKind, PendingCommentMode, PrAction, SUCCESS_DIALOG_AUTO_CLOSE,
    SetupDialog, text::truncate_inline,
};

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

fn info_message_dialog(title: impl Into<String>, body: impl Into<String>) -> MessageDialog {
    MessageDialog {
        title: title.into(),
        body: body.into(),
        kind: MessageDialogKind::Info,
        auto_close_at: None,
    }
}

pub(super) fn pr_action_success_title(action: PrAction) -> &'static str {
    match action {
        PrAction::Merge => "Pull Request Merged",
        PrAction::Close => "Pull Request Closed",
        PrAction::Approve => "Pull Request Approved",
        PrAction::Checkout => "Pull Request Checked Out",
    }
}

pub(super) fn pr_action_success_body(action: PrAction) -> &'static str {
    match action {
        PrAction::Merge => "GitHub accepted the merge. Refreshing details.",
        PrAction::Close => "GitHub accepted the close action. Refreshing details.",
        PrAction::Approve => "GitHub accepted the approval. Refreshing details.",
        PrAction::Checkout => "GitHub CLI checked out the pull request locally.",
    }
}

pub(super) fn pr_action_error_title(action: PrAction) -> &'static str {
    match action {
        PrAction::Merge => "Merge Failed",
        PrAction::Close => "Close Failed",
        PrAction::Approve => "Approve Failed",
        PrAction::Checkout => "Checkout Failed",
    }
}

pub(super) fn pr_action_error_status(action: PrAction) -> &'static str {
    match action {
        PrAction::Merge => "pull request merge failed",
        PrAction::Close => "pull request close failed",
        PrAction::Approve => "pull request approval failed",
        PrAction::Checkout => "pull request checkout failed",
    }
}

pub(super) fn pr_action_error_body(error: &str) -> String {
    operation_error_body(error)
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
