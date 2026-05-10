use super::*;

pub(super) fn editor_draft_item_key(item: &WorkItem) -> String {
    let kind = match item.kind {
        ItemKind::Notification => "notification",
        ItemKind::PullRequest => "pull",
        ItemKind::Issue => "issue",
    };
    match item.number {
        Some(number) => format!("{kind}:{}#{number}", item.repo),
        None => format!("{kind}:{}", item.id),
    }
}

pub(super) fn new_comment_draft_key(item: &WorkItem) -> String {
    format!("comment:{}:new", editor_draft_item_key(item))
}

pub(super) fn reply_comment_draft_key(
    item: &WorkItem,
    comment: &CommentPreview,
    index: usize,
) -> String {
    let target = comment
        .id
        .map(|id| id.to_string())
        .unwrap_or_else(|| format!("index-{index}"));
    format!("comment:{}:reply:{target}", editor_draft_item_key(item))
}

pub(super) fn edit_comment_draft_key(item: &WorkItem, comment_id: u64, is_review: bool) -> String {
    let kind = if is_review { "review" } else { "issue" };
    format!(
        "comment:{}:edit:{kind}:{comment_id}",
        editor_draft_item_key(item)
    )
}

pub(super) fn review_comment_draft_key(item: &WorkItem, target: &DiffReviewTarget) -> String {
    let start = target
        .start_line
        .map(|line| {
            format!(
                ":{}:{line}",
                target.start_side.map_or("-", DiffReviewSide::as_api_value)
            )
        })
        .unwrap_or_default();
    format!(
        "comment:{}:review:{}:{}:{}{start}",
        editor_draft_item_key(item),
        target.path,
        target.side.as_api_value(),
        target.line
    )
}

pub(super) fn item_metadata_draft_key(item: &WorkItem, field: ItemEditField) -> String {
    format!("metadata:{}:{}", editor_draft_item_key(item), field.label())
}

pub(super) fn new_issue_draft_key(repo: &str) -> String {
    format!("issue:new:{}", repo.trim())
}

pub(super) fn new_pr_draft_key(repo: &str) -> String {
    format!("pull:new:{}", repo.trim())
}

pub(super) fn issue_dialog_draft_payload(dialog: &IssueDialog) -> NewIssueDraft {
    NewIssueDraft {
        repo: dialog.repo.text().to_string(),
        title: dialog.title.text().to_string(),
        labels: dialog.labels.text().to_string(),
        body: dialog.body.text().to_string(),
    }
}

pub(super) fn pr_create_dialog_draft_payload(dialog: &PrCreateDialog) -> NewPrDraft {
    NewPrDraft {
        repo: dialog.repo.clone(),
        title: dialog.title.text().to_string(),
        body: dialog.body.text().to_string(),
    }
}

pub(super) fn encode_editor_draft<T: Serialize>(payload: &T) -> Result<String> {
    serde_json::to_string(payload).map_err(Into::into)
}

pub(super) fn issue_draft_has_content(payload: &NewIssueDraft) -> bool {
    !payload.title.trim().is_empty()
        || !payload.labels.trim().is_empty()
        || !payload.body.trim().is_empty()
}

pub(super) fn pr_draft_has_content(payload: &NewPrDraft) -> bool {
    !payload.title.trim().is_empty() || !payload.body.trim().is_empty()
}
