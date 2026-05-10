use super::*;

pub(super) fn draw(frame: &mut Frame<'_>, app: &AppState, paths: &Paths) {
    set_active_theme(app.theme_name);
    let area = frame.area();
    frame.buffer_mut().set_style(area, active_theme().base());
    let chunks = page_areas(area);

    draw_view_tabs(frame, app, chunks[0]);
    draw_top_status(frame, app, area);
    draw_section_tabs(frame, app, chunks[1]);

    if app.mouse_capture_enabled {
        let body = body_areas_with_ratio(chunks[2], app.list_width_percent);
        if app.details_mode == DetailsMode::Diff {
            draw_diff_files(frame, app, body[0]);
        } else {
            draw_table(frame, app, body[0]);
        }
        draw_details(frame, app, body[1]);
    } else {
        draw_details(frame, app, chunks[2]);
    }
    draw_footer(frame, app, paths, chunks[3]);

    if let Some(dialog) = app.setup_dialog {
        draw_setup_dialog(frame, dialog, area);
    } else if let Some(dialog) = app.startup_dialog {
        draw_startup_dialog(frame, app, paths, dialog, area);
    } else if let Some(dialog) = &app.message_dialog {
        draw_message_dialog(frame, dialog, area);
    } else if app.help_dialog {
        draw_help_dialog(frame, area, &app.command_palette_key);
    } else if let Some(dialog) = &app.item_edit_dialog {
        draw_item_edit_dialog(frame, dialog, area);
    } else if let Some(dialog) = &app.pr_action_dialog {
        draw_pr_action_dialog(frame, dialog, app.pr_action_running, area);
    } else if let Some(dialog) = &app.label_dialog {
        draw_label_dialog(frame, dialog, app.label_updating, area);
    } else if let Some(dialog) = &app.issue_dialog {
        draw_issue_dialog(frame, dialog, app.issue_creating, area);
    } else if let Some(dialog) = &app.pr_create_dialog {
        draw_pr_create_dialog(frame, dialog, app.pr_creating, area);
    } else if let Some(dialog) = &app.reaction_dialog {
        draw_reaction_dialog(frame, dialog, app.posting_reaction, area);
    } else if let Some(dialog) = &app.review_submit_dialog {
        draw_review_submit_dialog(frame, dialog, area);
    } else if let Some(dialog) = &app.milestone_dialog {
        draw_milestone_dialog(frame, dialog, app.milestone_action_running, area);
    } else if let Some(dialog) = &app.assignee_dialog {
        draw_assignee_dialog(frame, dialog, app.assignee_action_running, area);
    } else if let Some(dialog) = &app.reviewer_dialog {
        draw_reviewer_dialog(frame, dialog, app.reviewer_action_running, area);
    } else if let Some(dialog) = &app.comment_dialog {
        draw_comment_dialog(frame, dialog, area);
    } else if app.global_search_active {
        if let Some(dialog) = &app.global_search_dialog {
            draw_global_search_dialog(frame, dialog, app, area);
        }
    } else if app.global_search_running {
        draw_global_search_loading_dialog(frame, app, area);
    } else if let Some(loading) = &app.section_page_loading {
        draw_section_page_loading_dialog(frame, loading, area);
    }

    if let Some(palette) = &app.command_palette {
        draw_command_palette(frame, app, palette, area, &app.command_palette_key);
    }
    if let Some(switcher) = &app.project_switcher {
        draw_project_switcher(frame, app, switcher, area);
    }
    if let Some(switcher) = &app.top_menu_switcher {
        draw_top_menu_switcher(frame, app, switcher, area);
    }
    if let Some(switcher) = &app.theme_switcher {
        draw_theme_switcher(frame, app, switcher, area);
    }
    if let Some(dialog) = &app.recent_items_dialog {
        draw_recent_items_dialog(frame, app, dialog, area);
    }
    if let Some(dialog) = &app.saved_search_dialog {
        draw_saved_search_dialog(frame, app, dialog, area);
    }
    if let Some(dialog) = &app.save_search_dialog {
        draw_save_search_dialog(frame, dialog, area);
    }
    if let Some(dialog) = &app.project_add_dialog {
        draw_project_add_dialog(frame, dialog, area);
    }
    if let Some(dialog) = &app.project_remove_dialog {
        draw_project_remove_dialog(frame, dialog, area);
    }
    if let Some(dialog) = &app.cache_clear_dialog {
        draw_cache_clear_dialog(frame, dialog, area);
    }
}

fn draw_view_tabs(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let views = app.view_tabs();
    let titles = views
        .iter()
        .map(|view| Line::from(view.label.clone()))
        .collect::<Vec<_>>();
    let active = views
        .iter()
        .position(|view| view.key == app.active_view)
        .unwrap_or(0);
    let ghr_focused = app.focus == FocusTarget::Ghr;
    let border_style = if ghr_focused {
        active_theme().focus_border()
    } else {
        active_theme().border()
    };
    let border_type = if ghr_focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    };
    let title = if ghr_focused { "[Focus] ghr" } else { "ghr" };
    let title_style = view_tabs_title_style(app, border_style);

    let tabs = Tabs::new(titles)
        .select(active)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(border_type)
                .border_style(border_style)
                .style(active_theme().panel())
                .title(Span::styled(title, title_style)),
        )
        .style(active_theme().muted())
        .highlight_style(active_view_tab_style());
    frame.render_widget(tabs, area);
}

pub(super) fn view_tabs_title_style(app: &AppState, base_style: Style) -> Style {
    if app.focus == FocusTarget::Ghr
        || app.has_unread_notifications()
        || app.has_unseen_repo_items()
    {
        base_style.add_modifier(Modifier::BOLD)
    } else {
        base_style
    }
}

fn draw_section_tabs(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let sections = app.visible_sections();
    if sections.is_empty() {
        frame.render_widget(
            Paragraph::new("No sections configured").block(Block::default().borders(Borders::ALL)),
            area,
        );
        return;
    }

    let titles = sections
        .iter()
        .map(|section| Line::from(section_tab_label(app, section)))
        .collect::<Vec<_>>();
    let sections_focused = app.focus == FocusTarget::Sections;
    let border_style = if sections_focused {
        active_theme().focus_border()
    } else {
        active_theme().border()
    };
    let border_type = if sections_focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    };
    let title = if sections_focused {
        "[Focus] Sections"
    } else {
        "Sections"
    };

    let tabs = Tabs::new(titles)
        .select(app.current_section_position())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(border_type)
                .border_style(border_style)
                .style(active_theme().panel())
                .title(Span::styled(title, border_style)),
        )
        .style(active_theme().muted())
        .highlight_style(active_section_tab_style());
    frame.render_widget(tabs, area);
}

