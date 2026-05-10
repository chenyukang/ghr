use super::*;

pub(super) fn modal_surface_style() -> Style {
    active_theme().panel()
}

pub(super) fn modal_text_style() -> Style {
    active_theme().panel()
}

pub(super) fn modal_footer_style() -> Style {
    active_theme()
        .panel()
        .fg(active_theme().warning)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn modal_footer_line(text: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(text.into(), modal_footer_style()))
}

pub(super) fn draw_modal_footer(
    frame: &mut Frame<'_>,
    area: Rect,
    dialog_area: Rect,
    footer: Line<'static>,
) {
    let Some(footer_area) = modal_footer_area(area, dialog_area) else {
        return;
    };
    let paragraph = Paragraph::new(footer)
        .alignment(Alignment::Center)
        .style(modal_text_style());
    frame.render_widget(Clear, footer_area);
    frame.render_widget(paragraph, footer_area);
}

pub(super) fn modal_footer_area(area: Rect, dialog_area: Rect) -> Option<Rect> {
    if dialog_area.width == 0 || dialog_area.height == 0 {
        return None;
    }
    let y = dialog_area.y.saturating_add(dialog_area.height);
    (y < area.y.saturating_add(area.height)).then_some(Rect::new(
        dialog_area.x,
        y,
        dialog_area.width,
        1,
    ))
}

pub(super) fn draw_startup_dialog(
    frame: &mut Frame<'_>,
    app: &AppState,
    paths: &Paths,
    dialog: StartupDialog,
    area: Rect,
) {
    let elapsed_secs = app.last_refresh_request.elapsed().as_secs();
    let (title, lines, show_ok) = startup_dialog_content(dialog, app, paths, elapsed_secs);
    let dialog_area = startup_dialog_area(dialog, area);
    let accent = match dialog {
        StartupDialog::Initializing => Color::Cyan,
        StartupDialog::Ready => Color::LightGreen,
    };
    let accent = themed_hint_color(accent);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(active_theme().panel().fg(accent))
        .style(modal_surface_style())
        .title(Span::styled(
            title,
            active_theme()
                .panel()
                .fg(accent)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);

    if show_ok {
        let ok = Paragraph::new("[ OK ]").alignment(Alignment::Center).style(
            active_theme()
                .panel()
                .fg(active_theme().highlight_fg)
                .bg(active_theme().success)
                .add_modifier(Modifier::BOLD),
        );
        frame.render_widget(ok, startup_dialog_ok_area(dialog_area));
    }
}

pub(super) fn startup_dialog_area(dialog: StartupDialog, area: Rect) -> Rect {
    let height = match dialog {
        StartupDialog::Initializing => 11,
        StartupDialog::Ready => 13,
    };
    centered_rect(72, height, area)
}

pub(super) fn startup_dialog_ok_area(dialog_area: Rect) -> Rect {
    let width = 8.min(dialog_area.width.saturating_sub(4)).max(1);
    let x = dialog_area.x + dialog_area.width.saturating_sub(width) / 2;
    let y = dialog_area.y + dialog_area.height.saturating_sub(2);
    Rect::new(x, y, width, 1)
}

pub(super) fn startup_dialog_content(
    dialog: StartupDialog,
    app: &AppState,
    paths: &Paths,
    elapsed_secs: u64,
) -> (&'static str, Vec<Line<'static>>, bool) {
    match dialog {
        StartupDialog::Initializing => (
            "Initializing",
            vec![
                Line::from("ghr is preparing your GitHub workspace for the first time."),
                Line::from(""),
                startup_loading_line(elapsed_secs),
                startup_progress_line(elapsed_secs),
                Line::from(""),
                key_value_line("config.toml", paths.config_path.display().to_string()),
                key_value_line("database", paths.db_path.display().to_string()),
                Line::from(""),
                Line::from("Loading cache and refreshing remote data. Please wait ..."),
            ],
            false,
        ),
        StartupDialog::Ready => (
            "Ready",
            vec![
                Line::from("ghr is ready."),
                Line::from(""),
                key_value_line("loaded", startup_loaded_summary(app)),
                key_value_line("config.toml", paths.config_path.display().to_string()),
                Line::from(""),
                Line::from(vec![
                    Span::raw("Press "),
                    Span::styled("?", themed_bold_style(Color::Yellow)),
                    Span::raw(" anytime for the shortcut reference."),
                ]),
                Line::from("Click OK or press Enter/Esc to close this dialog."),
            ],
            true,
        ),
    }
}

pub(super) fn startup_loading_line(elapsed_secs: u64) -> Line<'static> {
    let dots = match elapsed_secs % 4 {
        0 => "",
        1 => ".",
        2 => "..",
        _ => "...",
    };
    Line::from(vec![
        Span::styled(format!("Loading{dots:<3}"), themed_bold_style(Color::Cyan)),
        Span::styled(
            format!("  {elapsed_secs}s"),
            themed_fg_style(Color::DarkGray),
        ),
    ])
}

pub(super) fn startup_progress_line(elapsed_secs: u64) -> Line<'static> {
    const WIDTH: usize = 28;
    const FILL: usize = 9;

    let span = WIDTH.saturating_sub(FILL).max(1);
    let offset = (elapsed_secs as usize) % (span + 1);
    let mut bar = String::with_capacity(WIDTH + 2);
    bar.push('[');
    for index in 0..WIDTH {
        if index >= offset && index < offset + FILL {
            bar.push('=');
        } else {
            bar.push(' ');
        }
    }
    bar.push(']');

    Line::from(vec![
        Span::raw("  "),
        Span::styled(bar, themed_fg_style(Color::Cyan)),
    ])
}

pub(super) fn startup_loaded_summary(app: &AppState) -> String {
    let section_count = app.sections.len();
    let item_count = app
        .sections
        .iter()
        .map(|section| section.items.len())
        .sum::<usize>();
    let error_count = app
        .sections
        .iter()
        .filter(|section| section.error.is_some())
        .count();
    let mut summary = format!("{item_count} item(s) across {section_count} section(s)");
    if error_count > 0 {
        summary.push_str(&format!("; {error_count} refresh error(s)"));
    }
    summary
}

