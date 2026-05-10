use super::*;

pub(super) fn global_search_dialog_query(
    dialog: &GlobalSearchDialog,
) -> std::result::Result<String, String> {
    let title = dialog.title.text().trim();
    let status = dialog.status.text().trim();
    let labels = dialog.label.text().trim();
    let author = dialog.author.text().trim();
    let assignee = dialog.assignee.text().trim();
    let sort = dialog.sort.text().trim();
    let has_filters =
        !status.is_empty() || !labels.is_empty() || !author.is_empty() || !assignee.is_empty();

    if !has_filters
        && is_plain_number_lookup_title(title)
        && global_search_sort_token(sort)?.is_none()
    {
        return Ok(title.trim_start_matches('#').to_string());
    }

    let mut tokens = Vec::new();
    if !title.is_empty() {
        tokens.push(title.to_string());
    }
    if let Some(token) = global_search_status_token(status)? {
        tokens.push(token);
    }
    for label in parse_global_search_labels(labels) {
        tokens.push(format!(
            "label:{}",
            quote_global_search_value_if_needed(&label)
        ));
    }
    if let Some(author) = global_search_login_value(author) {
        tokens.push(format!("author:{author}"));
    }
    if let Some(assignee) = global_search_login_value(assignee) {
        tokens.push(format!("assignee:{assignee}"));
    }
    if tokens.is_empty() {
        return Err("search query is empty".to_string());
    }
    if let Some(token) = global_search_sort_token(sort)? {
        tokens.push(token);
    }

    Ok(tokens.join(" "))
}

pub(super) fn is_plain_number_lookup_title(value: &str) -> bool {
    let value = value.trim().trim_start_matches('#');
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub(super) fn global_search_status_token(
    value: &str,
) -> std::result::Result<Option<String>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let value = value
        .strip_prefix("status:")
        .or_else(|| value.strip_prefix("state:"))
        .or_else(|| value.strip_prefix("is:"))
        .unwrap_or(value)
        .trim()
        .to_ascii_lowercase();
    match value.as_str() {
        "open" => Ok(Some("is:open".to_string())),
        "closed" | "close" => Ok(Some("is:closed".to_string())),
        "merged" => Ok(Some("is:merged".to_string())),
        "draft" => Ok(Some("is:draft".to_string())),
        "all" | "any" => Ok(None),
        _ => Err(format!("unknown status filter: {value}")),
    }
}

pub(super) fn parse_global_search_labels(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(|label| {
            label
                .strip_prefix("label:")
                .or_else(|| label.strip_prefix("labels:"))
                .unwrap_or(label)
                .trim()
                .trim_matches('"')
                .to_string()
        })
        .filter(|label| !label.is_empty())
        .collect()
}

pub(super) fn global_search_login_value(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    Some(
        value
            .split(|ch: char| ch == ',' || ch.is_whitespace())
            .next()
            .unwrap_or_default()
            .trim()
            .trim_start_matches('@')
            .to_string(),
    )
    .filter(|login| !login.is_empty())
}

pub(super) fn quote_global_search_value_if_needed(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/'))
    {
        return value.to_string();
    }
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

pub(super) fn global_search_sort_token(value: &str) -> std::result::Result<Option<String>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let value = value.strip_prefix("sort:").unwrap_or(value);
    let normalized = value
        .replace([':', ','], " ")
        .replace('-', " ")
        .to_ascii_lowercase();
    let mut parts = normalized.split_whitespace();
    let Some(field) = parts.next() else {
        return Ok(None);
    };
    let order = parts.next().unwrap_or("desc");
    if !matches!(order, "asc" | "desc") {
        return Err(format!("unknown sort order: {order}"));
    }
    let field = match field {
        "created" | "created_at" => "created",
        "updated" | "updated_at" => "updated",
        "comments" => "comments",
        "reactions" => "reactions",
        "interactions" => "interactions",
        _ => return Err(format!("unknown sort field: {field}")),
    };
    if field == "created" && order == "desc" {
        return Ok(None);
    }
    Ok(Some(format!("sort:{field}-{order}")))
}

