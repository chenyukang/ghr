use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MouseWheelTargetKind {
    CommentDialog,
    List,
    Details,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct MouseWheelTarget {
    kind: MouseWheelTargetKind,
    area: Rect,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PendingMouseScroll {
    target: MouseWheelTarget,
    steps: i16,
}

pub(super) fn try_accumulate_mouse_scroll(
    app: &AppState,
    mouse: MouseEvent,
    area: Rect,
    pending: &mut Option<PendingMouseScroll>,
) -> bool {
    let Some(step) = mouse_scroll_step(mouse.kind) else {
        return false;
    };
    let Some(target) = mouse_wheel_target(app, mouse, area) else {
        return false;
    };

    match pending {
        Some(pending_scroll) if pending_scroll.target.kind == target.kind => {
            pending_scroll.steps = pending_scroll.steps.saturating_add(step);
        }
        Some(_) => {
            // The caller will flush and re-process this event as a normal mouse event.
            return false;
        }
        None => {
            *pending = Some(PendingMouseScroll {
                target,
                steps: step,
            });
        }
    }
    true
}

pub(super) fn flush_pending_mouse_scroll(
    app: &mut AppState,
    pending: &mut Option<PendingMouseScroll>,
) {
    let Some(scroll) = pending.take() else {
        return;
    };
    let steps = scroll.steps.clamp(
        -MAX_COALESCED_MOUSE_SCROLL_STEPS,
        MAX_COALESCED_MOUSE_SCROLL_STEPS,
    );
    if steps == 0 {
        return;
    }

    match scroll.target.kind {
        MouseWheelTargetKind::CommentDialog => app.scroll_comment_dialog(
            steps.saturating_mul(MOUSE_COMMENT_SCROLL_LINES as i16),
            Some(scroll.target.area),
        ),
        MouseWheelTargetKind::List => handle_list_scroll(
            app,
            scroll.target.area,
            mouse_list_scroll_delta(app, steps as isize),
        ),
        MouseWheelTargetKind::Details => handle_details_scroll(
            app,
            scroll.target.area,
            steps.saturating_mul(MOUSE_DETAILS_SCROLL_LINES as i16),
        ),
    }
}

pub(super) fn mouse_scroll_step(kind: MouseEventKind) -> Option<i16> {
    match kind {
        MouseEventKind::ScrollDown => Some(1),
        MouseEventKind::ScrollUp => Some(-1),
        _ => None,
    }
}

pub(super) fn mouse_wheel_target(
    app: &AppState,
    mouse: MouseEvent,
    area: Rect,
) -> Option<MouseWheelTarget> {
    if !app.mouse_capture_enabled
        || app.setup_dialog.is_some()
        || app.startup_dialog.is_some()
        || app.command_palette.is_some()
        || app.project_switcher.is_some()
        || app.top_menu_switcher.is_some()
        || app.theme_switcher.is_some()
        || app.recent_items_dialog.is_some()
        || app.saved_search_dialog.is_some()
        || app.save_search_dialog.is_some()
        || app.project_add_dialog.is_some()
        || app.project_remove_dialog.is_some()
        || app.cache_clear_dialog.is_some()
        || app.help_dialog
        || app.message_dialog.is_some()
        || app.item_edit_dialog.is_some()
        || app.pr_action_dialog.is_some()
        || app.label_dialog.is_some()
        || app.issue_dialog.is_some()
        || app.pr_create_dialog.is_some()
        || app.milestone_dialog.is_some()
        || app.assignee_dialog.is_some()
        || app.reviewer_dialog.is_some()
    {
        return None;
    }

    if let Some(dialog) = &app.comment_dialog {
        let dialog_area = comment_dialog_area(dialog, area);
        return rect_contains(dialog_area, mouse.column, mouse.row).then_some(MouseWheelTarget {
            kind: MouseWheelTargetKind::CommentDialog,
            area,
        });
    }

    let page = page_areas(area);
    let body_area = page[2];
    let body = body_areas_with_ratio(body_area, app.list_width_percent);
    let table = body[0];
    let details = body[1];
    if rect_contains(table, mouse.column, mouse.row) {
        Some(MouseWheelTarget {
            kind: MouseWheelTargetKind::List,
            area: table,
        })
    } else if rect_contains(details, mouse.column, mouse.row) {
        Some(MouseWheelTarget {
            kind: MouseWheelTargetKind::Details,
            area: details,
        })
    } else {
        None
    }
}

pub(super) fn sync_mouse_capture(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &AppState,
    enabled: &mut bool,
) -> Result<()> {
    if *enabled == app.mouse_capture_enabled {
        return Ok(());
    }

    if app.mouse_capture_enabled {
        execute!(terminal.backend_mut(), EnableMouseCapture)?;
    } else {
        execute!(terminal.backend_mut(), DisableMouseCapture)?;
    }
    *enabled = app.mouse_capture_enabled;
    Ok(())
}

#[cfg(test)]
pub(super) fn handle_key(
    app: &mut AppState,
    key: KeyEvent,
    config: &Config,
    store: &SnapshotStore,
    tx: &UnboundedSender<AppMsg>,
) -> bool {
    handle_key_in_area(app, key, config, store, tx, None)
}

#[cfg(test)]
pub(super) fn handle_key_in_area(
    app: &mut AppState,
    key: KeyEvent,
    config: &Config,
    store: &SnapshotStore,
    tx: &UnboundedSender<AppMsg>,
    area: Option<Rect>,
) -> bool {
    let mut config = config.clone();
    let paths = test_key_paths();
    handle_key_in_area_mut(app, key, &mut config, &paths, store, tx, area)
}

#[cfg(test)]
pub(super) fn test_key_paths() -> Paths {
    let root = std::path::PathBuf::from("/tmp/ghr-test-key");
    Paths {
        config_path: root.join("config.toml"),
        db_path: root.join("ghr.db"),
        log_path: root.join("ghr.log"),
        state_path: root.join("state.toml"),
        root,
    }
}

pub(super) fn handle_key_in_area_mut(
    app: &mut AppState,
    key: KeyEvent,
    config: &mut Config,
    paths: &Paths,
    store: &SnapshotStore,
    tx: &UnboundedSender<AppMsg>,
    area: Option<Rect>,
) -> bool {
    app.command_palette_key = normalized_command_palette_key(&config.defaults.command_palette_key);

    if is_ctrl_c_key(key) {
        return true;
    }

    if app.setup_dialog.is_some() {
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Esc | KeyCode::Enter => app.dismiss_setup_dialog(),
            _ => {}
        }
        return false;
    }

    if let Some(dialog) = app.startup_dialog {
        match (dialog, key.code) {
            (StartupDialog::Initializing, _) => {}
            (StartupDialog::Ready, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) => {
                app.dismiss_startup_dialog()
            }
            (StartupDialog::Ready, KeyCode::Char('?')) => {
                app.dismiss_startup_dialog();
                app.show_help_dialog();
            }
            _ => {}
        }
        return false;
    }

    if app.message_dialog.is_some() {
        let retryable = app
            .message_dialog
            .as_ref()
            .is_some_and(|dialog| dialog.kind == MessageDialogKind::RetryableError);
        match key.code {
            KeyCode::Esc if retryable => app.dismiss_retryable_message_dialog(false),
            KeyCode::Enter | KeyCode::Char('q') if retryable => {
                app.dismiss_retryable_message_dialog(true)
            }
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => app.dismiss_message_dialog(),
            _ => {}
        }
        return false;
    }

    if app.command_palette.is_some() {
        return app.handle_command_palette_key(key, config, paths, store, tx, area);
    }

    if app.project_switcher.is_some() {
        app.handle_project_switcher_key(key);
        return false;
    }

    if app.top_menu_switcher.is_some() {
        app.handle_top_menu_switcher_key(key);
        return false;
    }

    if app.theme_switcher.is_some() {
        app.handle_theme_switcher_key(key, config, paths);
        return false;
    }

    if app.recent_items_dialog.is_some() {
        app.handle_recent_items_key(key);
        return false;
    }

    if app.saved_search_dialog.is_some() {
        app.handle_saved_search_key(key, config, tx);
        return false;
    }

    if app.save_search_dialog.is_some() {
        app.handle_save_search_key(key, config, paths);
        return false;
    }

    if app.project_add_dialog.is_some() {
        app.handle_project_add_key(key, config, paths, store, tx);
        return false;
    }

    if app.project_remove_dialog.is_some() {
        app.handle_project_remove_key(key, config, paths);
        return false;
    }

    if app.cache_clear_dialog.is_some() {
        app.handle_cache_clear_key(key, store);
        return false;
    }

    if should_open_command_palette(app, key) {
        app.show_command_palette();
        return false;
    }

    if app.comment_dialog.is_some() {
        if editor_save_draft_key(key) {
            app.save_active_comment_draft(store, DraftSaveTrigger::Manual, Instant::now());
        } else {
            app.handle_comment_dialog_key_with_store(key, Some(store), tx, area);
        }
        return false;
    }

    if app.label_dialog.is_some() {
        app.handle_label_dialog_key(key, tx);
        return false;
    }

    if app.issue_dialog.is_some() {
        if editor_save_draft_key(key) {
            app.save_active_issue_draft(store, DraftSaveTrigger::Manual, Instant::now());
        } else {
            app.handle_issue_dialog_key_with_store(key, Some(store), tx, area);
        }
        return false;
    }

    if app.pr_create_dialog.is_some() {
        if editor_save_draft_key(key) {
            app.save_active_pr_create_draft(store, DraftSaveTrigger::Manual, Instant::now());
        } else {
            app.handle_pr_create_dialog_key_with_store(key, Some(store), tx, area);
        }
        return false;
    }

    if app.reaction_dialog.is_some() {
        app.handle_reaction_dialog_key(key, tx);
        return false;
    }

    if app.review_submit_dialog.is_some() {
        app.handle_review_submit_dialog_key(key, tx, area);
        return false;
    }

    if app.item_edit_dialog.is_some() {
        app.handle_item_edit_dialog_key(key, tx, area);
        return false;
    }

    if app.pr_action_dialog.is_some() {
        app.handle_pr_action_dialog_key(key, config, store, tx);
        return false;
    }

    if app.milestone_dialog.is_some() {
        app.handle_milestone_dialog_key(key, config, store, tx);
        return false;
    }

    if app.assignee_dialog.is_some() {
        app.handle_assignee_dialog_key(key, tx);
        return false;
    }

    if app.reviewer_dialog.is_some() {
        app.handle_reviewer_dialog_key(key, config, store, tx);
        return false;
    }

    if app.comment_search_active {
        app.handle_comment_search_key(key, area);
        return false;
    }

    if app.filter_input_active {
        app.handle_filter_input_key(key, config, tx);
        return false;
    }

    if app.global_search_active {
        app.handle_global_search_key(key, config, tx);
        return false;
    }

    if app.help_dialog {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?') | KeyCode::Char('q') => {
                app.dismiss_help_dialog()
            }
            _ => {}
        }
        return false;
    }

    if app.search_active {
        match key.code {
            KeyCode::Esc => app.clear_search(),
            KeyCode::Enter => app.search_active = false,
            KeyCode::Backspace => app.pop_search_char(),
            KeyCode::Char(value) => app.push_search_char(value),
            _ => {}
        }
        return false;
    }

    if handle_global_focus_key(app, key) {
        return false;
    }
    if handle_diff_focus_toggle_key(app, key) {
        return false;
    }
    if handle_list_details_focus_toggle_key(app, key) {
        return false;
    }
    if handle_mouse_or_mark_key(app, key) {
        return false;
    }
    if is_diff_key(key) {
        app.show_diff();
        return false;
    }
    if is_ctrl_d_key(key) {
        app.discard_pending_review(tx);
        return false;
    }

    match key.code {
        KeyCode::Esc if app.details_mode == DetailsMode::Diff => {
            app.leave_diff();
            return false;
        }
        KeyCode::Char('q') if app.details_mode == DetailsMode::Diff => {
            app.leave_diff();
            return false;
        }
        KeyCode::Char('i') if app.details_mode == DetailsMode::Diff => {
            app.toggle_diff_inline_comments();
            return false;
        }
        KeyCode::Char('q') => return true,
        KeyCode::Char('?') => app.show_help_dialog(),
        KeyCode::Char('r') => trigger_refresh(app, config, store, tx),
        KeyCode::Char('[') if key.modifiers.contains(KeyModifiers::ALT) => {
            start_section_page_load(app, config, store, tx, -1)
        }
        KeyCode::Char(']') if key.modifiers.contains(KeyModifiers::ALT) => {
            start_section_page_load(app, config, store, tx, 1)
        }
        KeyCode::Char('S') => app.start_global_search_input_with_store(Some(store), Some(tx)),
        KeyCode::Char('f') => app.start_filter_input(),
        KeyCode::Tab => app.move_focused_tab_group(1),
        KeyCode::BackTab => app.move_focused_tab_group(-1),
        KeyCode::Char('o') => app.open_selected(),
        _ if is_ignore_key(key) => app.ignore_current_item(),
        KeyCode::Char('T') => app.start_item_edit_dialog_with_store(Some(store), Some(tx)),
        _ => {}
    }

    match app.focus {
        FocusTarget::Ghr => match key.code {
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(']') => app.move_view(1),
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('[') => app.move_view(-1),
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('n') | KeyCode::Enter => {
                app.focus_sections()
            }
            KeyCode::Esc => app.focus_list(),
            _ => {}
        },
        FocusTarget::Sections => match key.code {
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(']') => app.move_section(1),
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('[') => app.move_section(-1),
            KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('p') => app.focus_ghr(),
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('n') | KeyCode::Enter => {
                app.focus_list()
            }
            KeyCode::Esc => app.focus_list(),
            _ => {}
        },
        FocusTarget::List if app.details_mode == DetailsMode::Diff => {
            handle_diff_file_list_key(app, key, config, area, store, tx)
        }
        FocusTarget::List => match key.code {
            KeyCode::Esc if !app.search_query.is_empty() => app.clear_search(),
            KeyCode::Esc if app.is_global_search_results_view() => {
                app.leave_global_search_results()
            }
            KeyCode::Esc
                if app
                    .current_section()
                    .is_some_and(|section| app.has_active_section_filter(&section.key)) =>
            {
                start_filtered_section_load(app, config, tx, None)
            }
            KeyCode::Esc => {}
            KeyCode::Char('/') => {
                if app.current_section().is_some_and(|section| {
                    matches!(
                        section.kind,
                        SectionKind::PullRequests | SectionKind::Issues
                    )
                }) {
                    app.start_global_search_input_with_store(Some(store), Some(tx));
                } else {
                    app.start_search();
                }
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('n') => {
                app.move_selection(1);
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('p') => {
                if app.current_selected_position() == 0 {
                    app.focus_sections();
                } else {
                    app.move_selection(-1);
                }
            }
            KeyCode::PageDown | KeyCode::Char('d') => {
                app.move_selection(list_page_delta(app, area, 1));
            }
            KeyCode::PageUp | KeyCode::Char('u') => {
                app.move_selection(list_page_delta(app, area, -1));
            }
            KeyCode::Char('[') => start_section_page_load(app, config, store, tx, -1),
            KeyCode::Char(']') => start_section_page_load(app, config, store, tx, 1),
            KeyCode::Char('g') => {
                app.set_selection(0);
            }
            KeyCode::Char('G') => {
                app.select_last();
            }
            KeyCode::Char('M') => app.start_pr_action_dialog(PrAction::Merge),
            KeyCode::Char('C') => app.start_close_or_reopen_dialog(),
            KeyCode::Char('A') => app.start_review_submit_dialog(PullRequestReviewEvent::Approve),
            KeyCode::Char('s') => app.start_review_submit_dialog(PullRequestReviewEvent::Comment),
            KeyCode::Char('D') => app.start_pr_draft_ready_dialog(),
            KeyCode::Char('E') => app.start_pr_action_dialog(PrAction::EnableAutoMerge),
            KeyCode::Char('O') => app.start_pr_action_dialog(PrAction::DisableAutoMerge),
            KeyCode::Char('U') => app.start_pr_action_dialog(PrAction::UpdateBranch),
            KeyCode::Char('X') => app.start_pr_checkout_dialog(config),
            KeyCode::Char('F') => app.start_pr_action_dialog(PrAction::RerunFailedChecks),
            KeyCode::Char('e') => app.start_item_edit_dialog_with_store(Some(store), Some(tx)),
            KeyCode::Char('t') => app.start_milestone_dialog(tx),
            KeyCode::Char('P') => {
                app.start_reviewer_dialog_with_store(ReviewerAction::Request, Some(store), Some(tx))
            }
            KeyCode::Char('Y') => {
                app.start_reviewer_dialog_with_store(ReviewerAction::Remove, Some(store), Some(tx))
            }
            KeyCode::Char('Z') => app.start_pr_draft_ready_dialog(),
            KeyCode::Char('a') => app.start_new_comment_dialog(),
            KeyCode::Char('L') => app.start_add_label_dialog_with_store(Some(store), Some(tx)),
            KeyCode::Char('N') => app.start_new_issue_or_pull_request_dialog(config),
            _ if is_assignee_assign_key(key) => {
                app.start_assignee_dialog_with_store(AssigneeAction::Assign, Some(store), Some(tx))
            }
            KeyCode::Char('-') => app.start_assignee_dialog_with_store(
                AssigneeAction::Unassign,
                Some(store),
                Some(tx),
            ),
            _ if is_reaction_key(key) => app.start_item_reaction_dialog(),
            KeyCode::Enter => {
                app.focus_details();
                app.mark_current_notification_read(store, tx);
            }
            _ => {}
        },
        FocusTarget::Details => match key.code {
            KeyCode::Esc
                if app.details_mode == DetailsMode::Conversation
                    && !app.comment_search_query.is_empty() =>
            {
                app.clear_comment_search()
            }
            KeyCode::Esc => app.focus_list(),
            KeyCode::Char('/') if app.details_mode == DetailsMode::Conversation => {
                app.start_comment_search()
            }
            KeyCode::Char('M') => app.start_pr_action_dialog(PrAction::Merge),
            KeyCode::Char('C') => app.start_close_or_reopen_dialog(),
            KeyCode::Char('A') => app.start_review_submit_dialog(PullRequestReviewEvent::Approve),
            KeyCode::Char('s') => app.start_review_submit_dialog(PullRequestReviewEvent::Comment),
            KeyCode::Char('D') => app.start_pr_draft_ready_dialog(),
            KeyCode::Char('E') => app.start_pr_action_dialog(PrAction::EnableAutoMerge),
            KeyCode::Char('O') => app.start_pr_action_dialog(PrAction::DisableAutoMerge),
            KeyCode::Char('U') => app.start_pr_action_dialog(PrAction::UpdateBranch),
            KeyCode::Char('X') => app.start_pr_checkout_dialog(config),
            KeyCode::Char('L') if app.details_mode == DetailsMode::Conversation => {
                app.start_add_label_dialog_with_store(Some(store), Some(tx))
            }
            KeyCode::Char('N') => app.start_new_issue_or_pull_request_dialog(config),
            KeyCode::Char('F') => app.start_pr_action_dialog(PrAction::RerunFailedChecks),
            KeyCode::Char('t') => app.start_milestone_dialog(tx),
            _ if is_assignee_assign_key(key) => {
                app.start_assignee_dialog_with_store(AssigneeAction::Assign, Some(store), Some(tx))
            }
            KeyCode::Char('-') => app.start_assignee_dialog_with_store(
                AssigneeAction::Unassign,
                Some(store),
                Some(tx),
            ),
            KeyCode::Char('P') => {
                app.start_reviewer_dialog_with_store(ReviewerAction::Request, Some(store), Some(tx))
            }
            KeyCode::Char('Y') => {
                app.start_reviewer_dialog_with_store(ReviewerAction::Remove, Some(store), Some(tx))
            }
            KeyCode::Char('Z') => app.start_pr_draft_ready_dialog(),
            KeyCode::Char('c') if app.details_mode == DetailsMode::Diff => {
                app.start_review_comment_dialog()
            }
            KeyCode::Char('a') if app.details_mode == DetailsMode::Diff => {
                app.start_new_comment_dialog()
            }
            KeyCode::Char('c') | KeyCode::Char('a')
                if app.details_mode == DetailsMode::Conversation =>
            {
                app.start_new_comment_dialog()
            }
            KeyCode::Char('R') if app.details_mode == DetailsMode::Conversation => {
                app.start_reply_to_selected_comment()
            }
            _ if app.details_mode == DetailsMode::Conversation && is_reaction_key(key) => {
                app.start_keyboard_reaction_dialog(area)
            }
            KeyCode::Char('e') if app.details_mode == DetailsMode::Conversation => {
                app.start_item_edit_dialog_with_store(Some(store), Some(tx))
            }
            KeyCode::Char('n') => app.move_comment_in_view(1, area),
            KeyCode::Char('p') => app.move_comment_in_view(-1, area),
            KeyCode::Char('h') if app.details_mode == DetailsMode::Diff => {
                app.page_diff_lines(1, area)
            }
            KeyCode::Char('l') if app.details_mode == DetailsMode::Diff => {
                app.page_diff_lines(-1, area)
            }
            KeyCode::Char(']') if app.details_mode == DetailsMode::Diff => app.move_diff_file(1),
            KeyCode::Char('[') if app.details_mode == DetailsMode::Diff => app.move_diff_file(-1),
            KeyCode::Down | KeyCode::Char('j') if app.details_mode == DetailsMode::Diff => {
                app.move_diff_line(1, area)
            }
            KeyCode::Up | KeyCode::Char('k') if app.details_mode == DetailsMode::Diff => {
                app.move_diff_line(-1, area)
            }
            KeyCode::PageDown | KeyCode::Char('d') if app.details_mode == DetailsMode::Diff => {
                app.page_diff_lines(1, area)
            }
            KeyCode::PageUp | KeyCode::Char('u') if app.details_mode == DetailsMode::Diff => {
                app.page_diff_lines(-1, area)
            }
            KeyCode::Char('g') if app.details_mode == DetailsMode::Diff => {
                app.scroll_diff_details_to_top(area)
            }
            KeyCode::Char('G') if app.details_mode == DetailsMode::Diff => {
                app.scroll_diff_details_to_bottom(area)
            }
            KeyCode::Char('e') if app.details_mode == DetailsMode::Diff => app.end_diff_mark(),
            KeyCode::Enter if app.details_mode == DetailsMode::Conversation => {
                app.toggle_selected_comment_expanded()
            }
            KeyCode::Down | KeyCode::Char('j') => app.scroll_details(1),
            KeyCode::Up | KeyCode::Char('k') => app.scroll_details(-1),
            KeyCode::PageDown | KeyCode::Char('d') => app.scroll_details(8),
            KeyCode::PageUp | KeyCode::Char('u') => app.scroll_details(-8),
            KeyCode::Char('g') => app.scroll_conversation_details_to_top(),
            KeyCode::Char('G') => app.scroll_conversation_details_to_bottom(area),
            _ => {}
        },
    }

    false
}

pub(super) fn handle_global_focus_key(app: &mut AppState, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('1') => app.focus_ghr(),
        KeyCode::Char('2') => app.focus_sections(),
        KeyCode::Char('3') => app.focus_primary_list(),
        KeyCode::Char('4') => app.focus_details(),
        _ => return false,
    }
    true
}

