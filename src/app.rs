use std::collections::{HashMap, HashSet};
use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{DateTime, Utc};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use pulldown_cmark::{Event as MarkdownEvent, Options, Parser, Tag, TagEnd};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Row, Table, TableState, Tabs, Wrap};
use ratatui::{Frame, Terminal};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tracing::warn;

use crate::config::Config;
use crate::dirs::Paths;
use crate::github::{
    edit_issue_comment, fetch_issue_comments, post_issue_comment, refresh_dashboard,
};
use crate::model::{
    CommentPreview, ItemKind, SectionKind, SectionSnapshot, WorkItem, configured_sections,
    merge_cached_sections, merge_refreshed_sections, section_counts,
};
use crate::snapshot::SnapshotStore;
use crate::state::{DEFAULT_LIST_WIDTH_PERCENT, UiState, clamp_list_width_percent};

enum AppMsg {
    RefreshStarted,
    RefreshFinished {
        sections: Vec<SectionSnapshot>,
        save_error: Option<String>,
    },
    DetailsLoaded {
        item_id: String,
        result: std::result::Result<Vec<CommentPreview>, String>,
    },
    CommentPosted {
        item_id: String,
        result: std::result::Result<Vec<CommentPreview>, String>,
    },
    CommentUpdated {
        item_id: String,
        comment_index: usize,
        result: std::result::Result<Vec<CommentPreview>, String>,
    },
}

#[derive(Debug, Clone)]
enum DetailState {
    Loading,
    Loaded(Vec<CommentPreview>),
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusTarget {
    List,
    Details,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetupDialog {
    MissingGh,
    AuthRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CommentDialogMode {
    New,
    Reply {
        comment_index: usize,
        author: String,
    },
    Edit {
        comment_index: usize,
        comment_id: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommentDialog {
    mode: CommentDialogMode,
    body: String,
    scroll: u16,
}

struct PendingCommentSubmit {
    item: WorkItem,
    body: String,
    mode: PendingCommentMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingCommentMode {
    Post,
    Edit {
        comment_index: usize,
        comment_id: u64,
    },
}

const TABLE_HEADER_HEIGHT: u16 = 2;
const TAB_DIVIDER_WIDTH: u16 = 3;
const MOUSE_SCROLL_LINES: u16 = 3;
const COMMENT_DIALOG_WIDTH_PERCENT: u16 = 72;
const COMMENT_DIALOG_MIN_HEIGHT: u16 = 10;
const COMMENT_DIALOG_VERTICAL_MARGIN: u16 = 4;
const COMMENT_DIALOG_FOOTER_HEIGHT: u16 = 2;
const COMMENT_DIALOG_MIN_EDITOR_HEIGHT: u16 = 4;
const COMMENT_DIALOG_EDITOR_PADDING_LINES: u16 = 1;
const COMMENT_DIALOG_FALLBACK_EDITOR_HEIGHT: u16 = 10;
const COMMENT_DIALOG_FALLBACK_EDITOR_WIDTH: u16 = 48;

struct AppState {
    active_view: SectionKind,
    sections: Vec<SectionSnapshot>,
    section_index: [usize; 3],
    selected_index: [usize; 3],
    focus: FocusTarget,
    details_scroll: u16,
    list_width_percent: u16,
    dragging_split: bool,
    split_drag_changed: bool,
    search_active: bool,
    search_query: String,
    status: String,
    refreshing: bool,
    last_refresh_request: Instant,
    details: HashMap<String, DetailState>,
    details_stale: HashSet<String>,
    selected_comment_index: usize,
    comment_dialog: Option<CommentDialog>,
    posting_comment: bool,
    setup_dialog: Option<SetupDialog>,
    help_dialog: bool,
}

pub async fn run(config: Config, paths: Paths, store: SnapshotStore) -> Result<()> {
    let cached = store.load_all()?;
    let sections = merge_cached_sections(configured_sections(&config), cached);
    let ui_state = UiState::load_or_default(&paths.state_path);
    let mut app = AppState::with_ui_state(config.defaults.view, sections, ui_state);

    let (tx, mut rx) = mpsc::unbounded_channel();
    start_refresh(config.clone(), store.clone(), tx.clone());

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
        )
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(
        &mut terminal,
        &mut app,
        &config,
        &paths,
        &store,
        &tx,
        &mut rx,
    )
    .await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        PopKeyboardEnhancementFlags,
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;

    result
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut AppState,
    config: &Config,
    paths: &Paths,
    store: &SnapshotStore,
    tx: &UnboundedSender<AppMsg>,
    rx: &mut UnboundedReceiver<AppMsg>,
) -> Result<()> {
    loop {
        while let Ok(message) = rx.try_recv() {
            app.handle_msg(message);
        }
        app.ensure_current_details_loading(tx);

        if !app.refreshing
            && config.defaults.refetch_interval_seconds > 0
            && app.last_refresh_request.elapsed().as_secs()
                >= config.defaults.refetch_interval_seconds
        {
            start_refresh(config.clone(), store.clone(), tx.clone());
        }

        terminal.draw(|frame| draw(frame, app, paths))?;

        if event::poll(Duration::from_millis(120))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }

                    let size = terminal.size()?;
                    let area = Rect::new(0, 0, size.width, size.height);
                    if handle_key_in_area(app, key, config, store, tx, Some(area)) {
                        break;
                    }
                }
                Event::Mouse(mouse) => {
                    let size = terminal.size()?;
                    if handle_mouse(app, mouse, Rect::new(0, 0, size.width, size.height)) {
                        save_ui_state(app, paths);
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}

fn start_refresh(config: Config, store: SnapshotStore, tx: UnboundedSender<AppMsg>) {
    let _ = tx.send(AppMsg::RefreshStarted);
    tokio::spawn(async move {
        let sections = refresh_dashboard(&config).await;
        let mut save_error = None;
        for section in &sections {
            if section.error.is_some() {
                continue;
            }
            if let Err(error) = store.save_section(section) {
                let message = error.to_string();
                warn!(error = %message, "failed to save refreshed snapshot");
                save_error = Some(message);
                break;
            }
        }
        let _ = tx.send(AppMsg::RefreshFinished {
            sections,
            save_error,
        });
    });
}

fn start_details_load(item: WorkItem, tx: UnboundedSender<AppMsg>) {
    tokio::spawn(async move {
        let result = match item.number {
            Some(number) => fetch_issue_comments(&item.repo, number)
                .await
                .map_err(|error| error.to_string()),
            None => Ok(Vec::new()),
        };
        let _ = tx.send(AppMsg::DetailsLoaded {
            item_id: item.id,
            result,
        });
    });
}

fn start_comment_submit(item: WorkItem, body: String, tx: UnboundedSender<AppMsg>) {
    tokio::spawn(async move {
        let result = match item.number {
            Some(number) => match post_issue_comment(&item.repo, number, &body).await {
                Ok(()) => fetch_issue_comments(&item.repo, number)
                    .await
                    .map_err(|error| error.to_string()),
                Err(error) => Err(error.to_string()),
            },
            None => Err("selected item has no issue or pull request number".to_string()),
        };
        let _ = tx.send(AppMsg::CommentPosted {
            item_id: item.id,
            result,
        });
    });
}

fn start_comment_edit(
    item: WorkItem,
    comment_index: usize,
    comment_id: u64,
    body: String,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let result = match item.number {
            Some(number) => match edit_issue_comment(&item.repo, comment_id, &body).await {
                Ok(()) => fetch_issue_comments(&item.repo, number)
                    .await
                    .map_err(|error| error.to_string()),
                Err(error) => Err(error.to_string()),
            },
            None => Err("selected item has no issue or pull request number".to_string()),
        };
        let _ = tx.send(AppMsg::CommentUpdated {
            item_id: item.id,
            comment_index,
            result,
        });
    });
}

#[cfg(test)]
fn handle_key(
    app: &mut AppState,
    key: KeyEvent,
    config: &Config,
    store: &SnapshotStore,
    tx: &UnboundedSender<AppMsg>,
) -> bool {
    handle_key_in_area(app, key, config, store, tx, None)
}

fn handle_key_in_area(
    app: &mut AppState,
    key: KeyEvent,
    config: &Config,
    store: &SnapshotStore,
    tx: &UnboundedSender<AppMsg>,
    area: Option<Rect>,
) -> bool {
    if app.setup_dialog.is_some() {
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Esc | KeyCode::Enter => app.dismiss_setup_dialog(),
            _ => {}
        }
        return false;
    }

    if app.comment_dialog.is_some() {
        app.handle_comment_dialog_key(key, tx, area);
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

    if app.focus == FocusTarget::Details {
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Esc => app.focus_list(),
            KeyCode::Char('?') => app.show_help_dialog(),
            KeyCode::Char('4') => app.focus_primary_list(),
            KeyCode::Char('o') => app.open_selected(),
            KeyCode::Char('a') => app.start_new_comment_dialog(),
            KeyCode::Char('R') => app.start_reply_to_selected_comment(),
            KeyCode::Char('e') => app.start_edit_selected_comment_dialog(),
            KeyCode::Char('r') => trigger_refresh(app, config, store, tx),
            KeyCode::Char('n') => app.move_comment(1),
            KeyCode::Char('p') => app.move_comment(-1),
            KeyCode::Down | KeyCode::Char('j') => app.scroll_details(1),
            KeyCode::Up | KeyCode::Char('k') => app.scroll_details(-1),
            KeyCode::PageDown | KeyCode::Char('d') => app.scroll_details(8),
            KeyCode::PageUp | KeyCode::Char('u') => app.scroll_details(-8),
            KeyCode::Char('g') => app.details_scroll = 0,
            _ => {}
        }
        return false;
    }

    match key.code {
        KeyCode::Char('q') => return true,
        KeyCode::Esc if !app.search_query.is_empty() => app.clear_search(),
        KeyCode::Esc => {}
        KeyCode::Char('?') => app.show_help_dialog(),
        KeyCode::Char('/') => app.start_search(),
        KeyCode::Char('5') => app.focus_details(),
        KeyCode::Tab => app.switch_view(app.active_view.next()),
        KeyCode::BackTab => app.switch_view(app.active_view.previous()),
        KeyCode::Char('1') => app.switch_view(SectionKind::PullRequests),
        KeyCode::Char('2') => app.switch_view(SectionKind::Issues),
        KeyCode::Char('3') => app.switch_view(SectionKind::Notifications),
        KeyCode::Char('4') => app.focus_primary_list(),
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(']') => app.move_section(1),
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('[') => app.move_section(-1),
        KeyCode::Down | KeyCode::Char('j') => app.move_selection(1),
        KeyCode::Up | KeyCode::Char('k') => app.move_selection(-1),
        KeyCode::PageDown | KeyCode::Char('d') => {
            app.move_selection(list_page_delta(app, area, 1));
        }
        KeyCode::PageUp | KeyCode::Char('u') => {
            app.move_selection(list_page_delta(app, area, -1));
        }
        KeyCode::Char('g') => app.set_selection(0),
        KeyCode::Char('G') => app.select_last(),
        KeyCode::Char('r') => trigger_refresh(app, config, store, tx),
        KeyCode::Char('a') => app.start_new_comment_dialog(),
        KeyCode::Char('o') => app.open_selected(),
        KeyCode::Enter => app.focus_details(),
        _ => {}
    }

    false
}

fn trigger_refresh(
    app: &mut AppState,
    config: &Config,
    store: &SnapshotStore,
    tx: &UnboundedSender<AppMsg>,
) {
    if app.refreshing {
        app.status = "refresh already running".to_string();
    } else {
        start_refresh(config.clone(), store.clone(), tx.clone());
    }
}

fn save_ui_state(app: &mut AppState, paths: &Paths) {
    if let Err(error) = app.ui_state().save(&paths.state_path) {
        let message = error.to_string();
        warn!(error = %message, "failed to save ui state");
        app.status = format!("layout save failed: {message}");
    }
}

fn handle_mouse(app: &mut AppState, mouse: MouseEvent, area: Rect) -> bool {
    if app.setup_dialog.is_some() {
        return false;
    }
    if app.help_dialog {
        return false;
    }
    if let Some(dialog) = &app.comment_dialog {
        let dialog_area = comment_dialog_area(dialog, area);
        if rect_contains(dialog_area, mouse.column, mouse.row) {
            match mouse.kind {
                MouseEventKind::ScrollDown => {
                    app.scroll_comment_dialog(MOUSE_SCROLL_LINES as i16, Some(area))
                }
                MouseEventKind::ScrollUp => {
                    app.scroll_comment_dialog(-(MOUSE_SCROLL_LINES as i16), Some(area))
                }
                _ => {}
            }
        }
        return false;
    }

    let page = page_areas(area);
    let body_area = page[2];
    let body = body_areas_with_ratio(body_area, app.list_width_percent);
    let details_area = body[1];

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if splitter_contains(body_area, body[0], details_area, mouse.column, mouse.row) {
                app.start_split_drag();
                return false;
            }
            handle_left_click(app, mouse, page[0], page[1], body[0], details_area);
        }
        MouseEventKind::Drag(MouseButton::Left) if app.dragging_split => {
            app.update_split_drag(body_area, mouse.column);
        }
        MouseEventKind::Up(MouseButton::Left) if app.dragging_split => {
            app.update_split_drag(body_area, mouse.column);
            return app.finish_split_drag();
        }
        MouseEventKind::ScrollDown if rect_contains(body[0], mouse.column, mouse.row) => {
            handle_list_scroll(app, MOUSE_SCROLL_LINES as isize);
        }
        MouseEventKind::ScrollUp if rect_contains(body[0], mouse.column, mouse.row) => {
            handle_list_scroll(app, -(MOUSE_SCROLL_LINES as isize));
        }
        MouseEventKind::ScrollDown if rect_contains(details_area, mouse.column, mouse.row) => {
            handle_details_scroll(app, details_area, MOUSE_SCROLL_LINES as i16);
        }
        MouseEventKind::ScrollUp if rect_contains(details_area, mouse.column, mouse.row) => {
            handle_details_scroll(app, details_area, -(MOUSE_SCROLL_LINES as i16));
        }
        _ => {}
    }

    false
}

fn handle_left_click(
    app: &mut AppState,
    mouse: MouseEvent,
    view_tabs_area: Rect,
    section_tabs_area: Rect,
    table_area: Rect,
    details_area: Rect,
) {
    if let Some(view) = view_tab_at(view_tabs_area, mouse.column, mouse.row) {
        app.switch_view(view);
        app.search_active = false;
        app.status = "list focused".to_string();
        return;
    }

    if let Some(section_index) = section_tab_at(app, section_tabs_area, mouse.column, mouse.row) {
        app.select_section(section_index);
        return;
    }

    if rect_contains(table_area, mouse.column, mouse.row) {
        handle_table_click(app, mouse, table_area);
        return;
    }

    if !rect_contains(details_area, mouse.column, mouse.row) {
        return;
    }

    app.focus = FocusTarget::Details;
    app.search_active = false;

    let inner = block_inner(details_area);
    if !rect_contains(inner, mouse.column, mouse.row) {
        return;
    }

    let document = build_details_document(app, inner.width);
    let line_index = app.details_scroll as usize + (mouse.row - inner.y) as usize;
    let column = mouse.column - inner.x;
    if let Some(comment_index) = document.comment_at(line_index) {
        app.select_comment(comment_index);
    }
    if let Some(action) = document.action_at(line_index, column) {
        app.handle_detail_action(action);
        return;
    }
    if let Some(url) = document.link_at(line_index, column) {
        app.open_url(&url);
    }
}

fn handle_details_scroll(app: &mut AppState, area: Rect, delta: i16) {
    app.focus = FocusTarget::Details;
    app.search_active = false;

    let max_scroll = max_details_scroll(app, area);
    if max_scroll == 0 {
        app.details_scroll = 0;
        return;
    }

    if delta < 0 {
        app.details_scroll = app.details_scroll.saturating_sub(delta.unsigned_abs());
    } else {
        app.details_scroll = app.details_scroll.saturating_add(delta as u16);
    }
    app.details_scroll = app.details_scroll.min(max_scroll);
}

fn handle_list_scroll(app: &mut AppState, delta: isize) {
    app.focus = FocusTarget::List;
    app.search_active = false;
    app.move_selection(delta);
}

fn list_page_delta(app: &AppState, area: Option<Rect>, direction: isize) -> isize {
    let rows = area
        .map(|area| {
            let body = body_areas_with_ratio(page_areas(area)[2], app.list_width_percent);
            usize::from(table_visible_rows(body[0]).max(1))
        })
        .unwrap_or(10);
    direction.saturating_mul(rows as isize)
}

fn max_details_scroll(app: &AppState, area: Rect) -> u16 {
    let inner = block_inner(area);
    let document = build_details_document(app, inner.width);
    let max = document
        .lines
        .len()
        .saturating_sub(usize::from(inner.height));
    max.min(usize::from(u16::MAX)) as u16
}

fn handle_table_click(app: &mut AppState, mouse: MouseEvent, area: Rect) {
    let Some(position) = table_row_at(app, area, mouse.row) else {
        return;
    };

    app.set_selection(position);
    app.focus_details();
}

fn table_row_at(app: &AppState, area: Rect, row: u16) -> Option<usize> {
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

    let offset = table_viewport_offset(app.current_selected_position(), usize::from(visible_rows));
    let position = offset + usize::from(row - data_start);
    (position < filtered_len).then_some(position)
}

fn table_visible_rows(area: Rect) -> u16 {
    block_inner(area).height.saturating_sub(TABLE_HEADER_HEIGHT)
}

fn table_visible_range(selected: usize, visible_rows: usize, len: usize) -> Option<(usize, usize)> {
    if visible_rows == 0 || len == 0 {
        return None;
    }

    let offset = table_viewport_offset(selected.min(len - 1), visible_rows);
    let end = offset.saturating_add(visible_rows).min(len);
    Some((offset + 1, end))
}

fn table_viewport_offset(selected: usize, visible_rows: usize) -> usize {
    if visible_rows == 0 {
        return 0;
    }
    selected.saturating_sub(visible_rows - 1)
}

fn view_tab_at(area: Rect, column: u16, row: u16) -> Option<SectionKind> {
    let views = [
        SectionKind::PullRequests,
        SectionKind::Issues,
        SectionKind::Notifications,
    ];
    tab_index_at(
        &views.map(|view| view.label().to_string()),
        area,
        column,
        row,
    )
    .and_then(|index| views.get(index).copied())
}

fn section_tab_at(app: &AppState, area: Rect, column: u16, row: u16) -> Option<usize> {
    let labels = app
        .visible_sections()
        .iter()
        .map(|section| section_tab_label(app, section))
        .collect::<Vec<_>>();
    tab_index_at(&labels, area, column, row)
}

fn tab_index_at(labels: &[String], area: Rect, column: u16, row: u16) -> Option<usize> {
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

fn draw(frame: &mut Frame<'_>, app: &AppState, paths: &Paths) {
    let area = frame.area();
    let chunks = page_areas(area);

    draw_view_tabs(frame, app, chunks[0]);
    draw_section_tabs(frame, app, chunks[1]);

    let body = body_areas_with_ratio(chunks[2], app.list_width_percent);
    draw_table(frame, app, body[0]);
    draw_details(frame, app, body[1]);
    draw_footer(frame, app, paths, chunks[3]);

    if let Some(dialog) = app.setup_dialog {
        draw_setup_dialog(frame, dialog, area);
    } else if app.help_dialog {
        draw_help_dialog(frame, area);
    } else if let Some(dialog) = &app.comment_dialog {
        draw_comment_dialog(frame, dialog, area);
    }
}

#[cfg(test)]
fn body_area(area: Rect) -> Rect {
    page_areas(area)[2]
}

fn page_areas(area: Rect) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .split(area)
}

#[cfg(test)]
fn body_areas(area: Rect) -> std::rc::Rc<[Rect]> {
    body_areas_with_ratio(area, DEFAULT_LIST_WIDTH_PERCENT)
}

fn body_areas_with_ratio(area: Rect, list_width_percent: u16) -> std::rc::Rc<[Rect]> {
    let list_width_percent = clamp_list_width_percent(list_width_percent);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(list_width_percent),
            Constraint::Percentage(100 - list_width_percent),
        ])
        .split(area)
}