pub(super) fn global_search_static_status_choices() -> Vec<String> {
    ["open", "closed", "merged", "draft", "all"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

pub(super) fn global_search_static_sort_choices() -> Vec<String> {
    [
        "created_at",
        "created_at asc",
        "updated_at",
        "updated_at asc",
        "comments",
        "comments asc",
        "reactions",
        "interactions",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

pub(super) fn global_search_dialog_suggestion_matches(dialog: &GlobalSearchDialog) -> Vec<String> {
    match dialog.field {
        GlobalSearchField::Title => Vec::new(),
        GlobalSearchField::Status => prefix_filter_candidates(
            &global_search_static_status_choices(),
            global_search_dialog_status_suggestion_prefix(dialog),
        ),
        GlobalSearchField::Label => substring_filter_candidates(
            &dialog.labels,
            global_search_dialog_label_suggestion_prefix(dialog),
        ),
        GlobalSearchField::Author => substring_filter_candidates(
            &dialog.author_candidates,
            global_search_dialog_author_suggestion_prefix(dialog),
        ),
        GlobalSearchField::Assignee => substring_filter_candidates(
            &dialog.assignee_candidates,
            global_search_dialog_assignee_suggestion_prefix(dialog),
        ),
        GlobalSearchField::Sort => prefix_filter_candidates(
            &global_search_static_sort_choices(),
            global_search_dialog_sort_suggestion_prefix(dialog),
        ),
    }
}

pub(super) fn clamp_global_search_dialog_selection(dialog: &mut GlobalSearchDialog) {
    let count = global_search_dialog_suggestion_matches(dialog).len();
    if count == 0 {
        dialog.selected_suggestion = 0;
    } else {
        dialog.selected_suggestion = dialog.selected_suggestion.min(count - 1);
    }
}

pub(super) fn reset_global_search_dialog_suggestions(dialog: &mut GlobalSearchDialog) {
    dialog.suggestion_filter = None;
    let matches = global_search_dialog_suggestion_matches(dialog);
    dialog.selected_suggestion =
        global_search_dialog_current_suggestion_index(dialog, &matches).unwrap_or(0);
    clamp_global_search_dialog_selection(dialog);
}

pub(super) fn global_search_dialog_suggestion_state(
    dialog: &GlobalSearchDialog,
) -> Option<&'static str> {
    match dialog.field {
        GlobalSearchField::Label if dialog.labels_loading && dialog.labels.is_empty() => {
            Some("loading labels")
        }
        GlobalSearchField::Label if dialog.labels_error.is_some() && dialog.labels.is_empty() => {
            Some("label candidates unavailable")
        }
        GlobalSearchField::Author
            if dialog.assignees_loading && dialog.author_candidates.is_empty() =>
        {
            Some("loading users")
        }
        GlobalSearchField::Assignee
            if dialog.assignees_loading && dialog.assignee_candidates.is_empty() =>
        {
            Some("loading users")
        }
        GlobalSearchField::Author
            if dialog.assignees_error.is_some() && dialog.author_candidates.is_empty() =>
        {
            Some("user candidates unavailable")
        }
        GlobalSearchField::Assignee
            if dialog.assignees_error.is_some() && dialog.assignee_candidates.is_empty() =>
        {
            Some("user candidates unavailable")
        }
        _ => None,
    }
}

pub(super) fn global_search_dialog_locked_prefix(
    dialog: &GlobalSearchDialog,
    field: GlobalSearchField,
) -> Option<String> {
    dialog
        .suggestion_filter
        .as_ref()
        .filter(|filter| filter.field == field)
        .map(|filter| filter.prefix.clone())
}

pub(super) fn global_search_dialog_status_suggestion_prefix(dialog: &GlobalSearchDialog) -> String {
    global_search_dialog_locked_prefix(dialog, GlobalSearchField::Status).unwrap_or_else(|| {
        static_dropdown_suggestion_prefix(
            &global_search_static_status_choices(),
            normalized_global_search_status_prefix(dialog.status.text()),
        )
    })
}

pub(super) fn global_search_dialog_label_suggestion_prefix(dialog: &GlobalSearchDialog) -> String {
    global_search_dialog_locked_prefix(dialog, GlobalSearchField::Label)
        .unwrap_or_else(|| label_completion_prefix(dialog.label.text()))
}

pub(super) fn global_search_dialog_author_suggestion_prefix(dialog: &GlobalSearchDialog) -> String {
    global_search_dialog_locked_prefix(dialog, GlobalSearchField::Author)
        .unwrap_or_else(|| login_completion_prefix(dialog.author.text()))
}

pub(super) fn global_search_dialog_assignee_suggestion_prefix(
    dialog: &GlobalSearchDialog,
) -> String {
    global_search_dialog_locked_prefix(dialog, GlobalSearchField::Assignee)
        .unwrap_or_else(|| login_completion_prefix(dialog.assignee.text()))
}

pub(super) fn global_search_dialog_sort_suggestion_prefix(dialog: &GlobalSearchDialog) -> String {
    global_search_dialog_locked_prefix(dialog, GlobalSearchField::Sort).unwrap_or_else(|| {
        static_dropdown_suggestion_prefix(
            &global_search_static_sort_choices(),
            normalized_global_search_sort_prefix(dialog.sort.text()),
        )
    })
}

pub(super) fn global_search_dialog_current_suggestion_prefix(
    dialog: &GlobalSearchDialog,
) -> String {
    match dialog.field {
        GlobalSearchField::Title => dialog.title.text().trim().to_string(),
        GlobalSearchField::Status => global_search_dialog_status_suggestion_prefix(dialog),
        GlobalSearchField::Label => global_search_dialog_label_suggestion_prefix(dialog),
        GlobalSearchField::Author => global_search_dialog_author_suggestion_prefix(dialog),
        GlobalSearchField::Assignee => global_search_dialog_assignee_suggestion_prefix(dialog),
        GlobalSearchField::Sort => global_search_dialog_sort_suggestion_prefix(dialog),
    }
}

pub(super) fn global_search_dialog_current_suggestion_value(dialog: &GlobalSearchDialog) -> String {
    match dialog.field {
        GlobalSearchField::Title => String::new(),
        GlobalSearchField::Status => normalized_global_search_status_prefix(dialog.status.text()),
        GlobalSearchField::Label => label_completion_prefix(dialog.label.text()),
        GlobalSearchField::Author => login_completion_prefix(dialog.author.text()),
        GlobalSearchField::Assignee => login_completion_prefix(dialog.assignee.text()),
        GlobalSearchField::Sort => normalized_global_search_sort_prefix(dialog.sort.text()),
    }
}

pub(super) fn global_search_dialog_current_suggestion_index(
    dialog: &GlobalSearchDialog,
    matches: &[String],
) -> Option<usize> {
    let value = global_search_dialog_current_suggestion_value(dialog).to_ascii_lowercase();
    if value.is_empty() {
        return None;
    }
    matches
        .iter()
        .position(|candidate| candidate.to_ascii_lowercase() == value)
}

pub(super) fn static_dropdown_suggestion_prefix(choices: &[String], prefix: String) -> String {
    let prefix = prefix.trim().to_string();
    if prefix.is_empty()
        || choices
            .iter()
            .any(|choice| choice.eq_ignore_ascii_case(&prefix))
    {
        String::new()
    } else {
        prefix
    }
}

pub(super) fn prefix_filter_candidates(candidates: &[String], prefix: String) -> Vec<String> {
    let prefix = prefix.to_ascii_lowercase();
    candidates
        .iter()
        .filter(|candidate| {
            prefix.is_empty() || candidate.to_ascii_lowercase().starts_with(&prefix)
        })
        .cloned()
        .collect()
}

pub(super) fn substring_filter_candidates(candidates: &[String], query: String) -> Vec<String> {
    let query = query.to_ascii_lowercase();
    candidates
        .iter()
        .filter(|candidate| query.is_empty() || candidate.to_ascii_lowercase().contains(&query))
        .cloned()
        .collect()
}

pub(super) fn global_search_state_display(state: &GlobalSearchState) -> String {
    let mut parts = Vec::new();
    if !state.title.is_empty() {
        parts.push(format!("title:{}", state.title));
    }
    if !state.status.is_empty() {
        parts.push(format!("status:{}", state.status));
    }
    if !state.label.is_empty() {
        parts.push(format!("label:{}", state.label));
    }
    if !state.author.is_empty() {
        parts.push(format!("author:{}", state.author));
    }
    if !state.assignee.is_empty() {
        parts.push(format!("assignee:{}", state.assignee));
    }
    if state.sort != "created_at" {
        parts.push(format!("sort:{}", state.sort));
    }
    if parts.is_empty() {
        "default search".to_string()
    } else {
        parts.join(" | ")
    }
}

pub(super) fn saved_search_active_filter_label(candidate: &SavedSearchCandidate) -> String {
    let details = global_search_state_display(&candidate.search);
    if details == "default search" {
        format!("saved: {}", candidate.name)
    } else {
        format!("saved: {} ({details})", candidate.name)
    }
}

pub(super) fn saved_search_kind_value(kind: SectionKind) -> Option<&'static str> {
    match kind {
        SectionKind::PullRequests => Some("pull_requests"),
        SectionKind::Issues => Some("issues"),
        SectionKind::Notifications => None,
    }
}

pub(super) fn section_kind_from_saved_search_kind(value: &str) -> Option<SectionKind> {
    match value.trim().to_ascii_lowercase().as_str() {
        "pr" | "prs" | "pull_request" | "pull_requests" => Some(SectionKind::PullRequests),
        "issue" | "issues" => Some(SectionKind::Issues),
        _ => None,
    }
}

pub(super) fn saved_search_kind_label(kind: SectionKind) -> &'static str {
    match kind {
        SectionKind::PullRequests => "PR",
        SectionKind::Issues => "Issue",
        SectionKind::Notifications => "Notification",
    }
}

pub(super) fn global_search_kind_token(kind: SectionKind) -> Option<&'static str> {
    match kind {
        SectionKind::PullRequests => Some("type:pr"),
        SectionKind::Issues => Some("type:issue"),
        SectionKind::Notifications => None,
    }
}

