use crate::model::{SectionSnapshot, WorkItem};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct QuickFilter {
    state: Option<QuickFilterState>,
    assignee: Option<String>,
    author: Option<String>,
    labels: Vec<String>,
}

impl QuickFilter {
    pub(super) fn parse(input: &str) -> std::result::Result<Option<Self>, String> {
        let input = input.trim();
        if input.is_empty() || matches!(input, "clear" | "reset") {
            return Ok(None);
        }

        let mut filter = Self::default();
        for token in input.split_whitespace() {
            if let Some(state) = QuickFilterState::parse(token) {
                filter.state = Some(state);
                continue;
            }
            if let Some(value) = token
                .strip_prefix("state:")
                .or_else(|| token.strip_prefix("is:"))
            {
                filter.state = Some(
                    QuickFilterState::parse(value)
                        .ok_or_else(|| format!("unknown state filter: {value}"))?,
                );
                continue;
            }
            if let Some(value) = token.strip_prefix("assignee:") {
                filter.assignee = Some(non_empty_filter_value("assignee", value)?);
                continue;
            }
            if let Some(value) = token.strip_prefix("author:") {
                filter.author = Some(non_empty_filter_value("author", value)?);
                continue;
            }
            if let Some(value) = token
                .strip_prefix("label:")
                .or_else(|| token.strip_prefix("labels:"))
            {
                for label in comma_separated_filter_values("label", value)? {
                    if !filter.labels.contains(&label) {
                        filter.labels.push(label);
                    }
                }
                continue;
            }

            return Err(format!("unknown filter token: {token}"));
        }

        Ok(Some(filter))
    }

    pub(super) fn display(&self) -> String {
        let mut tokens = Vec::new();
        if let Some(state) = self.state {
            tokens.push(format!("state:{}", state.display()));
        }
        if let Some(assignee) = &self.assignee {
            tokens.push(format!("assignee:{assignee}"));
        }
        if let Some(author) = &self.author {
            tokens.push(format!("author:{author}"));
        }
        tokens.extend(self.labels.iter().map(|label| format!("label:{label}")));
        tokens.join(" ")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuickFilterState {
    Open,
    Closed,
    Merged,
    Draft,
    All,
}

impl QuickFilterState {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "open" => Some(Self::Open),
            "closed" | "close" => Some(Self::Closed),
            "merged" => Some(Self::Merged),
            "draft" => Some(Self::Draft),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    fn display(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Merged => "merged",
            Self::Draft => "draft",
            Self::All => "all",
        }
    }

    fn query_token(self) -> Option<&'static str> {
        match self {
            Self::Open => Some("is:open"),
            Self::Closed => Some("is:closed"),
            Self::Merged => Some("is:merged"),
            Self::Draft => Some("is:draft"),
            Self::All => None,
        }
    }
}

pub(super) fn filtered_indices(section: &SectionSnapshot, query: &str) -> Vec<usize> {
    let query = query.trim();
    if query.is_empty() {
        return (0..section.items.len()).collect();
    }

    let mut scored = section
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| fuzzy_score_item(item, query).map(|score| (index, score)))
        .collect::<Vec<_>>();
    scored.sort_by(|(left_index, left_score), (right_index, right_score)| {
        right_score
            .cmp(left_score)
            .then_with(|| left_index.cmp(right_index))
    });
    scored.into_iter().map(|(index, _)| index).collect()
}