pub(super) fn handle_diff_focus_toggle_key(app: &mut AppState, key: KeyEvent) -> bool {
    if app.details_mode != DetailsMode::Diff {
        return false;
    }
    if !matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
        return false;
    }

    match app.focus {
        FocusTarget::List => app.focus_details(),
        FocusTarget::Details => app.focus_list(),
        _ => return false,
    }
    true
}

pub(super) fn handle_list_details_focus_toggle_key(app: &mut AppState, key: KeyEvent) -> bool {
    if app.details_mode != DetailsMode::Conversation {
        return false;
    }
    if !matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
        return false;
    }

    match app.focus {
        FocusTarget::List => {
            app.focus_details();
            true
        }
        FocusTarget::Details => {
            app.focus_list();
            true
        }
        _ => false,
    }
}

pub(super) fn handle_mouse_or_mark_key(app: &mut AppState, key: KeyEvent) -> bool {
    if !matches!(key.code, KeyCode::Char('m')) {
        return false;
    }

    if !app.mouse_capture_enabled {
        app.toggle_mouse_capture();
    } else if app.details_mode == DetailsMode::Diff {
        if app.focus == FocusTarget::Details {
            app.begin_diff_mark();
        } else {
            app.status = "mark diff lines from Details; press 4 or enter first".to_string();
        }
    } else {
        app.toggle_mouse_capture();
    }
    true
}