pub(super) fn scoped_global_search_query(query: &str, kind: Option<SectionKind>) -> String {
    let query = query.trim();
    match kind.and_then(global_search_kind_token) {
        Some(token) if !query.split_whitespace().any(is_search_type_token) => {
            format!("{query} {token}")
        }
        _ => query.to_string(),
    }
}

pub(super) fn is_search_type_token(token: &str) -> bool {
    let token = token.trim().to_ascii_lowercase();
    matches!(
        token.as_str(),
        "type:pr"
            | "type:prs"
            | "is:pr"
            | "type:pull-request"
            | "type:pull_request"
            | "type:issue"
            | "type:issues"
            | "is:issue"
    )
}

pub(super) fn saved_search_default_name(
    repo: &str,
    kind: SectionKind,
    state: &GlobalSearchState,
) -> String {
    let mut name = format!("{repo} {}", saved_search_kind_label(kind));
    let display = global_search_state_display(state);
    if display != "default search" {
        name.push(' ');
        name.push_str(&display.replace(" | ", " "));
    }
    truncate_inline(&name, 64)
}

pub(super) fn upsert_named_saved_search(
    saved_by_repo: &mut HashMap<String, Vec<GlobalSearchSavedState>>,
    name: String,
    repo: String,
    kind: SectionKind,
    search: GlobalSearchState,
) {
    let key = name.trim().to_ascii_lowercase();
    if key.is_empty() {
        return;
    }
    for searches in saved_by_repo.values_mut() {
        searches.retain(|saved| saved.name.trim().to_ascii_lowercase() != key);
    }
    let repo_key = global_search_repo_state_key(Some(&repo));
    let Some(kind) = saved_search_kind_value(kind) else {
        return;
    };
    let searches = saved_by_repo.entry(repo_key).or_default();
    searches.insert(
        0,
        GlobalSearchSavedState {
            name,
            repo,
            kind: kind.to_string(),
            search,
        },
    );
    searches.truncate(MAX_GLOBAL_SAVED_SEARCHES_PER_REPO);
}