pub(super) fn fuzzy_score(query: &str, haystack: &str) -> Option<i64> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    if let Some(index) = haystack.find(&query) {
        return Some(10_000 - index as i64);
    }

    let mut score = 0;
    let mut search_start = 0;
    let mut previous_match = None;
    for needle in query.chars() {
        let mut matched = None;
        for (offset, candidate) in haystack[search_start..].char_indices() {
            if candidate == needle {
                matched = Some(search_start + offset);
                break;
            }
        }

        let index = matched?;
        score += 100;
        if let Some(previous) = previous_match {
            let gap = index.saturating_sub(previous + 1);
            if gap > 32 {
                return None;
            }
            if gap == 0 {
                score += 30;
            } else {
                score -= gap.min(30) as i64;
            }
        } else {
            score -= index.min(50) as i64;
        }
        if index == 0 || haystack[..index].ends_with([' ', '/', '#', '-', '_']) {
            score += 20;
        }

        previous_match = Some(index);
        search_start = index + needle.len_utf8();
    }

    Some(score)
}

fn fuzzy_score_item(item: &WorkItem, query: &str) -> Option<i64> {
    let haystack = searchable_text(item);
    let mut total = 0;
    for token in query.split_whitespace() {
        total += fuzzy_score(token, &haystack)?;
    }
    Some(total)
}

pub(super) fn quick_filter_query(base_filters: &str, filter: &QuickFilter) -> String {
    if base_filters.contains(" | ") {
        return base_filters
            .split(" | ")
            .map(|filters| quick_filter_query(filters, filter))
            .collect::<Vec<_>>()
            .join(" | ");
    }

    let base_tokens = base_filters
        .split_whitespace()
        .filter(|token| !quick_filter_replaces_token(token, filter))
        .map(str::to_string)
        .collect::<Vec<_>>();
    let overlay_tokens = quick_filter_tokens(filter);
    insert_tokens_before_sort(base_tokens, overlay_tokens).join(" ")
}

fn quick_filter_replaces_token(token: &str, filter: &QuickFilter) -> bool {
    (filter.state.is_some() && is_state_filter_token(token))
        || (filter.assignee.is_some() && token.starts_with("assignee:"))
        || (filter.author.is_some() && token.starts_with("author:"))
        || (!filter.labels.is_empty()
            && (token.starts_with("label:") || token.starts_with("labels:")))
}

fn quick_filter_tokens(filter: &QuickFilter) -> Vec<String> {
    let mut tokens = Vec::new();
    if let Some(token) = filter.state.and_then(QuickFilterState::query_token) {
        tokens.push(token.to_string());
    }
    if let Some(assignee) = &filter.assignee {
        tokens.push(format!("assignee:{assignee}"));
    }
    if let Some(author) = &filter.author {
        tokens.push(format!("author:{author}"));
    }
    tokens.extend(filter.labels.iter().map(|label| format!("label:{label}")));
    tokens
}

fn insert_tokens_before_sort(
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

fn is_state_filter_token(token: &str) -> bool {
    matches!(
        token,
        "is:open" | "is:closed" | "is:merged" | "is:draft" | "draft:true" | "draft:false"
    ) || token.starts_with("state:")
}

fn non_empty_filter_value(name: &str, value: &str) -> std::result::Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        Err(format!("{name} filter is empty"))
    } else {
        Ok(value.to_string())
    }
}

fn comma_separated_filter_values(
    name: &str,
    value: &str,
) -> std::result::Result<Vec<String>, String> {
    let values = value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if values.is_empty() {
        Err(format!("{name} filter is empty"))
    } else {
        Ok(values)
    }
}

fn searchable_text(item: &WorkItem) -> String {
    let mut parts = vec![item.repo.clone(), item.title.clone(), item.url.clone()];
    if let Some(number) = item.number {
        parts.push(format!("#{number}"));
        parts.push(number.to_string());
    }
    if let Some(author) = &item.author {
        parts.push(author.clone());
    }
    if let Some(state) = &item.state {
        parts.push(state.clone());
    }
    if let Some(reason) = &item.reason {
        parts.push(reason.clone());
    }
    if let Some(extra) = &item.extra {
        parts.push(extra.clone());
    }
    if let Some(body) = &item.body {
        parts.push(body.clone());
    }
    parts.extend(item.labels.iter().cloned());
    parts.join(" ").to_lowercase()
}