pub(super) fn active_view_tab_style() -> Style {
    active_navigation_tab_style()
}

pub(super) fn active_section_tab_style() -> Style {
    active_navigation_tab_style()
}

fn active_navigation_tab_style() -> Style {
    active_theme().active()
}

pub(super) fn section_tab_label(app: &AppState, section: &SectionSnapshot) -> String {
    let title = app
        .quick_filter_label_for_section(section)
        .map(|filter| format!("{} [{filter}]", section.title))
        .unwrap_or_else(|| section.title.clone());
    if matches!(section.kind, SectionKind::Notifications) {
        let unread = notification_unread_count_label(app, section);
        if unread == 0 {
            title
        } else {
            format!("{title} ({unread})")
        }
    } else if !app.search_query.is_empty() {
        format!(
            "{} ({}/{})",
            title,
            app.filtered_indices(section).len(),
            section
                .items
                .len()
                .saturating_sub(app.ignored_count_for_section(section))
        )
    } else {
        let count_label = section_count_label(app, section);
        format!("{title} ({count_label})")
    }
}

fn notification_unread_count_label(app: &AppState, section: &SectionSnapshot) -> usize {
    section
        .items
        .iter()
        .filter(|item| !app.ignored_items.contains(&item.id))
        .filter(|item| item.unread.unwrap_or(false))
        .count()
}

pub(super) fn section_count_label(app: &AppState, section: &SectionSnapshot) -> String {
    let loaded = section
        .items
        .len()
        .saturating_sub(app.ignored_count_for_section(section));
    match section.total_count {
        Some(total) if total > loaded => format!("{loaded}/{total}"),
        Some(total) => total.to_string(),
        None => loaded.to_string(),
    }
}

pub(super) fn section_page_size(section: &SectionSnapshot, config: &Config) -> usize {
    if section.page_size > 0 {
        return section.page_size.min(100);
    }
    if !section.items.is_empty() {
        return section.items.len().clamp(1, 100);
    }
    match section.kind {
        SectionKind::PullRequests => config.defaults.pr_per_page,
        SectionKind::Issues => config.defaults.issue_per_page,
        SectionKind::Notifications => config.defaults.notification_limit,
    }
    .clamp(1, 100)
}

pub(super) fn section_page_size_for_display(section: &SectionSnapshot) -> Option<usize> {
    if section.page_size > 0 {
        Some(section.page_size)
    } else if !section.items.is_empty() {
        Some(section.items.len().clamp(1, 100))
    } else {
        None
    }
}

pub(super) fn section_total_pages(total_count: usize, page_size: usize) -> (usize, bool) {
    let accessible = total_count.clamp(1, SEARCH_RESULT_WINDOW);
    let total_pages = accessible.div_ceil(page_size.max(1)).max(1);
    (total_pages, total_count > SEARCH_RESULT_WINDOW)
}

pub(super) fn section_page_label(section: &SectionSnapshot) -> Option<String> {
    let total_count = section.total_count?;
    let page_size = section_page_size_for_display(section)?;
    let (total_pages, total_is_capped) = section_total_pages(total_count, page_size);
    Some(section_page_status_label(
        section.page.max(1).min(total_pages),
        total_pages,
        total_is_capped,
    ))
}

pub(super) fn section_page_status_label(
    page: usize,
    total_pages: usize,
    total_is_capped: bool,
) -> String {
    if total_is_capped {
        format!("{page}/{total_pages}+")
    } else {
        format!("{page}/{total_pages}")
    }
}

fn draw_table(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let Some(section) = app.current_section() else {
        frame.render_widget(
            Paragraph::new("No data").block(Block::default().borders(Borders::ALL)),
            area,
        );
        return;
    };

    let filtered_indices = app.filtered_indices(section);
    let rows = filtered_indices
        .iter()
        .filter_map(|index| section.items.get(*index))
        .map(|item| {
            let row_style = list_item_row_style(app, item);
            Row::new(list_table_cells(section, item)).style(row_style)
        })
        .collect::<Vec<_>>();
    let empty_state = rows
        .is_empty()
        .then(|| list_empty_state_message(app, section, &filtered_indices))
        .flatten();

    let list_focused = app.focus == FocusTarget::List;
    let header_style = if list_focused {
        active_theme().focus_border()
    } else {
        active_theme().subtle().add_modifier(Modifier::BOLD)
    };
    let header = Row::new(list_table_header(section))
        .style(header_style)
        .bottom_margin(1);

    let mut title = if app.search_query.is_empty() {
        section.title.clone()
    } else {
        format!(
            "{} - {} / {} matching '{}'",
            section.title,
            filtered_indices.len(),
            section.items.len(),
            app.search_query
        )
    };
    if let Some(filter) = app.quick_filter_label_for_section(section) {
        title.push_str(&format!(" | filter: {filter}"));
    }
    if let Some(error) = &section.error {
        title.push_str(&format!(" - error: {}", compact_error_label(error)));
    };
    let ignored_count = app.ignored_count_for_section(section);
    if ignored_count > 0 {
        title.push_str(&format!(" | {ignored_count} ignored"));
    }
    let visible_rows = usize::from(table_visible_rows(area));
    let table_offset = app.current_list_scroll_offset(filtered_indices.len(), visible_rows);
    if let Some((start, end)) =
        table_visible_range(table_offset, visible_rows, filtered_indices.len())
    {
        title.push_str(&table_visible_range_label(
            section,
            app.search_query.is_empty() && ignored_count == 0,
            start,
            end,
            filtered_indices.len(),
        ));
    }

    let input_prompt = active_list_input_prompt(app);
    let (border_style, title_style, border_type, highlight_style) = if app.dragging_split {
        (
            active_theme()
                .panel()
                .fg(active_theme().action)
                .add_modifier(Modifier::BOLD),
            active_theme()
                .panel()
                .fg(active_theme().highlight_fg)
                .bg(active_theme().action)
                .add_modifier(Modifier::BOLD),
            BorderType::Thick,
            active_theme()
                .panel()
                .fg(active_theme().highlight_fg)
                .bg(active_theme().action)
                .add_modifier(Modifier::BOLD),
        )
    } else if let Some((_, color)) = &input_prompt {
        (
            active_theme()
                .panel()
                .fg(*color)
                .add_modifier(Modifier::BOLD),
            active_theme()
                .panel()
                .fg(active_theme().highlight_fg)
                .bg(*color)
                .add_modifier(Modifier::BOLD),
            BorderType::Thick,
            active_theme()
                .panel()
                .fg(active_theme().highlight_fg)
                .bg(*color)
                .add_modifier(Modifier::BOLD),
        )
    } else if list_focused {
        (
            active_theme().focus_border(),
            active_theme().active(),
            BorderType::Thick,
            active_theme().active(),
        )
    } else {
        (
            active_theme().border(),
            active_theme().muted(),
            BorderType::Plain,
            active_theme().selected(),
        )
    };
    if let Some((prompt, _)) = input_prompt {
        title = format!("{prompt} | {title}");
    }
    let title = focus_panel_title("List", &title, list_focused);

    let table = Table::new(rows, list_table_constraints(section))
        .header(header)
        .style(active_theme().panel())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(border_type)
                .border_style(border_style)
                .title(Span::styled(title, title_style)),
        )
        .row_highlight_style(highlight_style)
        .highlight_spacing(HighlightSpacing::Always)
        .highlight_symbol("> ");

    let mut table_state = TableState::default().with_offset(table_offset);
    let selected = app.current_selected_position();
    if !filtered_indices.is_empty()
        && selected >= table_offset
        && selected < table_offset.saturating_add(visible_rows)
    {
        table_state.select(Some(selected));
    }
    frame.render_stateful_widget(table, area, &mut table_state);
    if let Some((message, style)) = empty_state {
        draw_list_empty_state_message(frame, area, &message, style);
    }
}