pub(super) fn handle_diff_file_list_key(
    app: &mut AppState,
    key: KeyEvent,
    config: &Config,
    area: Option<Rect>,
    store: &SnapshotStore,
    tx: &UnboundedSender<AppMsg>,
) {
    match key.code {
        KeyCode::Esc => app.leave_diff(),
        KeyCode::Char('c') => app.start_review_comment_dialog(),
        KeyCode::Char('a') => app.start_new_comment_dialog(),
        KeyCode::Char('P') => {
            app.start_reviewer_dialog_with_store(ReviewerAction::Request, Some(store), Some(tx))
        }
        KeyCode::Char('Y') => {
            app.start_reviewer_dialog_with_store(ReviewerAction::Remove, Some(store), Some(tx))
        }
        KeyCode::Down | KeyCode::Char('j') => app.move_diff_file(1),
        KeyCode::Up | KeyCode::Char('k') => app.move_diff_file(-1),
        KeyCode::PageDown | KeyCode::Char('d') => {
            app.move_diff_file(diff_file_page_delta(app, area, 1));
        }
        KeyCode::PageUp | KeyCode::Char('u') => {
            app.move_diff_file(diff_file_page_delta(app, area, -1));
        }
        KeyCode::Char('h') => app.page_diff_lines(1, area),
        KeyCode::Char('l') => app.page_diff_lines(-1, area),
        KeyCode::Char('g') => {
            if let Some(order) = app.current_diff_file_order() {
                if let Some(file_index) = order.first() {
                    app.select_diff_file(*file_index);
                } else {
                    app.status = "no diff files".to_string();
                }
            } else {
                app.status = "diff still loading".to_string();
            }
        }
        KeyCode::Char('G') => {
            if let Some(order) = app.current_diff_file_order() {
                if let Some(file_index) = order.last() {
                    app.select_diff_file(*file_index);
                } else {
                    app.status = "no diff files".to_string();
                }
            } else {
                app.status = "diff still loading".to_string();
            }
        }
        KeyCode::Char('[') => app.move_diff_file(-1),
        KeyCode::Char(']') => app.move_diff_file(1),
        KeyCode::Char('M') => app.start_pr_action_dialog(PrAction::Merge),
        KeyCode::Char('C') => app.start_close_or_reopen_dialog(),
        KeyCode::Char('A') => app.start_review_submit_dialog(PullRequestReviewEvent::Approve),
        KeyCode::Char('s') => app.start_review_submit_dialog(PullRequestReviewEvent::Comment),
        KeyCode::Char('D') => app.start_pr_draft_ready_dialog(),
        KeyCode::Char('E') => app.start_pr_action_dialog(PrAction::EnableAutoMerge),
        KeyCode::Char('O') => app.start_pr_action_dialog(PrAction::DisableAutoMerge),
        KeyCode::Char('U') => app.start_pr_action_dialog(PrAction::UpdateBranch),
        KeyCode::Char('X') => app.start_pr_checkout_dialog(config),
        KeyCode::Char('F') => app.start_pr_action_dialog(PrAction::RerunFailedChecks),
        KeyCode::Char('t') => app.start_milestone_dialog(tx),
        _ if is_assignee_assign_key(key) => {
            app.start_assignee_dialog_with_store(AssigneeAction::Assign, Some(store), Some(tx))
        }
        KeyCode::Char('-') => {
            app.start_assignee_dialog_with_store(AssigneeAction::Unassign, Some(store), Some(tx))
        }
        KeyCode::Char('Z') => app.start_pr_draft_ready_dialog(),
        KeyCode::Enter => app.focus_details(),
        _ => {}
    }
}

