use super::*;

impl AppState {
    pub(super) fn mention_candidate_view_for_target(
        &self,
        target: MentionTarget,
    ) -> Option<MentionCandidateView> {
        let context = self.active_mention_context()?;
        if context.target != target {
            return None;
        }
        let candidates = self.mention_candidate_matches(&context);
        let user_search_key = mention_user_search_key(&context.query);
        let selected = if candidates.is_empty() {
            0
        } else {
            self.mention_selected.min(candidates.len() - 1)
        };
        Some(MentionCandidateView {
            repo: context.repo.clone(),
            query: context.query,
            candidates,
            selected,
            loading: self.mention_candidate_loading_repos.contains(&context.repo)
                || user_search_key
                    .as_ref()
                    .is_some_and(|key| self.mention_user_search_loading_queries.contains(key)),
            error: user_search_key
                .and_then(|key| self.mention_user_search_errors.get(&key).cloned())
                .or_else(|| self.mention_candidate_errors.get(&context.repo).cloned()),
        })
    }

    pub(super) fn ensure_mention_candidates_for_active_editor(
        &mut self,
        store: Option<&SnapshotStore>,
        tx: &UnboundedSender<AppMsg>,
    ) {
        let Some(context) = self.active_mention_context() else {
            self.mention_selected = 0;
            return;
        };
        let count = self.mention_candidate_matches(&context).len();
        if count == 0 {
            self.mention_selected = 0;
        } else {
            self.mention_selected = self.mention_selected.min(count - 1);
        }

        self.ensure_mention_user_search_candidates(&context, tx);
        if !self.assignee_suggestions_cache.contains_key(&context.repo)
            && !self
                .mention_candidate_loading_repos
                .contains(context.repo.as_str())
        {
            self.mention_candidate_errors.remove(&context.repo);
            if start_assignee_suggestions_load(context.repo.clone(), store.cloned(), tx.clone()) {
                self.mention_candidate_loading_repos
                    .insert(context.repo.clone());
                self.status = format!("loading mention candidates for {}", context.repo);
            }
        }
    }

    pub(super) fn handle_active_mention_key(&mut self, key: KeyEvent) -> bool {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
        {
            return false;
        }
        let Some(context) = self.active_mention_context() else {
            self.mention_selected = 0;
            return false;
        };
        let candidates = self.mention_candidate_matches(&context);
        if candidates.is_empty() {
            self.mention_selected = 0;
            return false;
        }

        match key.code {
            KeyCode::Up => {
                self.mention_selected = move_wrapping(self.mention_selected, candidates.len(), -1);
                true
            }
            KeyCode::Down => {
                self.mention_selected = move_wrapping(self.mention_selected, candidates.len(), 1);
                true
            }
            KeyCode::Enter | KeyCode::Tab => {
                let login = candidates[self.mention_selected.min(candidates.len() - 1)].clone();
                self.replace_active_mention(&context, &login);
                self.status = format!("inserted mention @{login}");
                true
            }
            _ => false,
        }
    }

    fn active_mention_context(&self) -> Option<MentionContext> {
        if self.comment_dialog.is_some() {
            return self.mention_context_for_target(MentionTarget::Comment);
        }
        if self.review_submit_dialog.is_some() {
            return self.mention_context_for_target(MentionTarget::ReviewSubmit);
        }
        if let Some(dialog) = &self.issue_dialog {
            return match dialog.field {
                IssueDialogField::Title => {
                    self.mention_context_for_target(MentionTarget::IssueTitle)
                }
                IssueDialogField::Body => self.mention_context_for_target(MentionTarget::IssueBody),
                _ => None,
            };
        }
        if let Some(dialog) = &self.pr_create_dialog {
            return match dialog.field {
                PrCreateField::Title => {
                    self.mention_context_for_target(MentionTarget::PrCreateTitle)
                }
                PrCreateField::Body => self.mention_context_for_target(MentionTarget::PrCreateBody),
            };
        }
        if let Some(dialog) = &self.item_edit_dialog {
            return match dialog.field {
                ItemEditField::Title => {
                    self.mention_context_for_target(MentionTarget::ItemEditTitle)
                }
                ItemEditField::Body => self.mention_context_for_target(MentionTarget::ItemEditBody),
                _ => None,
            };
        }
        None
    }