fn list_table_cells(section: &SectionSnapshot, item: &WorkItem) -> Vec<String> {
    let mut cells = Vec::new();
    if !list_table_hides_repo(section) {
        cells.push(item.repo.clone());
    }
    cells.extend([
        item.number
            .map(|number| format!("#{number}"))
            .unwrap_or_default(),
        item.title.clone(),
        relative_time(item.updated_at),
    ]);
    if !matches!(section.kind, SectionKind::Notifications) {
        cells.push(item_meta(item));
    }
    cells
}

fn list_table_header(section: &SectionSnapshot) -> Vec<&'static str> {
    if matches!(section.kind, SectionKind::Notifications) {
        vec!["Repo", "#", "Title", "Updated"]
    } else if list_table_hides_repo(section) {
        vec!["#", "Title", "Updated", "Meta"]
    } else {
        vec!["Repo", "#", "Title", "Updated", "Meta"]
    }
}

fn list_table_constraints(section: &SectionSnapshot) -> Vec<Constraint> {
    if matches!(section.kind, SectionKind::Notifications) {
        vec![
            Constraint::Length(20),
            Constraint::Length(8),
            Constraint::Min(20),
            Constraint::Length(8),
        ]
    } else if list_table_hides_repo(section) {
        vec![
            Constraint::Length(8),
            Constraint::Min(20),
            Constraint::Length(8),
            Constraint::Length(14),
        ]
    } else {
        vec![
            Constraint::Length(20),
            Constraint::Length(8),
            Constraint::Min(20),
            Constraint::Length(8),
            Constraint::Length(14),
        ]
    }
}

fn list_table_hides_repo(section: &SectionSnapshot) -> bool {
    !matches!(section.kind, SectionKind::Notifications)
        && section_view_key(section).starts_with("repo:")
}

pub(super) fn list_item_row_style(app: &AppState, item: &WorkItem) -> Style {
    match item.unread {
        Some(true) => active_theme()
            .panel()
            .fg(active_theme().text)
            .add_modifier(Modifier::BOLD),
        Some(false) => active_theme().subtle(),
        None if app.item_has_unseen_details(item) => {
            active_theme().panel().add_modifier(Modifier::BOLD)
        }
        None => active_theme().panel(),
    }
}

fn draw_list_empty_state_message(frame: &mut Frame<'_>, area: Rect, message: &str, style: Style) {
    let inner = block_inner(area);
    if inner.height <= 2 || inner.width <= 2 {
        return;
    }

    let message_area = Rect::new(
        inner.x.saturating_add(1),
        inner.y.saturating_add(2),
        inner.width.saturating_sub(2),
        1,
    );
    let width = usize::from(message_area.width.max(1));
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            truncate_inline(message, width),
            style,
        ))),
        message_area,
    );
}

fn list_empty_state_message(
    app: &AppState,
    section: &SectionSnapshot,
    filtered_indices: &[usize],
) -> Option<(String, Style)> {
    if !filtered_indices.is_empty() {
        return None;
    }
    if let Some(error) = &section.error {
        return Some((
            format!("Error: {}", compact_error_label(error)),
            active_theme().panel().fg(active_theme().error),
        ));
    }
    if section.items.is_empty() && section.refreshed_at.is_none() {
        if app.refreshing {
            let message = if section_view_key(section).starts_with("repo:") {
                "Git repo is loading ..."
            } else {
                "GitHub data is loading ..."
            };
            return Some((message.to_string(), active_theme().muted()));
        }
        return Some((
            "No cached data yet. Press r to refresh.".to_string(),
            active_theme().subtle(),
        ));
    }
    if section.items.is_empty() {
        return Some(("No items found.".to_string(), active_theme().subtle()));
    }
    if app.ignored_count_for_section(section) == section.items.len() {
        return Some((
            "All loaded items are ignored. Use the command palette to clear ignored items."
                .to_string(),
            active_theme().subtle(),
        ));
    }
    if !app.search_query.is_empty() {
        return Some((
            "No items match the current search.".to_string(),
            active_theme().subtle(),
        ));
    }
    None
}