pub(super) fn trigger_refresh(
    app: &mut AppState,
    config: &Config,
    store: &SnapshotStore,
    tx: &UnboundedSender<AppMsg>,
) {
    if app.refreshing {
        app.status = "refresh already running".to_string();
    } else {
        start_refresh(
            config.clone(),
            store.clone(),
            tx.clone(),
            RefreshPriority::User,
            RefreshScope::Full,
        );
    }
}

pub(super) fn refresh_started_status(scope: &RefreshScope) -> String {
    match scope {
        RefreshScope::Full => "refreshing from GitHub".to_string(),
        RefreshScope::View(view) => format!("refreshing {}", refresh_scope_view_label(view)),
    }
}

pub(super) fn refresh_finished_status(scope: &RefreshScope) -> String {
    match scope {
        RefreshScope::Full => "latest".to_string(),
        RefreshScope::View(_) => "current view latest".to_string(),
    }
}

pub(super) fn refresh_scope_view_label(view: &str) -> String {
    match view {
        "notifications" => "inbox".to_string(),
        "pull_requests" => "pull requests".to_string(),
        "issues" => "issues".to_string(),
        view if view.starts_with("repo:") => format!("repo {}", view.trim_start_matches("repo:")),
        _ => "current view".to_string(),
    }
}

pub(super) fn save_ui_state(app: &mut AppState, paths: &Paths) {
    app.sync_recent_details_visit(Instant::now());
    if let Err(error) = app.ui_state().save(&paths.state_path) {
        let message = error.to_string();
        warn!(error = %message, "failed to save ui state");
        app.status = format!("layout save failed: {message}");
    }
}

#[cfg(test)]
pub(super) fn handle_mouse(app: &mut AppState, mouse: MouseEvent, area: Rect) -> bool {
    handle_mouse_with_sync(app, mouse, area, None, None)
}

