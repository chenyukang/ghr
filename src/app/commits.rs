use super::*;
use ratatui::layout::Layout;
use std::borrow::Cow;

#[derive(Debug, Clone)]
pub(super) enum CommitsState {
    Loading,
    Loaded(Vec<PullRequestCommit>),
    Error(String),
}

#[derive(Debug, Clone)]
pub(super) struct CommitPicker {
    item_id: String,
    selected: usize,
    open_sha: Option<String>,
    selection: Option<CommitSelection>,
    all_commits: bool,
}

fn scope_key(item_id: &str, selection: Option<&CommitSelection>) -> String {
    match selection {
        Some(selection) => format!("{item_id}::commit:{}", selection.key()),
        None => item_id.to_string(),
    }
}

fn commit_indices(
    selection: &CommitSelection,
    commits: &[PullRequestCommit],
) -> Option<(usize, usize)> {
    let first = commits
        .iter()
        .position(|commit| commit.oid == selection.first())?;
    let last = commits
        .iter()
        .position(|commit| commit.oid == selection.last())?;
    (first <= last).then_some((first, last))
}

impl AppState {
    pub(super) fn diff_scope_key(&self, item_id: &str) -> String {
        scope_key(item_id, self.selected_commits.get(item_id))
    }

    /// The web range URL excludes the commit immediately before the selection.
    /// A range starting at the first PR commit has no excluded commit.
    pub(super) fn commit_range_base(
        &self,
        item_id: &str,
        selection: &CommitSelection,
    ) -> Option<Option<String>> {
        let CommitsState::Loaded(commits) = self.commits.get(item_id)? else {
            return None;
        };
        let (first, _) = commit_indices(selection, commits)?;
        Some(first.checked_sub(1).map(|index| commits[index].oid.clone()))
    }

    pub(super) fn ensure_commits_loading(&mut self, tx: Option<&UnboundedSender<AppMsg>>) {
        let Some(item) = self.current_item().cloned() else {
            return;
        };
        if item.kind != ItemKind::PullRequest
            || item.number.is_none()
            || self.commits.contains_key(&item.id)
        {
            return;
        }
        let Some(tx) = tx.cloned() else { return };
        self.commits.insert(item.id.clone(), CommitsState::Loading);
        start_commits_load(item, tx);
    }

    pub(super) fn open_commit_picker(
        &mut self,
        sha: Option<String>,
        tx: Option<&UnboundedSender<AppMsg>>,
    ) {
        let Some(item) = self.current_item() else {
            return;
        };
        if item.kind != ItemKind::PullRequest {
            self.status = "commits only available for pull requests".into();
            return;
        }
        let item_id = item.id.clone();
        let selected_sha = sha.as_deref().or_else(|| {
            self.selected_commits
                .get(&item_id)
                .map(CommitSelection::first)
        });
        let selected = match self.commits.get(&item_id) {
            Some(CommitsState::Loaded(commits)) => selected_sha
                .and_then(|sha| commits.iter().position(|commit| commit.oid == sha))
                .map_or(0, |index| index + 1),
            _ => 0,
        };
        self.commit_picker = Some(CommitPicker {
            item_id: item_id.clone(),
            selected,
            open_sha: sha,
            selection: self.selected_commits.get(&item_id).cloned(),
            all_commits: !self.selected_commits.contains_key(&item_id),
        });
        if matches!(self.commits.get(&item_id), Some(CommitsState::Error(_))) {
            self.commits.remove(&item_id);
        }
        self.ensure_commits_loading(tx);
        self.open_requested_commit();
    }