pub(super) fn draw_diff_files(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let files_focused = app.focus == FocusTarget::List;
    let (border_style, title_style, border_type, highlight_style) = if app.dragging_split {
        (
            active_theme()
                .panel()
                .fg(active_theme().action)
                .add_modifier(Modifier::BOLD),
            active_theme()
                .panel()
                .fg(active_theme().highlight_fg)
                .bg(active_theme().action)
                .add_modifier(Modifier::BOLD),
            BorderType::Thick,
            active_theme()
                .panel()
                .fg(active_theme().highlight_fg)
                .bg(active_theme().action)
                .add_modifier(Modifier::BOLD),
        )
    } else if files_focused {
        (
            active_theme().focus_border(),
            active_theme().active(),
            BorderType::Thick,
            active_theme().active(),
        )
    } else {
        (
            active_theme().border(),
            active_theme().panel().fg(active_theme().success),
            BorderType::Plain,
            active_theme().selected(),
        )
    };

    let title;
    let mut selected_row = None;
    let rows = match app.current_diff() {
        Some(DiffState::Loaded(diff)) => {
            title = format!(
                "Files | {} file(s) | +{} -{}",
                diff.files.len(),
                diff.additions,
                diff.deletions
            );
            let comment_counts = app
                .current_item()
                .and_then(|item| app.loaded_comments_for_item(&item.id))
                .map(|comments| diff_file_comment_counts(diff, comments))
                .unwrap_or_default();
            let entries = diff_tree_entries_with_comment_counts(diff, &comment_counts);
            if let Some(item_id) = app.current_item().map(|item| item.id.as_str()) {
                let selected_file = app.selected_diff_file_index_for(item_id, diff);
                selected_row = diff_tree_row_index_for_file(&entries, selected_file);
            }
            entries
                .into_iter()
                .map(|entry| {
                    let indent = "  ".repeat(entry.depth);
                    let (marker, style) = if entry.file_index.is_some() {
                        (" ", active_theme().panel())
                    } else {
                        ("▾ ", active_theme().panel().fg(active_theme().success))
                    };
                    let label = format!("{indent}{marker}{}", entry.label);
                    Row::new(diff_tree_row_cells(label, entry.stats)).style(style)
                })
                .collect::<Vec<_>>()
        }
        Some(DiffState::Error(error)) => {
            title = "Files | error".to_string();
            vec![
                Row::new(diff_tree_row_cells(compact_error_label(error), None))
                    .style(active_theme().panel().fg(active_theme().error)),
            ]
        }
        Some(DiffState::Loading) | None => {
            title = "Files | loading".to_string();
            vec![
                Row::new(diff_tree_row_cells("loading diff...".to_string(), None))
                    .style(active_theme().muted()),
            ]
        }
    };

    let title = focus_panel_title("List", &title, files_focused);
    let table = Table::new(
        rows,
        [
            Constraint::Min(12),
            Constraint::Length(1),
            Constraint::Length(5),
            Constraint::Length(5),
            Constraint::Length(4),
            Constraint::Length(8),
        ],
    )
    .style(active_theme().panel())
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(border_type)
            .border_style(border_style)
            .title(Span::styled(title, title_style)),
    )
    .row_highlight_style(highlight_style)
    .highlight_symbol("> ");

    let mut table_state = TableState::default();
    table_state.select(selected_row);
    frame.render_stateful_widget(table, area, &mut table_state);
}

fn diff_tree_row_cells(label: String, stats: Option<DiffFileStats>) -> Vec<Cell<'static>> {
    let Some(stats) = stats else {
        return vec![
            Cell::from(label),
            Cell::from(""),
            Cell::from(""),
            Cell::from(""),
            Cell::from(""),
            Cell::from(""),
        ];
    };

    vec![
        Cell::from(label),
        Cell::from(stats.status),
        right_aligned_cell(format!("+{}", stats.additions)),
        right_aligned_cell(format!("-{}", stats.deletions)),
        right_aligned_cell(if stats.comments > 0 {
            format!("{}c", stats.comments)
        } else {
            String::new()
        }),
        Cell::from(""),
    ]
}

fn right_aligned_cell(text: String) -> Cell<'static> {
    Cell::new(Text::from(text).alignment(Alignment::Right))
}

pub(super) fn active_list_input_prompt(app: &AppState) -> Option<(String, Color)> {
    let theme = active_theme();
    if app.filter_input_active {
        return Some((
            format!(
                "Filter: f{}_  Enter apply  empty/clear resets  Esc cancel",
                app.filter_input_query
            ),
            theme.focus,
        ));
    }

    if app.global_search_active {
        let scope = app
            .global_search_scope
            .clone()
            .or_else(|| app.current_repo_scope())
            .map(|repo| format!(" in {repo}"))
            .unwrap_or_default();
        return Some((
            format!(
                "Repo Search{scope}: dialog open  Enter choose/search  Ctrl+S save  Ctrl+U clear  Esc cancel"
            ),
            theme.action,
        ));
    }

    if app.is_global_search_results_view() {
        return Some((
            format!("Search results: {}  Esc back", app.global_search_query),
            theme.action,
        ));
    }

    if app.search_active {
        return Some((
            format!(
                "Local Search: /{}_  Enter search  Esc clear",
                app.search_query
            ),
            theme.search,
        ));
    }

    None
}

fn active_details_input_prompt(app: &AppState) -> Option<(String, Color)> {
    let theme = active_theme();
    if app.comment_search_active {
        return Some((
            format!(
                "Comment Search: /{}_  Enter keep  Esc clear",
                app.comment_search_query
            ),
            theme.search,
        ));
    }

    let query = app.comment_search_query.trim();
    if app.focus == FocusTarget::Details
        && app.details_mode == DetailsMode::Conversation
        && !query.is_empty()
    {
        return Some((
            format!("Comment Search: /{query}  n/p results  Esc clear"),
            theme.search,
        ));
    }

    None
}