pub(super) fn handle_mouse_with_sync(
    app: &mut AppState,
    mouse: MouseEvent,
    area: Rect,
    store: Option<&SnapshotStore>,
    tx: Option<&UnboundedSender<AppMsg>>,
) -> bool {
    if !app.mouse_capture_enabled {
        return false;
    }
    if app.setup_dialog.is_some() {
        return false;
    }
    if let Some(dialog) = app.startup_dialog {
        return handle_startup_dialog_mouse(app, dialog, mouse, area);
    }
    if app.help_dialog {
        return false;
    }
    if app.message_dialog.is_some() {
        return false;
    }
    if app.command_palette.is_some()
        || app.project_switcher.is_some()
        || app.top_menu_switcher.is_some()
        || app.theme_switcher.is_some()
        || app.recent_items_dialog.is_some()
        || app.saved_search_dialog.is_some()
        || app.save_search_dialog.is_some()
        || app.project_add_dialog.is_some()
        || app.project_remove_dialog.is_some()
        || app.cache_clear_dialog.is_some()
    {
        return false;
    }
    if let Some(dialog) = &app.pr_action_dialog {
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && let Some(url) = pr_action_dialog_link_at(dialog, area, mouse.column, mouse.row)
        {
            app.open_url(&url);
        }
        return false;
    }
    if app.label_dialog.is_some() {
        return false;
    }
    if app.issue_dialog.is_some() {
        handle_issue_dialog_mouse(app, mouse, area, store);
        return false;
    }
    if app.pr_create_dialog.is_some() {
        handle_pr_create_dialog_mouse(app, mouse, area, store);
        return false;
    }
    if app.reaction_dialog.is_some() {
        return false;
    }
    if app.item_edit_dialog.is_some() {
        return false;
    }
    if app.review_submit_dialog.is_some() {
        handle_review_submit_dialog_mouse(app, mouse, area);
        return false;
    }
    if app.milestone_dialog.is_some() {
        return false;
    }
    if app.global_search_active || app.filter_input_active || app.comment_search_active {
        return false;
    }
    if let Some(dialog) = &app.comment_dialog {
        handle_comment_dialog_mouse(app, dialog.clone(), mouse, area, store);
        return false;
    }

    let page = page_areas(area);
    let body_area = page[2];
    let body = body_areas_with_ratio(body_area, app.list_width_percent);
    let details_area = body[1];
    let layout = MouseLayout {
        view_tabs: page[0],
        section_tabs: page[1],
        table: body[0],
        details: details_area,
    };

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            debug!(
                column = mouse.column,
                row = mouse.row,
                view = %app.active_view,
                focus = ?app.focus,
                mode = ?app.details_mode,
                "mouse left click"
            );
            if splitter_contains(
                body_area,
                layout.table,
                layout.details,
                mouse.column,
                mouse.row,
            ) {
                debug!(
                    column = mouse.column,
                    row = mouse.row,
                    target = "splitter",
                    "mouse click handled"
                );
                app.start_split_drag();
                return false;
            }
            handle_left_click(app, mouse, layout, store, tx);
        }
        MouseEventKind::Drag(MouseButton::Left) if app.dragging_split => {
            app.update_split_drag(body_area, mouse.column);
        }
        MouseEventKind::Up(MouseButton::Left) if app.dragging_split => {
            app.update_split_drag(body_area, mouse.column);
            return app.finish_split_drag();
        }
        MouseEventKind::Drag(MouseButton::Left) if app.details_text_drag.is_some() => {
            handle_details_text_drag(app, mouse, layout.details);
        }
        MouseEventKind::Up(MouseButton::Left) if app.details_text_drag.is_some() => {
            finish_details_text_drag(app, mouse, layout.details);
        }
        MouseEventKind::ScrollDown if rect_contains(layout.table, mouse.column, mouse.row) => {
            handle_list_scroll(app, layout.table, mouse_list_scroll_delta(app, 1));
        }
        MouseEventKind::ScrollUp if rect_contains(layout.table, mouse.column, mouse.row) => {
            handle_list_scroll(app, layout.table, mouse_list_scroll_delta(app, -1));
        }
        MouseEventKind::ScrollDown if rect_contains(layout.details, mouse.column, mouse.row) => {
            handle_details_scroll(app, layout.details, MOUSE_DETAILS_SCROLL_LINES as i16);
        }
        MouseEventKind::ScrollUp if rect_contains(layout.details, mouse.column, mouse.row) => {
            handle_details_scroll(app, layout.details, -(MOUSE_DETAILS_SCROLL_LINES as i16));
        }
        MouseEventKind::Moved if rect_contains(layout.details, mouse.column, mouse.row) => {
            handle_details_hover(app, mouse, layout.details);
        }
        _ => {}
    }

    false
}

pub(super) fn handle_startup_dialog_mouse(
    app: &mut AppState,
    dialog: StartupDialog,
    mouse: MouseEvent,
    area: Rect,
) -> bool {
    if !matches!(dialog, StartupDialog::Ready)
        || !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
    {
        return false;
    }

    let dialog_area = startup_dialog_area(dialog, area);
    if rect_contains(startup_dialog_ok_area(dialog_area), mouse.column, mouse.row) {
        app.dismiss_startup_dialog();
        return true;
    }

    false
}

pub(super) fn handle_comment_dialog_mouse(
    app: &mut AppState,
    dialog: CommentDialog,
    mouse: MouseEvent,
    area: Rect,
    store: Option<&SnapshotStore>,
) {
    let dialog_area = comment_dialog_area(&dialog, area);
    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        && modal_footer_area(area, dialog_area)
            .is_some_and(|footer| rect_contains(footer, mouse.column, mouse.row))
    {
        if let Some(store) = store {
            app.save_active_comment_draft(store, DraftSaveTrigger::Manual, Instant::now());
        } else {
            app.status = "draft store unavailable".to_string();
        }
        return;
    }
    if !rect_contains(dialog_area, mouse.column, mouse.row) {
        return;
    }

    match mouse.kind {
        MouseEventKind::ScrollDown => {
            app.scroll_comment_dialog(MOUSE_COMMENT_SCROLL_LINES as i16, Some(area));
        }
        MouseEventKind::ScrollUp => {
            app.scroll_comment_dialog(-(MOUSE_COMMENT_SCROLL_LINES as i16), Some(area));
        }
        MouseEventKind::Down(MouseButton::Left) => {
            let inner = block_inner(dialog_area);
            if !rect_contains(inner, mouse.column, mouse.row) {
                return;
            }
            let line = usize::from(dialog.scroll).saturating_add(usize::from(mouse.row - inner.y));
            let column = mouse.column.saturating_sub(inner.x);
            if let Some(active) = &mut app.comment_dialog {
                let cursor = comment_dialog_cursor_for_position(
                    active.body.text(),
                    inner.width.max(1),
                    line,
                    column,
                );
                active.body.set_cursor_byte(cursor);
            }
            app.scroll_comment_dialog_to_cursor_in_area(Some(area));
            app.status = "comment cursor moved".to_string();
        }
        _ => {}
    }
}

pub(super) fn handle_review_submit_dialog_mouse(app: &mut AppState, mouse: MouseEvent, area: Rect) {
    let Some(dialog) = app.review_submit_dialog.clone() else {
        return;
    };
    let dialog_area = review_submit_dialog_area(&dialog, area);
    if !rect_contains(dialog_area, mouse.column, mouse.row) {
        return;
    }

    match mouse.kind {
        MouseEventKind::ScrollDown => {
            app.scroll_review_submit_dialog(MOUSE_COMMENT_SCROLL_LINES as i16, Some(area));
        }
        MouseEventKind::ScrollUp => {
            app.scroll_review_submit_dialog(-(MOUSE_COMMENT_SCROLL_LINES as i16), Some(area));
        }
        MouseEventKind::Down(MouseButton::Left) => {
            let inner = block_inner(dialog_area);
            let header_height = 3_u16.min(inner.height);
            let editor_area = Rect::new(
                inner.x,
                inner.y.saturating_add(header_height),
                inner.width,
                inner.height.saturating_sub(header_height),
            );
            if !rect_contains(editor_area, mouse.column, mouse.row) {
                return;
            }
            let line =
                usize::from(dialog.scroll).saturating_add(usize::from(mouse.row - editor_area.y));
            let column = mouse.column.saturating_sub(editor_area.x);
            if let Some(active) = &mut app.review_submit_dialog {
                let cursor = comment_dialog_cursor_for_position(
                    active.body.text(),
                    editor_area.width.max(1),
                    line,
                    column,
                );
                active.body.set_cursor_byte(cursor);
            }
            app.scroll_review_submit_dialog_to_cursor_in_area(Some(area));
            app.status = "review summary cursor moved".to_string();
        }
        _ => {}
    }
}