pub(super) fn draw_setup_dialog(frame: &mut Frame<'_>, dialog: SetupDialog, area: Rect) {
    let (title, lines) = setup_dialog_content(dialog);
    let dialog_area = centered_rect(90, 17, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(active_theme().panel().fg(active_theme().warning))
        .style(modal_surface_style())
        .title(Span::styled(
            title,
            active_theme()
                .panel()
                .fg(active_theme().warning)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
}

pub(super) fn draw_help_dialog(frame: &mut Frame<'_>, area: Rect, command_palette_key: &str) {
    let width = help_dialog_width(area);
    let lines = help_dialog_content_for_width(width.saturating_sub(2), command_palette_key);
    let height = help_dialog_height(lines.len(), area);
    let dialog_area = centered_rect_with_size(width, height, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(active_theme().panel().fg(active_theme().action))
        .style(modal_surface_style())
        .title(Span::styled(
            "Help",
            active_theme()
                .panel()
                .fg(active_theme().action)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
}

pub(super) fn help_dialog_width(area: Rect) -> u16 {
    let preferred = area
        .width
        .saturating_mul(HELP_DIALOG_WIDTH_PERCENT)
        .saturating_div(100);
    preferred
        .max(area.width.min(COMMENT_DIALOG_FALLBACK_EDITOR_WIDTH))
        .min(HELP_DIALOG_MAX_WIDTH.min(area.width))
}

pub(super) fn draw_command_palette(
    frame: &mut Frame<'_>,
    app: &AppState,
    palette: &CommandPalette,
    area: Rect,
    command_palette_key: &str,
) {
    let commands = command_palette_commands(command_palette_key);
    let matches = app.command_palette_match_indices(&commands, &palette.query);
    let dialog_area = command_palette_area(area);
    let inner = block_inner(dialog_area);
    let result_height = usize::from(inner.height.saturating_sub(2));
    let selected = palette.selected.min(matches.len().saturating_sub(1));
    let start = command_palette_visible_start(selected, matches.len(), result_height);
    let width = usize::from(inner.width.max(1));

    let mut lines = Vec::new();
    lines.push(command_palette_input_line(&palette.query, width));
    lines.push(Line::from(""));

    if matches.is_empty() {
        lines.push(Line::from(Span::styled(
            "No commands found",
            active_theme().subtle(),
        )));
    } else {
        for (position, command_index) in matches.iter().enumerate().skip(start).take(result_height)
        {
            let command = &commands[*command_index];
            lines.push(command_palette_result_line(
                command,
                position == selected,
                width,
            ));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(active_theme().panel().fg(active_theme().focus))
        .style(modal_surface_style())
        .title(Span::styled(
            "Command Palette",
            active_theme()
                .panel()
                .fg(active_theme().focus)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    draw_modal_footer(
        frame,
        area,
        dialog_area,
        modal_footer_line("Enter: run    Esc: close    Up/Down: select"),
    );

    let cursor_column =
        display_width(&palette.query).min(usize::from(inner.width.saturating_sub(3)));
    frame.set_cursor_position(Position::new(
        inner
            .x
            .saturating_add(2)
            .saturating_add(cursor_column as u16),
        inner.y,
    ));
}

pub(super) fn draw_project_switcher(
    frame: &mut Frame<'_>,
    app: &AppState,
    switcher: &ProjectSwitcher,
    area: Rect,
) {
    let candidates = app.project_switcher_candidates_for_query(&switcher.query);
    let dialog_area = project_switcher_area(area);
    let inner = block_inner(dialog_area);
    let result_height = usize::from(inner.height.saturating_sub(2));
    let selected = switcher.selected.min(candidates.len().saturating_sub(1));
    let start = command_palette_visible_start(selected, candidates.len(), result_height);
    let width = usize::from(inner.width.max(1));

    let mut lines = Vec::new();
    lines.push(project_switcher_input_line(&switcher.query, width));
    lines.push(Line::from(""));

    if candidates.is_empty() {
        lines.push(Line::from(Span::styled(
            "No projects found",
            active_theme().subtle(),
        )));
    } else {
        for (position, candidate) in candidates
            .iter()
            .enumerate()
            .skip(start)
            .take(result_height)
        {
            lines.push(project_switcher_candidate_line(
                candidate,
                candidate.key == app.active_view,
                position == selected,
                width,
            ));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(active_theme().panel().fg(active_theme().focus))
        .style(modal_surface_style())
        .title(Span::styled(
            "Project Switch",
            active_theme()
                .panel()
                .fg(active_theme().focus)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    draw_modal_footer(
        frame,
        area,
        dialog_area,
        modal_footer_line("Enter: switch    Esc: close    Up/Down: select"),
    );

    let cursor_column =
        display_width(&switcher.query).min(usize::from(inner.width.saturating_sub(3)));
    frame.set_cursor_position(Position::new(
        inner
            .x
            .saturating_add(2)
            .saturating_add(cursor_column as u16),
        inner.y,
    ));
}

pub(super) fn draw_top_menu_switcher(
    frame: &mut Frame<'_>,
    app: &AppState,
    switcher: &TopMenuSwitcher,
    area: Rect,
) {
    let candidates = app.top_menu_switcher_candidates_for_query(&switcher.query);
    let dialog_area = project_switcher_area(area);
    let inner = block_inner(dialog_area);
    let result_height = usize::from(inner.height.saturating_sub(2));
    let selected = switcher.selected.min(candidates.len().saturating_sub(1));
    let start = command_palette_visible_start(selected, candidates.len(), result_height);
    let width = usize::from(inner.width.max(1));

    let mut lines = Vec::new();
    lines.push(top_menu_switcher_input_line(&switcher.query, width));
    lines.push(Line::from(""));

    if candidates.is_empty() {
        lines.push(Line::from(Span::styled(
            "No top menu items found",
            active_theme().subtle(),
        )));
    } else {
        for (position, candidate) in candidates
            .iter()
            .enumerate()
            .skip(start)
            .take(result_height)
        {
            lines.push(project_switcher_candidate_line(
                candidate,
                candidate.key == app.active_view,
                position == selected,
                width,
            ));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(active_theme().panel().fg(active_theme().focus))
        .style(modal_surface_style())
        .title(Span::styled(
            "Top Menu Switch",
            active_theme()
                .panel()
                .fg(active_theme().focus)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    draw_modal_footer(
        frame,
        area,
        dialog_area,
        modal_footer_line("Enter: switch    Esc: close    Up/Down: select"),
    );

    let cursor_column =
        display_width(&switcher.query).min(usize::from(inner.width.saturating_sub(3)));
    frame.set_cursor_position(Position::new(
        inner
            .x
            .saturating_add(2)
            .saturating_add(cursor_column as u16),
        inner.y,
    ));
}

pub(super) fn draw_theme_switcher(
    frame: &mut Frame<'_>,
    app: &AppState,
    switcher: &ThemeSwitcher,
    area: Rect,
) {
    let candidates = app.theme_switcher_candidates_for_query(&switcher.query);
    let dialog_area = theme_switcher_area(area);
    let inner = block_inner(dialog_area);
    let result_height = usize::from(inner.height.saturating_sub(2));
    let selected = switcher.selected.min(candidates.len().saturating_sub(1));
    let start = command_palette_visible_start(selected, candidates.len(), result_height);
    let width = usize::from(inner.width.max(1));

    let mut lines = Vec::new();
    lines.push(theme_switcher_input_line(&switcher.query, width));
    lines.push(Line::from(""));

    if candidates.is_empty() {
        lines.push(Line::from(Span::styled(
            "No color themes found",
            active_theme().subtle(),
        )));
    } else {
        for (position, candidate) in candidates
            .iter()
            .enumerate()
            .skip(start)
            .take(result_height)
        {
            lines.push(theme_switcher_candidate_line(
                candidate,
                candidate.choice == switcher.current,
                position == selected,
                width,
            ));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(active_theme().panel().fg(active_theme().focus))
        .style(modal_surface_style())
        .title(Span::styled(
            "Set Color Theme",
            active_theme()
                .panel()
                .fg(active_theme().focus)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    draw_modal_footer(
        frame,
        area,
        dialog_area,
        modal_footer_line("Enter: set    Esc: close    Up/Down: select"),
    );

    let cursor_column =
        display_width(&switcher.query).min(usize::from(inner.width.saturating_sub(3)));
    frame.set_cursor_position(Position::new(
        inner
            .x
            .saturating_add(2)
            .saturating_add(cursor_column as u16),
        inner.y,
    ));
}

pub(super) fn project_switcher_area(area: Rect) -> Rect {
    let width = centered_rect_width(52, area).max(32).min(area.width);
    let max_height = area.height.saturating_sub(2).max(3);
    let height = 14.min(max_height).max(3);
    centered_rect_with_size(width, height, area)
}

pub(super) fn theme_switcher_area(area: Rect) -> Rect {
    let width = centered_rect_width(62, area).max(36).min(area.width);
    let max_height = area.height.saturating_sub(2).max(3);
    let height = 20.min(max_height).max(3);
    centered_rect_with_size(width, height, area)
}

pub(super) fn project_switcher_input_line(query: &str, width: usize) -> Line<'static> {
    if query.is_empty() {
        return Line::from(vec![
            Span::styled("> ", active_theme().panel().fg(active_theme().focus)),
            Span::styled("Type a project prefix", active_theme().subtle()),
        ]);
    }

    Line::from(vec![
        Span::styled("> ", active_theme().panel().fg(active_theme().focus)),
        Span::styled(
            truncate_inline(query, width.saturating_sub(2)),
            active_theme().panel(),
        ),
    ])
}

pub(super) fn top_menu_switcher_input_line(query: &str, width: usize) -> Line<'static> {
    if query.is_empty() {
        return Line::from(vec![
            Span::styled("> ", active_theme().panel().fg(active_theme().focus)),
            Span::styled("Type a top menu label", active_theme().subtle()),
        ]);
    }

    Line::from(vec![
        Span::styled("> ", active_theme().panel().fg(active_theme().focus)),
        Span::styled(
            truncate_inline(query, width.saturating_sub(2)),
            active_theme().panel(),
        ),
    ])
}

pub(super) fn theme_switcher_input_line(query: &str, width: usize) -> Line<'static> {
    if query.is_empty() {
        return Line::from(vec![
            Span::styled("> ", active_theme().panel().fg(active_theme().focus)),
            Span::styled("Type a color theme", active_theme().subtle()),
        ]);
    }

    Line::from(vec![
        Span::styled("> ", active_theme().panel().fg(active_theme().focus)),
        Span::styled(
            truncate_inline(query, width.saturating_sub(2)),
            active_theme().panel(),
        ),
    ])
}

pub(super) fn project_switcher_candidate_line(
    candidate: &ViewTab,
    current: bool,
    selected: bool,
    width: usize,
) -> Line<'static> {
    let marker = if selected { "> " } else { "  " };
    let current_label = if current { "  current" } else { "" };
    let text = truncate_inline(
        &format!("{marker}{}{current_label}", candidate.label),
        width,
    );
    let style = if selected {
        active_theme().active()
    } else if current {
        active_theme()
            .panel()
            .fg(active_theme().focus)
            .add_modifier(Modifier::BOLD)
    } else {
        active_theme().panel()
    };
    Line::from(Span::styled(text, style))
}

pub(super) fn theme_switcher_candidate_line(
    candidate: &ThemeCandidate,
    current: bool,
    selected: bool,
    width: usize,
) -> Line<'static> {
    let marker = if selected { "> " } else { "  " };
    let current_label = if current { "  current" } else { "" };
    let text = truncate_inline(
        &format!(
            "{marker}{:<22} {}{current_label}",
            candidate.label, candidate.detail
        ),
        width,
    );
    let style = if selected {
        active_theme().active()
    } else if current {
        active_theme()
            .panel()
            .fg(active_theme().focus)
            .add_modifier(Modifier::BOLD)
    } else {
        active_theme().panel()
    };
    Line::from(Span::styled(text, style))
}

pub(super) fn draw_recent_items_dialog(
    frame: &mut Frame<'_>,
    app: &AppState,
    dialog: &RecentItemsDialog,
    area: Rect,
) {
    let candidates = app.recent_item_candidates_for_query(&dialog.query);
    let dialog_area = recent_items_area(area);
    let inner = block_inner(dialog_area);
    let result_height = usize::from(inner.height.saturating_sub(2));
    let selected = dialog.selected.min(candidates.len().saturating_sub(1));
    let start = command_palette_visible_start(selected, candidates.len(), result_height);
    let width = usize::from(inner.width.max(1));

    let mut lines = Vec::new();
    lines.push(recent_items_input_line(&dialog.query, width));
    lines.push(Line::from(""));

    if candidates.is_empty() {
        let message =
            if app.recent_items.is_empty() || app.recent_item_candidates_for_query("").is_empty() {
                "No other recent PRs or issues yet"
            } else {
                "No recent items found"
            };
        lines.push(Line::from(Span::styled(message, active_theme().subtle())));
    } else {
        for (position, candidate) in candidates
            .iter()
            .enumerate()
            .skip(start)
            .take(result_height)
        {
            lines.push(recent_item_candidate_line(
                candidate,
                position == selected,
                width,
            ));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(active_theme().panel().fg(active_theme().focus))
        .style(modal_surface_style())
        .title(Span::styled(
            "Recent Items",
            active_theme()
                .panel()
                .fg(active_theme().focus)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    draw_modal_footer(
        frame,
        area,
        dialog_area,
        modal_footer_line("Enter: jump    Esc: close    Up/Down: select"),
    );

    let cursor_column =
        display_width(&dialog.query).min(usize::from(inner.width.saturating_sub(3)));
    frame.set_cursor_position(Position::new(
        inner
            .x
            .saturating_add(2)
            .saturating_add(cursor_column as u16),
        inner.y,
    ));
}

pub(super) fn recent_items_area(area: Rect) -> Rect {
    let width = centered_rect_width(90, area).max(40).min(area.width);
    let max_height = area.height.saturating_sub(2).max(3);
    let height = 18.min(max_height).max(3);
    centered_rect_with_size(width, height, area)
}

pub(super) fn recent_items_input_line(query: &str, width: usize) -> Line<'static> {
    if query.is_empty() {
        return Line::from(vec![
            Span::styled("> ", active_theme().panel().fg(active_theme().focus)),
            Span::styled(
                "Type to search recent PRs and issues",
                active_theme().subtle(),
            ),
        ]);
    }

    Line::from(vec![
        Span::styled("> ", active_theme().panel().fg(active_theme().focus)),
        Span::styled(
            truncate_inline(query, width.saturating_sub(2)),
            active_theme().panel(),
        ),
    ])
}

pub(super) fn recent_item_candidate_line(
    item: &RecentItem,
    selected: bool,
    width: usize,
) -> Line<'static> {
    let marker = if selected { "> " } else { "  " };
    let text = truncate_inline(&format!("{marker}{}", recent_item_label(item)), width);
    let style = if selected {
        active_theme().active()
    } else {
        active_theme().panel()
    };
    Line::from(Span::styled(text, style))
}

pub(super) fn draw_saved_search_dialog(
    frame: &mut Frame<'_>,
    app: &AppState,
    dialog: &SavedSearchDialog,
    area: Rect,
) {
    let candidates = app.saved_search_candidates_for_query(&dialog.query);
    let dialog_area = saved_search_area(area);
    let inner = block_inner(dialog_area);
    let result_height = usize::from(inner.height.saturating_sub(2));
    let selected = dialog.selected.min(candidates.len().saturating_sub(1));
    let start = command_palette_visible_start(selected, candidates.len(), result_height);
    let width = usize::from(inner.width.max(1));

    let mut lines = Vec::new();
    lines.push(saved_search_input_line(&dialog.query, width));
    lines.push(Line::from(""));

    if candidates.is_empty() {
        lines.push(Line::from(Span::styled(
            "No saved search filters found",
            active_theme().subtle(),
        )));
    } else {
        for (position, candidate) in candidates
            .iter()
            .enumerate()
            .skip(start)
            .take(result_height)
        {
            lines.push(saved_search_candidate_line(
                candidate,
                position == selected,
                width,
            ));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(active_theme().panel().fg(active_theme().focus))
        .style(modal_surface_style())
        .title(Span::styled(
            "Saved Search Filter",
            active_theme()
                .panel()
                .fg(active_theme().focus)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    draw_modal_footer(
        frame,
        area,
        dialog_area,
        modal_footer_line("Enter: run    Esc: close    Up/Down: select"),
    );

    let cursor_column =
        display_width(&dialog.query).min(usize::from(inner.width.saturating_sub(3)));
    frame.set_cursor_position(Position::new(
        inner
            .x
            .saturating_add(2)
            .saturating_add(cursor_column as u16),
        inner.y,
    ));
}

pub(super) fn saved_search_area(area: Rect) -> Rect {
    let width = centered_rect_width(104, area).max(48).min(area.width);
    let max_height = area.height.saturating_sub(2).max(3);
    let height = 18.min(max_height).max(3);
    centered_rect_with_size(width, height, area)
}

pub(super) fn saved_search_input_line(query: &str, width: usize) -> Line<'static> {
    if query.is_empty() {
        return Line::from(vec![
            Span::styled("> ", active_theme().panel().fg(active_theme().focus)),
            Span::styled(
                "Type a saved search name, repo, or filter",
                active_theme().subtle(),
            ),
        ]);
    }

    Line::from(vec![
        Span::styled("> ", active_theme().panel().fg(active_theme().focus)),
        Span::styled(
            truncate_inline(query, width.saturating_sub(2)),
            active_theme().panel(),
        ),
    ])
}

pub(super) fn saved_search_candidate_line(
    candidate: &SavedSearchCandidate,
    selected: bool,
    width: usize,
) -> Line<'static> {
    let marker = if selected { "> " } else { "  " };
    let text = truncate_inline(
        &format!(
            "{marker}{:<24} {:<28} {:<5} {}",
            candidate.name,
            candidate.repo,
            saved_search_kind_label(candidate.kind),
            global_search_state_display(&candidate.search)
        ),
        width,
    );
    let style = if selected {
        active_theme().active()
    } else {
        active_theme().panel()
    };
    Line::from(Span::styled(text, style))
}

pub(super) fn draw_save_search_dialog(
    frame: &mut Frame<'_>,
    dialog: &SaveSearchDialog,
    area: Rect,
) {
    let dialog_area = centered_rect(80, 9, area);
    let inner = block_inner(dialog_area);
    let width = inner.width.max(1);
    let name_prefix = "Name: ";
    let name_width =
        width.saturating_sub(display_width(name_prefix).min(usize::from(u16::MAX)) as u16);
    let lines = vec![
        key_value_line("repo", dialog.repo.clone()),
        key_value_line("type", saved_search_kind_label(dialog.kind).to_string()),
        key_value_line("filter", global_search_state_display(&dialog.search)),
        Line::from(""),
        Line::from(vec![
            Span::styled(name_prefix, active_theme().panel().fg(active_theme().focus)),
            Span::styled(
                issue_dialog_input_text(dialog.name.text(), name_width),
                active_theme().panel(),
            ),
        ]),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(active_theme().panel().fg(active_theme().focus))
        .style(modal_surface_style())
        .title(Span::styled(
            "Save Search Filter",
            active_theme()
                .panel()
                .fg(active_theme().focus)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    draw_modal_footer(
        frame,
        area,
        dialog_area,
        modal_footer_line("Enter: save    Esc: cancel"),
    );

    let cursor =
        text_before_cursor_width(dialog.name.text(), dialog.name.cursor_byte()).min(name_width);
    frame.set_cursor_position(Position::new(
        inner
            .x
            .saturating_add(display_width(name_prefix).min(usize::from(u16::MAX)) as u16)
            .saturating_add(cursor)
            .min(inner.right().saturating_sub(1)),
        inner.y.saturating_add(4),
    ));
}

pub(super) fn draw_project_add_dialog(
    frame: &mut Frame<'_>,
    dialog: &ProjectAddDialog,
    area: Rect,
) {
    let dialog_area = project_add_dialog_area(area);
    let inner = block_inner(dialog_area);
    let editor_width = inner.width.max(1);
    let lines = vec![
        project_add_dialog_field_input_line(
            "Title",
            &dialog.title,
            ProjectAddField::Title,
            dialog.field,
            editor_width,
        ),
        issue_dialog_separator_line(editor_width),
        project_add_dialog_field_input_line(
            "Repo url",
            &dialog.repo_url,
            ProjectAddField::RepoUrl,
            dialog.field,
            editor_width,
        ),
        issue_dialog_separator_line(editor_width),
        project_add_dialog_field_input_line(
            "local_dir",
            &dialog.local_dir,
            ProjectAddField::LocalDir,
            dialog.field,
            editor_width,
        ),
        Line::from(""),
        Line::from(Span::styled(
            "Title and local_dir may be empty. Empty local_dir is saved as local_dir = \"\".",
            active_theme().subtle(),
        )),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(active_theme().panel().fg(active_theme().success))
        .style(modal_surface_style())
        .title(Span::styled(
            "Project Add",
            active_theme()
                .panel()
                .fg(active_theme().success)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    draw_modal_footer(
        frame,
        area,
        dialog_area,
        modal_footer_line("Tab/Enter: field    Ctrl+Enter: save    Esc: cancel"),
    );
    if let Some(position) = project_add_dialog_cursor_position(dialog, dialog_area) {
        frame.set_cursor_position(position);
    }
}

pub(super) fn project_add_dialog_area(area: Rect) -> Rect {
    centered_rect(78, 12, area)
}

pub(super) fn project_add_dialog_field_input_line(
    label: &'static str,
    value: &str,
    field: ProjectAddField,
    current: ProjectAddField,
    width: u16,
) -> Line<'static> {
    let prefix = issue_dialog_field_prefix(label);
    let value_width =
        width.saturating_sub(display_width(&prefix).min(usize::from(u16::MAX)) as u16);
    Line::from(vec![
        Span::styled(prefix, project_add_dialog_field_label_style(field, current)),
        Span::styled(
            issue_dialog_input_text(value, value_width),
            active_theme().panel(),
        ),
    ])
}

pub(super) fn project_add_dialog_field_label_style(
    field: ProjectAddField,
    current: ProjectAddField,
) -> Style {
    if field == current {
        active_theme()
            .panel()
            .fg(active_theme().focus)
            .add_modifier(Modifier::BOLD)
    } else {
        active_theme().muted()
    }
}

pub(super) fn project_add_dialog_cursor_position(
    dialog: &ProjectAddDialog,
    dialog_area: Rect,
) -> Option<Position> {
    let inner = block_inner(dialog_area);
    let clamp_x = |x: u16| x.min(inner.right().saturating_sub(1));
    let field_position = |label: &'static str, value: &str, y_offset: u16| {
        let prefix_width =
            display_width(&issue_dialog_field_prefix(label)).min(usize::from(u16::MAX)) as u16;
        Position::new(
            clamp_x(
                inner
                    .x
                    .saturating_add(prefix_width)
                    .saturating_add(display_width(value).min(usize::from(u16::MAX)) as u16),
            ),
            inner.y.saturating_add(y_offset),
        )
    };
    Some(match dialog.field {
        ProjectAddField::Title => field_position("Title", &dialog.title, 0),
        ProjectAddField::RepoUrl => field_position("Repo url", &dialog.repo_url, 2),
        ProjectAddField::LocalDir => field_position("local_dir", &dialog.local_dir, 4),
    })
}

pub(super) fn draw_project_remove_dialog(
    frame: &mut Frame<'_>,
    dialog: &ProjectRemoveDialog,
    area: Rect,
) {
    if let Some(candidate) = &dialog.confirm {
        draw_project_remove_confirmation(frame, candidate, area);
    } else {
        draw_project_remove_picker(frame, dialog, area);
    }
}

pub(super) fn draw_project_remove_picker(
    frame: &mut Frame<'_>,
    dialog: &ProjectRemoveDialog,
    area: Rect,
) {
    let candidates = project_remove_filtered_candidates(dialog);
    let dialog_area = project_remove_area(area);
    let inner = block_inner(dialog_area);
    let result_height = usize::from(inner.height.saturating_sub(2));
    let selected = dialog.selected.min(candidates.len().saturating_sub(1));
    let start = command_palette_visible_start(selected, candidates.len(), result_height);
    let width = usize::from(inner.width.max(1));

    let mut lines = Vec::new();
    lines.push(project_switcher_input_line(&dialog.query, width));
    lines.push(Line::from(""));

    if candidates.is_empty() {
        lines.push(Line::from(Span::styled(
            "No projects found",
            active_theme().subtle(),
        )));
    } else {
        for (position, candidate) in candidates
            .iter()
            .enumerate()
            .skip(start)
            .take(result_height)
        {
            lines.push(project_remove_candidate_line(
                candidate,
                position == selected,
                width,
            ));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(active_theme().panel().fg(active_theme().error))
        .style(modal_surface_style())
        .title(Span::styled(
            "Project Remove",
            active_theme()
                .panel()
                .fg(active_theme().error)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    draw_modal_footer(
        frame,
        area,
        dialog_area,
        modal_footer_line("Enter: choose    Esc: close    Up/Down: select"),
    );

    let cursor_column =
        display_width(&dialog.query).min(usize::from(inner.width.saturating_sub(3)));
    frame.set_cursor_position(Position::new(
        inner
            .x
            .saturating_add(2)
            .saturating_add(cursor_column as u16),
        inner.y,
    ));
}

pub(super) fn draw_project_remove_confirmation(
    frame: &mut Frame<'_>,
    candidate: &ProjectRemoveCandidate,
    area: Rect,
) {
    let dialog_area = project_remove_confirm_area(area);
    let local_dir = candidate.local_dir.as_deref().unwrap_or("(none)");
    let lines = vec![
        Line::from("Remove this project from config.toml?"),
        Line::from(""),
        key_value_line("project", candidate.name.clone()),
        key_value_line("repo", candidate.repo.clone()),
        key_value_line("local_dir", local_dir.to_string()),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(active_theme().panel().fg(active_theme().error))
        .style(modal_surface_style())
        .title(Span::styled(
            "Confirm Project Remove",
            active_theme()
                .panel()
                .fg(active_theme().error)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    draw_modal_footer(
        frame,
        area,
        dialog_area,
        modal_footer_line("y/Enter: remove project    Esc: cancel"),
    );
}

pub(super) fn draw_cache_clear_dialog(
    frame: &mut Frame<'_>,
    dialog: &CacheClearDialog,
    area: Rect,
) {
    if let Some(target) = dialog.confirm {
        return draw_cache_clear_confirmation(frame, target, area);
    }

    let dialog_area = cache_clear_area(area);
    let inner = block_inner(dialog_area);
    let selected = dialog
        .selected
        .min(CACHE_CLEAR_TARGETS.len().saturating_sub(1));
    let width = usize::from(inner.width.max(1));
    let mut lines = Vec::new();
    lines.push(Line::from("Choose which local cache layer to clear."));
    lines.push(Line::from(""));
    for (index, target) in CACHE_CLEAR_TARGETS.iter().copied().enumerate() {
        lines.push(cache_clear_target_line(target, index == selected, width));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Config, logs, and saved UI preferences are not touched.",
        active_theme().subtle(),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(active_theme().panel().fg(active_theme().error))
        .style(modal_surface_style())
        .title(Span::styled(
            "Clear Cache",
            active_theme()
                .panel()
                .fg(active_theme().error)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    draw_modal_footer(
        frame,
        area,
        dialog_area,
        modal_footer_line("Enter: choose    Esc: close    Up/Down: select"),
    );
}

pub(super) fn draw_cache_clear_confirmation(
    frame: &mut Frame<'_>,
    target: CacheClearTarget,
    area: Rect,
) {
    let dialog_area = cache_clear_confirm_area(area);
    let lines = vec![
        Line::from("Clear this cache layer?"),
        Line::from(""),
        key_value_line("target", cache_clear_target_label(target).to_string()),
        key_value_line("scope", cache_clear_target_detail(target).to_string()),
        Line::from(""),
        Line::from(Span::styled(
            "This only affects local cache. GitHub data can be fetched again.",
            active_theme().subtle(),
        )),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(active_theme().panel().fg(active_theme().error))
        .style(modal_surface_style())
        .title(Span::styled(
            "Confirm Clear Cache",
            active_theme()
                .panel()
                .fg(active_theme().error)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    draw_modal_footer(
        frame,
        area,
        dialog_area,
        modal_footer_line("y/Enter: clear cache    Esc: cancel"),
    );
}

pub(super) fn project_remove_area(area: Rect) -> Rect {
    let width = centered_rect_width(58, area).max(36).min(area.width);
    let max_height = area.height.saturating_sub(2).max(3);
    let height = 14.min(max_height).max(3);
    centered_rect_with_size(width, height, area)
}

pub(super) fn project_remove_confirm_area(area: Rect) -> Rect {
    let width = centered_rect_width(58, area).max(42).min(area.width);
    let max_height = area.height.saturating_sub(2).max(3);
    let height = 11.min(max_height).max(3);
    centered_rect_with_size(width, height, area)
}

pub(super) fn cache_clear_area(area: Rect) -> Rect {
    let width = centered_rect_width(86, area).max(52).min(area.width);
    let max_height = area.height.saturating_sub(2).max(3);
    let height = 14.min(max_height).max(3);
    centered_rect_with_size(width, height, area)
}

pub(super) fn cache_clear_confirm_area(area: Rect) -> Rect {
    let width = centered_rect_width(72, area).max(48).min(area.width);
    let max_height = area.height.saturating_sub(2).max(3);
    let height = 11.min(max_height).max(3);
    centered_rect_with_size(width, height, area)
}

pub(super) fn project_remove_candidate_line(
    candidate: &ProjectRemoveCandidate,
    selected: bool,
    width: usize,
) -> Line<'static> {
    let marker = if selected { "> " } else { "  " };
    let text = format!("{marker}{:<20} {}", candidate.name, candidate.repo);
    let style = if selected {
        active_theme()
            .panel()
            .fg(active_theme().highlight_fg)
            .bg(active_theme().error)
            .add_modifier(Modifier::BOLD)
    } else {
        active_theme().panel()
    };
    Line::from(Span::styled(truncate_inline(&text, width), style))
}

pub(super) fn cache_clear_target_label(target: CacheClearTarget) -> &'static str {
    match target {
        CacheClearTarget::CurrentSection => "current section snapshot",
        CacheClearTarget::CurrentView => "current view snapshots",
        CacheClearTarget::ListSnapshots => "all list snapshots",
        CacheClearTarget::CandidateSuggestions => "candidate suggestions",
        CacheClearTarget::LoadedDetails => "loaded details and diffs",
        CacheClearTarget::All => "all cache",
    }
}

pub(super) fn cache_clear_target_detail(target: CacheClearTarget) -> &'static str {
    match target {
        CacheClearTarget::CurrentSection => "Clear only the selected section's cached list rows",
        CacheClearTarget::CurrentView => "Clear cached list rows for the active top-level view",
        CacheClearTarget::ListSnapshots => "Clear all cached PR, issue, and inbox list rows",
        CacheClearTarget::CandidateSuggestions => {
            "Clear cached label, assignee, and reviewer suggestions"
        }
        CacheClearTarget::LoadedDetails => "Clear loaded comments, diffs, checks, and branch hints",
        CacheClearTarget::All => "Clear list snapshots, suggestions, and loaded detail caches",
    }
}

pub(super) fn cache_clear_summary_status(
    target: CacheClearTarget,
    summary: CacheClearSummary,
) -> String {
    let mut parts = Vec::new();
    if summary.snapshot_rows > 0 {
        parts.push(format!("{} db snapshot row(s)", summary.snapshot_rows));
    }
    if summary.list_items > 0 {
        parts.push(format!("{} loaded list item(s)", summary.list_items));
    }
    if summary.candidate_rows > 0 {
        parts.push(format!("{} db suggestion row(s)", summary.candidate_rows));
    }
    if summary.candidate_entries > 0 {
        parts.push(format!(
            "{} loaded suggestion repo cache(s)",
            summary.candidate_entries
        ));
    }
    if summary.loaded_entries > 0 {
        parts.push(format!(
            "{} loaded detail/diff cache(s)",
            summary.loaded_entries
        ));
    }
    if parts.is_empty() {
        parts.push("nothing cached".to_string());
    }
    format!(
        "cleared cache: {} ({})",
        cache_clear_target_label(target),
        parts.join(", ")
    )
}

pub(super) fn cache_clear_target_line(
    target: CacheClearTarget,
    selected: bool,
    width: usize,
) -> Line<'static> {
    let marker = if selected { "> " } else { "  " };
    let label = cache_clear_target_label(target);
    let text = format!("{marker}{label:<28} {}", cache_clear_target_detail(target));
    let style = if selected {
        active_theme()
            .panel()
            .fg(active_theme().highlight_fg)
            .bg(active_theme().error)
            .add_modifier(Modifier::BOLD)
    } else {
        active_theme().panel()
    };
    Line::from(Span::styled(truncate_inline(&text, width), style))
}

pub(super) fn draw_pr_action_dialog(
    frame: &mut Frame<'_>,
    dialog: &PrActionDialog,
    running: bool,
    area: Rect,
) {
    let dialog_area = pr_action_dialog_area(dialog, area);
    let number = dialog
        .item
        .number
        .map(|number| format!("#{number}"))
        .unwrap_or_else(|| "-".to_string());
    let item_label = item_kind_label(dialog.item.kind);
    let confirm_item_label = item_kind_confirmation_label(dialog.item.kind);
    let action_label = match dialog.action {
        PrAction::Merge => "merge",
        PrAction::Close => "close",
        PrAction::Reopen => "reopen",
        PrAction::Approve => "approve",
        PrAction::EnableAutoMerge => "enable auto-merge for",
        PrAction::DisableAutoMerge => "disable auto-merge for",
        PrAction::Checkout => "checkout",
        PrAction::RerunFailedChecks => "rerun failed checks for",
        PrAction::UpdateBranch => "update",
        PrAction::ConvertToDraft => "convert",
        PrAction::MarkReadyForReview => "mark ready",
    };
    let prompt = match dialog.action {
        PrAction::Merge => "Merge this pull request on GitHub?",
        PrAction::Close => match dialog.item.kind {
            ItemKind::Issue => "Close this issue on GitHub?",
            _ => "Close this pull request on GitHub?",
        },
        PrAction::Reopen => match dialog.item.kind {
            ItemKind::Issue => "Reopen this issue on GitHub?",
            _ => "Reopen this pull request on GitHub?",
        },
        PrAction::Approve => "Approve this pull request on GitHub?",
        PrAction::EnableAutoMerge => "Enable auto-merge for this pull request on GitHub?",
        PrAction::DisableAutoMerge => "Disable auto-merge for this pull request on GitHub?",
        PrAction::Checkout => "Checkout this pull request locally?",
        PrAction::RerunFailedChecks => "Rerun failed GitHub Actions jobs for this pull request?",
        PrAction::UpdateBranch => "Update this pull request branch from its base branch?",
        PrAction::ConvertToDraft => "Convert this pull request to draft on GitHub?",
        PrAction::MarkReadyForReview => "Mark this pull request ready for review on GitHub?",
    };
    let status = if running {
        match dialog.action {
            PrAction::Merge => format!("working: {} merge...", dialog.merge_method.label()),
            _ => "working...".to_string(),
        }
    } else if dialog.action == PrAction::Merge {
        format!(
            "y/Enter: {} merge    m/s/r: method    Tab: next    Esc",
            dialog.merge_method.label()
        )
    } else {
        format!("y/Enter: yes, {action_label} {confirm_item_label}    Esc: cancel")
    };
    let mut lines = vec![
        Line::from(prompt),
        Line::from(""),
        key_value_line("repo", dialog.item.repo.clone()),
        key_value_line(item_label, number),
        key_value_line("title", dialog.item.title.clone()),
    ];
    if dialog.action == PrAction::Checkout {
        lines.push(key_value_line(
            "local dir",
            dialog
                .checkout
                .as_ref()
                .map(|checkout| checkout.directory.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
        ));
        lines.push(remote_branch_line(
            dialog
                .checkout
                .as_ref()
                .and_then(|checkout| checkout.branch.as_ref()),
        ));
    }
    for (key, value) in &dialog.summary {
        lines.push(key_value_line(key, value.clone()));
    }
    if dialog.action == PrAction::Merge {
        lines.push(key_value_line(
            "method",
            format!(
                "{}  (m: merge, s: squash, r: rebase)",
                dialog.merge_method.label()
            ),
        ));
    }
    lines.push(Line::from(""));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(themed_fg_style(Color::Yellow))
        .style(modal_surface_style())
        .title(Span::styled(
            match dialog.action {
                PrAction::Merge => "Merge Pull Request",
                PrAction::Close => match dialog.item.kind {
                    ItemKind::Issue => "Close Issue",
                    _ => "Close Pull Request",
                },
                PrAction::Reopen => match dialog.item.kind {
                    ItemKind::Issue => "Reopen Issue",
                    _ => "Reopen Pull Request",
                },
                PrAction::Approve => "Approve Pull Request",
                PrAction::EnableAutoMerge => "Enable Auto-Merge",
                PrAction::DisableAutoMerge => "Disable Auto-Merge",
                PrAction::Checkout => "Checkout Pull Request Locally",
                PrAction::RerunFailedChecks => "Rerun Failed Checks",
                PrAction::UpdateBranch => "Update Pull Request Branch",
                PrAction::ConvertToDraft => "Convert to Draft",
                PrAction::MarkReadyForReview => "Ready for Review",
            },
            themed_bold_style(Color::Yellow),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    draw_modal_footer(frame, area, dialog_area, modal_footer_line(status));
}

pub(super) fn draw_reaction_dialog(
    frame: &mut Frame<'_>,
    dialog: &ReactionDialog,
    running: bool,
    area: Rect,
) {
    let dialog_area = centered_rect(60, 14, area);
    let mut lines = vec![
        Line::from("Target"),
        Line::from(vec![Span::styled(
            truncate_inline(&dialog.target_label, 54),
            themed_fg_style(Color::Cyan),
        )]),
        Line::from(""),
    ];
    for (index, reaction) in ReactionContent::ALL.iter().copied().enumerate() {
        let selected = index == dialog.selected;
        let style = if selected {
            themed_bold_style(Color::Yellow)
        } else {
            themed_fg_style(Color::White)
        };
        lines.push(Line::from(vec![Span::styled(
            format!(
                "{} {}. {} {}",
                if selected { ">" } else { " " },
                index + 1,
                reaction.emoji(),
                reaction.label()
            ),
            style,
        )]));
    }
    let footer = if running {
        "working...".to_string()
    } else {
        "Enter: add reaction    Esc: cancel".to_string()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(themed_fg_style(Color::LightMagenta))
        .style(modal_surface_style())
        .title(Span::styled(
            "Add Reaction",
            themed_bold_style(Color::LightMagenta),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    draw_modal_footer(frame, area, dialog_area, modal_footer_line(footer));
}

pub(super) fn draw_label_dialog(
    frame: &mut Frame<'_>,
    dialog: &LabelDialog,
    running: bool,
    area: Rect,
) {
    let dialog_area = if matches!(dialog.mode, LabelDialogMode::Add { .. }) {
        centered_rect(74, 16, area)
    } else {
        centered_rect(66, 9, area)
    };
    let (title, lines, footer, accent, cursor) = match &dialog.mode {
        LabelDialogMode::Add { repo } => {
            let status = if running {
                "working...".to_string()
            } else {
                "Up/Down: choose    Enter: add    Esc: cancel".to_string()
            };
            let input = if dialog.input.is_empty() {
                " ".to_string()
            } else {
                dialog.input.clone()
            };
            let matches = label_dialog_suggestion_matches(dialog);
            let mut lines = vec![
                key_value_line("repo", repo.clone()),
                Line::from("Label prefix"),
                Line::from(vec![Span::styled(input, themed_fg_style(Color::Cyan))]),
                Line::from(""),
            ];
            if dialog.suggestions_loading {
                lines.push(Line::from(vec![Span::styled(
                    "Suggestions: loading...",
                    themed_fg_style(Color::Gray),
                )]));
            } else if let Some(error) = &dialog.suggestions_error {
                lines.push(Line::from(vec![Span::styled(
                    "Suggestions unavailable",
                    themed_bold_style(Color::LightRed),
                )]));
                lines.push(Line::from(vec![Span::styled(
                    truncate_text(error, 68),
                    themed_fg_style(Color::Gray),
                )]));
            } else if matches.is_empty() {
                let message = if dialog.input.trim().is_empty() {
                    "No labels available for this repo."
                } else {
                    "No prefix matches. Enter adds the typed label."
                };
                lines.push(Line::from(vec![Span::styled(
                    message,
                    themed_fg_style(Color::Gray),
                )]));
            } else {
                lines.push(Line::from(vec![Span::styled(
                    "Suggestions",
                    themed_bold_style(Color::Gray),
                )]));
                let start =
                    label_suggestion_window_start(matches.len(), dialog.selected_suggestion);
                for (index, label) in matches
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(LABEL_SUGGESTION_LIMIT)
                {
                    let selected = index == dialog.selected_suggestion;
                    let style = if selected {
                        themed_bold_style(Color::Yellow)
                    } else {
                        themed_fg_style(Color::Cyan)
                    };
                    lines.push(Line::from(vec![
                        Span::styled(if selected { "> " } else { "  " }, style),
                        Span::styled(label.clone(), style),
                    ]));
                }
            }
            while lines.len() < 12 {
                lines.push(Line::from(""));
            }
            (
                "Add Label",
                lines,
                status,
                Color::LightMagenta,
                Some(display_width(&dialog.input).min(usize::from(u16::MAX)) as u16),
            )
        }
        LabelDialogMode::Remove { label } => {
            let status = if running {
                "working...".to_string()
            } else {
                "y/Enter: remove label    Esc: cancel".to_string()
            };
            (
                "Remove Label",
                vec![
                    Line::from("Remove this label from the selected item?"),
                    Line::from(""),
                    key_value_line("label", label.clone()),
                ],
                status,
                Color::Yellow,
                None,
            )
        }
    };
    let accent = themed_hint_color(accent);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(active_theme().panel().fg(accent))
        .style(modal_surface_style())
        .title(Span::styled(
            title,
            active_theme()
                .panel()
                .fg(accent)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    draw_modal_footer(frame, area, dialog_area, modal_footer_line(footer));
    if let Some(column) = cursor {
        let inner = block_inner(dialog_area);
        frame.set_cursor_position(Position::new(
            inner
                .x
                .saturating_add(column)
                .min(inner.right().saturating_sub(1)),
            inner.y.saturating_add(2),
        ));
    }
}

pub(super) fn draw_issue_dialog(
    frame: &mut Frame<'_>,
    dialog: &IssueDialog,
    running: bool,
    area: Rect,
) {
    let dialog_area = issue_dialog_area(area);
    let inner = block_inner(dialog_area);
    let editor_width = inner.width.max(1);
    let editor_height = issue_dialog_body_editor_height(dialog_area);
    let body = dialog.body.text();
    let body_lines = comment_dialog_body_lines(body, editor_width);
    let max_scroll = max_comment_dialog_scroll(body, editor_width, editor_height);
    let scroll = dialog.body_scroll.min(max_scroll);
    let mut lines = vec![
        issue_dialog_field_input_line(
            "Repo",
            dialog.repo.text(),
            IssueDialogField::Repo,
            dialog.field,
            editor_width,
        ),
        issue_dialog_separator_line(editor_width),
        issue_dialog_field_input_line(
            "Title",
            dialog.title.text(),
            IssueDialogField::Title,
            dialog.field,
            editor_width,
        ),
        issue_dialog_separator_line(editor_width),
        issue_dialog_field_input_line(
            "Labels",
            dialog.labels.text(),
            IssueDialogField::Labels,
            dialog.field,
            editor_width,
        ),
        issue_dialog_separator_line(editor_width),
        issue_dialog_field_label("Body", IssueDialogField::Body, dialog.field),
    ];
    lines.extend(
        body_lines
            .into_iter()
            .skip(usize::from(scroll))
            .take(usize::from(editor_height))
            .map(Line::from),
    );
    while lines.len() < usize::from(7 + editor_height) {
        lines.push(Line::from(""));
    }
    let footer = if running {
        "working..."
    } else {
        "Tab: field  Ctrl+Enter: create  Ctrl+S/click: save draft  arrows/Home/End edit  Ctrl+W/U/K/X word/line"
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(themed_fg_style(Color::LightMagenta))
        .style(modal_surface_style())
        .title(Span::styled(
            "New Issue",
            themed_bold_style(Color::LightMagenta),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    draw_modal_footer(frame, area, dialog_area, modal_footer_line(footer));
    if let Some(position) =
        issue_dialog_cursor_position(dialog, scroll, dialog_area, editor_width, editor_height)
    {
        frame.set_cursor_position(position);
    }
}

pub(super) fn draw_pr_create_dialog(
    frame: &mut Frame<'_>,
    dialog: &PrCreateDialog,
    running: bool,
    area: Rect,
) {
    let dialog_area = pr_create_dialog_area(area);
    let inner = block_inner(dialog_area);
    let editor_width = inner.width.max(1);
    let editor_height = pr_create_dialog_body_editor_height(dialog_area);
    let body = dialog.body.text();
    let body_lines = comment_dialog_body_lines(body, editor_width);
    let max_scroll = max_comment_dialog_scroll(body, editor_width, editor_height);
    let scroll = dialog.body_scroll.min(max_scroll);
    let mut lines = vec![
        key_value_line("repo", dialog.repo.clone()),
        key_value_line("local dir", dialog.local_dir.display().to_string()),
        key_value_line("branch", dialog.branch.clone()),
        issue_dialog_separator_line(editor_width),
        pr_create_dialog_field_input_line(
            "Title",
            dialog.title.text(),
            PrCreateField::Title,
            dialog.field,
            editor_width,
        ),
        issue_dialog_separator_line(editor_width),
        pr_create_dialog_field_label("Body", PrCreateField::Body, dialog.field),
    ];
    lines.extend(
        body_lines
            .into_iter()
            .skip(usize::from(scroll))
            .take(usize::from(editor_height))
            .map(Line::from),
    );
    while lines.len() < usize::from(7 + editor_height) {
        lines.push(Line::from(""));
    }
    let footer = if running {
        "working..."
    } else {
        "Tab: field  Ctrl+Enter: create PR  Ctrl+S/click: save draft  arrows/Home/End edit  Ctrl+W/U/K/X word/line"
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(themed_fg_style(Color::LightMagenta))
        .style(modal_surface_style())
        .title(Span::styled(
            "New Pull Request",
            themed_bold_style(Color::LightMagenta),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    draw_modal_footer(frame, area, dialog_area, modal_footer_line(footer));
    if let Some(position) =
        pr_create_dialog_cursor_position(dialog, scroll, dialog_area, editor_width, editor_height)
    {
        frame.set_cursor_position(position);
    }
}

pub(super) fn issue_dialog_area(area: Rect) -> Rect {
    centered_rect(78, 22, area)
}

pub(super) fn pr_create_dialog_area(area: Rect) -> Rect {
    centered_rect(78, 20, area)
}

pub(super) fn issue_dialog_body_editor_height(dialog_area: Rect) -> u16 {
    let inner = block_inner(dialog_area);
    inner.height.saturating_sub(7).max(1)
}

pub(super) fn pr_create_dialog_body_editor_height(dialog_area: Rect) -> u16 {
    let inner = block_inner(dialog_area);
    inner.height.saturating_sub(7).max(1)
}

pub(super) fn issue_dialog_body_editor_size(area: Option<Rect>) -> (u16, u16) {
    if let Some(area) = area {
        let dialog_area = issue_dialog_area(area);
        let inner = block_inner(dialog_area);
        return (
            inner.width.max(1),
            issue_dialog_body_editor_height(dialog_area),
        );
    }

    (
        COMMENT_DIALOG_FALLBACK_EDITOR_WIDTH,
        COMMENT_DIALOG_FALLBACK_EDITOR_HEIGHT,
    )
}

pub(super) fn pr_create_dialog_body_editor_size(area: Option<Rect>) -> (u16, u16) {
    if let Some(area) = area {
        let dialog_area = pr_create_dialog_area(area);
        let inner = block_inner(dialog_area);
        return (
            inner.width.max(1),
            pr_create_dialog_body_editor_height(dialog_area),
        );
    }

    (
        COMMENT_DIALOG_FALLBACK_EDITOR_WIDTH,
        COMMENT_DIALOG_FALLBACK_EDITOR_HEIGHT,
    )
}

pub(super) fn issue_dialog_field_input_line(
    label: &'static str,
    value: &str,
    field: IssueDialogField,
    current: IssueDialogField,
    width: u16,
) -> Line<'static> {
    let prefix = issue_dialog_field_prefix(label);
    let value_width =
        width.saturating_sub(display_width(&prefix).min(usize::from(u16::MAX)) as u16);
    Line::from(vec![
        Span::styled(prefix, issue_dialog_field_label_style(field, current)),
        Span::styled(
            issue_dialog_input_text(value, value_width),
            themed_fg_style(Color::White),
        ),
    ])
}

pub(super) fn pr_create_dialog_field_input_line(
    label: &'static str,
    value: &str,
    field: PrCreateField,
    current: PrCreateField,
    width: u16,
) -> Line<'static> {
    let prefix = issue_dialog_field_prefix(label);
    let value_width =
        width.saturating_sub(display_width(&prefix).min(usize::from(u16::MAX)) as u16);
    Line::from(vec![
        Span::styled(prefix, pr_create_dialog_field_label_style(field, current)),
        Span::styled(
            issue_dialog_input_text(value, value_width),
            themed_fg_style(Color::White),
        ),
    ])
}

pub(super) fn issue_dialog_field_label(
    label: &'static str,
    field: IssueDialogField,
    current: IssueDialogField,
) -> Line<'static> {
    Line::from(Span::styled(
        issue_dialog_field_label_text(label),
        issue_dialog_field_label_style(field, current),
    ))
}

pub(super) fn pr_create_dialog_field_label(
    label: &'static str,
    field: PrCreateField,
    current: PrCreateField,
) -> Line<'static> {
    Line::from(Span::styled(
        issue_dialog_field_label_text(label),
        pr_create_dialog_field_label_style(field, current),
    ))
}

pub(super) fn issue_dialog_field_label_style(
    field: IssueDialogField,
    current: IssueDialogField,
) -> Style {
    if field == current {
        themed_bold_style(Color::Cyan)
    } else {
        themed_fg_style(Color::Gray)
    }
}

pub(super) fn pr_create_dialog_field_label_style(
    field: PrCreateField,
    current: PrCreateField,
) -> Style {
    if field == current {
        themed_bold_style(Color::Cyan)
    } else {
        themed_fg_style(Color::Gray)
    }
}

pub(super) fn issue_dialog_field_prefix(label: &'static str) -> String {
    format!("{label}: ")
}

pub(super) fn issue_dialog_field_label_text(label: &'static str) -> String {
    format!("{label}:")
}

pub(super) fn issue_dialog_separator_line(width: u16) -> Line<'static> {
    Line::from(Span::styled(
        "─".repeat(usize::from(width.max(1))),
        themed_fg_style(Color::DarkGray),
    ))
}

pub(super) fn issue_dialog_input_text(value: &str, width: u16) -> String {
    if value.is_empty() {
        " ".to_string()
    } else {
        truncate_inline(value, usize::from(width.max(1)))
    }
}

pub(super) fn clamp_text_cursor(text: &str, cursor: usize) -> usize {
    if cursor >= text.len() {
        return text.len();
    }
    let mut cursor = cursor;
    while cursor > 0 && !text.is_char_boundary(cursor) {
        cursor = cursor.saturating_sub(1);
    }
    cursor
}

pub(super) fn text_before_cursor_width(text: &str, cursor: usize) -> u16 {
    display_width(&text[..clamp_text_cursor(text, cursor)]).min(usize::from(u16::MAX)) as u16
}

pub(super) fn cursor_for_inline_column(text: &str, column: u16) -> usize {
    let target = usize::from(column);
    let mut current = 0_usize;
    for (index, ch) in text.char_indices() {
        if target <= current {
            return index;
        }
        let char_width = display_width_char(ch);
        let next = current.saturating_add(char_width);
        if target < next {
            let midpoint = current.saturating_add(char_width / 2);
            return if target <= midpoint {
                index
            } else {
                index.saturating_add(ch.len_utf8())
            };
        }
        current = next;
    }
    text.len()
}

pub(super) fn comment_dialog_cursor_for_position(
    text: &str,
    width: u16,
    target_line: usize,
    target_column: u16,
) -> usize {
    let width = usize::from(width.max(1));
    let target_column = usize::from(target_column);
    let mut line = 0_usize;
    let mut column = 0_usize;

    for (index, ch) in text.char_indices() {
        if ch == '\n' {
            if line == target_line {
                return index;
            }
            line = line.saturating_add(1);
            column = 0;
            continue;
        }

        let char_width = display_width_char(ch);
        if column > 0 && (column >= width || column.saturating_add(char_width) > width) {
            if line == target_line {
                return index;
            }
            line = line.saturating_add(1);
            column = 0;
        }

        if line == target_line {
            if target_column <= column {
                return index;
            }
            let next = column.saturating_add(char_width);
            if target_column < next {
                let midpoint = column.saturating_add(char_width / 2);
                return if target_column <= midpoint {
                    index
                } else {
                    index.saturating_add(ch.len_utf8())
                };
            }
        }

        column = column.saturating_add(char_width);
        if column == width {
            if line == target_line && target_column >= width {
                return index.saturating_add(ch.len_utf8());
            }
            line = line.saturating_add(1);
            column = 0;
        }
    }

    text.len()
}

pub(super) fn issue_dialog_cursor_position(
    dialog: &IssueDialog,
    scroll: u16,
    dialog_area: Rect,
    editor_width: u16,
    editor_height: u16,
) -> Option<Position> {
    let inner = block_inner(dialog_area);
    let clamp_x = |x: u16| x.min(inner.right().saturating_sub(1));
    let repo_prefix_width =
        display_width(&issue_dialog_field_prefix("Repo")).min(usize::from(u16::MAX)) as u16;
    let title_prefix_width =
        display_width(&issue_dialog_field_prefix("Title")).min(usize::from(u16::MAX)) as u16;
    let labels_prefix_width =
        display_width(&issue_dialog_field_prefix("Labels")).min(usize::from(u16::MAX)) as u16;
    match dialog.field {
        IssueDialogField::Repo => Some(Position::new(
            clamp_x(inner.x.saturating_add(repo_prefix_width).saturating_add(
                text_before_cursor_width(dialog.repo.text(), dialog.repo.cursor_byte()),
            )),
            inner.y,
        )),
        IssueDialogField::Title => Some(Position::new(
            clamp_x(inner.x.saturating_add(title_prefix_width).saturating_add(
                text_before_cursor_width(dialog.title.text(), dialog.title.cursor_byte()),
            )),
            inner.y.saturating_add(2),
        )),
        IssueDialogField::Labels => Some(Position::new(
            clamp_x(inner.x.saturating_add(labels_prefix_width).saturating_add(
                text_before_cursor_width(dialog.labels.text(), dialog.labels.cursor_byte()),
            )),
            inner.y.saturating_add(4),
        )),
        IssueDialogField::Body => {
            let body = dialog.body.text();
            let (line, column) =
                comment_dialog_cursor_offset_at(body, dialog.body.cursor_byte(), editor_width);
            let visible_end = scroll.saturating_add(editor_height.max(1));
            if line < scroll || line >= visible_end {
                return None;
            }
            Some(Position::new(
                clamp_x(inner.x.saturating_add(column)),
                inner.y.saturating_add(7).saturating_add(line - scroll),
            ))
        }
    }
}

pub(super) fn pr_create_dialog_cursor_position(
    dialog: &PrCreateDialog,
    scroll: u16,
    dialog_area: Rect,
    editor_width: u16,
    editor_height: u16,
) -> Option<Position> {
    let inner = block_inner(dialog_area);
    let clamp_x = |x: u16| x.min(inner.right().saturating_sub(1));
    let title_prefix_width =
        display_width(&issue_dialog_field_prefix("Title")).min(usize::from(u16::MAX)) as u16;
    match dialog.field {
        PrCreateField::Title => Some(Position::new(
            clamp_x(inner.x.saturating_add(title_prefix_width).saturating_add(
                text_before_cursor_width(dialog.title.text(), dialog.title.cursor_byte()),
            )),
            inner.y.saturating_add(4),
        )),
        PrCreateField::Body => {
            let body = dialog.body.text();
            let (line, column) =
                comment_dialog_cursor_offset_at(body, dialog.body.cursor_byte(), editor_width);
            let visible_end = scroll.saturating_add(editor_height.max(1));
            if line < scroll || line >= visible_end {
                return None;
            }
            Some(Position::new(
                clamp_x(inner.x.saturating_add(column)),
                inner.y.saturating_add(7).saturating_add(line - scroll),
            ))
        }
    }
}

pub(super) fn parse_issue_labels(input: &str) -> Vec<String> {
    let mut labels = Vec::new();
    for label in input
        .split(',')
        .map(str::trim)
        .filter(|label| !label.is_empty())
    {
        if !labels.iter().any(|existing| existing == label) {
            labels.push(label.to_string());
        }
    }
    labels
}

pub(super) fn label_dialog_suggestion_matches(dialog: &LabelDialog) -> Vec<String> {
    if !matches!(dialog.mode, LabelDialogMode::Add { .. }) {
        return Vec::new();
    }
    let query = dialog.input.trim().to_ascii_lowercase();
    dialog
        .suggestions
        .iter()
        .filter(|label| {
            !dialog
                .existing_labels
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(label))
        })
        .filter(|label| query.is_empty() || label.to_ascii_lowercase().starts_with(&query))
        .cloned()
        .collect()
}

pub(super) fn selected_label_for_add(dialog: &LabelDialog) -> String {
    let input = dialog.input.trim();
    let matches = label_dialog_suggestion_matches(dialog);
    if !input.is_empty()
        && let Some(exact) = dialog
            .suggestions
            .iter()
            .find(|label| label.eq_ignore_ascii_case(input))
    {
        return exact.clone();
    }
    if matches.is_empty() {
        input.to_string()
    } else {
        matches
            .get(dialog.selected_suggestion.min(matches.len() - 1))
            .cloned()
            .unwrap_or_else(|| input.to_string())
    }
}

pub(super) fn clamp_label_dialog_selection(dialog: &mut LabelDialog) {
    let count = label_dialog_suggestion_matches(dialog).len();
    if count == 0 {
        dialog.selected_suggestion = 0;
    } else {
        dialog.selected_suggestion = dialog.selected_suggestion.min(count - 1);
    }
}

pub(super) fn label_suggestion_window_start(total: usize, selected: usize) -> usize {
    if total <= LABEL_SUGGESTION_LIMIT {
        0
    } else {
        selected
            .saturating_add(1)
            .saturating_sub(LABEL_SUGGESTION_LIMIT)
            .min(total.saturating_sub(LABEL_SUGGESTION_LIMIT))
    }
}

pub(super) fn clamp_assignee_dialog_selection(dialog: &mut AssigneeDialog) {
    let count = assignee_dialog_suggestion_matches(dialog).len();
    if count == 0 {
        dialog.selected_suggestion = 0;
    } else {
        dialog.selected_suggestion = dialog.selected_suggestion.min(count - 1);
    }
}

pub(super) fn assignee_suggestion_window_start(total: usize, selected: usize) -> usize {
    if total <= ASSIGNEE_SUGGESTION_LIMIT {
        0
    } else {
        selected
            .saturating_add(1)
            .saturating_sub(ASSIGNEE_SUGGESTION_LIMIT)
            .min(total.saturating_sub(ASSIGNEE_SUGGESTION_LIMIT))
    }
}

pub(super) fn clamp_reviewer_dialog_selection(dialog: &mut ReviewerDialog) {
    let count = reviewer_dialog_suggestion_matches(dialog).len();
    if count == 0 {
        dialog.selected_suggestion = 0;
    } else {
        dialog.selected_suggestion = dialog.selected_suggestion.min(count - 1);
    }
}

pub(super) fn reviewer_suggestion_window_start(total: usize, selected: usize) -> usize {
    suggestion_window_start(total, selected, REVIEWER_SUGGESTION_LIMIT)
}

pub(super) fn suggestion_window_start(total: usize, selected: usize, limit: usize) -> usize {
    if total <= limit {
        0
    } else {
        selected
            .saturating_add(1)
            .saturating_sub(limit)
            .min(total.saturating_sub(limit))
    }
}

pub(super) fn pr_action_dialog_area(dialog: &PrActionDialog, area: Rect) -> Rect {
    let dialog_height = if matches!(dialog.action, PrAction::Checkout | PrAction::Merge) {
        14
    } else {
        12
    };
    centered_rect(66, dialog_height, area)
}

pub(super) fn remote_branch_line(branch: Option<&PullRequestBranch>) -> Line<'static> {
    let Some(branch) = branch else {
        return key_value_line("remote branch", "unavailable".to_string());
    };
    Line::from(vec![
        Span::styled("remote branch: ", themed_fg_style(Color::Gray)),
        Span::styled(pull_request_branch_label(branch), link_style()),
    ])
}

pub(super) fn pr_action_dialog_link_at(
    dialog: &PrActionDialog,
    area: Rect,
    column: u16,
    row: u16,
) -> Option<String> {
    if dialog.action != PrAction::Checkout {
        return None;
    }
    let branch = dialog
        .checkout
        .as_ref()
        .and_then(|checkout| checkout.branch.as_ref())?;
    let dialog_area = pr_action_dialog_area(dialog, area);
    let inner = block_inner(dialog_area);
    if !rect_contains(inner, column, row) {
        return None;
    }
    let content_row = row.saturating_sub(inner.y);
    if content_row != PR_ACTION_REMOTE_BRANCH_LINE {
        return None;
    }
    let label = pull_request_branch_label(branch);
    let start = display_width("remote branch: ") as u16;
    let end = start.saturating_add(display_width(&label) as u16);
    let clicked = column.saturating_sub(inner.x);
    (clicked >= start && clicked < end).then(|| pull_request_branch_url(branch))
}

pub(super) fn draw_milestone_dialog(
    frame: &mut Frame<'_>,
    dialog: &MilestoneDialog,
    running: bool,
    area: Rect,
) {
    let dialog_area = centered_rect(72, 18, area);
    let number = dialog
        .item
        .number
        .map(|number| format!("#{number}"))
        .unwrap_or_else(|| "-".to_string());
    let current = dialog
        .item
        .milestone
        .as_ref()
        .map(|milestone| milestone.title.clone())
        .unwrap_or_else(|| "(none)".to_string());
    let mut lines = vec![
        key_value_line("repo", dialog.item.repo.clone()),
        key_value_line("issue/pr", number),
        key_value_line("current", current),
        key_value_line("prefix", format!("{}_", dialog.input)),
        Line::from(""),
    ];

    match &dialog.state {
        MilestoneDialogState::Loading => {
            lines.push(Line::from("loading open milestones..."));
        }
        MilestoneDialogState::Error(error) => {
            lines.push(Line::from(vec![Span::styled(
                operation_error_body(error),
                themed_fg_style(Color::LightRed),
            )]));
        }
        MilestoneDialogState::Loaded(_) => {
            let choices = milestone_choices(dialog);
            if choices.is_empty() {
                lines.push(Line::from("No open milestones."));
            } else {
                for (index, choice) in choices.iter().take(9).enumerate() {
                    let marker = if index == dialog.selected { "> " } else { "  " };
                    let style = if index == dialog.selected {
                        themed_bold_style(Color::Yellow)
                    } else {
                        modal_text_style()
                    };
                    lines.push(Line::from(vec![Span::styled(
                        format!("{marker}{}", milestone_choice_label(choice)),
                        style,
                    )]));
                }
                if choices.len() > 9 {
                    lines.push(Line::from(format!("  ... {} more", choices.len() - 9)));
                }
            }
        }
    }

    let footer = if running {
        "working..."
    } else {
        "type prefix, Up/Down choose, Enter set/create, Esc cancel"
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(themed_fg_style(Color::Yellow))
        .style(modal_surface_style())
        .title(Span::styled(
            "Change Milestone",
            themed_bold_style(Color::Yellow),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    draw_modal_footer(frame, area, dialog_area, modal_footer_line(footer));
}

pub(super) fn draw_assignee_dialog(
    frame: &mut Frame<'_>,
    dialog: &AssigneeDialog,
    running: bool,
    area: Rect,
) {
    let dialog_area = centered_rect(70, 17, area);
    let number = dialog
        .item
        .number
        .map(|number| format!("#{number}"))
        .unwrap_or_else(|| "-".to_string());
    let action = assignee_action_label(dialog.action);
    let current = if dialog.item.assignees.is_empty() {
        "-".to_string()
    } else {
        dialog.item.assignees.join(", ")
    };
    let status = if running {
        "working...".to_string()
    } else if assignee_dialog_uses_default_unassign(dialog) {
        "Enter: unassign current    Esc: cancel".to_string()
    } else {
        format!("Up/Down: choose    Enter: {action}    Esc: cancel")
    };
    let matches = assignee_dialog_suggestion_matches(dialog);
    let mut lines = vec![
        key_value_line("repo", dialog.item.repo.clone()),
        key_value_line("item", number),
        key_value_line("current", current),
        Line::from(""),
        key_value_line("assignee(s)", format!("{}_", dialog.input)),
        Line::from(""),
    ];
    if dialog.suggestions_loading {
        lines.push(Line::from(vec![Span::styled(
            "Candidates: loading assignable users...",
            themed_fg_style(Color::Gray),
        )]));
    } else if let Some(error) = &dialog.suggestions_error {
        lines.push(Line::from(vec![Span::styled(
            "Candidates unavailable",
            themed_bold_style(Color::LightRed),
        )]));
        lines.push(Line::from(vec![Span::styled(
            truncate_text(error, 64),
            themed_fg_style(Color::Gray),
        )]));
    } else if matches.is_empty() {
        let message = match dialog.action {
            AssigneeAction::Assign if dialog.input.trim().is_empty() => {
                "No assignable users loaded. Type a login manually."
            }
            AssigneeAction::Assign => "No prefix matches. Enter uses the typed login.",
            AssigneeAction::Unassign => "No current assignee matches this prefix.",
        };
        lines.push(Line::from(vec![Span::styled(
            message,
            themed_fg_style(Color::Gray),
        )]));
    } else {
        lines.push(Line::from(vec![Span::styled(
            "Candidates",
            themed_bold_style(Color::Gray),
        )]));
        let start = assignee_suggestion_window_start(matches.len(), dialog.selected_suggestion);
        for (index, login) in matches
            .iter()
            .enumerate()
            .skip(start)
            .take(ASSIGNEE_SUGGESTION_LIMIT)
        {
            let selected = index == dialog.selected_suggestion;
            let style = if selected {
                themed_bold_style(Color::Yellow)
            } else {
                themed_fg_style(Color::Cyan)
            };
            lines.push(Line::from(vec![
                Span::styled(if selected { "> " } else { "  " }, style),
                Span::styled(login.clone(), style),
            ]));
        }
    }
    while lines.len() < 13 {
        lines.push(Line::from(""));
    }
    let title = match dialog.action {
        AssigneeAction::Assign => "Assign Assignee",
        AssigneeAction::Unassign => "Unassign Assignee",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(themed_fg_style(Color::Yellow))
        .style(modal_surface_style())
        .title(Span::styled(title, themed_bold_style(Color::Yellow)));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    draw_modal_footer(frame, area, dialog_area, modal_footer_line(status));
}

pub(super) fn draw_reviewer_dialog(
    frame: &mut Frame<'_>,
    dialog: &ReviewerDialog,
    running: bool,
    area: Rect,
) {
    let dialog_area = centered_rect(74, 20, area);
    let number = dialog
        .item
        .number
        .map(|number| format!("#{number}"))
        .unwrap_or_else(|| "-".to_string());
    let prompt = match dialog.action {
        ReviewerAction::Request => "Request or re-request review from GitHub users.",
        ReviewerAction::Remove => "Remove pending review requests from GitHub users.",
    };
    let status = if running {
        "working...".to_string()
    } else {
        "Enter: submit    comma separates logins    Esc: cancel".to_string()
    };
    let input = if dialog.input.is_empty() {
        "<reviewer logins>".to_string()
    } else {
        dialog.input.clone()
    };
    let mut lines = vec![
        Line::from(prompt),
        Line::from(""),
        key_value_line("repo", dialog.item.repo.clone()),
        key_value_line("pull request", number),
        key_value_line("title", dialog.item.title.clone()),
        Line::from(""),
        key_value_line("reviewers", input),
        Line::from(""),
    ];
    let matches = reviewer_dialog_suggestion_matches(dialog);
    if dialog.suggestions_loading {
        lines.push(Line::from(vec![Span::styled(
            "Candidates: loading reviewable users...",
            themed_fg_style(Color::Gray),
        )]));
    } else if let Some(error) = &dialog.suggestions_error {
        lines.push(Line::from(vec![Span::styled(
            "Candidates unavailable",
            themed_bold_style(Color::LightRed),
        )]));
        lines.push(Line::from(vec![Span::styled(
            truncate_text(error, 68),
            themed_fg_style(Color::Gray),
        )]));
    } else if matches.is_empty() {
        let message = if dialog.input.trim().is_empty() {
            "No reviewer candidates loaded. Type login manually."
        } else {
            "No prefix matches. Enter uses the typed login."
        };
        lines.push(Line::from(vec![Span::styled(
            message,
            themed_fg_style(Color::Gray),
        )]));
    } else {
        lines.push(Line::from(vec![Span::styled(
            "Candidates",
            themed_bold_style(Color::Gray),
        )]));
        let start = reviewer_suggestion_window_start(matches.len(), dialog.selected_suggestion);
        for (index, login) in matches
            .iter()
            .enumerate()
            .skip(start)
            .take(REVIEWER_SUGGESTION_LIMIT)
        {
            let selected = index == dialog.selected_suggestion;
            let style = if selected {
                themed_bold_style(Color::Yellow)
            } else {
                themed_fg_style(Color::Cyan)
            };
            lines.push(Line::from(vec![
                Span::styled(if selected { "> " } else { "  " }, style),
                Span::styled(login.clone(), style),
            ]));
        }
    }
    let title = match dialog.action {
        ReviewerAction::Request => "Request Reviewers",
        ReviewerAction::Remove => "Remove Review Requests",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(themed_fg_style(Color::Yellow))
        .style(modal_surface_style())
        .title(Span::styled(title, themed_bold_style(Color::Yellow)));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    draw_modal_footer(frame, area, dialog_area, modal_footer_line(status));
}

pub(super) fn draw_item_edit_dialog(frame: &mut Frame<'_>, dialog: &ItemEditDialog, area: Rect) {
    let dialog_area = centered_rect(66, 12, area);
    let number = dialog
        .item
        .number
        .map(|number| format!("#{number}"))
        .unwrap_or_else(|| "-".to_string());
    let item_kind = match dialog.item.kind {
        ItemKind::PullRequest => "pull request",
        ItemKind::Issue => "issue",
        ItemKind::Notification => "item",
    };
    let lines = vec![
        Line::from("Choose the field to edit on GitHub."),
        Line::from(""),
        key_value_line("repo", dialog.item.repo.clone()),
        key_value_line(item_kind, number),
        key_value_line("title", dialog.item.title.clone()),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(themed_fg_style(Color::LightMagenta))
        .style(modal_surface_style())
        .title(Span::styled(
            "Edit Item",
            themed_bold_style(Color::LightMagenta),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    draw_modal_footer(
        frame,
        area,
        dialog_area,
        modal_footer_line("t: title    b: body    Esc: cancel"),
    );
}

pub(super) fn draw_global_search_dialog(
    frame: &mut Frame<'_>,
    dialog: &GlobalSearchDialog,
    app: &AppState,
    area: Rect,
) {
    let dialog_area = centered_rect(74, 22, area);
    let inner = block_inner(dialog_area);
    let width = inner.width.max(1);
    let current_scope = app.current_repo_scope();
    let repo_scope = app
        .global_search_scope
        .as_deref()
        .or(current_scope.as_deref())
        .map(|repo| format!("scope: {repo}"))
        .unwrap_or_else(|| "scope: GitHub".to_string());
    let scope = app
        .global_search_preferred_kind
        .map(|kind| format!("{repo_scope} | {}", saved_search_kind_label(kind)))
        .unwrap_or(repo_scope);
    let mut lines = vec![
        Line::from(vec![Span::styled(
            scope,
            themed_fg_style(active_theme().muted),
        )]),
        Line::from(""),
        global_search_dialog_field_input_line(
            GlobalSearchField::Title,
            dialog.title.text(),
            dialog.field,
            width,
        ),
        global_search_dialog_field_input_line(
            GlobalSearchField::Status,
            dialog.status.text(),
            dialog.field,
            width,
        ),
        global_search_dialog_field_input_line(
            GlobalSearchField::Label,
            dialog.label.text(),
            dialog.field,
            width,
        ),
        global_search_dialog_field_input_line(
            GlobalSearchField::Author,
            dialog.author.text(),
            dialog.field,
            width,
        ),
        global_search_dialog_field_input_line(
            GlobalSearchField::Assignee,
            dialog.assignee.text(),
            dialog.field,
            width,
        ),
        global_search_dialog_field_input_line(
            GlobalSearchField::Sort,
            dialog.sort.text(),
            dialog.field,
            width,
        ),
    ];
    lines.push(Line::from(""));
    push_global_search_suggestion_lines(&mut lines, dialog, usize::from(width));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(themed_fg_style(Color::LightMagenta))
        .style(modal_surface_style())
        .title(Span::styled(
            "Search Issues and Pull Requests",
            themed_bold_style(Color::LightMagenta),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    draw_modal_footer(
        frame,
        area,
        dialog_area,
        modal_footer_line(
            "Tab field  ↑/↓ move  Enter choose/search  Ctrl+S save  Ctrl+U clear  Esc cancel",
        ),
    );
    if let Some(position) = global_search_dialog_cursor_position(dialog, dialog_area, width) {
        frame.set_cursor_position(position);
    }
}

pub(super) fn push_global_search_suggestion_lines(
    lines: &mut Vec<Line<'static>>,
    dialog: &GlobalSearchDialog,
    width: usize,
) {
    if let Some(state) = global_search_dialog_suggestion_state(dialog) {
        lines.push(Line::from(vec![Span::styled(
            state,
            themed_fg_style(active_theme().muted),
        )]));
        return;
    }

    let matches = global_search_dialog_suggestion_matches(dialog);
    if matches.is_empty() {
        if dialog.field != GlobalSearchField::Title {
            lines.push(Line::from(vec![Span::styled(
                "No candidates. Typed value will be used.",
                themed_fg_style(active_theme().muted),
            )]));
        }
        return;
    }

    let title = match dialog.field {
        GlobalSearchField::Status | GlobalSearchField::Sort => "Options",
        GlobalSearchField::Label | GlobalSearchField::Author | GlobalSearchField::Assignee => {
            "Candidates"
        }
        GlobalSearchField::Title => "Candidates",
    };
    lines.push(Line::from(vec![Span::styled(
        title,
        themed_bold_style(active_theme().muted),
    )]));
    let selected = dialog.selected_suggestion.min(matches.len() - 1);
    let start = suggestion_window_start(matches.len(), selected, GLOBAL_SEARCH_SUGGESTION_LIMIT);
    for (index, candidate) in matches
        .iter()
        .enumerate()
        .skip(start)
        .take(GLOBAL_SEARCH_SUGGESTION_LIMIT)
    {
        let selected = index == selected;
        let style = if selected {
            active_theme().active()
        } else {
            active_theme().panel().fg(active_theme().focus)
        };
        let marker = if selected { "> " } else { "  " };
        let text_width = width.saturating_sub(display_width(marker));
        lines.push(Line::from(vec![
            Span::styled(marker, style),
            Span::styled(truncate_inline(candidate, text_width), style),
        ]));
    }
}

pub(super) fn global_search_dialog_field_input_line(
    field: GlobalSearchField,
    value: &str,
    current: GlobalSearchField,
    width: u16,
) -> Line<'static> {
    let prefix = format!("{:<10}: ", field.label());
    let value_width =
        width.saturating_sub(display_width(&prefix).min(usize::from(u16::MAX)) as u16);
    Line::from(vec![
        Span::styled(
            prefix,
            global_search_dialog_field_label_style(field, current),
        ),
        Span::styled(
            issue_dialog_input_text(value, value_width),
            active_theme().panel().fg(active_theme().text),
        ),
    ])
}

pub(super) fn global_search_dialog_field_label_style(
    field: GlobalSearchField,
    current: GlobalSearchField,
) -> Style {
    if field == current {
        themed_bold_style(Color::LightMagenta)
    } else {
        active_theme().muted()
    }
}

pub(super) fn global_search_dialog_cursor_position(
    dialog: &GlobalSearchDialog,
    dialog_area: Rect,
    width: u16,
) -> Option<Position> {
    let inner = block_inner(dialog_area);
    let field_index = GlobalSearchField::FIELDS
        .iter()
        .position(|field| *field == dialog.field)?;
    let prefix = format!("{:<10}: ", dialog.field.label());
    let prefix_width = display_width(&prefix).min(usize::from(u16::MAX)) as u16;
    let input_width = width.saturating_sub(prefix_width);
    let editor = dialog.active_editor();
    let cursor = text_before_cursor_width(editor.text(), editor.cursor_byte()).min(input_width);
    Some(Position::new(
        inner
            .x
            .saturating_add(prefix_width)
            .saturating_add(cursor)
            .min(inner.right().saturating_sub(1)),
        inner.y.saturating_add(2 + field_index as u16),
    ))
}

pub(super) fn milestone_choices(dialog: &MilestoneDialog) -> Vec<MilestoneChoice> {
    let mut choices = vec![MilestoneChoice::Clear];
    if let MilestoneDialogState::Loaded(milestones) = &dialog.state {
        let matches = milestones
            .iter()
            .filter(|milestone| milestone_matches_prefix(milestone, &dialog.input))
            .cloned()
            .map(MilestoneChoice::Set)
            .collect::<Vec<_>>();
        choices.extend(matches);

        let title = dialog.input.trim();
        let has_exact = milestones
            .iter()
            .any(|milestone| milestone.title.eq_ignore_ascii_case(title));
        if !title.is_empty() && !has_exact {
            choices.push(MilestoneChoice::Create(title.to_string()));
        }
    }
    choices
}

pub(super) fn milestone_matches_prefix(milestone: &Milestone, prefix: &str) -> bool {
    let prefix = prefix.trim().to_ascii_lowercase();
    prefix.is_empty() || milestone.title.to_ascii_lowercase().starts_with(&prefix)
}

pub(super) fn milestone_choice_label(choice: &MilestoneChoice) -> String {
    match choice {
        MilestoneChoice::Clear => "Clear milestone".to_string(),
        MilestoneChoice::Set(milestone) => format!("{} (#{})", milestone.title, milestone.number),
        MilestoneChoice::Create(title) => format!("Create milestone \"{title}\""),
    }
}

pub(super) fn clamp_milestone_dialog_selection(dialog: &mut MilestoneDialog) {
    let count = milestone_choices(dialog).len();
    if count == 0 {
        dialog.selected = 0;
    } else {
        dialog.selected = dialog.selected.min(count - 1);
    }
}

pub(super) fn reset_milestone_dialog_selection(dialog: &mut MilestoneDialog) {
    let choices = milestone_choices(dialog);
    dialog.selected = if !dialog.input.trim().is_empty() && choices.len() > 1 {
        1
    } else {
        0
    };
    clamp_milestone_dialog_selection(dialog);
}

pub(super) fn draw_message_dialog(frame: &mut Frame<'_>, dialog: &MessageDialog, area: Rect) {
    let dialog_area = centered_rect(78, message_dialog_height(dialog, area), area);
    let footer = if dialog.kind == MessageDialogKind::RetryableError {
        "Enter: cancel  Esc: edit and retry"
    } else if dialog.auto_close_at.is_some() {
        "Auto closes shortly | Enter/Esc: close"
    } else {
        "Enter/Esc: close"
    };
    let accent = message_dialog_accent(dialog);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(active_theme().panel().fg(accent))
        .style(modal_surface_style())
        .title(Span::styled(
            dialog.title.clone(),
            active_theme()
                .panel()
                .fg(accent)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(dialog.body.clone())
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    draw_modal_footer(frame, area, dialog_area, modal_footer_line(footer));
}

pub(super) fn message_dialog_height(dialog: &MessageDialog, area: Rect) -> u16 {
    let line_count = dialog.body.lines().count().max(1);
    let desired = (line_count + 4).min(usize::from(u16::MAX)) as u16;
    let max_height = area.height.saturating_sub(2).max(1);
    desired.max(9.min(max_height)).min(max_height)
}

pub(super) fn message_dialog_accent(dialog: &MessageDialog) -> Color {
    let theme = active_theme();
    match dialog.kind {
        MessageDialogKind::Info => theme.warning,
        MessageDialogKind::Success => theme.success,
        MessageDialogKind::Error | MessageDialogKind::RetryableError => theme.error,
    }
}

pub(super) fn draw_global_search_loading_dialog(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let dialog_area = centered_rect(58, 9, area);
    let elapsed_secs = app
        .global_search_started_at
        .map(|started| started.elapsed().as_secs())
        .unwrap_or(0);
    let lines = global_search_loading_content(app, elapsed_secs);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(themed_fg_style(Color::LightMagenta))
        .style(modal_surface_style())
        .title(Span::styled(
            "Searching",
            themed_bold_style(Color::LightMagenta),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
}

pub(super) fn draw_section_page_loading_dialog(
    frame: &mut Frame<'_>,
    loading: &SectionPageLoading,
    area: Rect,
) {
    let dialog_area = centered_rect(58, 9, area);
    let elapsed_secs = loading.started_at.elapsed().as_secs();
    let lines = section_page_loading_content(loading, elapsed_secs);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(themed_fg_style(Color::Cyan))
        .style(modal_surface_style())
        .title(Span::styled("Loading Page", themed_bold_style(Color::Cyan)));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
}

pub(super) fn section_page_loading_content(
    loading: &SectionPageLoading,
    elapsed_secs: u64,
) -> Vec<Line<'static>> {
    vec![
        Line::from("Fetching GitHub result page."),
        Line::from(""),
        startup_loading_line(elapsed_secs),
        startup_progress_line(elapsed_secs),
        Line::from(""),
        key_value_line("section", loading.title.clone()),
        key_value_line("page", loading.page_label.clone()),
    ]
}

pub(super) fn section_page_loading_status(loading: &SectionPageLoading) -> String {
    format!("loading {} page {}", loading.title, loading.page_label)
}

pub(super) fn global_search_loading_content(
    app: &AppState,
    elapsed_secs: u64,
) -> Vec<Line<'static>> {
    let scope = app
        .global_search_scope
        .as_deref()
        .map(str::to_string)
        .unwrap_or_else(|| "GitHub".to_string());
    vec![
        Line::from("Searching pull requests and issues."),
        Line::from(""),
        startup_loading_line(elapsed_secs),
        startup_progress_line(elapsed_secs),
        Line::from(""),
        key_value_line("query", app.global_search_query.clone()),
        key_value_line("scope", scope),
    ]
}

pub(super) fn key_value_line(key: &'static str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key}: "), themed_fg_style(Color::Gray)),
        Span::raw(value),
    ])
}