fn draw_details(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let text_selection_mode = !app.mouse_capture_enabled;
    let details_focused = app.focus == FocusTarget::Details;
    let details_prompt = active_details_input_prompt(app);
    let raw_title = details_prompt
        .as_ref()
        .map(|(prompt, _)| format!("{} {prompt}", details_title()))
        .unwrap_or_else(|| details_title().to_string());
    let prompt_color = details_prompt.as_ref().map(|(_, color)| *color);
    let title = focus_panel_title("Details", &raw_title, details_focused);
    let (border_style, mut title_style, border_type) = if app.dragging_split {
        (
            active_theme()
                .panel()
                .fg(active_theme().action)
                .add_modifier(Modifier::BOLD),
            active_theme()
                .panel()
                .fg(active_theme().highlight_fg)
                .bg(active_theme().action)
                .add_modifier(Modifier::BOLD),
            BorderType::Thick,
        )
    } else if details_focused {
        (
            active_theme().focus_alt_border(),
            active_theme()
                .panel()
                .fg(active_theme().highlight_fg)
                .bg(active_theme().focus_alt)
                .add_modifier(Modifier::BOLD),
            BorderType::Thick,
        )
    } else {
        (
            active_theme().border(),
            active_theme()
                .panel()
                .fg(active_theme().action)
                .add_modifier(Modifier::BOLD),
            BorderType::Plain,
        )
    };
    if let Some(color) = prompt_color {
        title_style = active_theme()
            .panel()
            .fg(color)
            .add_modifier(Modifier::BOLD);
    }

    let document_width = if text_selection_mode {
        area.width
    } else {
        area.width.saturating_sub(2)
    };
    let mut document = build_details_document(app, document_width);
    apply_details_text_selection(app, &mut document);

    frame.render_widget(Clear, area);
    if text_selection_mode {
        frame.render_widget(
            DetailsLines::new(&document.lines, app.details_scroll, active_theme().panel()),
            area,
        );
        return;
    }

    let block = Block::default()
        .style(active_theme().panel())
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(border_style)
        .title(Span::styled(title, title_style));
    let inner = block_inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        DetailsLines::new(&document.lines, app.details_scroll, active_theme().panel()),
        inner,
    );
}

pub(super) struct DetailsLines<'a> {
    lines: &'a [Line<'static>],
    scroll: u16,
    base_style: Style,
}

impl<'a> DetailsLines<'a> {
    pub(super) const fn new(lines: &'a [Line<'static>], scroll: u16, base_style: Style) -> Self {
        Self {
            lines,
            scroll,
            base_style,
        }
    }
}

impl Widget for DetailsLines<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let area = area.intersection(buf.area);
        if area.is_empty() {
            return;
        }

        fill_details_area(buf, area, self.base_style);
        for (row, line) in self
            .lines
            .iter()
            .skip(usize::from(self.scroll))
            .take(usize::from(area.height))
            .enumerate()
        {
            let y = area.y.saturating_add(row as u16);
            render_details_line(buf, area, y, line, self.base_style);
        }
    }
}

fn fill_details_area(buf: &mut Buffer, area: Rect, style: Style) {
    for y in area.top()..area.bottom() {
        fill_details_row(buf, area, y, style);
    }
}

fn fill_details_row(buf: &mut Buffer, area: Rect, y: u16, style: Style) {
    for x in area.left()..area.right() {
        let cell = &mut buf[(x, y)];
        cell.reset();
        cell.set_symbol(" ").set_style(style).set_skip(false);
    }
}

fn render_details_line(
    buf: &mut Buffer,
    area: Rect,
    y: u16,
    line: &Line<'static>,
    base_style: Style,
) {
    let line_style = base_style.patch(line.style);
    fill_details_row(buf, area, y, line_style);
    let line_width = details_line_width(line);
    if line_width == 0 {
        return;
    }

    let available_width = usize::from(area.width);
    let offset = if line_width <= available_width {
        match line.alignment {
            Some(Alignment::Center) => (available_width - line_width) / 2,
            Some(Alignment::Right) => available_width - line_width,
            Some(Alignment::Left) | None => 0,
        }
    } else {
        0
    };
    let mut x = area
        .x
        .saturating_add(offset.min(usize::from(u16::MAX)) as u16);

    'spans: for span in &line.spans {
        let style = line_style.patch(span.style);
        for symbol in details_text_symbols(span.content.as_ref()) {
            let width = display_width(&symbol);
            if width == 0 {
                continue;
            }
            let width_u16 = width.min(usize::from(u16::MAX)) as u16;
            let next_x = x.saturating_add(width_u16);
            if next_x > area.right() {
                break 'spans;
            }

            let cell = &mut buf[(x, y)];
            cell.reset();
            cell.set_symbol(&symbol).set_style(style).set_skip(false);

            for hidden_x in x.saturating_add(1)..next_x {
                let cell = &mut buf[(hidden_x, y)];
                cell.reset();
                cell.set_symbol(" ").set_style(style).set_skip(true);
            }
            x = next_x;
        }
    }
}

fn details_line_width(line: &Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|span| display_width(span.content.as_ref()))
        .sum()
}

fn details_text_symbols(text: &str) -> Vec<String> {
    let mut symbols = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        let mut symbol = ch.to_string();
        if chars.peek().is_some_and(|next| *next == '\u{fe0f}') {
            symbol.push(chars.next().expect("peeked variation selector"));
        }
        symbols.push(symbol);
    }
    symbols
}

pub(super) fn apply_details_text_selection(app: &AppState, document: &mut DetailsDocument) {
    let Some(item_id) = app.current_item().map(|item| item.id.as_str()) else {
        return;
    };
    let Some(selection) = app
        .details_text_selection
        .as_ref()
        .filter(|selection| selection.item_id == item_id)
    else {
        return;
    };
    let ((start_line, start_col), (end_line, end_col)) = ordered_details_text_range(selection);
    if start_line == end_line && start_col == end_col {
        return;
    }
    if document.lines.is_empty() {
        return;
    }

    let last_line = end_line.min(document.lines.len().saturating_sub(1));
    for line_index in start_line..=last_line {
        let line_exclusions = document
            .copy_exclusions
            .iter()
            .filter(|region| region.line == line_index)
            .cloned()
            .collect::<Vec<_>>();
        let line = &mut document.lines[line_index];
        let line_width = display_width(&line.to_string());
        let selection_start = if line_index == start_line {
            usize::from(start_col).min(line_width)
        } else {
            0
        };
        let selection_end = if line_index == end_line {
            usize::from(end_col).min(line_width)
        } else {
            line_width
        };
        if selection_start >= selection_end {
            continue;
        }

        let spans = std::mem::take(&mut line.spans);
        line.spans =
            highlight_details_text_spans(spans, selection_start, selection_end, &line_exclusions);
    }
}