fn block_inner(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}

fn rect_contains(area: Rect, x: u16, y: u16) -> bool {
    x >= area.x
        && x < area.x.saturating_add(area.width)
        && y >= area.y
        && y < area.y.saturating_add(area.height)
}

fn splitter_contains(body: Rect, list: Rect, details: Rect, x: u16, y: u16) -> bool {
    if !rect_contains(body, x, y) {
        return false;
    }

    let list_border = list.x.saturating_add(list.width).saturating_sub(1);
    x == list_border || x == details.x
}

fn split_percent_from_column(body: Rect, column: u16) -> u16 {
    if body.width == 0 {
        return DEFAULT_LIST_WIDTH_PERCENT;
    }

    let left_width = column.saturating_sub(body.x).min(body.width);
    let percent = (u32::from(left_width) * 100 + u32::from(body.width) / 2) / u32::from(body.width);
    clamp_list_width_percent(percent as u16)
}

fn draw_view_tabs(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let views = [
        SectionKind::PullRequests,
        SectionKind::Issues,
        SectionKind::Notifications,
    ];
    let titles = views
        .iter()
        .map(|kind| Line::from(kind.label()))
        .collect::<Vec<_>>();
    let active = views
        .iter()
        .position(|kind| *kind == app.active_view)
        .unwrap_or(0);

    let tabs = Tabs::new(titles)
        .select(active)
        .block(Block::default().borders(Borders::ALL).title("ghr"))
        .style(Style::default().fg(Color::Gray))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_widget(tabs, area);
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

    let tabs = Tabs::new(titles)
        .select(app.current_section_position())
        .block(Block::default().borders(Borders::ALL).title("sections"))
        .style(Style::default().fg(Color::Gray))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_widget(tabs, area);
}

fn section_tab_label(app: &AppState, section: &SectionSnapshot) -> String {
    let (total, unread) = section_counts(section);
    if !app.search_query.is_empty() {
        format!(
            "{} ({}/{total})",
            section.title,
            app.filtered_indices(section).len()
        )
    } else if unread > 0 {
        format!("{} ({total}/{unread})", section.title)
    } else {
        format!("{} ({total})", section.title)
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
            Row::new(vec![
                relative_time(item.updated_at),
                item.repo.clone(),
                item.number
                    .map(|number| format!("#{number}"))
                    .unwrap_or_default(),
                item.title.clone(),
                item_meta(item),
            ])
        })
        .collect::<Vec<_>>();

    let header = Row::new(vec!["Updated", "Repo", "#", "Title", "Meta"])
        .style(
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        )
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
    if let Some(error) = &section.error {
        title.push_str(&format!(" - error: {error}"));
    };
    if let Some((start, end)) = table_visible_range(
        app.current_selected_position(),
        usize::from(table_visible_rows(area)),
        filtered_indices.len(),
    ) {
        title.push_str(&format!(
            " | showing {start}-{end}/{}",
            filtered_indices.len()
        ));
    }

    let border_style = if app.dragging_split {
        Style::default().fg(Color::LightMagenta)
    } else if app.focus == FocusTarget::List {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::Gray)
    };

    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Length(24),
            Constraint::Length(8),
            Constraint::Min(20),
            Constraint::Length(18),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(title),
    )
    .row_highlight_style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("> ");

    let mut table_state = TableState::default();
    if !filtered_indices.is_empty() {
        table_state.select(Some(app.current_selected_position()));
    }
    frame.render_stateful_widget(table, area, &mut table_state);
}