pub(super) fn handle_issue_dialog_mouse(
    app: &mut AppState,
    mouse: MouseEvent,
    area: Rect,
    store: Option<&SnapshotStore>,
) {
    let Some(dialog) = app.issue_dialog.clone() else {
        return;
    };
    let dialog_area = issue_dialog_area(area);
    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        && modal_footer_area(area, dialog_area)
            .is_some_and(|footer| rect_contains(footer, mouse.column, mouse.row))
    {
        if let Some(store) = store {
            app.save_active_issue_draft(store, DraftSaveTrigger::Manual, Instant::now());
        } else {
            app.status = "draft store unavailable".to_string();
        }
        return;
    }
    if !rect_contains(dialog_area, mouse.column, mouse.row) {
        return;
    }

    match mouse.kind {
        MouseEventKind::ScrollDown => {
            app.scroll_issue_dialog_body(MOUSE_COMMENT_SCROLL_LINES as i16, Some(area));
        }
        MouseEventKind::ScrollUp => {
            app.scroll_issue_dialog_body(-(MOUSE_COMMENT_SCROLL_LINES as i16), Some(area));
        }
        MouseEventKind::Down(MouseButton::Left) => {
            let inner = block_inner(dialog_area);
            if !rect_contains(inner, mouse.column, mouse.row) {
                return;
            }
            let row = mouse.row.saturating_sub(inner.y);
            if let Some(active) = &mut app.issue_dialog {
                match row {
                    0 => {
                        active.field = IssueDialogField::Repo;
                        let cursor = issue_dialog_mouse_input_cursor(
                            "Repo",
                            active.repo.text(),
                            inner,
                            mouse,
                        );
                        active.repo.set_cursor_byte(cursor);
                        app.status = "editing issue repo".to_string();
                    }
                    2 => {
                        active.field = IssueDialogField::Title;
                        let cursor = issue_dialog_mouse_input_cursor(
                            "Title",
                            active.title.text(),
                            inner,
                            mouse,
                        );
                        active.title.set_cursor_byte(cursor);
                        app.status = "editing issue title".to_string();
                    }
                    4 => {
                        active.field = IssueDialogField::Labels;
                        let cursor = issue_dialog_mouse_input_cursor(
                            "Labels",
                            active.labels.text(),
                            inner,
                            mouse,
                        );
                        active.labels.set_cursor_byte(cursor);
                        app.status = "editing issue labels".to_string();
                    }
                    6 => {
                        active.field = IssueDialogField::Body;
                        app.status = "editing issue body".to_string();
                    }
                    row if row >= 7 => {
                        active.field = IssueDialogField::Body;
                        let line = usize::from(dialog.body_scroll)
                            .saturating_add(usize::from(row.saturating_sub(7)));
                        let column = mouse.column.saturating_sub(inner.x);
                        let cursor = comment_dialog_cursor_for_position(
                            active.body.text(),
                            inner.width.max(1),
                            line,
                            column,
                        );
                        active.body.set_cursor_byte(cursor);
                        app.status = "editing issue body".to_string();
                    }
                    _ => {}
                }
            }
            app.scroll_issue_dialog_to_cursor_in_area(Some(area));
        }
        _ => {}
    }
}

pub(super) fn handle_pr_create_dialog_mouse(
    app: &mut AppState,
    mouse: MouseEvent,
    area: Rect,
    store: Option<&SnapshotStore>,
) {
    let Some(dialog) = app.pr_create_dialog.clone() else {
        return;
    };
    let dialog_area = pr_create_dialog_area(area);
    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        && modal_footer_area(area, dialog_area)
            .is_some_and(|footer| rect_contains(footer, mouse.column, mouse.row))
    {
        if let Some(store) = store {
            app.save_active_pr_create_draft(store, DraftSaveTrigger::Manual, Instant::now());
        } else {
            app.status = "draft store unavailable".to_string();
        }
        return;
    }
    if !rect_contains(dialog_area, mouse.column, mouse.row) {
        return;
    }

    match mouse.kind {
        MouseEventKind::ScrollDown => {
            app.scroll_pr_create_dialog_body(MOUSE_COMMENT_SCROLL_LINES as i16, Some(area));
        }
        MouseEventKind::ScrollUp => {
            app.scroll_pr_create_dialog_body(-(MOUSE_COMMENT_SCROLL_LINES as i16), Some(area));
        }
        MouseEventKind::Down(MouseButton::Left) => {
            let inner = block_inner(dialog_area);
            if !rect_contains(inner, mouse.column, mouse.row) {
                return;
            }
            let row = mouse.row.saturating_sub(inner.y);
            if let Some(active) = &mut app.pr_create_dialog {
                match row {
                    4 => {
                        active.field = PrCreateField::Title;
                        let cursor = issue_dialog_mouse_input_cursor(
                            "Title",
                            active.title.text(),
                            inner,
                            mouse,
                        );
                        active.title.set_cursor_byte(cursor);
                        app.status = "editing pull request title".to_string();
                    }
                    6 => {
                        active.field = PrCreateField::Body;
                        app.status = "editing pull request body".to_string();
                    }
                    row if row >= 7 => {
                        active.field = PrCreateField::Body;
                        let line = usize::from(dialog.body_scroll)
                            .saturating_add(usize::from(row.saturating_sub(7)));
                        let column = mouse.column.saturating_sub(inner.x);
                        let cursor = comment_dialog_cursor_for_position(
                            active.body.text(),
                            inner.width.max(1),
                            line,
                            column,
                        );
                        active.body.set_cursor_byte(cursor);
                        app.status = "editing pull request body".to_string();
                    }
                    _ => {}
                }
            }
            app.scroll_pr_create_dialog_to_cursor_in_area(Some(area));
        }
        _ => {}
    }
}

pub(super) fn issue_dialog_mouse_input_cursor(
    label: &'static str,
    value: &str,
    inner: Rect,
    mouse: MouseEvent,
) -> usize {
    let prefix_width =
        display_width(&issue_dialog_field_prefix(label)).min(usize::from(u16::MAX)) as u16;
    let input_start = inner.x.saturating_add(prefix_width);
    cursor_for_inline_column(value, mouse.column.saturating_sub(input_start))
}

#[derive(Debug, Clone, Copy)]
pub(super) struct MouseLayout {
    pub(super) view_tabs: Rect,
    pub(super) section_tabs: Rect,
    pub(super) table: Rect,
    pub(super) details: Rect,
}

pub(super) fn handle_left_click(
    app: &mut AppState,
    mouse: MouseEvent,
    layout: MouseLayout,
    store: Option<&SnapshotStore>,
    tx: Option<&UnboundedSender<AppMsg>>,
) {
    app.details_text_drag = None;
    app.details_text_selection = None;

    if let Some(view) = view_tab_at(app, layout.view_tabs, mouse.column, mouse.row) {
        let previous_view = app.active_view.clone();
        debug!(
            column = mouse.column,
            row = mouse.row,
            target = "view_tab",
            from = %previous_view,
            to = %view,
            "mouse click handled"
        );
        let restored = app.switch_view(view);
        app.search_active = false;
        app.comment_search_active = false;
        app.global_search_active = false;
        app.filter_input_active = false;
        if !restored {
            app.focus = FocusTarget::Ghr;
            app.status = "GHR focused".to_string();
        }
        return;
    }

    if let Some(section_index) = section_tab_at(app, layout.section_tabs, mouse.column, mouse.row) {
        debug!(
            column = mouse.column,
            row = mouse.row,
            target = "section_tab",
            section_index,
            "mouse click handled"
        );
        app.select_section(section_index);
        return;
    }

    if rect_contains(layout.table, mouse.column, mouse.row) {
        debug!(
            column = mouse.column,
            row = mouse.row,
            target = "table",
            "mouse click handled"
        );
        handle_table_click(app, mouse, layout.table, store, tx);
        return;
    }

    if !rect_contains(layout.details, mouse.column, mouse.row) {
        return;
    }

    let inner = block_inner(layout.details);
    if !rect_contains(inner, mouse.column, mouse.row) {
        return;
    }

    let document = build_details_document(app, inner.width);
    let line_index = app.details_scroll as usize + (mouse.row - inner.y) as usize;
    let column = mouse.column - inner.x;
    debug!(
        column = mouse.column,
        row = mouse.row,
        target = "details",
        details_line = line_index,
        details_column = column,
        "mouse click handled"
    );
    app.focus = FocusTarget::Details;
    app.search_active = false;
    app.comment_search_active = false;
    app.global_search_active = false;
    app.filter_input_active = false;

    if let Some(action) = document.action_at(line_index, column) {
        debug!(action = ?action, "details action clicked");
        app.handle_detail_action(action, store, tx);
        return;
    }
    let clicked_comment = document.comment_at(line_index);
    let clicked_description = document.description_at(line_index);
    if let Some(url) = document.link_at(line_index, column) {
        debug!(
            url = %url,
            comment = ?clicked_comment,
            description = clicked_description,
            "details link clicked"
        );
        if let Some(comment_index) = clicked_comment {
            app.select_comment(comment_index);
        } else if clicked_description {
            app.select_details_body_without_scroll();
        }
        app.open_url(&url);
        return;
    }
    if app.details_mode == DetailsMode::Diff
        && let Some(comment_indices) = document.inline_comment_marker_at(line_index)
    {
        debug!(comment_indices = ?comment_indices, "inline comment marker clicked");
        app.toggle_revealed_diff_inline_comments(comment_indices);
        return;
    }
    if app.details_mode == DetailsMode::Diff
        && let Some(diff_line) = document.diff_line_at(line_index)
    {
        debug!(line = ?diff_line, "diff line clicked");
        app.handle_diff_line_click(diff_line, None);
        return;
    }
    if document.lines.get(line_index).is_some()
        && let Some(item_id) = app.current_item().map(|item| item.id.clone())
    {
        app.details_text_drag = Some(DetailsTextDrag {
            item_id,
            start: DetailsTextPosition {
                line: line_index,
                column,
            },
        });
    }
    if let Some(comment_index) = clicked_comment {
        debug!(comment_index, "comment clicked");
        app.select_comment(comment_index);
    } else if clicked_description {
        debug!("description clicked");
        app.select_details_body_without_scroll();
    }
}