pub(super) fn ordered_details_text_range(
    selection: &DetailsTextSelection,
) -> ((usize, u16), (usize, u16)) {
    let start = (selection.start.line, selection.start.column);
    let end = (selection.end.line, selection.end.column);
    if start <= end {
        (start, end)
    } else {
        (end, start)
    }
}

fn highlight_details_text_spans(
    spans: Vec<Span<'static>>,
    selection_start: usize,
    selection_end: usize,
    exclusions: &[CopyExclusionRegion],
) -> Vec<Span<'static>> {
    let mut highlighted = Vec::new();
    let mut column = 0_usize;
    for span in spans {
        let base_style = span.style;
        let selected_style = details_text_selection_style(base_style);
        for ch in span.content.as_ref().chars() {
            let width = display_width_char(ch);
            let next_column = column.saturating_add(width);
            let selected = next_column > selection_start
                && column < selection_end
                && !selection_highlight_excluded(exclusions, column, next_column);
            push_span_text(
                &mut highlighted,
                ch.to_string(),
                if selected { selected_style } else { base_style },
            );
            column = next_column;
        }
    }
    highlighted
}

fn selection_highlight_excluded(
    exclusions: &[CopyExclusionRegion],
    start: usize,
    end: usize,
) -> bool {
    exclusions
        .iter()
        .any(|region| end > usize::from(region.start) && start < usize::from(region.end))
}

fn details_text_selection_style(base: Style) -> Style {
    base.fg(active_theme().highlight_fg)
        .bg(active_theme().highlight_bg)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn details_title() -> &'static str {
    "Details"
}

pub(super) fn focus_panel_title(_label: &str, title: &str, focused: bool) -> String {
    if focused {
        format!("[Focus] {title}")
    } else {
        title.to_string()
    }
}

pub(super) fn draw_footer(frame: &mut Frame<'_>, app: &AppState, paths: &Paths, area: Rect) {
    let footer = Paragraph::new(footer_line_for_width(app, paths, usize::from(area.width)))
        .style(active_theme().muted());
    frame.render_widget(footer, area);
}

pub(super) fn draw_top_status(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let Some(status_area) = top_status_area(area) else {
        return;
    };
    let line = top_status_line(app, usize::from(status_area.width));
    frame.render_widget(Clear, status_area);
    frame.render_widget(
        Paragraph::new(line)
            .style(active_theme().panel())
            .alignment(Alignment::Right),
        status_area,
    );
}

pub(super) fn top_status_area(area: Rect) -> Option<Rect> {
    let max_width = area.width.saturating_sub(4).min(46);
    if max_width < 12 || area.height == 0 {
        return None;
    }

    Some(Rect::new(
        area.x + area.width.saturating_sub(max_width).saturating_sub(1),
        area.y,
        max_width,
        1,
    ))
}

pub(super) fn top_status_line(app: &AppState, width: usize) -> Line<'static> {
    let value_width = width.saturating_sub(display_width("status: "));
    let value = truncate_inline(&footer_status(app), value_width);
    let mut spans = Vec::new();
    push_status_spans(&mut spans, value);
    Line::from(fit_footer_spans_to_width(spans, width))
}

#[cfg(test)]
pub(super) fn footer_line(app: &AppState, paths: &Paths) -> Line<'static> {
    footer_line_for_width(app, paths, usize::MAX)
}

pub(super) fn footer_line_for_width(app: &AppState, _paths: &Paths, width: usize) -> Line<'static> {
    Line::from(fit_footer_groups(footer_groups(app), width))
}

pub(super) fn footer_groups(app: &AppState) -> Vec<Vec<Span<'static>>> {
    let search = if app.comment_search_active {
        Some(format!("comment-search: /{}_", app.comment_search_query))
    } else if app.focus == FocusTarget::Details
        && app.details_mode == DetailsMode::Conversation
        && !app.comment_search_query.is_empty()
    {
        Some(format!("comment-search: /{}", app.comment_search_query))
    } else if app.global_search_active {
        Some(
            app.global_search_dialog
                .as_ref()
                .map(|dialog| format!("repo-search: {}", dialog.field.label()))
                .unwrap_or_else(|| "repo-search".to_string()),
        )
    } else if app.global_search_running {
        Some("repo search running".to_string())
    } else if app.filter_input_active {
        Some(format!("filter: f{}_", app.filter_input_query))
    } else if app.search_active {
        Some(format!("local-search: /{}_", app.search_query))
    } else if app.search_query.is_empty() {
        None
    } else {
        Some(format!("local-search: /{}", app.search_query))
    };
    let active_filter = app.current_filter_label();
    let (mouse, text_selection_state) = footer_mouse_shortcut(app);

    let mut groups = Vec::new();

    groups.push(footer_focus_primary_shortcuts(app));

    if let Some(search) = search {
        let mut spans = Vec::new();
        push_footer_state(&mut spans, "search", search, Color::Yellow);
        groups.push(spans);
    }
    if let Some(active_filter) = active_filter {
        let mut spans = Vec::new();
        push_footer_state(&mut spans, "filter", active_filter, Color::LightCyan);
        groups.push(spans);
    }
    if let Some(text_selection_state) = text_selection_state {
        let mut spans = Vec::new();
        push_footer_state(&mut spans, "mode", text_selection_state, Color::LightBlue);
        groups.push(spans);
    }

    groups.push(footer_global_primary_shortcuts(app));
    if let Some(mouse) = mouse {
        let mut spans = Vec::new();
        push_footer_pair(&mut spans, "m", mouse, Color::LightBlue);
        groups.push(spans);
    }

    groups
        .into_iter()
        .filter(|group| !group.is_empty())
        .collect()
}

pub(super) fn footer_status(app: &AppState) -> String {
    if let Some(loading) = &app.section_page_loading {
        return section_page_loading_status(loading);
    }
    if app.refreshing {
        return "refreshing".to_string();
    }
    let age = refresh_age_status(app);
    if app.status.is_empty() {
        age
    } else if age.is_empty() {
        app.status.clone()
    } else {
        format!("{} · {}", app.status, age)
    }
}

pub(super) fn refresh_age_status(app: &AppState) -> String {
    if app.focus == FocusTarget::Details
        && app.details_mode == DetailsMode::Conversation
        && let Some(item) = app.current_item()
        && let Some(refreshed_at) = app.details_refreshed_at.get(&item.id)
    {
        return format!("details refreshed {}", time_ago(*refreshed_at));
    }

    let Some(refreshed_at) = app.current_section().and_then(|s| s.refreshed_at) else {
        return String::new();
    };
    format!("list refreshed {}", time_ago(refreshed_at))
}