    fn open_requested_commit(&mut self) {
        let Some(picker) = self.commit_picker.as_ref() else {
            return;
        };
        if !self
            .current_item()
            .is_some_and(|item| item.id == picker.item_id)
        {
            self.commit_picker = None;
            return;
        }
        let Some(sha) = picker.open_sha.as_ref() else {
            return;
        };
        let Some(CommitsState::Loaded(commits)) = self.commits.get(&picker.item_id) else {
            return;
        };
        if let Some(index) = commits.iter().position(|commit| &commit.oid == sha) {
            let sha = commits[index].oid.clone();
            self.commit_picker = None;
            self.select_commit_diff(Some(sha));
        } else {
            self.status = "commit is no longer in this pull request".into();
            if let Some(picker) = &mut self.commit_picker {
                picker.open_sha = None;
            }
        }
    }

    pub(super) fn finish_commits_load(
        &mut self,
        item_id: String,
        result: Result<Vec<PullRequestCommit>, String>,
    ) {
        self.commits.insert(
            item_id.clone(),
            match result {
                Ok(commits) => CommitsState::Loaded(commits),
                Err(error) => CommitsState::Error(error),
            },
        );
        if let Some(picker) = &mut self.commit_picker
            && picker.item_id == item_id
            && let Some(CommitsState::Loaded(commits)) = self.commits.get(&item_id)
        {
            picker.selected = self
                .selected_commits
                .get(&item_id)
                .and_then(|selection| commit_indices(selection, commits).map(|(first, _)| first))
                .map_or(0, |index| index + 1);
            if picker
                .selection
                .as_ref()
                .is_some_and(|selection| commit_indices(selection, commits).is_none())
            {
                picker.selection = None;
                picker.all_commits = true;
            } else if let Some(selection) = &picker.selection {
                picker.all_commits = commit_indices(selection, commits)
                    .is_some_and(|(first, last)| first == 0 && last + 1 == commits.len());
            }
        }
        if self.current_item().is_some_and(|item| item.id == item_id) {
            self.open_requested_commit();
            // A force push may have removed the previously selected commit.
            if let Some(selection) = self.selected_commits.get(&item_id)
                && let Some(CommitsState::Loaded(commits)) = self.commits.get(&item_id)
                && commit_indices(selection, commits).is_none()
            {
                if self.details_mode == DetailsMode::Diff {
                    self.select_commit_diff(None);
                } else {
                    self.selected_commits.remove(&item_id);
                }
                self.status = "selected commit was removed; showing entire PR".into();
            }
        }
    }

    pub(super) fn select_commit_diff(&mut self, sha: Option<String>) {
        self.select_commit_selection(sha.map(CommitSelection::Single));
    }

    pub(super) fn select_commit_selection(&mut self, selection: Option<CommitSelection>) {
        let Some(item_id) = self.current_item().map(|item| item.id.clone()) else {
            return;
        };
        if self.details_mode != DetailsMode::Diff {
            self.show_diff();
        }
        self.save_current_diff_mode_state();
        match selection {
            Some(selection) => {
                self.selected_commits.insert(item_id.clone(), selection);
            }
            None => {
                self.selected_commits.remove(&item_id);
            }
        }
        let key = self.diff_scope_key(&item_id);
        if matches!(self.diffs.get(&key), Some(DiffState::Error(_))) {
            self.diffs.remove(&key);
        }
        self.diff_mark.remove(&item_id);
        self.last_diff_click = None;
        self.revealed_diff_inline_comments.remove(&item_id);
        self.selected_comment_index = NO_SELECTED_COMMENT_INDEX;
        self.selected_diff_file.insert(item_id.clone(), 0);
        self.selected_diff_line.insert(item_id.clone(), 0);
        let saved = self
            .diff_mode_state
            .get(&self.diff_scope_key(&item_id))
            .cloned();
        if let Some(saved) = &saved {
            self.restore_diff_mode_state_for(&item_id, saved);
        }
        self.focus = saved
            .as_ref()
            .map_or(FocusTarget::List, |saved| saved.focus);
        let scroll = saved.as_ref().map_or(0, |saved| saved.details_scroll);
        self.restore_selected_diff_file_details_scroll(&item_id, scroll);
        self.status = if self.current_diff().is_some() {
            "files focused"
        } else {
            "loading diff"
        }
        .into();
    }