pub(super) fn handle_details_text_drag(app: &mut AppState, mouse: MouseEvent, area: Rect) {
    let Some(drag) = app.details_text_drag.clone() else {
        return;
    };
    let Some(position) = details_text_position_from_mouse(app, mouse, area, true) else {
        return;
    };

    app.focus = FocusTarget::Details;
    app.search_active = false;
    app.comment_search_active = false;
    app.global_search_active = false;
    app.filter_input_active = false;
    app.details_text_selection = Some(DetailsTextSelection {
        item_id: drag.item_id,
        start: drag.start,
        end: position,
    });
    app.status = "selecting details text; release to copy".to_string();
}

pub(super) fn finish_details_text_drag(app: &mut AppState, mouse: MouseEvent, area: Rect) {
    if let Some(position) = details_text_position_from_mouse(app, mouse, area, true)
        && let Some(selection) = &mut app.details_text_selection
    {
        selection.end = position;
    }
    app.details_text_drag = None;

    let Some(selection) = app.details_text_selection.clone() else {
        return;
    };
    let Some(item_id) = app.current_item().map(|item| item.id.as_str()) else {
        app.details_text_selection = None;
        return;
    };
    if selection.item_id != item_id {
        app.details_text_selection = None;
        return;
    }

    let inner = block_inner(area);
    let document = build_details_document(app, inner.width);
    let selected = selected_details_text(&document, &selection);
    if selected.trim().is_empty() {
        app.details_text_selection = None;
        app.status = "no details text selected".to_string();
        return;
    }

    match copy_text_to_clipboard(&selected) {
        Ok(()) => {
            app.status = "copied selected details text".to_string();
        }
        Err(error) => {
            app.status = format!("copy failed: {error}");
        }
    }
}

pub(super) fn details_text_position_from_mouse(
    app: &AppState,
    mouse: MouseEvent,
    area: Rect,
    clamp: bool,
) -> Option<DetailsTextPosition> {
    let inner = block_inner(area);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }

    let (row, column) = if clamp {
        let row = mouse.row.clamp(
            inner.y,
            inner.y.saturating_add(inner.height.saturating_sub(1)),
        );
        let column = mouse.column.clamp(
            inner.x,
            inner.x.saturating_add(inner.width.saturating_sub(1)),
        );
        (row, column)
    } else {
        if !rect_contains(inner, mouse.column, mouse.row) {
            return None;
        }
        (mouse.row, mouse.column)
    };

    let document = build_details_document(app, inner.width);
    if document.lines.is_empty() {
        return None;
    }
    let visible_line = row.saturating_sub(inner.y);
    let line = usize::from(app.details_scroll)
        .saturating_add(usize::from(visible_line))
        .min(document.lines.len().saturating_sub(1));
    let line_width = display_width(&document.lines[line].to_string()).min(usize::from(u16::MAX));
    let column = usize::from(column.saturating_sub(inner.x))
        .min(line_width)
        .min(usize::from(u16::MAX)) as u16;

    Some(DetailsTextPosition { line, column })
}

pub(super) fn selected_details_text(
    document: &DetailsDocument,
    selection: &DetailsTextSelection,
) -> String {
    if document.lines.is_empty() {
        return String::new();
    }
    let ((start_line, start_col), (end_line, end_col)) = ordered_details_text_range(selection);
    let last_line = end_line.min(document.lines.len().saturating_sub(1));
    if start_line > last_line {
        return String::new();
    }

    let mut lines = Vec::new();
    for line_index in start_line..=last_line {
        if document.copy_skip_lines.contains(&line_index) {
            continue;
        }
        let line = document.lines[line_index].to_string();
        let line_width = display_width(&line);
        let start = if line_index == start_line {
            usize::from(start_col).min(line_width)
        } else {
            0
        };
        let end = if line_index == end_line {
            usize::from(end_col).min(line_width)
        } else {
            line_width
        };
        if let Some(selected) =
            selected_copyable_details_text_line(document, line_index, start, end)
        {
            lines.push(selected);
        }
    }

    lines.join("\n").trim_end_matches('\n').to_string()
}

pub(super) fn selected_copyable_details_text_line(
    document: &DetailsDocument,
    line_index: usize,
    start: usize,
    end: usize,
) -> Option<String> {
    if start >= end {
        return Some(String::new());
    }

    let text = document.lines[line_index].to_string();
    let mut output = String::new();
    let mut column = 0_usize;
    let mut selected_visible = false;
    let mut selected_copyable = false;
    for ch in text.chars() {
        let width = display_width_char(ch);
        let next_column = column.saturating_add(width);
        if next_column > start && column < end {
            selected_visible = true;
            if !details_copy_excluded(document, line_index, column, next_column) {
                output.push(ch);
                selected_copyable = true;
            }
        }
        column = next_column;
        if column >= end {
            break;
        }
    }

    if selected_visible && !selected_copyable {
        Some(String::new())
    } else {
        Some(output)
    }
}

pub(super) fn details_copy_excluded(
    document: &DetailsDocument,
    line_index: usize,
    start: usize,
    end: usize,
) -> bool {
    document.copy_exclusions.iter().any(|region| {
        region.line == line_index
            && end > usize::from(region.start)
            && start < usize::from(region.end)
    })
}

pub(super) fn handle_details_scroll(app: &mut AppState, area: Rect, delta: i16) {
    app.focus = FocusTarget::Details;
    app.search_active = false;
    app.comment_search_active = false;
    app.global_search_active = false;
    app.filter_input_active = false;

    let max_scroll = max_details_scroll(app, area);
    if max_scroll == 0 {
        app.details_scroll = 0;
        app.remember_current_conversation_details_position();
        return;
    }

    if delta < 0 {
        app.details_scroll = app.details_scroll.saturating_sub(delta.unsigned_abs());
    } else {
        app.details_scroll = app.details_scroll.saturating_add(delta as u16);
    }
    app.details_scroll = app.details_scroll.min(max_scroll);
    app.remember_current_conversation_details_position();
}

pub(super) fn handle_list_scroll(app: &mut AppState, area: Rect, delta: isize) {
    app.focus = FocusTarget::List;
    app.search_active = false;
    app.comment_search_active = false;
    app.global_search_active = false;
    app.filter_input_active = false;
    if app.details_mode == DetailsMode::Diff {
        app.move_diff_file(delta);
    } else {
        app.scroll_list_viewport(area, delta);
    }
}