pub(super) fn time_ago(value: DateTime<Utc>) -> String {
    let delta = Utc::now().signed_duration_since(value);
    if delta.num_minutes() < 1 {
        format!("{}s ago", delta.num_seconds())
    } else if delta.num_hours() < 1 {
        let mins = delta.num_minutes();
        let secs = delta.num_seconds() % 60;
        format!("{mins}m{secs}s ago")
    } else if delta.num_days() < 1 {
        let hours = delta.num_hours();
        let mins = delta.num_minutes() % 60;
        format!("{hours}h{mins}m ago")
    } else {
        let days = delta.num_days();
        let hours = delta.num_hours() % 24;
        format!("{days}d{hours}h ago")
    }
}

pub(super) fn footer_mouse_shortcut(
    app: &AppState,
) -> (Option<&'static str>, Option<&'static str>) {
    if !app.mouse_capture_enabled {
        return (Some("restore mouse"), Some("text-select: drag copy"));
    }
    if app.details_mode == DetailsMode::Diff {
        return (None, None);
    }
    (Some("text-select"), None)
}

pub(super) fn footer_has_selected_comment(app: &AppState) -> bool {
    app.details_mode == DetailsMode::Conversation
        && app
            .current_selected_comment()
            .is_some_and(|comment| !comment.kind.is_activity())
}

pub(super) fn footer_selected_comment_is_editable(app: &AppState) -> bool {
    app.details_mode == DetailsMode::Conversation
        && app
            .current_selected_comment()
            .is_some_and(|comment| comment.is_mine && comment.id.is_some())
}

pub(super) fn footer_focus_primary_shortcuts(app: &AppState) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if let Some(dialog) = &app.pr_action_dialog {
        push_footer_pair(&mut spans, "y/enter", "run", Color::Yellow);
        push_footer_pair(&mut spans, "esc", "cancel", Color::Cyan);
        if dialog.action == PrAction::Merge {
            push_footer_pair(&mut spans, "m/s/r", "method", Color::LightMagenta);
        }
        return spans;
    }

    match app.focus {
        FocusTarget::Ghr => {
            push_footer_pair(&mut spans, "tab/h/l/[ ]", "switch", Color::Cyan);
            push_footer_pair(&mut spans, "j/n/enter", "Sections", Color::Cyan);
            push_footer_pair(&mut spans, "esc", "List", Color::Cyan);
        }
        FocusTarget::Sections => {
            push_footer_pair(&mut spans, "tab/h/l/[ ]", "switch", Color::Cyan);
            push_footer_pair(&mut spans, "k/p", "ghr", Color::Cyan);
            push_footer_pair(&mut spans, "j/n/enter", "List", Color::Cyan);
            push_footer_pair(&mut spans, "esc", "List", Color::Cyan);
        }
        FocusTarget::List => {
            if app.details_mode == DetailsMode::Diff {
                push_footer_pair(&mut spans, "j/k", "file", Color::Cyan);
                push_footer_pair(&mut spans, "tab", "diff", Color::Cyan);
                push_footer_pair(&mut spans, "enter", "diff", Color::Cyan);
                push_footer_pair(&mut spans, "esc", "back", Color::Cyan);
                push_footer_pair(&mut spans, "[ ]", "file", Color::Cyan);
                push_footer_pair(&mut spans, "i", "comments", Color::Yellow);
                push_footer_pair(&mut spans, "c", "inline", Color::LightBlue);
                push_footer_pair(&mut spans, "a", "comment", Color::LightBlue);
            } else {
                push_footer_pair(&mut spans, "j/k/n/p", "move", Color::Cyan);
                push_footer_pair(&mut spans, "[ ]", "page", Color::Cyan);
                push_footer_pair(&mut spans, "tab", "Details", Color::Cyan);
                push_footer_pair(&mut spans, "enter", "Details", Color::Cyan);
                push_footer_pair(&mut spans, "/", "search", Color::Yellow);
                if app.is_global_search_results_view() {
                    push_footer_pair(&mut spans, "esc", "back", Color::Cyan);
                }
                push_footer_pair(&mut spans, "v", "diff", Color::LightMagenta);
                push_footer_pair(&mut spans, "i", "ignore", Color::LightRed);
                push_footer_pair(&mut spans, "a", "comment", Color::LightBlue);
            }
        }
        FocusTarget::Details => {
            if app.details_mode == DetailsMode::Diff {
                push_footer_pair(&mut spans, "j/k", "line", Color::Cyan);
                push_footer_pair(&mut spans, "tab", "files", Color::Cyan);
                push_footer_pair(&mut spans, "n/p", "comment", Color::LightBlue);
                push_footer_pair(&mut spans, "h/l", "page", Color::Cyan);
                push_footer_pair(&mut spans, "i", "comments", Color::Yellow);
                push_footer_pair(&mut spans, "c", "inline", Color::LightBlue);
                push_footer_pair(&mut spans, "a", "comment", Color::LightBlue);
            } else {
                push_footer_pair(&mut spans, "j/k", "scroll", Color::Cyan);
                push_footer_pair(&mut spans, "tab", "List", Color::Cyan);
                push_footer_pair(&mut spans, "v", "diff", Color::LightMagenta);
                push_footer_pair(&mut spans, "/", "search", Color::Yellow);
                push_footer_pair(&mut spans, "c/a", "comment", Color::LightBlue);
                if footer_has_selected_comment(app) {
                    push_footer_pair(&mut spans, "n/p", "comment", Color::LightBlue);
                    push_footer_pair(&mut spans, "enter", "expand", Color::Yellow);
                }
                if footer_has_selected_comment(app) {
                    push_footer_pair(&mut spans, "R", "reply", Color::LightBlue);
                }
                if footer_selected_comment_is_editable(app) {
                    push_footer_pair(&mut spans, "e", "edit", Color::LightBlue);
                }
            }
            let esc_action = if app.details_mode == DetailsMode::Diff {
                "back"
            } else {
                "List"
            };
            push_footer_pair(&mut spans, "esc", esc_action, Color::Cyan);
        }
    }
    spans
}