    fn mention_context_for_target(&self, target: MentionTarget) -> Option<MentionContext> {
        match target {
            MentionTarget::Comment => {
                let item = self.current_item()?;
                let dialog = self.comment_dialog.as_ref()?;
                mention_context_from_editor(target, &item.repo, &dialog.body)
            }
            MentionTarget::ReviewSubmit => {
                let dialog = self.review_submit_dialog.as_ref()?;
                mention_context_from_editor(target, &dialog.item.repo, &dialog.body)
            }
            MentionTarget::IssueTitle => {
                let dialog = self.issue_dialog.as_ref()?;
                mention_context_from_editor(target, dialog.repo.text(), &dialog.title)
            }
            MentionTarget::IssueBody => {
                let dialog = self.issue_dialog.as_ref()?;
                mention_context_from_editor(target, dialog.repo.text(), &dialog.body)
            }
            MentionTarget::PrCreateTitle => {
                let dialog = self.pr_create_dialog.as_ref()?;
                mention_context_from_editor(target, &dialog.repo, &dialog.title)
            }
            MentionTarget::PrCreateBody => {
                let dialog = self.pr_create_dialog.as_ref()?;
                mention_context_from_editor(target, &dialog.repo, &dialog.body)
            }
            MentionTarget::ItemEditTitle => {
                let dialog = self.item_edit_dialog.as_ref()?;
                mention_context_from_editor(target, &dialog.item.repo, &dialog.title)
            }
            MentionTarget::ItemEditBody => {
                let dialog = self.item_edit_dialog.as_ref()?;
                mention_context_from_editor(target, &dialog.item.repo, &dialog.body)
            }
        }
    }

    fn mention_candidate_matches(&self, context: &MentionContext) -> Vec<String> {
        let mut candidates =
            global_search_author_candidates_from_sections(&self.sections, Some(&context.repo));
        candidates = merge_candidate_lists(
            candidates,
            self.sections
                .iter()
                .flat_map(|section| section.items.iter())
                .filter(|item| item.repo.eq_ignore_ascii_case(&context.repo))
                .flat_map(|item| item.assignees.clone()),
        );
        if let Some(assignees) = self.assignee_suggestions_cache.get(&context.repo) {
            candidates = merge_candidate_lists(candidates, assignees.clone());
        }
        if let Some(reviewers) = self.reviewer_suggestions_cache.get(&context.repo) {
            candidates = merge_candidate_lists(candidates, reviewers.clone());
        }
        candidates = merge_candidate_lists(
            candidates,
            self.loaded_comment_author_candidates_for_repo(&context.repo),
        );
        if let Some(key) = mention_user_search_key(&context.query)
            && let Some(users) = self.mention_user_search_cache.get(&key)
        {
            candidates = merge_candidate_lists(candidates, users.clone());
        }

        let query = context.query.to_ascii_lowercase();
        candidates
            .into_iter()
            .filter(|login| {
                query.is_empty() || login.to_ascii_lowercase().starts_with(query.as_str())
            })
            .collect()
    }

    fn loaded_comment_author_candidates_for_repo(&self, repo: &str) -> Vec<String> {
        let item_ids = self
            .sections
            .iter()
            .flat_map(|section| section.items.iter())
            .filter(|item| item.repo.eq_ignore_ascii_case(repo))
            .map(|item| item.id.clone())
            .collect::<HashSet<_>>();
        merge_candidate_lists(
            Vec::new(),
            self.details.iter().flat_map(|(item_id, state)| {
                if !item_ids.contains(item_id) {
                    return Vec::new();
                }
                match state {
                    DetailState::Loaded(comments) => comments
                        .iter()
                        .map(|comment| comment.author.clone())
                        .collect::<Vec<_>>(),
                    _ => Vec::new(),
                }
            }),
        )
    }