pub(super) fn saved_search_map_from_config(
    config: &Config,
) -> HashMap<String, Vec<GlobalSearchSavedState>> {
    let mut saved_by_repo: HashMap<String, Vec<GlobalSearchSavedState>> = HashMap::new();
    for saved in &config.saved_search_filters {
        let Some(saved) = saved.clone().into_saved_state() else {
            continue;
        };
        let repo_key = global_search_repo_state_key(Some(&saved.repo));
        saved_by_repo.entry(repo_key).or_default().push(saved);
    }
    normalize_saved_search_map(saved_by_repo)
}

pub(super) fn saved_search_filters_from_map(
    saved_by_repo: &HashMap<String, Vec<GlobalSearchSavedState>>,
) -> Vec<SavedSearchFilterConfig> {
    let mut saved = normalize_saved_search_map(saved_by_repo.clone())
        .into_values()
        .flatten()
        .filter_map(SavedSearchFilterConfig::from_saved_state)
        .collect::<Vec<_>>();
    saved.sort_by(|left, right| {
        left.repo
            .cmp(&right.repo)
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| {
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase())
            })
    });
    saved
}

pub(super) fn normalize_saved_search_map(
    saved_by_repo: HashMap<String, Vec<GlobalSearchSavedState>>,
) -> HashMap<String, Vec<GlobalSearchSavedState>> {
    saved_by_repo
        .into_iter()
        .filter_map(|(repo_key, searches)| {
            let repo_key = global_search_repo_state_key(Some(&repo_key));
            let mut normalized = searches
                .into_iter()
                .filter_map(|mut saved| {
                    if saved.repo.trim().is_empty() {
                        saved.repo = repo_key.clone();
                    }
                    saved.normalized()
                })
                .collect::<Vec<_>>();
            normalized.sort_by(|left, right| {
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase())
            });

            let mut seen = HashSet::new();
            normalized.retain(|saved| seen.insert(saved.name.to_ascii_lowercase()));
            normalized.truncate(MAX_GLOBAL_SAVED_SEARCHES_PER_REPO);
            (!normalized.is_empty()).then_some((repo_key, normalized))
        })
        .collect()
}