pub(super) fn footer_global_primary_shortcuts(app: &AppState) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    push_footer_pair(&mut spans, "?", "help", Color::Yellow);
    push_footer_pair(
        &mut spans,
        app.command_palette_key.clone(),
        "cmd",
        Color::Yellow,
    );
    push_footer_pair(&mut spans, "f", "filter", Color::LightCyan);
    push_footer_pair(&mut spans, "r", "refresh", Color::Yellow);
    let q_action = if app.details_mode == DetailsMode::Diff {
        "back"
    } else {
        "quit"
    };
    push_footer_pair(&mut spans, "q", q_action, Color::Yellow);
    push_footer_pair(&mut spans, "o", "open", Color::Yellow);
    spans
}

pub(super) fn fit_footer_groups(
    groups: Vec<Vec<Span<'static>>>,
    width: usize,
) -> Vec<Span<'static>> {
    if width == usize::MAX {
        let mut spans = Vec::new();
        for group in groups {
            push_footer_group(&mut spans, group);
        }
        return spans;
    }

    let mut spans = Vec::new();
    let mut omitted = false;
    for group in groups {
        if group.is_empty() {
            continue;
        }

        let separator_width = if spans.is_empty() {
            0
        } else {
            display_width(" | ")
        };
        let group_width = footer_spans_width(&group);
        let next_width = footer_spans_width(&spans)
            .saturating_add(separator_width)
            .saturating_add(group_width);
        if next_width <= width {
            push_footer_group(&mut spans, group);
        } else if spans.is_empty() {
            spans.extend(fit_footer_spans_to_width(group, width));
            omitted = true;
        } else {
            omitted = true;
        }
    }

    if omitted {
        append_footer_more_marker(&mut spans, width);
    }
    spans
}

pub(super) fn push_footer_group(spans: &mut Vec<Span<'static>>, group: Vec<Span<'static>>) {
    if group.is_empty() {
        return;
    }
    if !spans.is_empty() {
        push_footer_separator(spans);
    }
    spans.extend(group);
}

pub(super) fn append_footer_more_marker(spans: &mut Vec<Span<'static>>, width: usize) {
    let marker = vec![
        Span::styled("...", active_theme().subtle()),
        Span::raw(" "),
        Span::styled(
            "?",
            active_theme()
                .panel()
                .fg(active_theme().warning)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled("help", active_theme().muted()),
    ];
    let separator_width = if spans.is_empty() {
        0
    } else {
        display_width(" | ")
    };
    let next_width = footer_spans_width(spans)
        .saturating_add(separator_width)
        .saturating_add(footer_spans_width(&marker));
    if next_width <= width {
        push_footer_group(spans, marker);
    }
}

pub(super) fn fit_footer_spans_to_width(
    spans: Vec<Span<'static>>,
    width: usize,
) -> Vec<Span<'static>> {
    let mut fitted = Vec::new();
    let mut used = 0_usize;
    for span in spans {
        let content = span.content.as_ref();
        let span_width = display_width(content);
        if used.saturating_add(span_width) <= width {
            used = used.saturating_add(span_width);
            fitted.push(span);
            continue;
        }

        let remaining = width.saturating_sub(used);
        if remaining > 0 {
            fitted.push(Span::styled(
                truncate_inline(content, remaining),
                span.style,
            ));
        }
        break;
    }
    fitted
}

pub(super) fn footer_spans_width(spans: &[Span<'static>]) -> usize {
    spans
        .iter()
        .map(|span| display_width(span.content.as_ref()))
        .sum()
}

pub(super) fn push_footer_separator(spans: &mut Vec<Span<'static>>) {
    spans.push(Span::styled(" | ", active_theme().subtle()));
}

pub(super) fn push_status_spans(spans: &mut Vec<Span<'static>>, value: impl Into<String>) {
    if !spans.is_empty() && !footer_ends_with_separator(spans) {
        spans.push(Span::raw("  "));
    }
    spans.push(Span::styled(
        "status:",
        active_theme()
            .panel()
            .fg(active_theme().focus)
            .add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        value.into(),
        active_theme().panel().fg(themed_status_value_color()),
    ));
}

pub(super) fn push_footer_pair(
    spans: &mut Vec<Span<'static>>,
    key: impl Into<String>,
    label: impl Into<String>,
    key_color: Color,
) {
    let key_color = themed_hint_color(key_color);
    if !spans.is_empty() && !footer_ends_with_separator(spans) {
        spans.push(Span::raw("  "));
    }
    spans.push(Span::styled(
        key.into(),
        active_theme()
            .panel()
            .fg(key_color)
            .add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(label.into(), active_theme().muted()));
}

pub(super) fn push_footer_state(
    spans: &mut Vec<Span<'static>>,
    key: &'static str,
    value: impl Into<String>,
    value_color: Color,
) {
    let value_color = themed_hint_color(value_color);
    if !spans.is_empty() && !footer_ends_with_separator(spans) {
        spans.push(Span::raw("  "));
    }
    spans.push(Span::styled(
        key,
        active_theme()
            .panel()
            .fg(active_theme().subtle)
            .add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        value.into(),
        active_theme().panel().fg(value_color),
    ));
}

pub(super) fn themed_hint_color(color: Color) -> Color {
    let theme = active_theme();
    match color {
        Color::White => theme.text,
        Color::Gray => theme.muted,
        Color::DarkGray => theme.subtle,
        Color::Cyan | Color::LightCyan => theme.focus,
        Color::LightBlue => theme.link,
        Color::LightMagenta => theme.action,
        Color::Yellow | Color::LightYellow => theme.warning,
        Color::LightRed | Color::Red => theme.error,
        Color::Green | Color::LightGreen => theme.success,
        _ => color,
    }
}

pub(super) fn themed_fg_style(color: Color) -> Style {
    active_theme().panel().fg(themed_hint_color(color))
}

pub(super) fn themed_bold_style(color: Color) -> Style {
    themed_fg_style(color).add_modifier(Modifier::BOLD)
}

pub(super) fn themed_status_value_color() -> Color {
    if active_theme().background == Color::Reset {
        Color::Green
    } else {
        active_theme().success
    }
}

pub(super) fn footer_ends_with_separator(spans: &[Span<'static>]) -> bool {
    spans
        .last()
        .map(|span| span.content.as_ref() == " | ")
        .unwrap_or(false)
}