    pub(super) fn finish_diff_load(
        &mut self,
        item_id: String,
        selection: Option<CommitSelection>,
        result: Result<PullRequestDiff, String>,
    ) {
        let key = scope_key(&item_id, selection.as_ref());
        let active_scope = self.selected_commits.get(&item_id) == selection.as_ref();
        let visible = active_scope
            && self.details_mode == DetailsMode::Diff
            && self.current_item().is_some_and(|item| item.id == item_id);
        match result {
            Ok(diff) => {
                let count = diff.files.len();
                if active_scope {
                    let file = self.selected_diff_file.entry(item_id.clone()).or_insert(0);
                    *file = (*file).min(count.saturating_sub(1));
                    let lines = diff
                        .files
                        .get(*file)
                        .map_or(0, |file| diff_review_targets(file).len());
                    let line = self.selected_diff_line.entry(item_id.clone()).or_insert(0);
                    *line = (*line).min(lines.saturating_sub(1));
                }
                self.diffs.insert(key, DiffState::Loaded(diff));
                if visible {
                    self.restore_selected_diff_file_details_scroll(&item_id, self.details_scroll);
                    self.status = format!("diff loaded: {count} file(s)");
                }
            }
            Err(error) => {
                if visible {
                    if self.setup_dialog.is_none() {
                        self.setup_dialog = setup_dialog_from_error(&error);
                    }
                    self.status = "diff load failed; press r to retry".into();
                }
                self.diffs.insert(key, DiffState::Error(error));
            }
        }
    }

    pub(super) fn commit_diff_label(&self, item_id: &str) -> String {
        let Some(selection) = self.selected_commits.get(item_id) else {
            return "All commits".to_string();
        };
        let short = |sha: &str| sha.chars().take(7).collect::<String>();
        let label = match selection {
            CommitSelection::Single(sha) => short(sha),
            CommitSelection::Range { first, last } => format!("{}..{}", short(first), short(last)),
        };
        if let Some(CommitsState::Loaded(commits)) = self.commits.get(item_id)
            && let Some((first, last)) = commit_indices(selection, commits)
        {
            return if first == last {
                format!(
                    "{label} ({}/{}) {}",
                    first + 1,
                    commits.len(),
                    commits[first].message_headline
                )
            } else {
                format!("{label} ({}–{}/{})", first + 1, last + 1, commits.len())
            };
        }
        match self.commits.get(item_id) {
            Some(CommitsState::Error(_)) => format!("{label} (commits unavailable · V retry)"),
            _ => format!("{label} (loading commits…)"),
        }
    }