pub(super) fn saved_search_filter_query(
    base_filters: &str,
    search: &GlobalSearchState,
) -> std::result::Result<String, String> {
    if base_filters.contains(" | ") {
        return base_filters
            .split(" | ")
            .map(|filters| saved_search_filter_query(filters, search))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map(|queries| queries.join(" | "));
    }

    let overlay_tokens = saved_search_filter_tokens(search)?;
    let base_tokens = base_filters
        .split_whitespace()
        .filter(|token| !saved_search_replaces_token(token, search))
        .map(str::to_string)
        .collect::<Vec<_>>();
    Ok(insert_search_tokens_before_sort(base_tokens, overlay_tokens).join(" "))
}

pub(super) fn saved_search_filter_tokens(
    search: &GlobalSearchState,
) -> std::result::Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let title = search.title.trim();
    if !title.is_empty() {
        tokens.push(title.trim_start_matches('#').to_string());
        if !is_plain_number_lookup_title(title) {
            tokens.push("in:title".to_string());
        }
    }
    if let Some(token) = global_search_status_token(&search.status)? {
        tokens.push(token);
    }
    for label in parse_global_search_labels(&search.label) {
        tokens.push(format!(
            "label:{}",
            quote_global_search_value_if_needed(&label)
        ));
    }
    if let Some(author) = global_search_login_value(&search.author) {
        tokens.push(format!("author:{author}"));
    }
    if let Some(assignee) = global_search_login_value(&search.assignee) {
        tokens.push(format!("assignee:{assignee}"));
    }
    if let Some(token) = global_search_sort_token(&search.sort)? {
        tokens.push(token);
    }
    Ok(tokens)
}

pub(super) fn saved_search_replaces_token(token: &str, search: &GlobalSearchState) -> bool {
    (!search.status.trim().is_empty() && is_saved_search_state_token(token))
        || (!search.label.trim().is_empty()
            && (token.starts_with("label:") || token.starts_with("labels:")))
        || (!search.author.trim().is_empty() && token.starts_with("author:"))
        || (!search.assignee.trim().is_empty() && token.starts_with("assignee:"))
        || (!search.title.trim().is_empty() && token == "in:title")
        || (global_search_sort_token(&search.sort)
            .ok()
            .flatten()
            .is_some()
            && token.starts_with("sort:"))
}

pub(super) fn is_saved_search_state_token(token: &str) -> bool {
    matches!(
        token,
        "is:open" | "is:closed" | "is:merged" | "is:draft" | "draft:true" | "draft:false"
    ) || token.starts_with("state:")
}

pub(super) fn insert_search_tokens_before_sort(
    mut base_tokens: Vec<String>,
    overlay_tokens: Vec<String>,
) -> Vec<String> {
    if overlay_tokens.is_empty() {
        return base_tokens;
    }
    let sort_index = base_tokens
        .iter()
        .position(|token| token.starts_with("sort:"))
        .unwrap_or(base_tokens.len());
    base_tokens.splice(sort_index..sort_index, overlay_tokens);
    base_tokens
}

pub(super) fn normalized_global_search_status_prefix(value: &str) -> String {
    value
        .trim()
        .strip_prefix("status:")
        .or_else(|| value.trim().strip_prefix("state:"))
        .or_else(|| value.trim().strip_prefix("is:"))
        .unwrap_or(value.trim())
        .to_string()
}

pub(super) fn normalized_global_search_sort_prefix(value: &str) -> String {
    value
        .trim()
        .strip_prefix("sort:")
        .unwrap_or(value.trim())
        .replace([':', '-'], " ")
}

