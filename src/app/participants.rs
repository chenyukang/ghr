use super::*;

pub(super) fn assignee_action_label(action: AssigneeAction) -> &'static str {
    match action {
        AssigneeAction::Assign => "assign",
        AssigneeAction::Unassign => "unassign",
    }
}

pub(super) fn assignee_action_success_title(action: AssigneeAction) -> &'static str {
    match action {
        AssigneeAction::Assign => "Assignee Added",
        AssigneeAction::Unassign => "Assignee Removed",
    }
}

pub(super) fn assignee_action_success_body(action: AssigneeAction) -> &'static str {
    match action {
        AssigneeAction::Assign => "GitHub added the assignee and refreshed the item.",
        AssigneeAction::Unassign => "GitHub removed the assignee and refreshed the item.",
    }
}

pub(super) fn assignee_action_error_title(action: AssigneeAction) -> &'static str {
    match action {
        AssigneeAction::Assign => "Assign Failed",
        AssigneeAction::Unassign => "Unassign Failed",
    }
}

pub(super) fn parse_assignee_input(input: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut assignees = Vec::new();
    for raw in input.split(|ch: char| ch == ',' || ch.is_whitespace()) {
        let login = raw.trim().trim_start_matches('@').trim();
        if login.is_empty() {
            continue;
        }
        let key = login.to_ascii_lowercase();
        if seen.insert(key) {
            assignees.push(login.to_string());
        }
    }
    assignees
}

pub(super) fn dedupe_assignee_logins(logins: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for login in logins {
        let key = login.to_ascii_lowercase();
        if seen.insert(key) {
            deduped.push(login);
        }
    }
    deduped
}

pub(super) fn assignee_input_prefix(input: &str) -> String {
    input
        .rsplit(|ch: char| ch == ',' || ch.is_whitespace())
        .next()
        .unwrap_or_default()
        .trim()
        .trim_start_matches('@')
        .to_ascii_lowercase()
}

pub(super) fn assignee_dialog_suggestion_matches(dialog: &AssigneeDialog) -> Vec<String> {
    let prefix = assignee_input_prefix(&dialog.input);
    let candidates = match dialog.action {
        AssigneeAction::Assign => &dialog.suggestions,
        AssigneeAction::Unassign => &dialog.item.assignees,
    };
    candidates
        .iter()
        .filter(|login| {
            dialog.action != AssigneeAction::Assign
                || !dialog
                    .item
                    .assignees
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(login))
        })
        .filter(|login| prefix.is_empty() || login.to_ascii_lowercase().starts_with(&prefix))
        .cloned()
        .collect()
}

pub(super) fn selected_assignee_suggestion(dialog: &AssigneeDialog) -> Option<String> {
    let matches = assignee_dialog_suggestion_matches(dialog);
    if matches.is_empty() {
        None
    } else {
        matches
            .get(dialog.selected_suggestion.min(matches.len() - 1))
            .cloned()
    }
}

pub(super) fn assignee_dialog_uses_default_unassign(dialog: &AssigneeDialog) -> bool {
    dialog.action == AssigneeAction::Unassign
        && dialog.input.trim().is_empty()
        && dialog.item.assignees.len() == 1
}

pub(super) fn assignee_dialog_submit_logins(dialog: &AssigneeDialog) -> Vec<String> {
    let mut assignees = parse_assignee_input(&dialog.input);
    if assignees.is_empty() && assignee_dialog_uses_default_unassign(dialog) {
        return dialog.item.assignees.clone();
    }
    if !assignees.is_empty()
        && !assignee_input_prefix(&dialog.input).is_empty()
        && let Some(selected) = selected_assignee_suggestion(dialog)
        && let Some(last) = assignees.last_mut()
    {
        *last = selected;
    }
    dedupe_assignee_logins(assignees)
}

pub(super) fn parse_reviewer_logins(input: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut reviewers = Vec::new();
    for login in input
        .split(',')
        .map(str::trim)
        .map(|login| login.trim_start_matches('@').trim())
        .filter(|login| !login.is_empty())
    {
        let key = login.to_ascii_lowercase();
        if seen.insert(key) {
            reviewers.push(login.to_string());
        }
    }
    reviewers
}

pub(super) fn reviewer_input_prefix(input: &str) -> String {
    input
        .rsplit(',')
        .next()
        .unwrap_or_default()
        .trim()
        .trim_start_matches('@')
        .to_ascii_lowercase()
}

pub(super) fn reviewer_dialog_suggestion_matches(dialog: &ReviewerDialog) -> Vec<String> {
    let prefix = reviewer_input_prefix(&dialog.input);
    dialog
        .suggestions
        .iter()
        .filter(|login| prefix.is_empty() || login.to_ascii_lowercase().starts_with(&prefix))
        .cloned()
        .collect()
}

pub(super) fn selected_reviewer_suggestion(dialog: &ReviewerDialog) -> Option<String> {
    let matches = reviewer_dialog_suggestion_matches(dialog);
    if matches.is_empty() {
        None
    } else {
        matches
            .get(dialog.selected_suggestion.min(matches.len() - 1))
            .cloned()
    }
}

pub(super) fn reviewer_dialog_submit_logins(dialog: &ReviewerDialog) -> Vec<String> {
    let mut reviewers = parse_reviewer_logins(&dialog.input);
    if !reviewers.is_empty()
        && !reviewer_input_prefix(&dialog.input).is_empty()
        && let Some(selected) = selected_reviewer_suggestion(dialog)
        && let Some(last) = reviewers.last_mut()
    {
        *last = selected;
    }
    dedupe_reviewer_logins(reviewers)
}

pub(super) fn dedupe_reviewer_logins(logins: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for login in logins {
        let key = login.to_ascii_lowercase();
        if seen.insert(key) {
            deduped.push(login);
        }
    }
    deduped
}