    /// Keep indices stable: mouse actions and replies use the conversation's indices.
    pub(super) fn diff_comments_for_item(
        &self,
        item_id: &str,
    ) -> Option<Cow<'_, [CommentPreview]>> {
        let comments = self.loaded_comments_for_item(item_id)?;
        let Some(selection) = self.selected_commits.get(item_id) else {
            return Some(Cow::Borrowed(comments));
        };
        let sha = selection.last();
        let mut projected = comments.to_vec();
        for comment in &mut projected {
            let Some(review) = comment.review.as_mut() else {
                continue;
            };
            if review.original_commit_id.as_deref() == Some(sha) {
                review.line = review.original_line;
                review.start_line = review.original_start_line;
            } else if review.commit_id.as_deref() != Some(sha) || review.line.is_none() {
                comment.review = None;
                continue;
            }
            // The selected snapshot's lines are current within this diff.
            review.is_outdated = false;
        }
        Some(Cow::Owned(projected))
    }

    pub(super) fn handle_commit_picker_key(&mut self, key: KeyEvent, tx: &UnboundedSender<AppMsg>) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.commit_picker = None,
            KeyCode::Down | KeyCode::Char('j') => self.move_commit_picker(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_commit_picker(-1),
            KeyCode::PageDown => self.move_commit_picker(10),
            KeyCode::PageUp => self.move_commit_picker(-10),
            KeyCode::Home | KeyCode::Char('g') => self.move_commit_picker(isize::MIN),
            KeyCode::End | KeyCode::Char('G') => self.move_commit_picker(isize::MAX),
            KeyCode::Char(' ') => self.toggle_commit_picker(),
            KeyCode::Enter => self.accept_commit_picker(),
            KeyCode::Char('r') => {
                if let Some(picker) = &mut self.commit_picker {
                    if matches!(
                        self.commits.get(&picker.item_id),
                        Some(CommitsState::Loading)
                    ) {
                        return;
                    }
                    self.commits.remove(&picker.item_id);
                    picker.selected = 0;
                    picker.open_sha = None;
                    self.ensure_commits_loading(Some(tx));
                }
            }
            _ => {}
        }
    }

    fn move_commit_picker(&mut self, delta: isize) {
        let Some(picker) = &mut self.commit_picker else {
            return;
        };
        let max = match self.commits.get(&picker.item_id) {
            Some(CommitsState::Loaded(commits)) => commits.len(),
            _ => 0,
        };
        picker.selected = picker.selected.saturating_add_signed(delta).min(max);
    }

    fn accept_commit_picker(&mut self) {
        let Some(picker) = self.commit_picker.as_ref() else {
            return;
        };
        if !self
            .current_item()
            .is_some_and(|item| item.id == picker.item_id)
        {
            self.commit_picker = None;
            return;
        }
        let selection = if picker.selected == 0 || picker.all_commits {
            None
        } else {
            let Some(selection) = &picker.selection else {
                self.status = "select at least one commit or All commits".into();
                return;
            };
            let Some(CommitsState::Loaded(commits)) = self.commits.get(&picker.item_id) else {
                return;
            };
            if commit_indices(selection, commits).is_none() {
                return;
            }
            Some(selection.clone())
        };
        self.commit_picker = None;
        self.select_commit_selection(selection);
    }

    fn toggle_commit_picker(&mut self) {
        let Some(picker) = self.commit_picker.as_mut() else {
            return;
        };
        if picker.selected == 0 {
            return;
        }
        let Some(CommitsState::Loaded(commits)) = self.commits.get(&picker.item_id) else {
            return;
        };
        let index = picker.selected - 1;
        if index >= commits.len() {
            return;
        }
        let range = picker
            .selection
            .as_ref()
            .and_then(|selection| commit_indices(selection, commits));
        let range = match range {
            // Removing the first commit preserves the rest of the range.
            Some((first, last)) if index == first => (first < last).then_some((first + 1, last)),
            // Removing a middle or last commit keeps only the earlier part.
            Some((first, last)) if (first..=last).contains(&index) => {
                (index > first).then(|| (first, index - 1))
            }
            // Selecting outside the range fills the entire gap.
            Some((first, last)) => Some((first.min(index), last.max(index))),
            None => Some((index, index)),
        };
        picker.selection = range.map(|(first, last)| {
            CommitSelection::new(commits[first].oid.clone(), commits[last].oid.clone())
        });
        picker.all_commits =
            range.is_some_and(|(first, last)| first == 0 && last + 1 == commits.len());
    }

    pub(super) fn handle_commit_picker_mouse(&mut self, mouse: MouseEvent, area: Rect) -> bool {
        let popup = commit_picker_area(area);
        if !rect_contains(popup, mouse.column, mouse.row) {
            return false;
        }
        match mouse.kind {
            MouseEventKind::ScrollDown => self.move_commit_picker(1),
            MouseEventKind::ScrollUp => self.move_commit_picker(-1),
            MouseEventKind::Down(MouseButton::Left) => {
                let [all, _, list, footer] = commit_picker_layout(block_inner(popup));
                if rect_contains(footer, mouse.column, mouse.row) && mouse.column < footer.x + 13 {
                    self.accept_commit_picker();
                    return true;
                }
                if rect_contains(all, mouse.column, mouse.row) {
                    if let Some(picker) = self.commit_picker.as_mut() {
                        picker.selected = 0;
                        self.accept_commit_picker();
                    }
                    return true;
                }
                if !rect_contains(list, mouse.column, mouse.row) {
                    return false;
                }
                let Some(picker) = self.commit_picker.as_ref() else {
                    return false;
                };
                let index = commit_picker_start(picker.selected, list.height as usize)
                    + (mouse.row - list.y) as usize
                    + 1;
                let max = match self.commits.get(&picker.item_id) {
                    Some(CommitsState::Loaded(commits)) => commits.len(),
                    _ => 0,
                };
                if index > max {
                    return false;
                }
                self.commit_picker.as_mut().unwrap().selected = index;
                self.toggle_commit_picker();
            }
            _ => return false,
        }
        true
    }
}