pub(super) fn label_completion_prefix(value: &str) -> String {
    value
        .rsplit(',')
        .next()
        .unwrap_or_default()
        .trim()
        .strip_prefix("label:")
        .or_else(|| {
            value
                .rsplit(',')
                .next()
                .unwrap_or_default()
                .trim()
                .strip_prefix("labels:")
        })
        .unwrap_or_else(|| value.rsplit(',').next().unwrap_or_default().trim())
        .trim_matches('"')
        .to_string()
}

pub(super) fn login_completion_prefix(value: &str) -> String {
    value
        .rsplit(|ch: char| ch == ',' || ch.is_whitespace())
        .next()
        .unwrap_or_default()
        .trim()
        .trim_start_matches('@')
        .to_string()
}

pub(super) fn apply_global_search_dialog_suggestion(
    dialog: &mut GlobalSearchDialog,
    suggestion: &str,
) {
    match dialog.field {
        GlobalSearchField::Title => {}
        GlobalSearchField::Status => dialog.status.set_text(suggestion),
        GlobalSearchField::Label => {
            let value = replace_last_comma_component(dialog.label.text(), suggestion);
            dialog.label.set_text(value);
        }
        GlobalSearchField::Author => dialog.author.set_text(suggestion),
        GlobalSearchField::Assignee => dialog.assignee.set_text(suggestion),
        GlobalSearchField::Sort => dialog.sort.set_text(suggestion),
    }
}

pub(super) fn apply_global_search_state(
    dialog: &mut GlobalSearchDialog,
    state: &GlobalSearchState,
) {
    dialog.title.set_text(&state.title);
    dialog.status.set_text(&state.status);
    dialog.label.set_text(&state.label);
    dialog.author.set_text(&state.author);
    dialog.assignee.set_text(&state.assignee);
    dialog.sort.set_text(&state.sort);
    dialog.field = GlobalSearchField::from_state_str(&state.field);
    reset_global_search_dialog_suggestions(dialog);
}

pub(super) fn clear_global_search_dialog_conditions(dialog: &mut GlobalSearchDialog) {
    dialog.title.set_text("");
    dialog.status.set_text("");
    dialog.label.set_text("");
    dialog.author.set_text("");
    dialog.assignee.set_text("");
    dialog.sort.set_text("created_at");
    dialog.field = GlobalSearchField::Title;
    reset_global_search_dialog_suggestions(dialog);
}

pub(super) fn global_search_dialog_state(dialog: &GlobalSearchDialog) -> Option<GlobalSearchState> {
    GlobalSearchState {
        title: dialog.title.text().to_string(),
        status: dialog.status.text().to_string(),
        label: dialog.label.text().to_string(),
        author: dialog.author.text().to_string(),
        assignee: dialog.assignee.text().to_string(),
        sort: dialog.sort.text().to_string(),
        field: dialog.field.as_state_str().to_string(),
    }
    .normalized()
}

pub(super) fn global_search_repo_state_key(repo: Option<&str>) -> String {
    repo.map(str::trim)
        .filter(|repo| !repo.is_empty())
        .map(|repo| repo.to_ascii_lowercase())
        .unwrap_or_else(|| "__global__".to_string())
}

pub(super) fn replace_last_comma_component(value: &str, replacement: &str) -> String {
    match value.rsplit_once(',') {
        Some((head, _)) => format!("{head}, {replacement}"),
        None => replacement.to_string(),
    }
}

pub(super) fn merge_candidate_lists<I>(base: Vec<String>, extra: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let mut seen = HashSet::new();
    let mut merged = Vec::new();
    for candidate in base.into_iter().chain(extra) {
        let candidate = candidate.trim().trim_start_matches('@').to_string();
        if candidate.is_empty() {
            continue;
        }
        if seen.insert(candidate.to_ascii_lowercase()) {
            merged.push(candidate);
        }
    }
    merged
}

pub(super) fn global_search_author_candidates_from_sections(
    sections: &[SectionSnapshot],
    repo: Option<&str>,
) -> Vec<String> {
    merge_candidate_lists(
        Vec::new(),
        sections
            .iter()
            .flat_map(|section| section.items.iter())
            .filter(|item| repo.is_none_or(|repo| item.repo.eq_ignore_ascii_case(repo)))
            .filter_map(|item| item.author.clone()),
    )
}