pub(super) fn mouse_list_scroll_delta(app: &AppState, direction: isize) -> isize {
    if app.details_mode == DetailsMode::Diff {
        return direction
            .signum()
            .saturating_mul(MOUSE_DIFF_FILE_SCROLL_LINES as isize);
    }

    direction.saturating_mul(MOUSE_LIST_SCROLL_LINES as isize)
}

pub(super) fn list_page_delta(app: &AppState, area: Option<Rect>, direction: isize) -> isize {
    let rows = area
        .map(|area| {
            let body = body_areas_with_ratio(page_areas(area)[2], app.list_width_percent);
            usize::from(table_visible_rows(body[0]).max(1))
        })
        .unwrap_or(10);
    direction.saturating_mul(rows as isize)
}

pub(super) fn diff_file_page_delta(app: &AppState, area: Option<Rect>, direction: isize) -> isize {
    let rows = area
        .map(|area| {
            let body = body_areas_with_ratio(page_areas(area)[2], app.list_width_percent);
            usize::from(block_inner(body[0]).height.max(1))
        })
        .unwrap_or(10);
    direction.saturating_mul(rows as isize)
}

pub(super) fn diff_line_page_delta(app: &AppState, area: Option<Rect>, direction: isize) -> isize {
    let rows = area
        .map(|area| usize::from(block_inner(details_area_for(app, area)).height.max(1)))
        .unwrap_or(10);
    direction.saturating_mul(rows as isize)
}

pub(super) fn max_details_scroll(app: &AppState, area: Rect) -> u16 {
    let inner = block_inner(area);
    let document = build_details_document(app, inner.width);
    let max = document
        .lines
        .len()
        .saturating_sub(usize::from(inner.height));
    max.min(usize::from(u16::MAX)) as u16
}

pub(super) fn handle_table_click(
    app: &mut AppState,
    mouse: MouseEvent,
    area: Rect,
    store: Option<&SnapshotStore>,
    tx: Option<&UnboundedSender<AppMsg>>,
) {
    if app.details_mode == DetailsMode::Diff {
        let Some(file_index) = diff_file_row_at(app, area, mouse.row) else {
            return;
        };
        app.select_diff_file(file_index);
        app.focus = FocusTarget::Details;
        return;
    }

    let Some(position) = table_row_at(app, area, mouse.row) else {
        return;
    };

    app.set_selection(position);
    app.focus_details();
    mark_current_notification_read_if_possible(app, store, tx);
}

pub(super) fn handle_details_hover(app: &mut AppState, mouse: MouseEvent, area: Rect) {
    let inner = block_inner(area);
    if !rect_contains(inner, mouse.column, mouse.row) {
        return;
    }

    let document = build_details_document(app, inner.width);
    let line_index = app.details_scroll as usize + (mouse.row - inner.y) as usize;
    let column = mouse.column - inner.x;
    if document.link_at(line_index, column).is_some() {
        app.status = "link under pointer; click to open".to_string();
    } else if document.action_at(line_index, column).is_some() {
        app.status = "action under pointer; click to run".to_string();
    }
}

pub(super) fn mark_current_notification_read_if_possible(
    app: &mut AppState,
    store: Option<&SnapshotStore>,
    tx: Option<&UnboundedSender<AppMsg>>,
) {
    if let (Some(store), Some(tx)) = (store, tx) {
        app.mark_current_notification_read(store, tx);
    }
}

pub(super) fn table_row_at(app: &AppState, area: Rect, row: u16) -> Option<usize> {
    let section = app.current_section()?;
    let filtered_len = app.filtered_indices(section).len();
    if filtered_len == 0 {
        return None;
    }

    let inner = block_inner(area);
    let data_start = inner.y.saturating_add(TABLE_HEADER_HEIGHT);
    if row < data_start {
        return None;
    }

    let visible_rows = table_visible_rows(area);
    if visible_rows == 0 || row >= data_start.saturating_add(visible_rows) {
        return None;
    }

    let offset = app.current_list_scroll_offset(filtered_len, usize::from(visible_rows));
    let position = offset + usize::from(row - data_start);
    (position < filtered_len).then_some(position)
}

pub(super) fn diff_file_row_at(app: &AppState, area: Rect, row: u16) -> Option<usize> {
    let item = app.current_item()?;
    let diff = match app.diffs.get(&item.id)? {
        DiffState::Loaded(diff) => diff,
        _ => return None,
    };
    let entries = diff_tree_entries(diff);
    if entries.is_empty() {
        return None;
    }

    let inner = block_inner(area);
    if row < inner.y || row >= inner.y.saturating_add(inner.height) {
        return None;
    }

    let selected_file = app.selected_diff_file_index_for(&item.id, diff);
    let selected_row = diff_tree_row_index_for_file(&entries, selected_file).unwrap_or(0);
    let visible_rows = usize::from(inner.height.max(1));
    let offset = table_viewport_offset(selected_row, visible_rows);
    let position = offset + usize::from(row - inner.y);
    entries.get(position).and_then(|entry| entry.file_index)
}

pub(super) fn table_visible_rows(area: Rect) -> u16 {
    block_inner(area).height.saturating_sub(TABLE_HEADER_HEIGHT)
}

pub(super) fn table_visible_range(
    offset: usize,
    visible_rows: usize,
    len: usize,
) -> Option<(usize, usize)> {
    if visible_rows == 0 || len == 0 {
        return None;
    }

    let offset = offset.min(max_table_viewport_offset(len, visible_rows));
    let end = offset.saturating_add(visible_rows).min(len);
    Some((offset + 1, end))
}

pub(super) fn table_visible_range_label(
    section: &SectionSnapshot,
    unfiltered: bool,
    start: usize,
    end: usize,
    visible_len: usize,
) -> String {
    if unfiltered
        && let (Some(total_count), Some(page_size)) =
            (section.total_count, section_page_size_for_display(section))
    {
        let offset = section.page.saturating_sub(1).saturating_mul(page_size);
        let global_start = offset.saturating_add(start).min(total_count);
        let global_end = offset.saturating_add(end).min(total_count);
        let mut label = format!(" | showing {global_start}-{global_end}/{total_count}");
        if let Some(page_label) = section_page_label(section) {
            label.push_str(&format!(" | page {page_label}"));
        }
        return label;
    }

    let mut label = format!(" | showing {start}-{end}/{visible_len}");
    if let Some(page_label) = section_page_label(section) {
        label.push_str(&format!(" | page {page_label}"));
    }
    label
}

pub(super) fn table_viewport_offset(selected: usize, visible_rows: usize) -> usize {
    if visible_rows == 0 {
        return 0;
    }
    selected.saturating_sub(visible_rows - 1)
}

pub(super) fn max_table_viewport_offset(len: usize, visible_rows: usize) -> usize {
    if visible_rows == 0 {
        0
    } else {
        len.saturating_sub(visible_rows)
    }
}

pub(super) fn view_tab_at(app: &AppState, area: Rect, column: u16, row: u16) -> Option<String> {
    let views = app.view_tabs();
    let labels = views
        .iter()
        .map(|view| view.label.clone())
        .collect::<Vec<_>>();
    tab_index_at(&labels, area, column, row)
        .and_then(|index| views.get(index).map(|view| view.key.clone()))
}

pub(super) fn section_tab_at(app: &AppState, area: Rect, column: u16, row: u16) -> Option<usize> {
    let labels = app
        .visible_sections()
        .iter()
        .map(|section| section_tab_label(app, section))
        .collect::<Vec<_>>();
    tab_index_at(&labels, area, column, row)
}

pub(super) fn tab_index_at(labels: &[String], area: Rect, column: u16, row: u16) -> Option<usize> {
    let inner = block_inner(area);
    if !rect_contains(inner, column, row) {
        return None;
    }

    let clicked = column.saturating_sub(inner.x);
    let mut offset = 0_u16;
    for (index, label) in labels.iter().enumerate() {
        let width = display_width(label) as u16;
        let end = offset.saturating_add(width);
        if clicked >= offset && clicked < end {
            return Some(index);
        }
        offset = end.saturating_add(TAB_DIVIDER_WIDTH);
    }

    None
}