pub(super) fn commit_picker_area(area: Rect) -> Rect {
    centered_rect_with_size(100.min(area.width), 20.min(area.height), area)
}

fn commit_picker_start(selected: usize, height: usize) -> usize {
    selected.saturating_sub(height)
}

fn commit_picker_layout(inner: Rect) -> [Rect; 4] {
    Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(inner)
}

pub(super) fn draw_commit_picker(
    frame: &mut Frame<'_>,
    app: &AppState,
    picker: &CommitPicker,
    area: Rect,
) {
    let area = commit_picker_area(area);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Select commits ")
        .border_style(active_theme().focus_border())
        .style(active_theme().panel());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let [all, divider, list, footer] = commit_picker_layout(inner);
    frame.render_widget(
        Paragraph::new(format!(
            "{} {} All commits",
            if picker.selected == 0 { "›" } else { " " },
            if picker.all_commits { "✓" } else { " " }
        ))
        .style(if picker.selected == 0 {
            active_theme().active()
        } else {
            active_theme().panel().add_modifier(Modifier::BOLD)
        }),
        all,
    );
    frame.render_widget(
        Paragraph::new(format!(
            "─ Select commits {}",
            "─".repeat(divider.width.saturating_sub(17) as usize)
        ))
        .style(active_theme().muted()),
        divider,
    );
    let mut rows = Vec::new();
    match app.commits.get(&picker.item_id) {
        Some(CommitsState::Loaded(commits)) => {
            let range = picker
                .selection
                .as_ref()
                .and_then(|selection| commit_indices(selection, commits));
            rows.extend(commits.iter().enumerate().map(|(index, commit)| {
                let author = commit
                    .author
                    .as_ref()
                    .and_then(|author| author.name.as_deref())
                    .unwrap_or("unknown");
                format!(
                    "[{}] {} ({}/{}) {} · {} · {}",
                    if range.is_some_and(|(first, last)| (first..=last).contains(&index)) {
                        "x"
                    } else {
                        " "
                    },
                    &commit.oid[..commit.oid.len().min(7)],
                    index + 1,
                    commits.len(),
                    commit.committed_date.format("%Y-%m-%d %H:%M UTC"),
                    truncate_inline(author, 16),
                    commit.message_headline
                )
            }));
            if commits.is_empty() {
                rows.push("No commits".into());
            }
        }
        Some(CommitsState::Error(error)) => rows.push(format!("Failed to load: {error}")),
        _ => rows.push("Loading commits…".into()),
    }
    let start = commit_picker_start(picker.selected, list.height as usize);
    let lines = rows
        .into_iter()
        .enumerate()
        .skip(start)
        .take(list.height as usize)
        .map(|(index, row)| {
            let selected = index + 1 == picker.selected;
            Line::styled(
                format!("{} {row}", if selected { "›" } else { " " }),
                if selected {
                    active_theme().active()
                } else {
                    active_theme().panel()
                },
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), list);
    frame.render_widget(
        Paragraph::new(if picker.selected == 0 {
            "[Enter apply] · All commits · j/k move · r reload · Esc cancel"
        } else {
            "[Enter apply] · Space/click toggle · j/k move · r reload · Esc cancel"
        })
        .style(active_theme().muted()),
        footer,
    );
}