    fn ensure_mention_user_search_candidates(
        &mut self,
        context: &MentionContext,
        tx: &UnboundedSender<AppMsg>,
    ) {
        let Some(key) = mention_user_search_key(&context.query) else {
            return;
        };
        if self.mention_user_search_cache.contains_key(&key)
            || self.mention_user_search_loading_queries.contains(&key)
        {
            return;
        }
        self.mention_user_search_errors.remove(&key);
        if start_mention_user_search_load(key.clone(), tx.clone()) {
            self.mention_user_search_loading_queries.insert(key);
        }
    }

    fn replace_active_mention(&mut self, context: &MentionContext, login: &str) {
        let replacement = format!("@{login} ");
        match context.target {
            MentionTarget::Comment => {
                if let Some(dialog) = &mut self.comment_dialog {
                    dialog
                        .body
                        .replace_range(context.trigger_start, context.cursor, &replacement);
                }
            }
            MentionTarget::ReviewSubmit => {
                if let Some(dialog) = &mut self.review_submit_dialog {
                    dialog
                        .body
                        .replace_range(context.trigger_start, context.cursor, &replacement);
                }
            }
            MentionTarget::IssueTitle => {
                if let Some(dialog) = &mut self.issue_dialog {
                    dialog
                        .title
                        .replace_range(context.trigger_start, context.cursor, &replacement);
                }
            }
            MentionTarget::IssueBody => {
                if let Some(dialog) = &mut self.issue_dialog {
                    dialog
                        .body
                        .replace_range(context.trigger_start, context.cursor, &replacement);
                }
            }
            MentionTarget::PrCreateTitle => {
                if let Some(dialog) = &mut self.pr_create_dialog {
                    dialog
                        .title
                        .replace_range(context.trigger_start, context.cursor, &replacement);
                }
            }
            MentionTarget::PrCreateBody => {
                if let Some(dialog) = &mut self.pr_create_dialog {
                    dialog
                        .body
                        .replace_range(context.trigger_start, context.cursor, &replacement);
                }
            }
            MentionTarget::ItemEditTitle => {
                if let Some(dialog) = &mut self.item_edit_dialog {
                    dialog
                        .title
                        .replace_range(context.trigger_start, context.cursor, &replacement);
                }
            }
            MentionTarget::ItemEditBody => {
                if let Some(dialog) = &mut self.item_edit_dialog {
                    dialog
                        .body
                        .replace_range(context.trigger_start, context.cursor, &replacement);
                }
            }
        }
        self.mention_selected = 0;
    }
}

fn mention_context_from_editor(
    target: MentionTarget,
    repo: &str,
    editor: &EditorText,
) -> Option<MentionContext> {
    let repo = repo.trim();
    if !repo.contains('/') {
        return None;
    }
    let cursor = editor.cursor_byte();
    let (trigger_start, query) = mention_query_at_cursor(editor.text(), cursor)?;
    Some(MentionContext {
        target,
        repo: repo.to_string(),
        query,
        trigger_start,
        cursor,
    })
}

pub(super) fn mention_query_at_cursor(text: &str, cursor: usize) -> Option<(usize, String)> {
    let cursor = clamp_text_cursor(text, cursor);
    let before = &text[..cursor];
    for (index, ch) in before.char_indices().rev() {
        if ch == '@' {
            return Some((index, before[index + ch.len_utf8()..].to_string()));
        }
        if !is_github_login_char(ch) {
            return None;
        }
    }
    None
}

pub(super) fn mention_user_search_key(query: &str) -> Option<String> {
    let query = query.trim().trim_start_matches('@').to_ascii_lowercase();
    (!query.is_empty()).then_some(query)
}

fn is_github_login_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '-'
}
