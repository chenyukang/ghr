use crate::model::{SectionSnapshot, WorkItem};

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