fn draw_details(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let title = details_title();
    let (border_style, title_style) = if app.dragging_split {
        (
            Style::default().fg(Color::LightMagenta),
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        )
    } else if app.focus == FocusTarget::Details {
        (
            Style::default().fg(Color::Yellow),
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            Style::default().fg(Color::Gray),
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        )
    };

    let document = build_details_document(app, area.width.saturating_sub(2));

    let details = Paragraph::new(Text::from(document.lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(Span::styled(title.to_string(), title_style)),
        )
        .scroll((app.details_scroll, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(details, area);
}

fn details_title() -> &'static str {
    "Details:"
}

fn draw_footer(frame: &mut Frame<'_>, app: &AppState, paths: &Paths, area: Rect) {
    let refresh = if app.refreshing { "refreshing" } else { "idle" };
    let focus = match app.focus {
        FocusTarget::List => "list",
        FocusTarget::Details => "details",
    };
    let search = if app.search_active {
        format!("search: /{}_", app.search_query)
    } else if app.search_query.is_empty() {
        "/ search".to_string()
    } else {
        format!("filter: /{}", app.search_query)
    };
    let text = format!(
        "tab/1-3 view  ? help  4 list  h/l section  j/k move  pg/d/u page  enter/5 details  a comment  R reply  e edit  r refresh  o open  q quit | drag split | focus {focus} | {search} | {refresh} | {} | db {}",
        app.status,
        paths.db_path.display()
    );
    let footer = Paragraph::new(text).style(Style::default().fg(Color::Gray));
    frame.render_widget(footer, area);
}

fn draw_setup_dialog(frame: &mut Frame<'_>, dialog: SetupDialog, area: Rect) {
    let (title, lines) = setup_dialog_content(dialog);
    let dialog_area = centered_rect(66, 15, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .style(Style::default().bg(Color::Black))
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(Style::default().fg(Color::White).bg(Color::Black))
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
}

fn draw_help_dialog(frame: &mut Frame<'_>, area: Rect) {
    let lines = help_dialog_content();
    let height = help_dialog_height(lines.len(), area);
    let dialog_area = centered_rect(70, height, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::LightMagenta))
        .style(Style::default().bg(Color::Black))
        .title(Span::styled(
            "Help",
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(Style::default().fg(Color::White).bg(Color::Black))
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
}

fn help_dialog_height(line_count: usize, area: Rect) -> u16 {
    if area.height == 0 {
        return 0;
    }

    let desired = (line_count + 2).min(usize::from(u16::MAX)) as u16;
    desired
        .max(12.min(area.height))
        .min(area.height.saturating_sub(2).max(1))
}

fn draw_comment_dialog(frame: &mut Frame<'_>, dialog: &CommentDialog, area: Rect) {
    let title = match &dialog.mode {
        CommentDialogMode::New => "New Comment",
        CommentDialogMode::Reply { author, .. } => {
            return draw_reply_dialog(frame, dialog, author, area);
        }
        CommentDialogMode::Edit { .. } => "Edit Comment",
    };
    draw_comment_editor(frame, title, dialog, area);
}

fn draw_reply_dialog(frame: &mut Frame<'_>, dialog: &CommentDialog, author: &str, area: Rect) {
    draw_comment_editor(frame, &format!("Reply to @{author}"), dialog, area);
}

fn draw_comment_editor(frame: &mut Frame<'_>, title: &str, dialog: &CommentDialog, area: Rect) {
    let dialog_area = comment_dialog_area(dialog, area);
    let inner = block_inner(dialog_area);
    let footer_height = COMMENT_DIALOG_FOOTER_HEIGHT.min(inner.height);
    let editor_height = inner.height.saturating_sub(footer_height).max(1);
    let editor_width = inner.width.max(1);
    let body_lines = comment_dialog_body_lines(&dialog.body, editor_width);
    let max_scroll = max_comment_dialog_scroll(&dialog.body, editor_width, editor_height);
    let scroll = dialog.scroll.min(max_scroll);
    let mut lines = body_lines
        .into_iter()
        .skip(usize::from(scroll))
        .take(usize::from(editor_height))
        .map(Line::from)
        .collect::<Vec<_>>();
    while lines.len() < usize::from(editor_height) {
        lines.push(Line::from(""));
    }
    let footer_line = Line::from(vec![
        Span::styled("Ctrl+Enter", Style::default().fg(Color::Yellow)),
        Span::raw(" send  "),
        Span::styled("Enter", Style::default().fg(Color::Yellow)),
        Span::raw(" newline  "),
        Span::styled("Pg/Wheel", Style::default().fg(Color::Yellow)),
        Span::raw(" scroll  "),
        Span::styled("Esc", Style::default().fg(Color::Yellow)),
        Span::raw(" cancel"),
    ]);
    lines.push(Line::from(""));
    lines.push(footer_line);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::LightMagenta))
        .style(Style::default().bg(Color::Black))
        .title(Span::styled(
            title.to_string(),
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(Style::default().fg(Color::White).bg(Color::Black));

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
    if let Some(position) = comment_dialog_cursor_position(
        &dialog.body,
        scroll,
        dialog_area,
        editor_width,
        editor_height,
    ) {
        frame.set_cursor_position(position);
    }
}

fn comment_dialog_cursor_position(
    body: &str,
    scroll: u16,
    area: Rect,
    editor_width: u16,
    editor_height: u16,
) -> Option<Position> {
    let inner = block_inner(area);
    let width = editor_width.max(1);
    let height = editor_height.max(1);
    let (line, column) = comment_dialog_cursor_offset(body, width);
    let visible_end = scroll.saturating_add(height);
    if line < scroll || line >= visible_end {
        return None;
    }

    let visible_line = line.saturating_sub(scroll);
    Some(Position::new(
        inner.x.saturating_add(column.min(width.saturating_sub(1))),
        inner.y.saturating_add(visible_line),
    ))
}

fn comment_dialog_area(dialog: &CommentDialog, area: Rect) -> Rect {
    let width = centered_rect_width(COMMENT_DIALOG_WIDTH_PERCENT, area);
    let editor_width = width.saturating_sub(2).max(1);
    let editor_height = comment_dialog_desired_editor_height(&dialog.body, editor_width);
    let desired_height = editor_height
        .saturating_add(COMMENT_DIALOG_FOOTER_HEIGHT)
        .saturating_add(2);
    let min_height = comment_dialog_min_height(area);
    let max_height = comment_dialog_max_height(area);
    let height = desired_height.max(min_height).min(max_height);
    centered_rect_with_size(width, height, area)
}

fn comment_dialog_min_height(area: Rect) -> u16 {
    if area.height == 0 {
        0
    } else {
        COMMENT_DIALOG_MIN_HEIGHT.min(area.height)
    }
}

fn comment_dialog_max_height(area: Rect) -> u16 {
    if area.height == 0 {
        return 0;
    }

    let min_height = comment_dialog_min_height(area);
    area.height
        .saturating_sub(COMMENT_DIALOG_VERTICAL_MARGIN)
        .max(min_height)
        .min(area.height)
}

fn comment_dialog_desired_editor_height(text: &str, width: u16) -> u16 {
    let line_count = comment_dialog_body_lines(text, width)
        .len()
        .min(usize::from(u16::MAX)) as u16;
    line_count
        .saturating_add(COMMENT_DIALOG_EDITOR_PADDING_LINES)
        .max(COMMENT_DIALOG_MIN_EDITOR_HEIGHT)
}

fn comment_dialog_editor_size(dialog: &CommentDialog, area: Option<Rect>) -> (u16, u16) {
    if let Some(area) = area {
        let dialog_area = comment_dialog_area(dialog, area);
        let inner = block_inner(dialog_area);
        let footer_height = COMMENT_DIALOG_FOOTER_HEIGHT.min(inner.height);
        return (
            inner.width.max(1),
            inner.height.saturating_sub(footer_height).max(1),
        );
    }

    (
        COMMENT_DIALOG_FALLBACK_EDITOR_WIDTH,
        COMMENT_DIALOG_FALLBACK_EDITOR_HEIGHT,
    )
}

fn comment_dialog_body_lines(text: &str, width: u16) -> Vec<String> {
    let width = usize::from(width.max(1));
    let mut lines = Vec::new();
    for raw_line in text.split('\n') {
        if raw_line.is_empty() {
            lines.push(String::new());
            continue;
        }

        let mut current = String::new();
        let mut column = 0_usize;
        for ch in raw_line.chars() {
            if column >= width {
                lines.push(std::mem::take(&mut current));
                column = 0;
            }
            current.push(ch);
            column += 1;
        }
        lines.push(current);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

fn max_comment_dialog_scroll(text: &str, width: u16, editor_height: u16) -> u16 {
    let line_count = comment_dialog_scrollable_line_count(text, width);
    line_count
        .saturating_sub(usize::from(editor_height.max(1)))
        .min(usize::from(u16::MAX)) as u16
}

fn comment_dialog_scrollable_line_count(text: &str, width: u16) -> usize {
    let body_line_count = comment_dialog_body_lines(text, width).len();
    let (cursor_line, _) = comment_dialog_cursor_offset(text, width);
    body_line_count.max(usize::from(cursor_line).saturating_add(1))
}

fn scroll_for_comment_dialog_cursor(
    text: &str,
    width: u16,
    height: u16,
    current_scroll: u16,
) -> u16 {
    let width = width.max(1);
    let height = height.max(1);
    let (line, _) = comment_dialog_cursor_offset(text, width);
    let max_scroll = max_comment_dialog_scroll(text, width, height);
    if line < current_scroll {
        line
    } else if line >= current_scroll.saturating_add(height) {
        line.saturating_sub(height.saturating_sub(1))
            .min(max_scroll)
    } else {
        current_scroll.min(max_scroll)
    }
}

fn comment_dialog_cursor_offset(text: &str, width: u16) -> (u16, u16) {
    let width = usize::from(width.max(1));
    let mut line = 0_usize;
    let mut parts = text.split('\n').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            let len = part.chars().count();
            line = line.saturating_add(len / width);
            let column = len % width;
            return (
                line.min(usize::from(u16::MAX)) as u16,
                column.min(usize::from(u16::MAX)) as u16,
            );
        }

        line = line.saturating_add(comment_dialog_raw_line_height(part, width));
    }

    (0, 0)
}

fn comment_dialog_raw_line_height(text: &str, width: usize) -> usize {
    let len = text.chars().count();
    if len == 0 { 1 } else { len.div_ceil(width) }
}

fn setup_dialog_content(dialog: SetupDialog) -> (&'static str, Vec<Line<'static>>) {
    match dialog {
        SetupDialog::MissingGh => (
            "GitHub CLI Required",
            vec![
                Line::from("ghr uses GitHub CLI for authentication and GitHub API access."),
                Line::from(""),
                Line::from("Install GitHub CLI for your system: cli.github.com"),
                command_line("macOS: brew install gh"),
                command_line("Debian/Ubuntu: sudo apt install gh"),
                Line::from(""),
                Line::from("Then authenticate:"),
                command_line("gh auth login"),
                Line::from(""),
                Line::from("After setup, press Esc and then r to refresh."),
                Line::from("Esc: close and use cached data    q: quit"),
            ],
        ),
        SetupDialog::AuthRequired => (
            "GitHub Login Required",
            vec![
                Line::from("GitHub CLI is installed, but it is not authenticated."),
                Line::from(""),
                Line::from("Run this in your terminal:"),
                command_line("gh auth login"),
                Line::from(""),
                Line::from("You can also launch ghr with GH_TOKEN set."),
                Line::from("After setup, press Esc and then r to refresh."),
                Line::from("Esc: close and use cached data    q: quit"),
            ],
        ),
    }
}

fn command_line(command: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            command,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

fn help_dialog_content() -> Vec<Line<'static>> {
    vec![
        help_heading("General"),
        help_key_line("? / Esc / Enter / q", "close this help"),
        help_key_line("q", "quit ghr outside help"),
        help_key_line("r", "refresh from GitHub"),
        help_key_line(
            "Tab / Shift+Tab",
            "switch Pull Requests / Issues / Notification",
        ),
        help_key_line("1 / 2 / 3", "jump to Pull Requests / Issues / Notification"),
        help_key_line("/", "start fuzzy search filtering"),
        help_key_line("Esc", "leave details or clear search"),
        Line::from(""),
        help_heading("List"),
        help_key_line("j/k or Up/Down", "move selection"),
        help_key_line("PgDown/PgUp or d/u", "move by visible page"),
        help_key_line("g / G", "first / last item"),
        help_key_line("h/l or [ / ]", "switch section tabs"),
        help_key_line("Enter or 5", "focus Details"),
        help_key_line("4", "focus primary list"),
        help_key_line("o", "open selected item in browser"),
        help_key_line("a", "add a new issue or PR comment"),
        Line::from(""),
        help_heading("Details"),
        help_key_line("j/k or Up/Down", "scroll details"),
        help_key_line("PgDown/PgUp or d/u", "scroll details by page"),
        help_key_line("g", "scroll details to top"),
        help_key_line("n / p", "focus next / previous comment"),
        help_key_line("a", "add a new comment"),
        help_key_line("R", "reply to focused comment"),
        help_key_line("e", "edit focused comment when it is yours"),
        help_key_line("o", "open selected item in browser"),
        Line::from(""),
        help_heading("Comment Editor"),
        help_key_line("Enter", "insert newline"),
        help_key_line("Ctrl+Enter", "send or update comment"),
        help_key_line("Backspace", "delete previous character"),
        help_key_line("PgDown/PgUp or mouse wheel", "scroll long drafts"),
        help_key_line("Esc", "cancel editing"),
        Line::from(""),
        help_heading("Mouse"),
        help_key_line("click tabs / sections", "switch view or section"),
        help_key_line("click list row", "select item and focus Details"),
        help_key_line("click links / open / reply / edit", "run that action"),
        help_key_line("wheel over list/details/dialog", "scroll that area"),
        help_key_line("drag split border", "resize list/details ratio"),
    ]
}

fn help_heading(text: &'static str) -> Line<'static> {
    Line::from(Span::styled(
        text,
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    ))
}

fn help_key_line(keys: &'static str, description: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("{keys:<24}"),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(description),
    ])
}

fn centered_rect(width_percent: u16, height: u16, area: Rect) -> Rect {
    let width = centered_rect_width(width_percent, area);
    let height = height.min(area.height);
    centered_rect_with_size(width, height, area)
}

fn centered_rect_width(width_percent: u16, area: Rect) -> u16 {
    let mut width = area.width.saturating_mul(width_percent).saturating_div(100);
    width = width.max(48.min(area.width)).min(area.width);
    width
}

fn centered_rect_with_size(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width, height)
}

#[derive(Debug, Clone)]
struct DetailsDocument {
    lines: Vec<Line<'static>>,
    links: Vec<LinkRegion>,
    actions: Vec<ActionRegion>,
    comments: Vec<CommentRegion>,
}

impl DetailsDocument {
    fn link_at(&self, line: usize, column: u16) -> Option<String> {
        self.links
            .iter()
            .find(|link| link.line == line && column >= link.start && column < link.end)
            .map(|link| link.url.clone())
    }

    fn action_at(&self, line: usize, column: u16) -> Option<DetailAction> {
        self.actions
            .iter()
            .find(|action| action.line == line && column >= action.start && column < action.end)
            .map(|action| action.action.clone())
    }

    fn comment_at(&self, line: usize) -> Option<usize> {
        self.comments
            .iter()
            .find(|comment| line >= comment.start_line && line < comment.end_line)
            .map(|comment| comment.index)
    }
}

#[derive(Debug, Clone)]
struct LinkRegion {
    line: usize,
    start: u16,
    end: u16,
    url: String,
}

#[derive(Debug, Clone)]
struct ActionRegion {
    line: usize,
    start: u16,
    end: u16,
    action: DetailAction,
}

#[derive(Debug, Clone)]
struct CommentRegion {
    index: usize,
    start_line: usize,
    end_line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DetailAction {
    ReplyComment(usize),
    EditComment(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DetailSegment {
    text: String,
    style: Style,
    link: Option<String>,
    action: Option<DetailAction>,
}

impl DetailSegment {
    fn raw(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: Style::default(),
            link: None,
            action: None,
        }
    }

    fn styled(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
            link: None,
            action: None,
        }
    }

    fn link(text: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: link_style(),
            link: Some(url.into()),
            action: None,
        }
    }

    fn action(text: impl Into<String>, action: DetailAction) -> Self {
        Self {
            text: text.into(),
            style: action_style(),
            link: None,
            action: Some(action),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MarkdownBlock {
    quote_depth: u8,
    segments: Vec<DetailSegment>,
}

struct DetailsBuilder {
    document: DetailsDocument,
    width: usize,
}

impl DetailsBuilder {
    fn new(width: u16) -> Self {
        Self {
            document: DetailsDocument {
                lines: Vec::new(),
                links: Vec::new(),
                actions: Vec::new(),
                comments: Vec::new(),
            },
            width: usize::from(width.max(1)),
        }
    }

    fn finish(self) -> DetailsDocument {
        self.document
    }

    fn push_blank(&mut self) {
        self.document.lines.push(Line::from(""));
    }

    fn push_line(&mut self, segments: Vec<DetailSegment>) {
        let line_index = self.document.lines.len();
        let mut column = 0_u16;
        let mut spans = Vec::new();
        for segment in segments {
            let width = display_width(&segment.text) as u16;
            if let Some(url) = &segment.link {
                self.document.links.push(LinkRegion {
                    line: line_index,
                    start: column,
                    end: column.saturating_add(width),
                    url: url.clone(),
                });
            }
            if let Some(action) = &segment.action {
                self.document.actions.push(ActionRegion {
                    line: line_index,
                    start: column,
                    end: column.saturating_add(width),
                    action: action.clone(),
                });
            }
            column = column.saturating_add(width);
            spans.push(Span::styled(segment.text, segment.style));
        }
        self.document.lines.push(Line::from(spans));
    }

    fn push_plain(&mut self, text: impl Into<String>) {
        self.push_line(vec![DetailSegment::raw(text)]);
    }

    fn push_heading(&mut self, text: &str) {
        self.push_line(vec![DetailSegment::styled(
            text.to_string(),
            heading_style(),
        )]);
    }

    fn push_key_value(&mut self, key: &str, value: impl Into<String>) {
        self.push_wrapped_limited(
            vec![
                DetailSegment::styled(format!("{key}: "), Style::default().fg(Color::Gray)),
                DetailSegment::raw(value.into()),
            ],
            1,
        );
    }

    fn push_link_value(&mut self, key: &str, url: &str) {
        self.push_wrapped_limited(
            vec![
                DetailSegment::styled(format!("{key}: "), Style::default().fg(Color::Gray)),
                DetailSegment::link(url.to_string(), url.to_string()),
            ],
            3,
        );
    }

    fn push_markdown_block(
        &mut self,
        text: &str,
        empty_message: &str,
        max_lines: usize,
        max_chars: usize,
    ) {
        let text = truncate_text(&normalize_text(text), max_chars);
        if text.trim().is_empty() {
            self.push_plain(empty_message.to_string());
            return;
        }

        let blocks = markdown_blocks(&text);
        let mut emitted = 0;
        for block in blocks {
            let prefix = quote_prefix(block.quote_depth);
            if !self.push_wrapped_prefixed(
                &block.segments,
                prefix.as_slice(),
                &mut emitted,
                max_lines,
            ) {
                return;
            }
        }
    }

    fn push_wrapped_limited(&mut self, segments: Vec<DetailSegment>, max_lines: usize) {
        let mut emitted = 0;
        let _ = self.push_wrapped(&segments, &mut emitted, max_lines);
    }

    fn push_wrapped_prefixed(
        &mut self,
        segments: &[DetailSegment],
        prefix: &[DetailSegment],
        emitted: &mut usize,
        max_lines: usize,
    ) -> bool {
        if prefix.is_empty() {
            return self.push_wrapped(segments, emitted, max_lines);
        }

        let prefix_width: usize = prefix
            .iter()
            .map(|segment| display_width(&segment.text))
            .sum();
        if prefix_width >= self.width {
            return self.push_wrapped(segments, emitted, max_lines);
        }

        let mut current = prefix.to_vec();
        let mut column = prefix_width;
        let mut wrote_any = false;

        for segment in segments {
            for ch in segment.text.chars() {
                if ch == '\n' {
                    if !self.flush_wrapped_line(&mut current, emitted, max_lines) {
                        return false;
                    }
                    current = prefix.to_vec();
                    column = prefix_width;
                    wrote_any = false;
                    continue;
                }

                if column >= self.width {
                    if !self.flush_wrapped_line(&mut current, emitted, max_lines) {
                        return false;
                    }
                    current = prefix.to_vec();
                    column = prefix_width;
                }

                push_char_segment(&mut current, segment, ch);
                column += 1;
                wrote_any = true;
            }
        }

        if wrote_any || current.len() > prefix.len() {
            self.flush_wrapped_line(&mut current, emitted, max_lines)
        } else {
            true
        }
    }

    fn push_wrapped(
        &mut self,
        segments: &[DetailSegment],
        emitted: &mut usize,
        max_lines: usize,
    ) -> bool {
        let mut current = Vec::new();
        let mut column = 0;
        let mut wrote_any = false;

        for segment in segments {
            for ch in segment.text.chars() {
                if ch == '\n' {
                    if !self.flush_wrapped_line(&mut current, emitted, max_lines) {
                        return false;
                    }
                    column = 0;
                    wrote_any = false;
                    continue;
                }

                if column >= self.width {
                    if !self.flush_wrapped_line(&mut current, emitted, max_lines) {
                        return false;
                    }
                    column = 0;
                }

                push_char_segment(&mut current, segment, ch);
                column += 1;
                wrote_any = true;
            }
        }

        if wrote_any || !current.is_empty() {
            self.flush_wrapped_line(&mut current, emitted, max_lines)
        } else {
            true
        }
    }

    fn flush_wrapped_line(
        &mut self,
        current: &mut Vec<DetailSegment>,
        emitted: &mut usize,
        max_lines: usize,
    ) -> bool {
        if *emitted >= max_lines {
            self.push_plain("...");
            return false;
        }
        let line = std::mem::take(current);
        self.push_line(line);
        *emitted += 1;
        true
    }
}

fn build_details_document(app: &AppState, width: u16) -> DetailsDocument {
    let mut builder = DetailsBuilder::new(width);
    let Some(item) = app.current_item() else {
        builder.push_plain("No item selected");
        return builder.finish();
    };

    builder.push_line(vec![DetailSegment::styled(
        item.title.clone(),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )]);
    builder.push_blank();
    builder.push_key_value("repo", item.repo.clone());
    builder.push_key_value(
        "number",
        item.number
            .map(|number| format!("#{number}"))
            .unwrap_or_else(|| "-".to_string()),
    );
    builder.push_key_value("updated", relative_time(item.updated_at));
    builder.push_key_value(
        "author",
        item.author.clone().unwrap_or_else(|| "-".to_string()),
    );
    builder.push_key_value(
        "state",
        item.state.clone().unwrap_or_else(|| "-".to_string()),
    );
    builder.push_key_value(
        "reason",
        item.reason.clone().unwrap_or_else(|| "-".to_string()),
    );
    builder.push_key_value(
        "comments",
        item.comments
            .map(|comments| comments.to_string())
            .unwrap_or_else(|| "-".to_string()),
    );
    builder.push_link_value("url", &item.url);

    if !item.labels.is_empty() {
        builder.push_blank();
        builder.push_key_value("labels", item.labels.join(", "));
    }

    if let Some(extra) = &item.extra {
        builder.push_blank();
        builder.push_key_value("extra", extra.clone());
    }

    builder.push_blank();
    builder.push_heading("Description");
    builder.push_markdown_block(
        item.body.as_deref().unwrap_or(""),
        "No description.",
        22,
        2_400,
    );

    if matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest) {
        builder.push_blank();
        builder.push_heading("Recent Comments");
        match app.details.get(&item.id) {
            Some(DetailState::Loading) => {
                builder.push_plain("loading comments...");
            }
            Some(DetailState::Loaded(comments)) if comments.is_empty() => {
                builder.push_plain("No comments.");
            }
            Some(DetailState::Loaded(comments)) => {
                for (index, comment) in comments.iter().enumerate() {
                    push_comment(
                        &mut builder,
                        index,
                        comment,
                        app.focus == FocusTarget::Details && index == app.selected_comment_index,
                    );
                }
            }
            Some(DetailState::Error(error)) => {
                builder.push_plain(format!("Failed to load comments: {error}"));
            }
            None => {
                builder.push_plain("loading comments...");
            }
        }
    }

    builder.finish()
}

fn push_comment(
    builder: &mut DetailsBuilder,
    index: usize,
    comment: &CommentPreview,
    selected: bool,
) {
    let timestamp = comment
        .updated_at
        .as_ref()
        .or(comment.created_at.as_ref())
        .cloned();
    let start_line = builder.document.lines.len();
    push_comment_separator(builder, selected);

    let mut header = vec![
        DetailSegment::styled(
            if selected { "▸ " } else { "  " },
            comment_marker_style(selected),
        ),
        DetailSegment::styled(comment.author.clone(), comment_author_style(selected)),
        DetailSegment::raw(format!(" - {}", relative_time(timestamp))),
    ];
    if let Some(url) = &comment.url {
        header.push(DetailSegment::raw("  "));
        header.push(DetailSegment::link("open", url.clone()));
    }
    header.push(DetailSegment::raw("  "));
    header.push(DetailSegment::action(
        "reply",
        DetailAction::ReplyComment(index),
    ));
    if comment.is_mine && comment.id.is_some() {
        header.push(DetailSegment::raw("  "));
        header.push(DetailSegment::action(
            "edit",
            DetailAction::EditComment(index),
        ));
    }
    builder.push_wrapped_limited(header, 2);
    builder.push_markdown_block(&comment.body, "No comment body.", usize::MAX, usize::MAX);
    builder.document.comments.push(CommentRegion {
        index,
        start_line,
        end_line: builder.document.lines.len(),
    });
}

fn push_comment_separator(builder: &mut DetailsBuilder, selected: bool) {
    let width = builder.width.max(12);
    builder.push_line(vec![DetailSegment::styled(
        "─".repeat(width.min(72)),
        comment_separator_style(selected),
    )]);
}

fn quote_comment_for_reply(comment: &CommentPreview) -> String {
    let quote = truncate_text(&normalize_text(&comment.body), 1_200);
    let mut body = format!("> @{} wrote:\n", comment.author);
    if quote.trim().is_empty() {
        body.push_str(">\n");
    } else {
        for line in quote.lines().take(18) {
            if line.trim().is_empty() {
                body.push_str(">\n");
            } else {
                body.push_str("> ");
                body.push_str(line);
                body.push('\n');
            }
        }
        if quote.lines().count() > 18 {
            body.push_str("> ...\n");
        }
    }
    body.push('\n');
    body
}

fn markdown_blocks(text: &str) -> Vec<MarkdownBlock> {
    let mut blocks = Vec::new();
    let mut current = Vec::new();
    let mut link: Option<String> = None;
    let mut code_block = String::new();
    let mut in_code_block = false;
    let mut strong_depth = 0_u8;
    let mut emphasis_depth = 0_u8;
    let mut quote_depth = 0_u8;
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;

    for event in Parser::new_ext(text, options) {
        match event {
            MarkdownEvent::Start(Tag::Paragraph) => {}
            MarkdownEvent::End(TagEnd::Paragraph) => {
                flush_markdown_block(&mut blocks, &mut current, quote_depth)
            }
            MarkdownEvent::Start(Tag::Heading { .. }) => {}
            MarkdownEvent::End(TagEnd::Heading(_)) => {
                flush_markdown_block(&mut blocks, &mut current, quote_depth)
            }
            MarkdownEvent::Start(Tag::BlockQuote(_)) => {
                flush_markdown_block(&mut blocks, &mut current, quote_depth);
                quote_depth = quote_depth.saturating_add(1);
            }
            MarkdownEvent::End(TagEnd::BlockQuote(_)) => {
                flush_markdown_block(&mut blocks, &mut current, quote_depth);
                quote_depth = quote_depth.saturating_sub(1);
            }
            MarkdownEvent::Start(Tag::Item) => {
                current.push(DetailSegment::styled(
                    "- ",
                    Style::default().fg(Color::Gray),
                ));
            }
            MarkdownEvent::End(TagEnd::Item) => {
                flush_markdown_block(&mut blocks, &mut current, quote_depth)
            }
            MarkdownEvent::Start(Tag::Link { dest_url, .. }) => {
                link = Some(dest_url.to_string());
            }
            MarkdownEvent::End(TagEnd::Link) => {
                link = None;
            }
            MarkdownEvent::Start(Tag::Strong) => {
                strong_depth = strong_depth.saturating_add(1);
            }
            MarkdownEvent::End(TagEnd::Strong) => {
                strong_depth = strong_depth.saturating_sub(1);
            }
            MarkdownEvent::Start(Tag::Emphasis) => {
                emphasis_depth = emphasis_depth.saturating_add(1);
            }
            MarkdownEvent::End(TagEnd::Emphasis) => {
                emphasis_depth = emphasis_depth.saturating_sub(1);
            }
            MarkdownEvent::Start(Tag::CodeBlock(_)) => {
                flush_markdown_block(&mut blocks, &mut current, quote_depth);
                in_code_block = true;
                code_block.clear();
            }
            MarkdownEvent::End(TagEnd::CodeBlock) => {
                for line in code_block.lines() {
                    blocks.push(MarkdownBlock {
                        quote_depth,
                        segments: vec![DetailSegment::styled(
                            line.to_string(),
                            Style::default().fg(Color::LightGreen),
                        )],
                    });
                }
                in_code_block = false;
                code_block.clear();
            }
            MarkdownEvent::Text(text) => {
                if in_code_block {
                    code_block.push_str(&text);
                } else {
                    append_text_segments(
                        &mut current,
                        &text,
                        inline_style(strong_depth, emphasis_depth, link.is_some()),
                        link.clone(),
                    );
                }
            }
            MarkdownEvent::Code(text) => current.push(DetailSegment::styled(
                text.to_string(),
                Style::default().fg(Color::LightGreen),
            )),
            MarkdownEvent::SoftBreak => current.push(DetailSegment::raw(" ")),
            MarkdownEvent::HardBreak => {
                flush_markdown_block(&mut blocks, &mut current, quote_depth)
            }
            MarkdownEvent::Rule => blocks.push(MarkdownBlock {
                quote_depth,
                segments: vec![DetailSegment::styled(
                    "─".repeat(24),
                    Style::default().fg(Color::DarkGray),
                )],
            }),
            MarkdownEvent::TaskListMarker(checked) => {
                current.push(DetailSegment::raw(if checked { "[x] " } else { "[ ] " }));
            }
            _ => {}
        }
    }
    flush_markdown_block(&mut blocks, &mut current, quote_depth);
    blocks
}

fn flush_markdown_block(
    blocks: &mut Vec<MarkdownBlock>,
    current: &mut Vec<DetailSegment>,
    quote_depth: u8,
) {
    if current.iter().any(|segment| !segment.text.is_empty()) {
        blocks.push(MarkdownBlock {
            quote_depth,
            segments: std::mem::take(current),
        });
    }
}

fn quote_prefix(depth: u8) -> Vec<DetailSegment> {
    if depth == 0 {
        return Vec::new();
    }

    vec![DetailSegment::styled(
        "│ ".repeat(depth.min(3) as usize),
        quote_style(),
    )]
}

fn append_text_segments(
    current: &mut Vec<DetailSegment>,
    text: &str,
    style: Style,
    link: Option<String>,
) {
    if let Some(url) = link {
        current.push(DetailSegment {
            text: text.to_string(),
            style,
            link: Some(url),
            action: None,
        });
        return;
    }

    for (part, url) in split_raw_urls(text) {
        match url {
            Some(url) => current.push(DetailSegment::link(part, url)),
            None => current.push(DetailSegment::styled(part, style)),
        }
    }
}

fn split_raw_urls(text: &str) -> Vec<(String, Option<String>)> {
    let mut result = Vec::new();
    let mut rest = text;

    while let Some(start) = find_url_start(rest) {
        if start > 0 {
            result.push((rest[..start].to_string(), None));
        }

        let after_start = &rest[start..];
        let end = after_start
            .find(char::is_whitespace)
            .unwrap_or(after_start.len());
        let mut url = after_start[..end].to_string();
        let trailing = trim_url_trailing_punctuation(&mut url);
        if url.is_empty() {
            result.push((after_start[..end].to_string(), None));
        } else {
            result.push((url.clone(), Some(url)));
            if !trailing.is_empty() {
                result.push((trailing, None));
            }
        }
        rest = &after_start[end..];
    }

    if !rest.is_empty() {
        result.push((rest.to_string(), None));
    }

    result
}

fn find_url_start(text: &str) -> Option<usize> {
    match (text.find("https://"), text.find("http://")) {
        (Some(https), Some(http)) => Some(https.min(http)),
        (Some(https), None) => Some(https),
        (None, Some(http)) => Some(http),
        (None, None) => None,
    }
}

fn trim_url_trailing_punctuation(url: &mut String) -> String {
    let mut trailing = String::new();
    while matches!(
        url.chars().last(),
        Some('.') | Some(',') | Some(';') | Some(':') | Some(')')
    ) {
        let Some(ch) = url.pop() else {
            break;
        };
        trailing.insert(0, ch);
    }
    trailing
}

fn push_char_segment(current: &mut Vec<DetailSegment>, template: &DetailSegment, ch: char) {
    if let Some(last) = current.last_mut()
        && last.style == template.style
        && last.link == template.link
        && last.action == template.action
    {
        last.text.push(ch);
        return;
    }

    current.push(DetailSegment {
        text: ch.to_string(),
        style: template.style,
        link: template.link.clone(),
        action: template.action.clone(),
    });
}

fn inline_style(strong_depth: u8, emphasis_depth: u8, is_link: bool) -> Style {
    let mut style = if is_link {
        link_style()
    } else {
        Style::default()
    };
    if strong_depth > 0 {
        style = style.add_modifier(Modifier::BOLD);
    }
    if emphasis_depth > 0 {
        style = style.add_modifier(Modifier::ITALIC);
    }
    style
}

fn heading_style() -> Style {
    Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD)
}

fn link_style() -> Style {
    Style::default()
        .fg(Color::LightBlue)
        .add_modifier(Modifier::UNDERLINED)
}

fn action_style() -> Style {
    Style::default()
        .fg(Color::LightMagenta)
        .add_modifier(Modifier::UNDERLINED)
}

fn quote_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn comment_author_style(selected: bool) -> Style {
    let style = if selected {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Cyan)
    };
    style.add_modifier(Modifier::BOLD)
}

fn comment_marker_style(selected: bool) -> Style {
    if selected {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn comment_separator_style(selected: bool) -> Style {
    if selected {
        Style::default().fg(Color::LightMagenta)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn display_width(text: &str) -> usize {
    text.chars().count()
}

fn normalize_text(text: &str) -> String {
    text.replace('\r', "")
        .replace('\t', "    ")
        .trim()
        .to_string()
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    let mut truncated = text.chars().take(max_chars).collect::<String>();
    truncated.push_str("\n...");
    truncated
}

fn refresh_error_status(count: usize, first_error: Option<&str>) -> String {
    let Some(first_error) = first_error else {
        return format!("refresh complete with {count} failed section(s)");
    };

    if first_error.contains("GitHub CLI `gh` is required") {
        return "GitHub CLI missing: install `gh`, then run `gh auth login`".to_string();
    }

    if first_error.contains("Run `gh auth login`") {
        return "GitHub CLI auth required: run `gh auth login`".to_string();
    }

    format!("refresh complete with {count} failed section(s)")
}

fn is_comment_submit_key(key: KeyEvent) -> bool {
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return false;
    }

    matches!(key.code, KeyCode::Enter | KeyCode::Char('\n'))
}

fn setup_dialog_from_error(error: &str) -> Option<SetupDialog> {
    if error.contains("GitHub CLI `gh` is required") {
        return Some(SetupDialog::MissingGh);
    }

    if error.contains("Run `gh auth login`") {
        return Some(SetupDialog::AuthRequired);
    }

    None
}

impl AppState {
    #[cfg(test)]
    fn new(active_view: SectionKind, sections: Vec<SectionSnapshot>) -> Self {
        Self::with_ui_state(active_view, sections, UiState::default())
    }

    fn with_ui_state(
        active_view: SectionKind,
        sections: Vec<SectionSnapshot>,
        ui_state: UiState,
    ) -> Self {
        let ui_state = ui_state.normalized();
        Self {
            active_view,
            sections,
            section_index: [0; 3],
            selected_index: [0; 3],
            focus: FocusTarget::List,
            details_scroll: 0,
            list_width_percent: ui_state.list_width_percent,
            dragging_split: false,
            split_drag_changed: false,
            search_active: false,
            search_query: String::new(),
            status: "loading snapshot; background refresh started".to_string(),
            refreshing: false,
            last_refresh_request: Instant::now(),
            details: HashMap::new(),
            details_stale: HashSet::new(),
            selected_comment_index: 0,
            comment_dialog: None,
            posting_comment: false,
            setup_dialog: None,
            help_dialog: false,
        }
    }

    fn ui_state(&self) -> UiState {
        UiState {
            list_width_percent: self.list_width_percent,
        }
    }

    fn start_split_drag(&mut self) {
        self.dragging_split = true;
        self.split_drag_changed = false;
        self.search_active = false;
        self.status = "split selected; drag left or right".to_string();
    }

    fn update_split_drag(&mut self, body: Rect, column: u16) {
        let next = split_percent_from_column(body, column);
        if next != self.list_width_percent {
            self.list_width_percent = next;
            self.split_drag_changed = true;
        }
        self.status = format!(
            "layout {} / {}",
            self.list_width_percent,
            100 - self.list_width_percent
        );
        let details_area = body_areas_with_ratio(body, self.list_width_percent)[1];
        self.details_scroll = self
            .details_scroll
            .min(max_details_scroll(self, details_area));
    }

    fn finish_split_drag(&mut self) -> bool {
        self.dragging_split = false;
        let changed = self.split_drag_changed;
        self.split_drag_changed = false;
        if !changed {
            self.status = "layout unchanged".to_string();
            return false;
        }
        self.status = format!(
            "layout saved {} / {}",
            self.list_width_percent,
            100 - self.list_width_percent
        );
        true
    }

    fn handle_msg(&mut self, message: AppMsg) {
        match message {
            AppMsg::RefreshStarted => {
                self.refreshing = true;
                self.last_refresh_request = Instant::now();
                self.status = "refreshing from GitHub".to_string();
            }
            AppMsg::RefreshFinished {
                sections,
                save_error,
            } => {
                let previous_item_id = self.current_item().map(|item| item.id.clone());
                let previous_details_scroll = self.details_scroll;
                let previous_comment_index = self.selected_comment_index;
                let errors = sections
                    .iter()
                    .filter(|section| section.error.is_some())
                    .count();
                let first_error = sections
                    .iter()
                    .find_map(|section| section.error.as_deref())
                    .map(str::to_string);
                let setup_dialog = first_error.as_deref().and_then(setup_dialog_from_error);
                let current = std::mem::take(&mut self.sections);
                self.sections = merge_refreshed_sections(current, sections);
                self.clamp_positions();
                let restored_item = previous_item_id
                    .as_deref()
                    .is_some_and(|item_id| self.select_current_item_by_id(item_id));
                if restored_item {
                    self.details_scroll = previous_details_scroll;
                    self.selected_comment_index = previous_comment_index;
                    if let Some(item_id) = previous_item_id {
                        self.details_stale.insert(item_id);
                    }
                    self.clamp_selected_comment();
                } else {
                    self.details_scroll = 0;
                    self.selected_comment_index = 0;
                }
                self.refreshing = false;
                self.setup_dialog = setup_dialog;
                self.status = match (errors, save_error) {
                    (0, None) => "refresh complete".to_string(),
                    (count, None) => refresh_error_status(count, first_error.as_deref()),
                    (_, Some(error)) => format!("snapshot save failed: {error}"),
                };
            }
            AppMsg::DetailsLoaded { item_id, result } => match result {
                Ok(comments) => {
                    self.details_stale.remove(&item_id);
                    self.details.insert(item_id, DetailState::Loaded(comments));
                    self.clamp_selected_comment();
                }
                Err(error) => {
                    if self.setup_dialog.is_none() {
                        self.setup_dialog = setup_dialog_from_error(&error);
                    }
                    self.details_stale.remove(&item_id);
                    self.details.insert(item_id, DetailState::Error(error));
                }
            },
            AppMsg::CommentPosted { item_id, result } => match result {
                Ok(comments) => {
                    self.selected_comment_index = comments.len().saturating_sub(1);
                    self.details_stale.remove(&item_id);
                    self.details.insert(item_id, DetailState::Loaded(comments));
                    self.clamp_selected_comment();
                    self.posting_comment = false;
                    self.status = "comment posted".to_string();
                }
                Err(error) => {
                    if self.setup_dialog.is_none() {
                        self.setup_dialog = setup_dialog_from_error(&error);
                    }
                    self.posting_comment = false;
                    self.status = format!("comment post failed: {error}");
                }
            },
            AppMsg::CommentUpdated {
                item_id,
                comment_index,
                result,
            } => match result {
                Ok(comments) => {
                    self.selected_comment_index =
                        comment_index.min(comments.len().saturating_sub(1));
                    self.details_stale.remove(&item_id);
                    self.details.insert(item_id, DetailState::Loaded(comments));
                    self.clamp_selected_comment();
                    self.posting_comment = false;
                    self.status = "comment updated".to_string();
                }
                Err(error) => {
                    if self.setup_dialog.is_none() {
                        self.setup_dialog = setup_dialog_from_error(&error);
                    }
                    self.posting_comment = false;
                    self.status = format!("comment update failed: {error}");
                }
            },
        }
    }

    fn dismiss_setup_dialog(&mut self) {
        self.setup_dialog = None;
        self.status = "setup hint dismissed; cached data still available".to_string();
    }

    fn show_help_dialog(&mut self) {
        self.help_dialog = true;
        self.search_active = false;
        self.status = "help".to_string();
    }

    fn dismiss_help_dialog(&mut self) {
        self.help_dialog = false;
        self.status = "help dismissed".to_string();
    }

    fn ensure_current_details_loading(&mut self, tx: &UnboundedSender<AppMsg>) {
        let Some(item) = self.current_item().cloned() else {
            return;
        };
        if !matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest) || item.number.is_none() {
            return;
        }
        if !self.should_start_details_load(&item.id) {
            return;
        }

        if !self.details.contains_key(&item.id) {
            self.details.insert(item.id.clone(), DetailState::Loading);
        }
        start_details_load(item, tx.clone());
    }

    fn should_start_details_load(&mut self, item_id: &str) -> bool {
        let should_refresh = self.details_stale.remove(item_id);
        !self.details.contains_key(item_id) || should_refresh
    }

    fn switch_view(&mut self, view: SectionKind) {
        self.active_view = view;
        self.focus = FocusTarget::List;
        self.details_scroll = 0;
        self.selected_comment_index = 0;
        self.comment_dialog = None;
        self.clamp_positions();
    }

    fn focus_primary_list(&mut self) {
        self.focus = FocusTarget::List;
        self.details_scroll = 0;
        self.selected_comment_index = 0;
        self.comment_dialog = None;
        self.search_active = false;
        self.status = "list focused".to_string();
        self.clamp_positions();
    }

    fn move_section(&mut self, delta: isize) {
        let len = self.visible_sections().len();
        if len == 0 {
            return;
        }
        let slot = kind_slot(self.active_view);
        let current = self.section_index[slot].min(len - 1);
        self.section_index[slot] = move_bounded(current, len, delta);
        self.selected_index[slot] = 0;
        self.focus = FocusTarget::List;
        self.details_scroll = 0;
        self.selected_comment_index = 0;
        self.comment_dialog = None;
    }

    fn select_section(&mut self, index: usize) {
        let len = self.visible_sections().len();
        if len == 0 {
            return;
        }
        let slot = kind_slot(self.active_view);
        self.section_index[slot] = index.min(len - 1);
        self.selected_index[slot] = 0;
        self.focus = FocusTarget::List;
        self.details_scroll = 0;
        self.selected_comment_index = 0;
        self.comment_dialog = None;
        self.search_active = false;
        self.status = "list focused".to_string();
    }

    fn move_selection(&mut self, delta: isize) {
        let Some(section) = self.current_section() else {
            return;
        };
        let len = self.filtered_indices(section).len();
        if len == 0 {
            return;
        }
        let slot = kind_slot(self.active_view);
        let current = self.selected_index[slot].min(len - 1);
        self.selected_index[slot] = move_bounded(current, len, delta);
        self.details_scroll = 0;
        self.selected_comment_index = 0;
        self.comment_dialog = None;
    }

    fn set_selection(&mut self, index: usize) {
        let slot = kind_slot(self.active_view);
        self.selected_index[slot] = index;
        self.details_scroll = 0;
        self.selected_comment_index = 0;
        self.comment_dialog = None;
        self.clamp_positions();
    }

    fn select_last(&mut self) {
        let Some(section) = self.current_section() else {
            return;
        };
        let len = self.filtered_indices(section).len();
        if len > 0 {
            let slot = kind_slot(self.active_view);
            self.selected_index[slot] = len - 1;
            self.details_scroll = 0;
            self.selected_comment_index = 0;
            self.comment_dialog = None;
        }
    }

    fn focus_details(&mut self) {
        if self.current_item().is_some() {
            self.focus = FocusTarget::Details;
            self.search_active = false;
            self.clamp_selected_comment();
            self.status = "details focused".to_string();
        } else {
            self.status = "nothing to focus".to_string();
        }
    }

    fn focus_list(&mut self) {
        self.focus = FocusTarget::List;
        self.status = "list focused".to_string();
    }

    fn scroll_details(&mut self, delta: i16) {
        if delta < 0 {
            self.details_scroll = self.details_scroll.saturating_sub(delta.unsigned_abs());
        } else {
            self.details_scroll = self.details_scroll.saturating_add(delta as u16);
        }
    }

    fn select_comment(&mut self, index: usize) {
        self.selected_comment_index = index;
        self.clamp_selected_comment();
        self.status = format!("comment {} focused", self.selected_comment_index + 1);
    }

    fn move_comment(&mut self, delta: isize) {
        let Some(len) = self.current_comments().map(Vec::len) else {
            self.status = "no comments".to_string();
            return;
        };
        if len == 0 {
            self.status = "no comments".to_string();
            return;
        }
        self.selected_comment_index =
            move_bounded(self.selected_comment_index.min(len - 1), len, delta);
        self.status = format!("comment {} focused", self.selected_comment_index + 1);
    }

    fn handle_detail_action(&mut self, action: DetailAction) {
        match action {
            DetailAction::ReplyComment(index) => {
                self.select_comment(index);
                self.start_reply_to_selected_comment();
            }
            DetailAction::EditComment(index) => {
                self.select_comment(index);
                self.start_edit_selected_comment_dialog();
            }
        }
    }

    fn start_new_comment_dialog(&mut self) {
        if !self.current_item_supports_comments() {
            self.status = "selected item cannot be commented on".to_string();
            return;
        }
        self.focus = FocusTarget::Details;
        self.search_active = false;
        self.comment_dialog = Some(CommentDialog {
            mode: CommentDialogMode::New,
            body: String::new(),
            scroll: 0,
        });
        self.scroll_comment_dialog_to_cursor();
        self.status = "new comment".to_string();
    }

    fn start_reply_to_selected_comment(&mut self) {
        if !self.current_item_supports_comments() {
            self.status = "selected item cannot be commented on".to_string();
            return;
        }
        let Some(comment) = self.current_selected_comment().cloned() else {
            self.status = "no comment selected".to_string();
            return;
        };
        let author = comment.author.clone();
        self.focus = FocusTarget::Details;
        self.search_active = false;
        self.comment_dialog = Some(CommentDialog {
            mode: CommentDialogMode::Reply {
                comment_index: self.selected_comment_index,
                author: author.clone(),
            },
            body: quote_comment_for_reply(&comment),
            scroll: 0,
        });
        self.scroll_comment_dialog_to_cursor();
        self.status = format!("replying to @{author}");
    }

    fn start_edit_selected_comment_dialog(&mut self) {
        if !self.current_item_supports_comments() {
            self.status = "selected item cannot be commented on".to_string();
            return;
        }
        let Some(comment) = self.current_selected_comment().cloned() else {
            self.status = "no comment selected".to_string();
            return;
        };
        if !comment.is_mine {
            self.status = "only your comments can be edited".to_string();
            return;
        }
        let Some(comment_id) = comment.id else {
            self.status = "comment id unavailable; cannot edit".to_string();
            return;
        };

        self.focus = FocusTarget::Details;
        self.search_active = false;
        self.comment_dialog = Some(CommentDialog {
            mode: CommentDialogMode::Edit {
                comment_index: self.selected_comment_index,
                comment_id,
            },
            body: comment.body,
            scroll: 0,
        });
        self.scroll_comment_dialog_to_cursor();
        self.status = "editing comment".to_string();
    }

    fn handle_comment_dialog_key(
        &mut self,
        key: KeyEvent,
        tx: &UnboundedSender<AppMsg>,
        area: Option<Rect>,
    ) {
        let tx = tx.clone();
        self.handle_comment_dialog_key_with_submit(key, area, move |submit| match submit.mode {
            PendingCommentMode::Post => {
                start_comment_submit(submit.item, submit.body, tx.clone());
            }
            PendingCommentMode::Edit {
                comment_index,
                comment_id,
            } => {
                start_comment_edit(
                    submit.item,
                    comment_index,
                    comment_id,
                    submit.body,
                    tx.clone(),
                );
            }
        });
    }

    fn handle_comment_dialog_key_with_submit<F>(
        &mut self,
        key: KeyEvent,
        area: Option<Rect>,
        mut submit: F,
    ) where
        F: FnMut(PendingCommentSubmit),
    {
        if self.posting_comment {
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.comment_dialog = None;
                self.status = "comment cancelled".to_string();
            }
            KeyCode::PageDown => self.scroll_comment_dialog(6, area),
            KeyCode::PageUp => self.scroll_comment_dialog(-6, area),
            _ if is_comment_submit_key(key) => {
                if let Some(pending) = self.prepare_comment_submit() {
                    submit(pending);
                }
            }
            KeyCode::Enter => {
                if let Some(dialog) = &mut self.comment_dialog {
                    dialog.body.push('\n');
                }
                self.scroll_comment_dialog_to_cursor_in_area(area);
            }
            KeyCode::Backspace => {
                if let Some(dialog) = &mut self.comment_dialog {
                    dialog.body.pop();
                }
                self.scroll_comment_dialog_to_cursor_in_area(area);
            }
            KeyCode::Char(value) => {
                if let Some(dialog) = &mut self.comment_dialog {
                    dialog.body.push(value);
                }
                self.scroll_comment_dialog_to_cursor_in_area(area);
            }
            _ => {}
        }
    }

    fn scroll_comment_dialog(&mut self, delta: i16, area: Option<Rect>) {
        let Some(dialog) = &mut self.comment_dialog else {
            return;
        };
        let (width, height) = comment_dialog_editor_size(dialog, area);
        let max_scroll = max_comment_dialog_scroll(&dialog.body, width, height);
        if delta < 0 {
            dialog.scroll = dialog.scroll.saturating_sub(delta.unsigned_abs());
        } else {
            dialog.scroll = dialog.scroll.saturating_add(delta as u16);
        }
        dialog.scroll = dialog.scroll.min(max_scroll);
    }

    fn scroll_comment_dialog_to_cursor(&mut self) {
        self.scroll_comment_dialog_to_cursor_in_area(None);
    }

    fn scroll_comment_dialog_to_cursor_in_area(&mut self, area: Option<Rect>) {
        if let Some(dialog) = &mut self.comment_dialog {
            let (width, height) = comment_dialog_editor_size(dialog, area);
            dialog.scroll =
                scroll_for_comment_dialog_cursor(&dialog.body, width, height, dialog.scroll);
        }
    }

    fn prepare_comment_submit(&mut self) -> Option<PendingCommentSubmit> {
        let Some(dialog) = self.comment_dialog.take() else {
            return None;
        };
        let body = dialog.body.trim().to_string();
        if body.is_empty() {
            self.comment_dialog = Some(dialog);
            self.status = "comment is empty".to_string();
            return None;
        }
        let Some(item) = self.current_item().cloned() else {
            self.comment_dialog = Some(dialog);
            self.status = "nothing to comment on".to_string();
            return None;
        };
        let mode = match dialog.mode {
            CommentDialogMode::New | CommentDialogMode::Reply { .. } => PendingCommentMode::Post,
            CommentDialogMode::Edit {
                comment_index,
                comment_id,
            } => PendingCommentMode::Edit {
                comment_index,
                comment_id,
            },
        };
        self.posting_comment = true;
        self.status = match mode {
            PendingCommentMode::Post => "posting comment".to_string(),
            PendingCommentMode::Edit { .. } => "updating comment".to_string(),
        };
        Some(PendingCommentSubmit { item, body, mode })
    }

    fn start_search(&mut self) {
        self.focus = FocusTarget::List;
        self.search_active = true;
        self.comment_dialog = None;
        self.status = "search mode".to_string();
        self.clamp_positions();
    }

    fn clear_search(&mut self) {
        self.search_active = false;
        self.search_query.clear();
        self.focus = FocusTarget::List;
        self.details_scroll = 0;
        self.status = "search cleared".to_string();
        self.clamp_positions();
    }

    fn push_search_char(&mut self, value: char) {
        self.search_query.push(value);
        self.selected_index[kind_slot(self.active_view)] = 0;
        self.details_scroll = 0;
        self.clamp_positions();
    }

    fn pop_search_char(&mut self) {
        self.search_query.pop();
        self.selected_index[kind_slot(self.active_view)] = 0;
        self.details_scroll = 0;
        self.clamp_positions();
    }

    fn open_selected(&mut self) {
        let Some(item) = self.current_item() else {
            self.status = "nothing to open".to_string();
            return;
        };

        let url = item.url.clone();
        self.open_url(&url);
    }

    fn open_url(&mut self, url: &str) {
        match open::that(url) {
            Ok(_) => {
                self.status = format!("opened {url}");
            }
            Err(error) => {
                self.status = format!("open failed: {error}");
            }
        }
    }

    fn visible_sections(&self) -> Vec<&SectionSnapshot> {
        self.sections
            .iter()
            .filter(|section| section.kind == self.active_view)
            .collect()
    }

    fn current_section(&self) -> Option<&SectionSnapshot> {
        let sections = self.visible_sections();
        if sections.is_empty() {
            return None;
        }
        sections
            .get(self.current_section_position().min(sections.len() - 1))
            .copied()
    }

    fn current_item(&self) -> Option<&WorkItem> {
        let section = self.current_section()?;
        let filtered_indices = self.filtered_indices(section);
        let item_index = filtered_indices.get(self.current_selected_position())?;
        section.items.get(*item_index)
    }

    fn current_item_supports_comments(&self) -> bool {
        self.current_item()
            .map(|item| {
                matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest)
                    && item.number.is_some()
            })
            .unwrap_or(false)
    }

    fn current_comments(&self) -> Option<&Vec<CommentPreview>> {
        let item = self.current_item()?;
        match self.details.get(&item.id) {
            Some(DetailState::Loaded(comments)) => Some(comments),
            _ => None,
        }
    }

    fn current_selected_comment(&self) -> Option<&CommentPreview> {
        self.current_comments()?.get(self.selected_comment_index)
    }

    fn current_section_position(&self) -> usize {
        self.section_index[kind_slot(self.active_view)]
    }

    fn current_selected_position(&self) -> usize {
        self.selected_index[kind_slot(self.active_view)]
    }

    fn select_current_item_by_id(&mut self, item_id: &str) -> bool {
        let Some(section) = self.current_section() else {
            return false;
        };
        let Some(position) = self
            .filtered_indices(section)
            .into_iter()
            .enumerate()
            .find_map(|(position, item_index)| {
                section
                    .items
                    .get(item_index)
                    .is_some_and(|item| item.id == item_id)
                    .then_some(position)
            })
        else {
            return false;
        };

        self.selected_index[kind_slot(self.active_view)] = position;
        true
    }

    fn clamp_positions(&mut self) {
        for kind in [
            SectionKind::PullRequests,
            SectionKind::Issues,
            SectionKind::Notifications,
        ] {
            let slot = kind_slot(kind);
            let section_count = self
                .sections
                .iter()
                .filter(|section| section.kind == kind)
                .count();
            if section_count == 0 {
                self.section_index[slot] = 0;
                self.selected_index[slot] = 0;
                continue;
            }

            self.section_index[slot] = self.section_index[slot].min(section_count - 1);
            let item_count = self
                .sections
                .iter()
                .filter(|section| section.kind == kind)
                .nth(self.section_index[slot])
                .map(|section| self.filtered_indices(section).len())
                .unwrap_or(0);
            if item_count == 0 {
                self.selected_index[slot] = 0;
            } else {
                self.selected_index[slot] = self.selected_index[slot].min(item_count - 1);
            }
        }
        self.clamp_selected_comment();
    }

    fn clamp_selected_comment(&mut self) {
        let len = self.current_comments().map(Vec::len).unwrap_or(0);
        if len == 0 {
            self.selected_comment_index = 0;
        } else {
            self.selected_comment_index = self.selected_comment_index.min(len - 1);
        }
    }

    fn filtered_indices(&self, section: &SectionSnapshot) -> Vec<usize> {
        filtered_indices(section, &self.search_query)
    }
}

fn kind_slot(kind: SectionKind) -> usize {
    match kind {
        SectionKind::PullRequests => 0,
        SectionKind::Issues => 1,
        SectionKind::Notifications => 2,
    }
}

fn filtered_indices(section: &SectionSnapshot, query: &str) -> Vec<usize> {
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

fn fuzzy_score(query: &str, haystack: &str) -> Option<i64> {
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

fn move_bounded(current: usize, len: usize, delta: isize) -> usize {
    let next = current as isize + delta;
    next.clamp(0, len.saturating_sub(1) as isize) as usize
}

fn relative_time(value: Option<DateTime<Utc>>) -> String {
    let Some(value) = value else {
        return "-".to_string();
    };
    let delta = Utc::now().signed_duration_since(value);
    if delta.num_minutes() < 1 {
        "now".to_string()
    } else if delta.num_hours() < 1 {
        format!("{}m", delta.num_minutes())
    } else if delta.num_days() < 1 {
        format!("{}h", delta.num_hours())
    } else if delta.num_days() < 30 {
        format!("{}d", delta.num_days())
    } else {
        format!("{}mo", delta.num_days() / 30)
    }
}

fn item_meta(item: &WorkItem) -> String {
    let mut parts = Vec::new();
    if item.unread.unwrap_or(false) {
        parts.push("unread".to_string());
    }
    if let Some(reason) = &item.reason {
        parts.push(reason.clone());
    }
    if let Some(state) = &item.state {
        parts.push(state.clone());
    }
    if let Some(comments) = item.comments {
        parts.push(format!("{comments}c"));
    }
    if let Some(extra) = &item.extra {
        parts.push(extra.clone());
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_score_matches_ordered_subsequence() {
        assert!(fuzzy_score("frc", "fix rust closure").is_some());
        assert!(fuzzy_score("zz", "fix rust closure").is_none());
    }

    #[test]
    fn refresh_error_status_guides_missing_gh_and_auth() {
        assert_eq!(
            refresh_error_status(3, Some("GitHub CLI `gh` is required but was not found.")),
            "GitHub CLI missing: install `gh`, then run `gh auth login`"
        );
        assert_eq!(
            refresh_error_status(
                3,
                Some("GitHub CLI is installed but not authenticated. Run `gh auth login`.")
            ),
            "GitHub CLI auth required: run `gh auth login`"
        );
        assert_eq!(
            refresh_error_status(3, Some("rate limited")),
            "refresh complete with 3 failed section(s)"
        );
    }

    #[test]
    fn setup_dialog_from_error_classifies_gh_setup_failures() {
        assert_eq!(
            setup_dialog_from_error("GitHub CLI `gh` is required but was not found."),
            Some(SetupDialog::MissingGh)
        );
        assert_eq!(
            setup_dialog_from_error(
                "GitHub CLI is installed but not authenticated. Run `gh auth login`."
            ),
            Some(SetupDialog::AuthRequired)
        );
        assert_eq!(setup_dialog_from_error("HTTP 500"), None);
    }

    #[test]
    fn refresh_failure_opens_setup_dialog() {
        let mut failed_section = test_section();
        failed_section.items.clear();
        failed_section.error = Some("GitHub CLI `gh` is required but was not found.".to_string());
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);

        app.handle_msg(AppMsg::RefreshFinished {
            sections: vec![failed_section],
            save_error: None,
        });

        assert_eq!(app.setup_dialog, Some(SetupDialog::MissingGh));
        assert_eq!(
            app.status,
            "GitHub CLI missing: install `gh`, then run `gh auth login`"
        );
    }

    #[test]
    fn refresh_preserves_details_scroll_when_current_item_survives() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.set_selection(1);
        app.focus_details();
        app.details_scroll = 9;
        app.selected_comment_index = 1;
        app.details.insert(
            "2".to_string(),
            DetailState::Loaded(vec![
                comment("alice", "first", None),
                comment("bob", "second", None),
            ]),
        );
        let refreshed = SectionSnapshot {
            key: "pull_requests:test".to_string(),
            kind: SectionKind::PullRequests,
            title: "Test".to_string(),
            filters: String::new(),
            items: vec![
                work_item("2", "nervosnetwork/fiber", 2, "Funding state updated", None),
                work_item("1", "rust-lang/rust", 1, "Compiler diagnostics", None),
            ],
            refreshed_at: None,
            error: None,
        };

        app.handle_msg(AppMsg::RefreshFinished {
            sections: vec![refreshed],
            save_error: None,
        });

        assert_eq!(app.current_item().map(|item| item.id.as_str()), Some("2"));
        assert_eq!(app.current_selected_position(), 0);
        assert_eq!(app.details_scroll, 9);
        assert_eq!(app.selected_comment_index, 1);
        assert!(app.details_stale.contains("2"));
    }

    #[test]
    fn refresh_resets_details_scroll_when_current_item_disappears() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.set_selection(1);
        app.focus_details();
        app.details_scroll = 9;
        app.selected_comment_index = 1;
        let refreshed = SectionSnapshot {
            key: "pull_requests:test".to_string(),
            kind: SectionKind::PullRequests,
            title: "Test".to_string(),
            filters: String::new(),
            items: vec![work_item("3", "rust-lang/rust", 3, "New item", None)],
            refreshed_at: None,
            error: None,
        };

        app.handle_msg(AppMsg::RefreshFinished {
            sections: vec![refreshed],
            save_error: None,
        });

        assert_eq!(app.current_item().map(|item| item.id.as_str()), Some("3"));
        assert_eq!(app.details_scroll, 0);
        assert_eq!(app.selected_comment_index, 0);
    }

    #[test]
    fn stale_details_refresh_preserves_loaded_comments_until_new_load_finishes() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.details.insert(
            "1".to_string(),
            DetailState::Loaded(vec![comment("alice", "old cached comment", None)]),
        );
        app.details_stale.insert("1".to_string());

        assert!(app.should_start_details_load("1"));

        assert!(
            matches!(
                app.details.get("1"),
                Some(DetailState::Loaded(comments)) if comments[0].body == "old cached comment"
            ),
            "stale refresh should keep old comments visible while the async reload runs"
        );
        assert!(!app.details_stale.contains("1"));
    }

    #[test]
    fn modal_keys_dismiss_dialog_before_regular_input() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        app.setup_dialog = Some(SetupDialog::AuthRequired);
        app.search_active = true;
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Esc),
            &config,
            &store,
            &tx
        ));

        assert_eq!(app.setup_dialog, None);
        assert!(app.search_active);
        assert_eq!(
            app.status,
            "setup hint dismissed; cached data still available"
        );
    }

    #[test]
    fn question_mark_opens_and_dismisses_help_dialog() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('?')),
            &config,
            &store,
            &tx
        ));
        assert!(app.help_dialog);
        assert_eq!(app.status, "help");

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('q')),
            &config,
            &store,
            &tx
        ));
        assert!(!app.help_dialog);
        assert_eq!(app.status, "help dismissed");
    }

    #[test]
    fn question_mark_inside_comment_editor_is_text() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.start_new_comment_dialog();

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('?')),
            &config,
            &store,
            &tx
        ));

        assert!(!app.help_dialog);
        assert_eq!(
            app.comment_dialog
                .as_ref()
                .map(|dialog| dialog.body.as_str()),
            Some("?")
        );
    }

    #[test]
    fn help_dialog_content_lists_core_shortcuts() {
        let text = help_dialog_content()
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("Tab / Shift+Tab"));
        assert!(text.contains("Ctrl+Enter"));
        assert!(text.contains("R"));
        assert!(text.contains("edit focused comment"));
        assert!(text.contains("drag split border"));
    }

    #[test]
    fn setup_dialog_content_contains_actionable_commands() {
        let (_title, missing_lines) = setup_dialog_content(SetupDialog::MissingGh);
        let missing_text = missing_lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(missing_text.contains("brew install gh"));
        assert!(missing_text.contains("sudo apt install gh"));
        assert!(missing_text.contains("gh auth login"));

        let (_title, auth_lines) = setup_dialog_content(SetupDialog::AuthRequired);
        let auth_text = auth_lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(auth_text.contains("gh auth login"));
        assert!(auth_text.contains("GH_TOKEN"));
    }

    #[test]
    fn filtered_indices_match_multiple_fields_and_rank_substrings() {
        let section = SectionSnapshot {
            key: "pull_requests:test".to_string(),
            kind: SectionKind::PullRequests,
            title: "Test".to_string(),
            filters: String::new(),
            items: vec![
                work_item("1", "rust-lang/rust", 1, "Fix closure span", Some("alice")),
                work_item(
                    "2",
                    "nervosnetwork/fiber",
                    2,
                    "Channel persistence",
                    Some("bob"),
                ),
                work_item("3", "nervosnetwork/ckb", 3, "RPC docs", Some("carol")),
            ],
            refreshed_at: None,
            error: None,
        };

        assert_eq!(filtered_indices(&section, "fiber"), vec![1]);
        assert_eq!(filtered_indices(&section, "alice closure"), vec![0]);
        assert!(filtered_indices(&section, "not-present").is_empty());
    }

    #[test]
    fn current_item_uses_filtered_selection() {
        let section = SectionSnapshot {
            key: "issues:test".to_string(),
            kind: SectionKind::Issues,
            title: "Test".to_string(),
            filters: String::new(),
            items: vec![
                work_item("1", "rust-lang/rust", 1, "Compiler diagnostics", None),
                work_item("2", "nervosnetwork/fiber", 2, "Funding state", None),
            ],
            refreshed_at: None,
            error: None,
        };
        let mut app = AppState::new(SectionKind::Issues, vec![section]);

        app.search_query = "fiber".to_string();
        app.clamp_positions();

        assert_eq!(app.current_item().map(|item| item.id.as_str()), Some("2"));
    }

    #[test]
    fn details_url_line_tracks_clickable_region() {
        let app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let document = build_details_document(&app, 100);
        let url = "https://github.com/rust-lang/rust/pull/1";
        let line_index = document
            .lines
            .iter()
            .position(|line| line.to_string().contains(url))
            .expect("url line");
        let column = document.lines[line_index]
            .to_string()
            .find(url)
            .expect("url column") as u16;

        assert_eq!(document.link_at(line_index, column), Some(url.to_string()));
        assert_eq!(document.link_at(line_index, column.saturating_sub(1)), None);
    }

    #[test]
    fn details_markdown_renders_without_raw_syntax_and_keeps_links() {
        let mut item = work_item("1", "rust-lang/rust", 1, "Compiler diagnostics", None);
        item.body = Some(
            "**Fixes** [tracking issue](https://example.com/issues/1).\n\n- first item\n\n```rust\nlet x = 1;\n```"
                .to_string(),
        );
        let section = SectionSnapshot {
            key: "pull_requests:test".to_string(),
            kind: SectionKind::PullRequests,
            title: "Test".to_string(),
            filters: String::new(),
            items: vec![item],
            refreshed_at: None,
            error: None,
        };
        let app = AppState::new(SectionKind::PullRequests, vec![section]);
        let document = build_details_document(&app, 100);
        let rendered = document
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Fixes tracking issue."));
        assert!(rendered.contains("- first item"));
        assert!(rendered.contains("let x = 1;"));
        assert!(!rendered.contains("**Fixes**"));
        assert!(!rendered.contains("[tracking issue]"));
        assert!(!rendered.contains("```"));

        let line_index = document
            .lines
            .iter()
            .position(|line| line.to_string().contains("tracking issue"))
            .expect("markdown link line");
        let column = document.lines[line_index]
            .to_string()
            .find("tracking issue")
            .expect("markdown link column") as u16;
        assert_eq!(
            document.link_at(line_index, column),
            Some("https://example.com/issues/1".to_string())
        );
    }

    #[test]
    fn markdown_blockquotes_render_with_quote_marker_on_wrapped_lines() {
        let mut builder = DetailsBuilder::new(12);
        builder.push_markdown_block(
            "> quoted reply with enough text to wrap\n\nnormal reply",
            "empty",
            usize::MAX,
            usize::MAX,
        );
        let document = builder.finish();
        let rendered = document
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        let quote_lines = rendered
            .iter()
            .take_while(|line| !line.contains("normal reply"))
            .collect::<Vec<_>>();

        assert!(
            quote_lines.len() > 1,
            "quote should wrap in the narrow details pane"
        );
        assert!(
            quote_lines.iter().all(|line| line.starts_with("│ ")),
            "each wrapped quote line should keep the quote marker: {quote_lines:?}"
        );
        assert!(rendered.iter().any(|line| line == "normal reply"));
    }

    #[test]
    fn details_comments_have_separators_and_raw_urls_are_clickable() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.details.insert(
            "1".to_string(),
            DetailState::Loaded(vec![
                CommentPreview {
                    id: None,
                    author: "alice".to_string(),
                    body: "See https://example.com/one.".to_string(),
                    created_at: None,
                    updated_at: None,
                    url: None,
                    is_mine: false,
                },
                CommentPreview {
                    id: None,
                    author: "bob".to_string(),
                    body: "Second comment".to_string(),
                    created_at: None,
                    updated_at: None,
                    url: Some(
                        "https://github.com/rust-lang/rust/pull/1#issuecomment-2".to_string(),
                    ),
                    is_mine: false,
                },
            ]),
        );

        let document = build_details_document(&app, 100);
        let separator_count = document
            .lines
            .iter()
            .filter(|line| line.to_string().starts_with('─'))
            .count();
        assert_eq!(separator_count, 2);

        let line_index = document
            .lines
            .iter()
            .position(|line| line.to_string().contains("https://example.com/one"))
            .expect("raw url line");
        let column = document.lines[line_index]
            .to_string()
            .find("https://example.com/one")
            .expect("raw url column") as u16;
        assert_eq!(
            document.link_at(line_index, column),
            Some("https://example.com/one".to_string())
        );
    }

    #[test]
    fn selected_comment_is_highlighted_and_has_reply_action() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.focus_details();
        app.selected_comment_index = 1;
        app.details.insert(
            "1".to_string(),
            DetailState::Loaded(vec![
                comment("alice", "First comment", None),
                comment(
                    "bob",
                    "Second comment",
                    Some("https://example.com/comment-2"),
                ),
            ]),
        );

        let document = build_details_document(&app, 100);
        let bob_line_index = document
            .lines
            .iter()
            .position(|line| line.to_string().contains("▸ bob"))
            .expect("selected comment header");
        let reply_column = document.lines[bob_line_index]
            .to_string()
            .find("reply")
            .expect("reply button") as u16;

        assert_eq!(
            document.action_at(bob_line_index, reply_column),
            Some(DetailAction::ReplyComment(1))
        );
        assert_eq!(document.comment_at(bob_line_index), Some(1));
    }

    #[test]
    fn own_comment_has_edit_action() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.focus_details();
        app.details.insert(
            "1".to_string(),
            DetailState::Loaded(vec![
                comment("alice", "Not mine", None),
                own_comment(42, "chenyukang", "My editable comment", None),
            ]),
        );

        let document = build_details_document(&app, 100);
        let own_line_index = document
            .lines
            .iter()
            .position(|line| line.to_string().contains("chenyukang"))
            .expect("own comment header");
        let edit_column = document.lines[own_line_index]
            .to_string()
            .find("edit")
            .expect("edit button") as u16;

        assert_eq!(
            document.action_at(own_line_index, edit_column),
            Some(DetailAction::EditComment(1))
        );
    }

    #[test]
    fn edit_action_opens_dialog_prefilled_with_comment_body() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.details.insert(
            "1".to_string(),
            DetailState::Loaded(vec![own_comment(42, "chenyukang", "Original body", None)]),
        );

        app.handle_detail_action(DetailAction::EditComment(0));

        let dialog = app.comment_dialog.expect("edit dialog");
        assert_eq!(
            dialog.mode,
            CommentDialogMode::Edit {
                comment_index: 0,
                comment_id: 42
            }
        );
        assert_eq!(dialog.body, "Original body");
        assert_eq!(app.status, "editing comment");
    }

    #[test]
    fn reply_action_opens_dialog_prefilled_with_quote() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.details.insert(
            "1".to_string(),
            DetailState::Loaded(vec![comment("alice", "Quoted body", None)]),
        );

        app.handle_detail_action(DetailAction::ReplyComment(0));

        let dialog = app.comment_dialog.expect("reply dialog");
        assert_eq!(
            dialog.mode,
            CommentDialogMode::Reply {
                comment_index: 0,
                author: "alice".to_string()
            }
        );
        assert!(dialog.body.contains("> @alice wrote:"));
        assert!(dialog.body.contains("> Quoted body"));
    }

    #[test]
    fn capital_r_replies_in_details_while_lowercase_r_keeps_refreshing() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.focus_details();
        app.details.insert(
            "1".to_string(),
            DetailState::Loaded(vec![comment("alice", "Quoted body", None)]),
        );

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('R')),
            &config,
            &store,
            &tx
        ));
        assert!(matches!(
            app.comment_dialog.as_ref().map(|dialog| &dialog.mode),
            Some(CommentDialogMode::Reply { .. })
        ));

        app.comment_dialog = None;
        app.refreshing = true;
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('r')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.status, "refresh already running");
        assert!(app.comment_dialog.is_none());
    }

    #[test]
    fn e_key_edits_selected_own_comment() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.focus_details();
        app.details.insert(
            "1".to_string(),
            DetailState::Loaded(vec![own_comment(42, "chenyukang", "Original body", None)]),
        );

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('e')),
            &config,
            &store,
            &tx
        ));

        assert_eq!(
            app.comment_dialog.as_ref().map(|dialog| &dialog.mode),
            Some(&CommentDialogMode::Edit {
                comment_index: 0,
                comment_id: 42
            })
        );
        assert_eq!(
            app.comment_dialog
                .as_ref()
                .map(|dialog| dialog.body.as_str()),
            Some("Original body")
        );
    }

    #[test]
    fn e_key_rejects_comments_not_owned_by_viewer() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.focus_details();
        app.details.insert(
            "1".to_string(),
            DetailState::Loaded(vec![comment("alice", "Not mine", None)]),
        );

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('e')),
            &config,
            &store,
            &tx
        ));

        assert!(app.comment_dialog.is_none());
        assert_eq!(app.status, "only your comments can be edited");
    }

    #[test]
    fn a_key_opens_new_comment_dialog() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('a')),
            &config,
            &store,
            &tx
        ));

        assert_eq!(
            app.comment_dialog.map(|dialog| dialog.mode),
            Some(CommentDialogMode::New)
        );
    }

    #[test]
    fn enter_in_comment_dialog_adds_newline_without_submitting() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        app.start_new_comment_dialog();
        app.comment_dialog.as_mut().unwrap().body = "hello".to_string();

        app.handle_comment_dialog_key(key(KeyCode::Enter), &tx, None);

        assert_eq!(
            app.comment_dialog
                .as_ref()
                .map(|dialog| dialog.body.as_str()),
            Some("hello\n")
        );
        assert!(!app.posting_comment);
    }

    #[test]
    fn ctrl_enter_in_comment_dialog_submits() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.start_new_comment_dialog();
        app.comment_dialog.as_mut().unwrap().body = "hello".to_string();
        let mut submitted = None;

        app.handle_comment_dialog_key_with_submit(
            KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::CONTROL),
            None,
            |pending| submitted = Some((pending.item.id, pending.body, pending.mode)),
        );

        assert!(app.comment_dialog.is_none());
        assert!(app.posting_comment);
        assert_eq!(app.status, "posting comment");
        assert_eq!(
            submitted,
            Some((
                "1".to_string(),
                "hello".to_string(),
                PendingCommentMode::Post
            ))
        );
    }

    #[test]
    fn ctrl_enter_in_edit_dialog_submits_update() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.details.insert(
            "1".to_string(),
            DetailState::Loaded(vec![own_comment(42, "chenyukang", "old", None)]),
        );
        app.start_edit_selected_comment_dialog();
        app.comment_dialog.as_mut().unwrap().body = "updated".to_string();
        let mut submitted = None;

        app.handle_comment_dialog_key_with_submit(
            KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::CONTROL),
            None,
            |pending| submitted = Some((pending.item.id, pending.body, pending.mode)),
        );

        assert!(app.comment_dialog.is_none());
        assert!(app.posting_comment);
        assert_eq!(app.status, "updating comment");
        assert_eq!(
            submitted,
            Some((
                "1".to_string(),
                "updated".to_string(),
                PendingCommentMode::Edit {
                    comment_index: 0,
                    comment_id: 42
                }
            ))
        );
    }

    #[test]
    fn ctrl_j_variant_also_submits_comment_dialog_for_terminals_without_enhanced_enter() {
        assert!(is_comment_submit_key(KeyEvent::new(
            KeyCode::Char('\n'),
            crossterm::event::KeyModifiers::CONTROL
        )));
        assert!(!is_comment_submit_key(key(KeyCode::Enter)));
    }

    #[test]
    fn comment_dialog_cursor_tracks_end_of_multiline_body() {
        let dialog = CommentDialog {
            mode: CommentDialogMode::New,
            body: "hello\nworld".to_string(),
            scroll: 0,
        };
        let area = Rect::new(10, 5, 30, 10);

        assert_eq!(
            comment_dialog_cursor_position(&dialog.body, dialog.scroll, area, 28, 6),
            Some(Position::new(16, 7))
        );
    }

    #[test]
    fn comment_dialog_cursor_is_hidden_when_scrolled_away_from_input() {
        let body = (1..=80)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let area = Rect::new(10, 5, 30, 10);

        assert_eq!(comment_dialog_cursor_position(&body, 20, area, 28, 6), None);
    }

    #[test]
    fn comment_dialog_cursor_wraps_to_next_line_at_exact_width() {
        let area = Rect::new(10, 5, 7, 10);

        assert_eq!(
            comment_dialog_cursor_position("abcde", 0, area, 5, 6),
            Some(Position::new(11, 7))
        );
    }

    #[test]
    fn comment_dialog_scroll_tracks_cursor_for_long_body() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.start_new_comment_dialog();
        app.comment_dialog.as_mut().unwrap().body = (1..=20)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");

        app.scroll_comment_dialog_to_cursor();

        assert_eq!(
            app.comment_dialog.as_ref().map(|dialog| dialog.scroll),
            Some(10)
        );
    }

    #[test]
    fn comment_dialog_page_and_mouse_scroll_are_clamped() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        app.start_new_comment_dialog();
        app.comment_dialog.as_mut().unwrap().body = (1..=80)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");

        app.handle_comment_dialog_key(key(KeyCode::PageDown), &tx, None);
        assert_eq!(
            app.comment_dialog.as_ref().map(|dialog| dialog.scroll),
            Some(6)
        );

        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 50,
                row: 20,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
            Rect::new(0, 0, 120, 40),
        );
        assert_eq!(
            app.comment_dialog.as_ref().map(|dialog| dialog.scroll),
            Some(9)
        );
    }

    #[test]
    fn comment_dialog_area_grows_with_content_until_it_needs_scroll() {
        let area = Rect::new(0, 0, 120, 40);
        let short = CommentDialog {
            mode: CommentDialogMode::New,
            body: String::new(),
            scroll: 0,
        };
        let medium = CommentDialog {
            mode: CommentDialogMode::New,
            body: (1..=18)
                .map(|line| format!("line {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
            scroll: 0,
        };
        let long = CommentDialog {
            mode: CommentDialogMode::New,
            body: (1..=100)
                .map(|line| format!("line {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
            scroll: 0,
        };

        let short_area = comment_dialog_area(&short, area);
        let medium_area = comment_dialog_area(&medium, area);
        let long_area = comment_dialog_area(&long, area);

        assert!(medium_area.height > short_area.height);
        assert_eq!(long_area.height, comment_dialog_max_height(area));

        let inner = block_inner(long_area);
        let editor_height = inner
            .height
            .saturating_sub(COMMENT_DIALOG_FOOTER_HEIGHT)
            .max(1);
        assert!(max_comment_dialog_scroll(&long.body, inner.width, editor_height) > 0);
    }

    #[test]
    fn details_comment_bodies_are_not_truncated() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let body = (1..=12)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        app.details.insert(
            "1".to_string(),
            DetailState::Loaded(vec![CommentPreview {
                id: None,
                author: "alice".to_string(),
                body,
                created_at: None,
                updated_at: None,
                url: None,
                is_mine: false,
            }]),
        );

        let document = build_details_document(&app, 100);
        let rendered = document
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("line 1 line 2 line 3"));
        assert!(rendered.contains("line 10 line 11 line 12"));
        assert!(!rendered.contains("\n...\n"));
    }

    #[test]
    fn mouse_clicking_table_row_selects_item_and_focuses_details() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let area = Rect::new(0, 0, 100, 40);
        let table = body_areas(body_area(area))[0];
        let inner = block_inner(table);
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: inner.x + 4,
            row: inner.y + TABLE_HEADER_HEIGHT + 1,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };

        handle_mouse(&mut app, mouse, area);

        assert_eq!(app.focus, FocusTarget::Details);
        assert_eq!(app.current_selected_position(), 1);
        assert_eq!(app.current_item().map(|item| item.id.as_str()), Some("2"));
        assert_eq!(app.status, "details focused");
    }

    #[test]
    fn mouse_clicking_table_header_does_not_change_selection() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let area = Rect::new(0, 0, 100, 40);
        let table = body_areas(body_area(area))[0];
        let inner = block_inner(table);
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: inner.x + 4,
            row: inner.y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };

        handle_mouse(&mut app, mouse, area);

        assert_eq!(app.focus, FocusTarget::List);
        assert_eq!(app.current_selected_position(), 0);
    }

    #[test]
    fn mouse_clicking_view_tab_switches_view_and_focuses_list() {
        let sections = vec![
            test_section(),
            SectionSnapshot {
                key: "issues:test".to_string(),
                kind: SectionKind::Issues,
                title: "Issues".to_string(),
                filters: String::new(),
                items: vec![work_item("3", "nervosnetwork/fiber", 3, "Issue", None)],
                refreshed_at: None,
                error: None,
            },
        ];
        let mut app = AppState::new(SectionKind::PullRequests, sections);
        app.focus_details();
        app.search_active = true;
        let area = Rect::new(0, 0, 120, 40);
        let inner = block_inner(page_areas(area)[0]);
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: inner.x
                + display_width(SectionKind::PullRequests.label()) as u16
                + TAB_DIVIDER_WIDTH
                + 1,
            row: inner.y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };

        handle_mouse(&mut app, mouse, area);

        assert_eq!(app.active_view, SectionKind::Issues);
        assert_eq!(app.focus, FocusTarget::List);
        assert!(!app.search_active);
        assert_eq!(app.status, "list focused");
    }

    #[test]
    fn mouse_clicking_section_tab_switches_section_and_focuses_list() {
        let sections = vec![
            SectionSnapshot {
                key: "pull_requests:Mine".to_string(),
                kind: SectionKind::PullRequests,
                title: "Mine".to_string(),
                filters: String::new(),
                items: vec![work_item("1", "rust-lang/rust", 1, "Compiler", None)],
                refreshed_at: None,
                error: None,
            },
            SectionSnapshot {
                key: "pull_requests:Assigned".to_string(),
                kind: SectionKind::PullRequests,
                title: "Assigned".to_string(),
                filters: String::new(),
                items: vec![work_item("2", "nervosnetwork/fiber", 2, "Fiber", None)],
                refreshed_at: None,
                error: None,
            },
        ];
        let mut app = AppState::new(SectionKind::PullRequests, sections);
        app.focus_details();
        app.search_active = true;
        let area = Rect::new(0, 0, 120, 40);
        let inner = block_inner(page_areas(area)[1]);
        let first_label = section_tab_label(&app, app.visible_sections()[0]);
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: inner.x + display_width(&first_label) as u16 + TAB_DIVIDER_WIDTH + 1,
            row: inner.y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };

        handle_mouse(&mut app, mouse, area);

        assert_eq!(app.current_section_position(), 1);
        assert_eq!(
            app.current_section().map(|section| section.title.as_str()),
            Some("Assigned")
        );
        assert_eq!(app.current_selected_position(), 0);
        assert_eq!(app.focus, FocusTarget::List);
        assert!(!app.search_active);
        assert_eq!(app.status, "list focused");
    }

    #[test]
    fn mouse_dragging_splitter_changes_list_details_ratio_and_requests_save() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let area = Rect::new(0, 0, 120, 40);
        let body = body_area(area);
        let split = body_areas_with_ratio(body, app.list_width_percent)[1].x;
        let row = body.y + 3;

        let should_save = handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: split,
                row,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
            area,
        );

        assert!(!should_save);
        assert!(app.dragging_split);

        let target = body.x + body.width.saturating_mul(60) / 100;
        let should_save = handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: target,
                row,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
            area,
        );

        assert!(!should_save);
        assert_eq!(app.list_width_percent, 60);
        assert_eq!(app.status, "layout 60 / 40");

        let should_save = handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: target,
                row,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
            area,
        );

        assert!(should_save);
        assert!(!app.dragging_split);
        assert_eq!(app.status, "layout saved 60 / 40");
    }

    #[test]
    fn split_drag_clamps_to_reasonable_widths() {
        let body = Rect::new(10, 3, 100, 20);

        assert_eq!(
            split_percent_from_column(body, 0),
            crate::state::MIN_LIST_WIDTH_PERCENT
        );
        assert_eq!(
            split_percent_from_column(body, 500),
            crate::state::MAX_LIST_WIDTH_PERCENT
        );
    }

    #[test]
    fn list_page_keys_move_by_visible_page_size() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![many_items_section(30)]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        let area = Rect::new(0, 0, 120, 20);
        let table = body_areas_with_ratio(body_area(area), app.list_width_percent)[0];
        let visible_rows = usize::from(table_visible_rows(table));

        assert!(!handle_key_in_area(
            &mut app,
            key(KeyCode::PageDown),
            &config,
            &store,
            &tx,
            Some(area)
        ));
        assert_eq!(app.current_selected_position(), visible_rows);

        assert!(!handle_key_in_area(
            &mut app,
            key(KeyCode::PageUp),
            &config,
            &store,
            &tx,
            Some(area)
        ));
        assert_eq!(app.current_selected_position(), 0);
    }

    #[test]
    fn mouse_wheel_scrolls_list_when_pointer_is_over_list() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![many_items_section(30)]);
        app.focus_details();
        app.search_active = true;
        let area = Rect::new(0, 0, 120, 40);
        let table = body_areas_with_ratio(body_area(area), app.list_width_percent)[0];
        let inner = block_inner(table);

        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: inner.x + 2,
                row: inner.y + TABLE_HEADER_HEIGHT + 1,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
            area,
        );

        assert_eq!(app.focus, FocusTarget::List);
        assert!(!app.search_active);
        assert_eq!(
            app.current_selected_position(),
            usize::from(MOUSE_SCROLL_LINES)
        );
    }

    #[test]
    fn table_visible_range_tracks_current_page() {
        assert_eq!(table_visible_range(0, 10, 25), Some((1, 10)));
        assert_eq!(table_visible_range(10, 10, 25), Some((2, 11)));
        assert_eq!(table_visible_range(24, 10, 25), Some((16, 25)));
        assert_eq!(table_visible_range(0, 10, 0), None);
    }

    #[test]
    fn details_panel_title_is_static() {
        assert_eq!(details_title(), "Details:");
    }

    #[test]
    fn mouse_wheel_scrolls_details_when_content_overflows() {
        let mut item = work_item("1", "rust-lang/rust", 1, "Compiler diagnostics", None);
        item.body = Some(
            (1..=30)
                .map(|index| format!("paragraph {index}"))
                .collect::<Vec<_>>()
                .join("\n\n"),
        );
        let section = SectionSnapshot {
            key: "pull_requests:test".to_string(),
            kind: SectionKind::PullRequests,
            title: "Test".to_string(),
            filters: String::new(),
            items: vec![item],
            refreshed_at: None,
            error: None,
        };
        let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
        let area = Rect::new(0, 0, 100, 20);
        let details = body_areas(body_area(area))[1];
        let inner = block_inner(details);

        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: inner.x + 2,
                row: inner.y + 2,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
            area,
        );

        assert_eq!(app.focus, FocusTarget::Details);
        assert_eq!(app.details_scroll, MOUSE_SCROLL_LINES);

        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::ScrollUp,
                column: inner.x + 2,
                row: inner.y + 2,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
            area,
        );

        assert_eq!(app.details_scroll, 0);
    }

    #[test]
    fn mouse_wheel_does_not_scroll_details_without_overflow() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let area = Rect::new(0, 0, 100, 80);
        let details = body_areas(body_area(area))[1];
        let inner = block_inner(details);

        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: inner.x + 2,
                row: inner.y + 2,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
            area,
        );

        assert_eq!(app.focus, FocusTarget::Details);
        assert_eq!(app.details_scroll, 0);
    }

    #[test]
    fn enter_and_five_focus_details_without_quitting() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Enter),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.focus, FocusTarget::Details);

        app.focus_list();
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('5')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.focus, FocusTarget::Details);
    }

    #[test]
    fn escape_returns_from_details_to_list() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        app.focus_details();
        app.scroll_details(5);
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Esc),
            &config,
            &store,
            &tx
        ));

        assert_eq!(app.focus, FocusTarget::List);
        assert_eq!(app.details_scroll, 5);
    }

    #[test]
    fn details_scroll_does_not_move_list_selection() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        app.focus_details();
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('j')),
            &config,
            &store,
            &tx
        ));

        assert_eq!(app.current_selected_position(), 0);
        assert_eq!(app.details_scroll, 1);
    }

    #[test]
    fn four_focuses_primary_list_without_changing_page() {
        let sections = vec![
            SectionSnapshot {
                key: "pull_requests:My Pull Requests".to_string(),
                kind: SectionKind::PullRequests,
                title: "My Pull Requests".to_string(),
                filters: String::new(),
                items: vec![work_item(
                    "1",
                    "rust-lang/rust",
                    1,
                    "Compiler diagnostics",
                    None,
                )],
                refreshed_at: None,
                error: None,
            },
            SectionSnapshot {
                key: "pull_requests:Assigned to Me".to_string(),
                kind: SectionKind::PullRequests,
                title: "Assigned to Me".to_string(),
                filters: String::new(),
                items: vec![work_item(
                    "2",
                    "nervosnetwork/fiber",
                    2,
                    "Funding state",
                    None,
                )],
                refreshed_at: None,
                error: None,
            },
            SectionSnapshot {
                key: "issues:Test".to_string(),
                kind: SectionKind::Issues,
                title: "Test".to_string(),
                filters: String::new(),
                items: vec![work_item("3", "nervosnetwork/ckb", 3, "Issue", None)],
                refreshed_at: None,
                error: None,
            },
        ];
        let mut app = AppState::new(SectionKind::Issues, sections);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        app.focus_details();
        app.scroll_details(3);
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('4')),
            &config,
            &store,
            &tx
        ));

        assert_eq!(app.active_view, SectionKind::Issues);
        assert_eq!(app.current_section_position(), 0);
        assert_eq!(app.current_selected_position(), 0);
        assert_eq!(app.focus, FocusTarget::List);
        assert_eq!(app.details_scroll, 0);
        assert_eq!(
            app.current_section().map(|section| section.title.as_str()),
            Some("Test")
        );
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    fn test_section() -> SectionSnapshot {
        SectionSnapshot {
            key: "pull_requests:test".to_string(),
            kind: SectionKind::PullRequests,
            title: "Test".to_string(),
            filters: String::new(),
            items: vec![
                work_item("1", "rust-lang/rust", 1, "Compiler diagnostics", None),
                work_item("2", "nervosnetwork/fiber", 2, "Funding state", None),
            ],
            refreshed_at: None,
            error: None,
        }
    }

    fn many_items_section(count: u64) -> SectionSnapshot {
        SectionSnapshot {
            key: "pull_requests:many".to_string(),
            kind: SectionKind::PullRequests,
            title: "Many".to_string(),
            filters: String::new(),
            items: (1..=count)
                .map(|number| {
                    work_item(
                        &number.to_string(),
                        "rust-lang/rust",
                        number,
                        &format!("Item {number}"),
                        None,
                    )
                })
                .collect(),
            refreshed_at: None,
            error: None,
        }
    }

    fn comment(author: &str, body: &str, url: Option<&str>) -> CommentPreview {
        CommentPreview {
            id: None,
            author: author.to_string(),
            body: body.to_string(),
            created_at: None,
            updated_at: None,
            url: url.map(str::to_string),
            is_mine: false,
        }
    }

    fn own_comment(id: u64, author: &str, body: &str, url: Option<&str>) -> CommentPreview {
        CommentPreview {
            id: Some(id),
            author: author.to_string(),
            body: body.to_string(),
            created_at: None,
            updated_at: None,
            url: url.map(str::to_string),
            is_mine: true,
        }
    }

    fn work_item(id: &str, repo: &str, number: u64, title: &str, author: Option<&str>) -> WorkItem {
        WorkItem {
            id: id.to_string(),
            kind: ItemKind::PullRequest,
            repo: repo.to_string(),
            number: Some(number),
            title: title.to_string(),
            body: Some("A body with useful context".to_string()),
            author: author.map(str::to_string),
            state: Some("open".to_string()),
            url: format!("https://github.com/{repo}/pull/{number}"),
            updated_at: None,
            labels: vec!["T-compiler".to_string()],
            comments: Some(0),
            unread: None,
            reason: None,
            extra: None,
        }
    }
}
