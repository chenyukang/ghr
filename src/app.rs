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
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, Paragraph, Row, Table, TableState, Tabs, Wrap,
};
use ratatui::{Frame, Terminal};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tracing::warn;

use crate::config::Config;
use crate::dirs::Paths;
use crate::github::{
    approve_pull_request, close_pull_request, edit_issue_comment, fetch_issue_comments,
    fetch_pull_request_action_hints, merge_pull_request, post_issue_comment, refresh_dashboard,
    refresh_section_page, search_global,
};
use crate::model::{
    ActionHints, CheckSummary, CommentPreview, ItemKind, SectionKind, SectionSnapshot, WorkItem,
    builtin_view_key, configured_sections, global_search_view_key, merge_cached_sections,
    merge_refreshed_sections, section_counts, section_view_key,
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
        comments: std::result::Result<Vec<CommentPreview>, String>,
        actions: Option<std::result::Result<ActionHints, String>>,
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
    PrActionFinished {
        item_id: String,
        action: PrAction,
        result: std::result::Result<(), String>,
    },
    SectionPageLoaded {
        section_key: String,
        section: SectionSnapshot,
        save_error: Option<String>,
    },
    GlobalSearchFinished {
        query: String,
        sections: Vec<SectionSnapshot>,
    },
}

#[derive(Debug, Clone)]
enum DetailState {
    Loading,
    Loaded(Vec<CommentPreview>),
    Error(String),
}

#[derive(Debug, Clone)]
enum ActionHintState {
    Loading,
    Loaded(ActionHints),
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusTarget {
    Ghr,
    Sections,
    List,
    Details,
}

impl FocusTarget {
    fn as_state_str(self) -> &'static str {
        match self {
            Self::Ghr => "ghr",
            Self::Sections => "sections",
            Self::List => "list",
            Self::Details => "details",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Ghr => "ghr",
            Self::Sections => "Sections",
            Self::List => "List",
            Self::Details => "Details",
        }
    }

    fn from_state_str(value: &str) -> Self {
        match value {
            "ghr" => Self::Ghr,
            "sections" => Self::Sections,
            "details" => Self::Details,
            _ => Self::List,
        }
    }
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
enum PrAction {
    Merge,
    Close,
    Approve,
}

#[derive(Debug, Clone)]
struct PrActionDialog {
    item: WorkItem,
    action: PrAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MessageDialog {
    title: String,
    body: String,
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
const MOUSE_SCROLL_LINES: u16 = 2;
const COMMENT_DIALOG_WIDTH_PERCENT: u16 = 72;
const COMMENT_DIALOG_MIN_HEIGHT: u16 = 10;
const COMMENT_DIALOG_VERTICAL_MARGIN: u16 = 4;
const COMMENT_DIALOG_FOOTER_HEIGHT: u16 = 2;
const COMMENT_DIALOG_MIN_EDITOR_HEIGHT: u16 = 4;
const COMMENT_DIALOG_EDITOR_PADDING_LINES: u16 = 1;
const COMMENT_DIALOG_FALLBACK_EDITOR_HEIGHT: u16 = 10;
const COMMENT_DIALOG_FALLBACK_EDITOR_WIDTH: u16 = 48;
const COMMENT_LEFT_PADDING: usize = 2;
const COMMENT_RIGHT_PADDING: usize = 4;
const SEARCH_RESULT_WINDOW: usize = 1000;

struct AppState {
    active_view: String,
    sections: Vec<SectionSnapshot>,
    section_index: HashMap<String, usize>,
    selected_index: HashMap<String, usize>,
    focus: FocusTarget,
    details_scroll: u16,
    list_width_percent: u16,
    dragging_split: bool,
    split_drag_changed: bool,
    search_active: bool,
    search_query: String,
    global_search_active: bool,
    global_search_query: String,
    global_search_running: bool,
    status: String,
    refreshing: bool,
    last_refresh_request: Instant,
    details: HashMap<String, DetailState>,
    action_hints: HashMap<String, ActionHintState>,
    details_stale: HashSet<String>,
    selected_comment_index: usize,
    comment_dialog: Option<CommentDialog>,
    posting_comment: bool,
    pr_action_dialog: Option<PrActionDialog>,
    pr_action_running: bool,
    setup_dialog: Option<SetupDialog>,
    message_dialog: Option<MessageDialog>,
    mouse_capture_enabled: bool,
    help_dialog: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ViewTab {
    key: String,
    label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RefreshAnchor {
    active_view: String,
    section_key: Option<String>,
    item_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SectionPageRequest {
    section_key: String,
    view: String,
    kind: SectionKind,
    title: String,
    filters: String,
    page: usize,
    page_size: usize,
    total_pages: usize,
    total_is_capped: bool,
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
    let mut mouse_capture_enabled = true;
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

        let mut should_quit = false;
        if event::poll(Duration::from_millis(120))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }

                    let size = terminal.size()?;
                    let area = Rect::new(0, 0, size.width, size.height);
                    if handle_key_in_area(app, key, config, store, tx, Some(area)) {
                        save_ui_state(app, paths);
                        should_quit = true;
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
        sync_mouse_capture(terminal, app, &mut mouse_capture_enabled)?;
        if should_quit {
            break;
        }
    }

    save_ui_state(app, paths);
    Ok(())
}

fn sync_mouse_capture(
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

fn start_section_page_load(
    app: &mut AppState,
    config: &Config,
    store: &SnapshotStore,
    tx: &UnboundedSender<AppMsg>,
    delta: isize,
) {
    if app.refreshing {
        app.status = "refresh already running".to_string();
        return;
    }

    let request = match app.section_page_request(delta, config) {
        Ok(request) => request,
        Err(message) => {
            app.status = message;
            return;
        }
    };
    let page_label =
        section_page_status_label(request.page, request.total_pages, request.total_is_capped);
    app.refreshing = true;
    app.last_refresh_request = Instant::now();
    app.status = format!("loading {} page {page_label}", request.title);

    let config = config.clone();
    let store = store.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let section = refresh_section_page(
            request.view,
            request.kind,
            request.title,
            request.filters,
            request.page,
            request.page_size,
            &config,
        )
        .await;
        let mut save_error = None;
        if section.error.is_none()
            && let Err(error) = store.save_section(&section)
        {
            let message = error.to_string();
            warn!(error = %message, "failed to save paged snapshot");
            save_error = Some(message);
        }
        let _ = tx.send(AppMsg::SectionPageLoaded {
            section_key: request.section_key,
            section,
            save_error,
        });
    });
}

fn start_global_search(
    query: String,
    repo_scope: Option<String>,
    config: Config,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let sections = search_global(&query, repo_scope.as_deref(), &config).await;
        let _ = tx.send(AppMsg::GlobalSearchFinished { query, sections });
    });
}

fn start_details_load(item: WorkItem, tx: UnboundedSender<AppMsg>) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let number = item.number;
        let comments = match number {
            Some(number) => fetch_issue_comments(&item.repo, number)
                .await
                .map_err(|error| error.to_string()),
            None => Ok(Vec::new()),
        };
        let actions = match (item.kind, number) {
            (ItemKind::PullRequest, Some(number)) => Some(
                fetch_pull_request_action_hints(&item.repo, number)
                    .await
                    .map_err(|error| error.to_string()),
            ),
            _ => None,
        };
        let _ = tx.send(AppMsg::DetailsLoaded {
            item_id,
            comments,
            actions,
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

fn start_pr_action(
    item: WorkItem,
    action: PrAction,
    config: Config,
    store: SnapshotStore,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let result = match item.number {
            Some(number) => match action {
                PrAction::Merge => merge_pull_request(&item.repo, number)
                    .await
                    .map_err(|error| error.to_string()),
                PrAction::Close => close_pull_request(&item.repo, number)
                    .await
                    .map_err(|error| error.to_string()),
                PrAction::Approve => approve_pull_request(&item.repo, number)
                    .await
                    .map_err(|error| error.to_string()),
            },
            None => Err("selected item has no pull request number".to_string()),
        };
        let should_refresh = result.is_ok();
        let _ = tx.send(AppMsg::PrActionFinished {
            item_id,
            action,
            result,
        });

        if should_refresh {
            let _ = tx.send(AppMsg::RefreshStarted);
            let sections = refresh_dashboard(&config).await;
            let mut save_error = None;
            for section in &sections {
                if section.error.is_some() {
                    continue;
                }
                if let Err(error) = store.save_section(section) {
                    let message = error.to_string();
                    warn!(error = %message, "failed to save refreshed snapshot after PR action");
                    save_error = Some(message);
                    break;
                }
            }
            let _ = tx.send(AppMsg::RefreshFinished {
                sections,
                save_error,
            });
        }
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

    if app.message_dialog.is_some() {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => app.dismiss_message_dialog(),
            _ => {}
        }
        return false;
    }

    if app.comment_dialog.is_some() {
        app.handle_comment_dialog_key(key, tx, area);
        return false;
    }

    if app.pr_action_dialog.is_some() {
        app.handle_pr_action_dialog_key(key, config, store, tx);
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
    if matches!(key.code, KeyCode::Char('m')) {
        app.toggle_mouse_capture();
        return false;
    }

    match key.code {
        KeyCode::Char('q') => return true,
        KeyCode::Char('?') => app.show_help_dialog(),
        KeyCode::Char('r') => trigger_refresh(app, config, store, tx),
        KeyCode::Char('S') => app.start_global_search_input(),
        KeyCode::Tab => app.move_view(1),
        KeyCode::BackTab => app.move_view(-1),
        KeyCode::Char('o') => app.open_selected(),
        _ => {}
    }

    match app.focus {
        FocusTarget::Ghr => match key.code {
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(']') => app.move_view(1),
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('[') => app.move_view(-1),
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Enter => app.focus_sections(),
            KeyCode::Esc => app.focus_list(),
            _ => {}
        },
        FocusTarget::Sections => match key.code {
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(']') => app.move_section(1),
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('[') => app.move_section(-1),
            KeyCode::Up | KeyCode::Char('k') => app.focus_ghr(),
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Enter => app.focus_list(),
            KeyCode::Esc => app.focus_list(),
            _ => {}
        },
        FocusTarget::List => match key.code {
            KeyCode::Esc if !app.search_query.is_empty() => app.clear_search(),
            KeyCode::Esc => {}
            KeyCode::Char('/') => app.start_search(),
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
            KeyCode::Char('[') => start_section_page_load(app, config, store, tx, -1),
            KeyCode::Char(']') => start_section_page_load(app, config, store, tx, 1),
            KeyCode::Char('M') => app.start_pr_action_dialog(PrAction::Merge),
            KeyCode::Char('C') => app.start_pr_action_dialog(PrAction::Close),
            KeyCode::Char('A') => app.start_pr_action_dialog(PrAction::Approve),
            KeyCode::Char('a') => app.start_new_comment_dialog(),
            KeyCode::Enter => app.focus_details(),
            _ => {}
        },
        FocusTarget::Details => match key.code {
            KeyCode::Esc => app.focus_list(),
            KeyCode::Char('M') => app.start_pr_action_dialog(PrAction::Merge),
            KeyCode::Char('C') => app.start_pr_action_dialog(PrAction::Close),
            KeyCode::Char('A') => app.start_pr_action_dialog(PrAction::Approve),
            KeyCode::Char('a') => app.start_new_comment_dialog(),
            KeyCode::Char('R') => app.start_reply_to_selected_comment(),
            KeyCode::Char('e') => app.start_edit_selected_comment_dialog(),
            KeyCode::Char('n') => app.move_comment(1),
            KeyCode::Char('p') => app.move_comment(-1),
            KeyCode::Down | KeyCode::Char('j') => app.scroll_details(1),
            KeyCode::Up | KeyCode::Char('k') => app.scroll_details(-1),
            KeyCode::PageDown | KeyCode::Char('d') => app.scroll_details(8),
            KeyCode::PageUp | KeyCode::Char('u') => app.scroll_details(-8),
            KeyCode::Char('g') => app.details_scroll = 0,
            _ => {}
        },
    }

    false
}

fn handle_global_focus_key(app: &mut AppState, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('1') => app.focus_ghr(),
        KeyCode::Char('2') => app.focus_sections(),
        KeyCode::Char('3') => app.focus_primary_list(),
        KeyCode::Char('4') => app.focus_details(),
        _ => return false,
    }
    true
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
    if !app.mouse_capture_enabled {
        return false;
    }
    if app.setup_dialog.is_some() {
        return false;
    }
    if app.help_dialog {
        return false;
    }
    if app.message_dialog.is_some() {
        return false;
    }
    if app.pr_action_dialog.is_some() {
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
    if let Some(view) = view_tab_at(app, view_tabs_area, mouse.column, mouse.row) {
        app.switch_view(view);
        app.focus = FocusTarget::Ghr;
        app.search_active = false;
        app.global_search_active = false;
        app.status = "ghr focused".to_string();
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
    app.global_search_active = false;

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
    app.global_search_active = false;

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
    app.global_search_active = false;
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

fn table_visible_range_label(
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

fn table_viewport_offset(selected: usize, visible_rows: usize) -> usize {
    if visible_rows == 0 {
        return 0;
    }
    selected.saturating_sub(visible_rows - 1)
}

fn view_tab_at(app: &AppState, area: Rect, column: u16, row: u16) -> Option<String> {
    let views = app.view_tabs();
    let labels = views
        .iter()
        .map(|view| view.label.clone())
        .collect::<Vec<_>>();
    tab_index_at(&labels, area, column, row)
        .and_then(|index| views.get(index).map(|view| view.key.clone()))
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

    if app.mouse_capture_enabled {
        let body = body_areas_with_ratio(chunks[2], app.list_width_percent);
        draw_table(frame, app, body[0]);
        draw_details(frame, app, body[1]);
    } else {
        draw_details(frame, app, chunks[2]);
    }
    draw_footer(frame, app, paths, chunks[3]);

    if let Some(dialog) = app.setup_dialog {
        draw_setup_dialog(frame, dialog, area);
    } else if let Some(dialog) = &app.message_dialog {
        draw_message_dialog(frame, dialog, area);
    } else if app.help_dialog {
        draw_help_dialog(frame, area);
    } else if let Some(dialog) = &app.pr_action_dialog {
        draw_pr_action_dialog(frame, dialog, app.pr_action_running, area);
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
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let border_type = if ghr_focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    };
    let title = if ghr_focused { "[FOCUS] ghr" } else { "ghr" };

    let tabs = Tabs::new(titles)
        .select(active)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(border_type)
                .border_style(border_style)
                .title(Span::styled(title, border_style)),
        )
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
    let sections_focused = app.focus == FocusTarget::Sections;
    let border_style = if sections_focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let border_type = if sections_focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    };
    let title = if sections_focused {
        "[FOCUS] Sections"
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
                .title(Span::styled(title, border_style)),
        )
        .style(Style::default().fg(Color::Gray))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_widget(tabs, area);
}

fn section_tab_label(app: &AppState, section: &SectionSnapshot) -> String {
    let (_, unread) = section_counts(section);
    let count_label = section_count_label(section);
    if !app.search_query.is_empty() {
        format!(
            "{} ({}/{})",
            section.title,
            app.filtered_indices(section).len(),
            section.items.len()
        )
    } else if unread > 0 {
        format!("{} ({count_label}/{unread})", section.title)
    } else {
        format!("{} ({count_label})", section.title)
    }
}

fn section_count_label(section: &SectionSnapshot) -> String {
    let loaded = section.items.len();
    match section.total_count {
        Some(total) if total > loaded => format!("{loaded}/{total}"),
        Some(total) => total.to_string(),
        None => loaded.to_string(),
    }
}

fn section_page_size(section: &SectionSnapshot, config: &Config) -> usize {
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

fn section_page_size_for_display(section: &SectionSnapshot) -> Option<usize> {
    if section.page_size > 0 {
        Some(section.page_size)
    } else if !section.items.is_empty() {
        Some(section.items.len().clamp(1, 100))
    } else {
        None
    }
}

fn section_total_pages(total_count: usize, page_size: usize) -> (usize, bool) {
    let accessible = total_count.clamp(1, SEARCH_RESULT_WINDOW);
    let total_pages = accessible.div_ceil(page_size.max(1)).max(1);
    (total_pages, total_count > SEARCH_RESULT_WINDOW)
}

fn section_page_label(section: &SectionSnapshot) -> Option<String> {
    let total_count = section.total_count?;
    let page_size = section_page_size_for_display(section)?;
    let (total_pages, total_is_capped) = section_total_pages(total_count, page_size);
    Some(section_page_status_label(
        section.page.max(1).min(total_pages),
        total_pages,
        total_is_capped,
    ))
}

fn section_page_status_label(page: usize, total_pages: usize, total_is_capped: bool) -> String {
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
            Row::new(vec![
                item.repo.clone(),
                item.number
                    .map(|number| format!("#{number}"))
                    .unwrap_or_default(),
                item.title.clone(),
                relative_time(item.updated_at),
                item_meta(item),
            ])
        })
        .collect::<Vec<_>>();

    let list_focused = app.focus == FocusTarget::List;
    let header_style = if list_focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD)
    };
    let header = Row::new(vec!["Repo", "#", "Title", "Updated", "Meta"])
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
    if let Some(error) = &section.error {
        title.push_str(&format!(" - error: {}", compact_error_label(error)));
    };
    if let Some((start, end)) = table_visible_range(
        app.current_selected_position(),
        usize::from(table_visible_rows(area)),
        filtered_indices.len(),
    ) {
        title.push_str(&table_visible_range_label(
            section,
            app.search_query.is_empty(),
            start,
            end,
            filtered_indices.len(),
        ));
    }

    let input_prompt = active_list_input_prompt(app);
    let (border_style, title_style, border_type, highlight_style) = if app.dragging_split {
        (
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
            Style::default()
                .fg(Color::Black)
                .bg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
            BorderType::Thick,
            Style::default()
                .fg(Color::Black)
                .bg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        )
    } else if let Some((_, color)) = &input_prompt {
        (
            Style::default().fg(*color).add_modifier(Modifier::BOLD),
            Style::default()
                .fg(Color::Black)
                .bg(*color)
                .add_modifier(Modifier::BOLD),
            BorderType::Thick,
            Style::default()
                .fg(Color::Black)
                .bg(*color)
                .add_modifier(Modifier::BOLD),
        )
    } else if list_focused {
        (
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            BorderType::Thick,
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            Style::default().fg(Color::DarkGray),
            Style::default().fg(Color::Gray),
            BorderType::Plain,
            Style::default().fg(Color::White).bg(Color::DarkGray),
        )
    };
    if let Some((prompt, _)) = input_prompt {
        title = format!("{prompt} | {title}");
    }
    let title = focus_panel_title("List", &title, list_focused);

    let table = Table::new(
        rows,
        [
            Constraint::Length(24),
            Constraint::Length(8),
            Constraint::Min(20),
            Constraint::Length(8),
            Constraint::Length(18),
        ],
    )
    .header(header)
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
    if !filtered_indices.is_empty() {
        table_state.select(Some(app.current_selected_position()));
    }
    frame.render_stateful_widget(table, area, &mut table_state);
}

fn active_list_input_prompt(app: &AppState) -> Option<(String, Color)> {
    if app.global_search_active {
        let scope = app
            .current_repo_scope()
            .map(|repo| format!(" in {repo}"))
            .unwrap_or_default();
        return Some((
            format!(
                "Repo Search{scope}: S{}_  Enter search  Esc cancel",
                app.global_search_query
            ),
            Color::LightMagenta,
        ));
    }

    if app.search_active {
        return Some((
            format!("Filter: /{}_  Enter apply  Esc clear", app.search_query),
            Color::Yellow,
        ));
    }

    None
}

fn draw_details(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let details_focused = app.focus == FocusTarget::Details;
    let title = focus_panel_title("Details", details_title(), details_focused);
    let (border_style, title_style, border_type) = if app.dragging_split {
        (
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
            Style::default()
                .fg(Color::Black)
                .bg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
            BorderType::Thick,
        )
    } else if details_focused {
        (
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
            BorderType::Thick,
        )
    } else {
        (
            Style::default().fg(Color::DarkGray),
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
            BorderType::Plain,
        )
    };

    let document = build_details_document(app, area.width.saturating_sub(2));

    let details = Paragraph::new(Text::from(document.lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(border_type)
                .border_style(border_style)
                .title(Span::styled(title, title_style)),
        )
        .scroll((app.details_scroll, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(details, area);
}

fn details_title() -> &'static str {
    "Details:"
}

fn focus_panel_title(label: &str, title: &str, focused: bool) -> String {
    if focused {
        format!("[FOCUS {label}] {title}")
    } else {
        title.to_string()
    }
}

fn same_view_key(left: &str, right: &str) -> bool {
    left == right
        || (left.starts_with("repo:")
            && right.starts_with("repo:")
            && left.eq_ignore_ascii_case(right))
}

fn section_repo_scope(section: &SectionSnapshot) -> Option<String> {
    section
        .filters
        .split_whitespace()
        .find_map(repo_token_value)
}

fn repo_token_value(token: &str) -> Option<String> {
    token
        .strip_prefix("repo:")
        .filter(|repo| !repo.trim().is_empty())
        .map(str::to_string)
}

fn draw_footer(frame: &mut Frame<'_>, app: &AppState, paths: &Paths, area: Rect) {
    let footer = Paragraph::new(footer_line(app, paths)).style(Style::default().fg(Color::Gray));
    frame.render_widget(footer, area);
}

fn footer_line(app: &AppState, paths: &Paths) -> Line<'static> {
    let refresh = if app.refreshing { "refreshing" } else { "idle" };
    let focus = app.focus.label();
    let search = if app.global_search_active {
        Some(format!("repo-search: S{}_", app.global_search_query))
    } else if app.global_search_running {
        Some("repo search running".to_string())
    } else if app.search_active {
        Some(format!("filter: /{}_", app.search_query))
    } else if app.search_query.is_empty() {
        None
    } else {
        Some(format!("filter: /{}", app.search_query))
    };
    let (mouse, text_selection_state) = if app.mouse_capture_enabled {
        ("text-select", None)
    } else {
        ("restore mouse", Some("text-select: drag copy"))
    };

    let mut spans = Vec::new();
    push_footer_focus_shortcuts(&mut spans, app);

    push_footer_separator(&mut spans);
    push_footer_pair(&mut spans, "1-4", "focus", Color::Cyan);
    push_footer_pair(&mut spans, "?", "help", Color::Yellow);
    push_footer_pair(&mut spans, "S", "repo", Color::Yellow);
    push_footer_pair(&mut spans, "r", "refresh", Color::Yellow);
    push_footer_pair(&mut spans, "o", "open", Color::Yellow);
    push_footer_pair(&mut spans, "m", mouse, Color::LightBlue);
    push_footer_pair(&mut spans, "q", "quit", Color::Yellow);

    push_footer_separator(&mut spans);
    push_footer_state(&mut spans, "focus", focus, Color::Cyan);
    if let Some(search) = search {
        push_footer_state(&mut spans, "search", search, Color::Yellow);
    }
    if let Some(text_selection_state) = text_selection_state {
        push_footer_state(&mut spans, "mode", text_selection_state, Color::LightBlue);
    }
    push_footer_state(&mut spans, "refresh", refresh, Color::Green);
    push_footer_state(&mut spans, "state", app.status.clone(), Color::Green);
    push_footer_state(
        &mut spans,
        "db",
        paths.db_path.display().to_string(),
        Color::DarkGray,
    );

    Line::from(spans)
}

fn push_footer_focus_shortcuts(spans: &mut Vec<Span<'static>>, app: &AppState) {
    match app.focus {
        FocusTarget::Ghr => {
            push_footer_context(spans, "ghr", "tabs");
            push_footer_pair(spans, "h/l", "switch", Color::Cyan);
            push_footer_pair(spans, "j/enter", "Sections", Color::Cyan);
            push_footer_pair(spans, "esc", "List", Color::Cyan);
        }
        FocusTarget::Sections => {
            push_footer_context(spans, "Sections", "tabs");
            push_footer_pair(spans, "h/l", "switch", Color::Cyan);
            push_footer_pair(spans, "k", "ghr", Color::Cyan);
            push_footer_pair(spans, "j/enter", "List", Color::Cyan);
            push_footer_pair(spans, "esc", "List", Color::Cyan);
        }
        FocusTarget::List => {
            push_footer_context(spans, "List", "items");
            push_footer_pair(spans, "j/k", "move", Color::Cyan);
            push_footer_pair(spans, "pg d/u", "page", Color::Cyan);
            push_footer_pair(spans, "[ ]", "results", Color::Cyan);
            push_footer_pair(spans, "g/G", "ends", Color::Cyan);
            push_footer_pair(spans, "enter", "Details", Color::Cyan);
            push_footer_pair(spans, "/", "filter", Color::Yellow);
            push_footer_pair(spans, "a", "comment", Color::LightBlue);
            push_footer_pair(spans, "M/C/A", "pr action", Color::LightMagenta);
        }
        FocusTarget::Details => {
            push_footer_context(spans, "Details", "content");
            push_footer_pair(spans, "j/k", "scroll", Color::Cyan);
            push_footer_pair(spans, "pg d/u", "page", Color::Cyan);
            push_footer_pair(spans, "g", "top", Color::Cyan);
            push_footer_pair(spans, "n/p", "comment", Color::LightBlue);
            push_footer_pair(spans, "a", "comment", Color::LightBlue);
            push_footer_pair(spans, "R", "reply", Color::LightBlue);
            push_footer_pair(spans, "e", "edit", Color::LightBlue);
            push_footer_pair(spans, "M/C/A", "pr action", Color::LightMagenta);
            push_footer_pair(spans, "esc", "List", Color::Cyan);
        }
    }
}

fn push_footer_separator(spans: &mut Vec<Span<'static>>) {
    spans.push(Span::styled(" | ", Style::default().fg(Color::DarkGray)));
}

fn push_footer_pair(
    spans: &mut Vec<Span<'static>>,
    key: impl Into<String>,
    label: impl Into<String>,
    key_color: Color,
) {
    if !spans.is_empty() && !footer_ends_with_separator(spans) {
        spans.push(Span::raw("  "));
    }
    spans.push(Span::styled(
        key.into(),
        Style::default().fg(key_color).add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(label.into(), Style::default().fg(Color::Gray)));
}

fn push_footer_state(
    spans: &mut Vec<Span<'static>>,
    key: &'static str,
    value: impl Into<String>,
    value_color: Color,
) {
    if !spans.is_empty() && !footer_ends_with_separator(spans) {
        spans.push(Span::raw("  "));
    }
    spans.push(Span::styled(
        key,
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(value.into(), Style::default().fg(value_color)));
}

fn push_footer_context(
    spans: &mut Vec<Span<'static>>,
    key: &'static str,
    value: impl Into<String>,
) {
    if !spans.is_empty() && !footer_ends_with_separator(spans) {
        spans.push(Span::raw("  "));
    }
    spans.push(Span::styled(
        key,
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(value.into(), Style::default().fg(Color::Gray)));
}

fn footer_ends_with_separator(spans: &[Span<'static>]) -> bool {
    spans
        .last()
        .map(|span| span.content.as_ref() == " | ")
        .unwrap_or(false)
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

fn draw_pr_action_dialog(
    frame: &mut Frame<'_>,
    dialog: &PrActionDialog,
    running: bool,
    area: Rect,
) {
    let dialog_area = centered_rect(66, 12, area);
    let number = dialog
        .item
        .number
        .map(|number| format!("#{number}"))
        .unwrap_or_else(|| "-".to_string());
    let action_label = match dialog.action {
        PrAction::Merge => "merge",
        PrAction::Close => "close",
        PrAction::Approve => "approve",
    };
    let prompt = match dialog.action {
        PrAction::Merge => "Merge this pull request on GitHub?",
        PrAction::Close => "Close this pull request on GitHub?",
        PrAction::Approve => "Approve this pull request on GitHub?",
    };
    let status = if running {
        "working...".to_string()
    } else {
        format!("y/Enter: yes, {action_label} PR    Esc: cancel")
    };
    let lines = vec![
        Line::from(prompt),
        Line::from(""),
        key_value_line("repo", dialog.item.repo.clone()),
        key_value_line("pull request", number),
        key_value_line("title", dialog.item.title.clone()),
        Line::from(""),
        Line::from(vec![Span::styled(
            status,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .style(Style::default().bg(Color::Black))
        .title(Span::styled(
            match dialog.action {
                PrAction::Merge => "Merge Pull Request",
                PrAction::Close => "Close Pull Request",
                PrAction::Approve => "Approve Pull Request",
            },
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

fn draw_message_dialog(frame: &mut Frame<'_>, dialog: &MessageDialog, area: Rect) {
    let dialog_area = centered_rect(78, 14, area);
    let text = format!("{}\n\nEnter/Esc: close", dialog.body);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::LightRed))
        .style(Style::default().bg(Color::Black))
        .title(Span::styled(
            dialog.title.clone(),
            Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(text)
        .block(block)
        .style(Style::default().fg(Color::White).bg(Color::Black))
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
}

fn key_value_line(key: &'static str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key}: "), Style::default().fg(Color::Gray)),
        Span::raw(value),
    ])
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
        help_key_line("1 / 2 / 3 / 4", "focus ghr / Sections / List / Details"),
        help_key_line("/", "start fuzzy search filtering"),
        help_key_line("S", "search PRs and issues in the current repo"),
        help_key_line("m", "toggle mouse text selection mode"),
        help_key_line("Esc", "leave details or clear search"),
        Line::from(""),
        help_heading("ghr and Sections"),
        help_key_line("h/l or Left/Right", "switch the focused tab group"),
        help_key_line(
            "j/k or Up/Down",
            "move focus between ghr, Sections, and List",
        ),
        Line::from(""),
        help_heading("List"),
        help_key_line("j/k or Up/Down", "move selection"),
        help_key_line("PgDown/PgUp or d/u", "move by visible page"),
        help_key_line("[ / ]", "load previous / next GitHub result page"),
        help_key_line("g / G", "first / last item"),
        help_key_line("Enter or 4", "focus Details"),
        help_key_line("o", "open selected item in browser"),
        help_key_line("S", "search PRs and issues in the current repo"),
        help_key_line("M", "open PR merge confirmation"),
        help_key_line("C", "open PR close confirmation"),
        help_key_line("A", "open PR approve confirmation"),
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
        help_key_line("S", "search PRs and issues in the current repo"),
        help_key_line("M", "open PR merge confirmation"),
        help_key_line("C", "open PR close confirmation"),
        help_key_line("A", "open PR approve confirmation"),
        help_key_line("o", "open selected item in browser"),
        Line::from(""),
        help_heading("Pull Request Confirmation"),
        help_key_line("y / Enter", "run the confirmed PR action"),
        help_key_line("Esc", "cancel PR action"),
        Line::from(""),
        help_heading("Repo Search"),
        help_key_line("S", "open search input"),
        help_key_line("Enter", "run gh search prs and gh search issues"),
        help_key_line("Esc", "cancel global search input"),
        Line::from(""),
        help_heading("Comment Editor"),
        help_key_line("Enter", "insert newline"),
        help_key_line("Ctrl+Enter", "send or update comment"),
        help_key_line("Backspace", "delete previous character"),
        help_key_line("PgDown/PgUp or mouse wheel", "scroll long drafts"),
        help_key_line("Esc", "cancel editing"),
        Line::from(""),
        help_heading("Mouse"),
        help_key_line(
            "m",
            "toggle between TUI mouse controls and terminal text selection",
        ),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WrapTokenKind {
    Word,
    Whitespace,
    Newline,
}

#[derive(Debug, Clone)]
struct WrapToken {
    kind: WrapTokenKind,
    segments: Vec<DetailSegment>,
    width: usize,
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

    fn push_meta_line(&mut self, fields: Vec<(&str, Vec<DetailSegment>)>) {
        let mut segments = Vec::new();
        for (index, (key, mut value)) in fields.into_iter().enumerate() {
            if index > 0 {
                segments.push(DetailSegment::raw("  "));
            }
            segments.push(DetailSegment::styled(
                format!("{key}: "),
                Style::default().fg(Color::Gray),
            ));
            segments.append(&mut value);
        }
        self.push_wrapped_limited(segments, 2);
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

    fn push_markdown_block_indented(
        &mut self,
        text: &str,
        empty_message: &str,
        max_lines: usize,
        max_chars: usize,
        left_padding: usize,
        right_padding: usize,
    ) {
        let text = truncate_text(&normalize_text(text), max_chars);
        if text.trim().is_empty() {
            self.push_indented_wrapped_limited(
                vec![DetailSegment::raw(empty_message.to_string())],
                left_padding,
                right_padding,
                1,
            );
            return;
        }

        let blocks = markdown_blocks(&text);
        let original_width = self.width;
        self.width = reserved_width(self.width, right_padding);
        let indent = padding_prefix(left_padding);
        let mut emitted = 0;
        for block in blocks {
            let mut prefix = indent.clone();
            prefix.extend(quote_prefix(block.quote_depth));
            if !self.push_wrapped_prefixed(
                &block.segments,
                prefix.as_slice(),
                &mut emitted,
                max_lines,
            ) {
                break;
            }
        }
        self.width = original_width;
    }

    fn push_wrapped_limited(&mut self, segments: Vec<DetailSegment>, max_lines: usize) {
        let mut emitted = 0;
        let _ = self.push_wrapped(&segments, &mut emitted, max_lines);
    }

    fn push_indented_wrapped_limited(
        &mut self,
        segments: Vec<DetailSegment>,
        left_padding: usize,
        right_padding: usize,
        max_lines: usize,
    ) {
        let original_width = self.width;
        self.width = reserved_width(self.width, right_padding);
        let prefix = padding_prefix(left_padding);
        let mut emitted = 0;
        if prefix.is_empty() {
            let _ = self.push_wrapped(&segments, &mut emitted, max_lines);
        } else {
            let _ =
                self.push_wrapped_prefixed(&segments, prefix.as_slice(), &mut emitted, max_lines);
        }
        self.width = original_width;
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

        self.push_wrapped_with_prefix(segments, prefix, prefix_width, emitted, max_lines)
    }

    fn push_wrapped(
        &mut self,
        segments: &[DetailSegment],
        emitted: &mut usize,
        max_lines: usize,
    ) -> bool {
        self.push_wrapped_with_prefix(segments, &[], 0, emitted, max_lines)
    }

    fn push_wrapped_with_prefix(
        &mut self,
        segments: &[DetailSegment],
        prefix: &[DetailSegment],
        prefix_width: usize,
        emitted: &mut usize,
        max_lines: usize,
    ) -> bool {
        let mut current = prefix.to_vec();
        let mut column = prefix_width;
        let mut wrote_content = false;
        let content_width = self.width.saturating_sub(prefix_width).max(1);

        for token in wrap_tokens(segments) {
            match token.kind {
                WrapTokenKind::Newline => {
                    trim_trailing_wrap_whitespace(&mut current, &mut column, prefix_width);
                    if !self.flush_wrapped_line(&mut current, emitted, max_lines) {
                        return false;
                    }
                    current = prefix.to_vec();
                    column = prefix_width;
                    wrote_content = false;
                }
                WrapTokenKind::Whitespace => {
                    if !wrote_content {
                        continue;
                    }

                    if column + token.width <= self.width {
                        append_token_segments(&mut current, &token);
                        column += token.width;
                    }
                }
                WrapTokenKind::Word if token.width > content_width => {
                    if wrote_content && column > prefix_width {
                        trim_trailing_wrap_whitespace(&mut current, &mut column, prefix_width);
                        if !self.flush_wrapped_line(&mut current, emitted, max_lines) {
                            return false;
                        }
                        current = prefix.to_vec();
                        column = prefix_width;
                        wrote_content = false;
                    }

                    if !self.push_hard_wrapped_token(
                        &token,
                        prefix,
                        prefix_width,
                        &mut current,
                        &mut column,
                        &mut wrote_content,
                        emitted,
                        max_lines,
                    ) {
                        return false;
                    }
                }
                WrapTokenKind::Word => {
                    if wrote_content && column + token.width > self.width {
                        trim_trailing_wrap_whitespace(&mut current, &mut column, prefix_width);
                        if !self.flush_wrapped_line(&mut current, emitted, max_lines) {
                            return false;
                        }
                        current = prefix.to_vec();
                        column = prefix_width;
                    }

                    append_token_segments(&mut current, &token);
                    column += token.width;
                    wrote_content = true;
                }
            }
        }

        trim_trailing_wrap_whitespace(&mut current, &mut column, prefix_width);
        if wrote_content || column > prefix_width {
            self.flush_wrapped_line(&mut current, emitted, max_lines)
        } else {
            true
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push_hard_wrapped_token(
        &mut self,
        token: &WrapToken,
        prefix: &[DetailSegment],
        prefix_width: usize,
        current: &mut Vec<DetailSegment>,
        column: &mut usize,
        wrote_content: &mut bool,
        emitted: &mut usize,
        max_lines: usize,
    ) -> bool {
        for segment in &token.segments {
            for ch in segment.text.chars() {
                if *column >= self.width {
                    if !self.flush_wrapped_line(current, emitted, max_lines) {
                        return false;
                    }
                    *current = prefix.to_vec();
                    *column = prefix_width;
                    *wrote_content = false;
                }

                push_char_segment(current, segment, ch);
                *column += display_width_char(ch);
                *wrote_content = true;
            }
        }
        true
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

fn wrap_tokens(segments: &[DetailSegment]) -> Vec<WrapToken> {
    let mut tokens = Vec::new();
    for segment in segments {
        for ch in segment.text.chars() {
            if ch == '\n' {
                tokens.push(WrapToken {
                    kind: WrapTokenKind::Newline,
                    segments: Vec::new(),
                    width: 0,
                });
                continue;
            }

            let kind = if ch.is_whitespace() {
                WrapTokenKind::Whitespace
            } else {
                WrapTokenKind::Word
            };
            push_wrap_token_char(&mut tokens, segment, ch, kind);
        }
    }
    tokens
}

fn push_wrap_token_char(
    tokens: &mut Vec<WrapToken>,
    template: &DetailSegment,
    ch: char,
    kind: WrapTokenKind,
) {
    if let Some(last) = tokens.last_mut()
        && last.kind == kind
    {
        push_char_segment(&mut last.segments, template, ch);
        last.width += display_width_char(ch);
        return;
    }

    let mut segments = Vec::new();
    push_char_segment(&mut segments, template, ch);
    tokens.push(WrapToken {
        kind,
        segments,
        width: display_width_char(ch),
    });
}

fn append_token_segments(current: &mut Vec<DetailSegment>, token: &WrapToken) {
    for segment in &token.segments {
        push_text_segment(current, segment, &segment.text);
    }
}

fn push_text_segment(current: &mut Vec<DetailSegment>, template: &DetailSegment, text: &str) {
    if text.is_empty() {
        return;
    }

    if let Some(last) = current.last_mut()
        && last.style == template.style
        && last.link == template.link
        && last.action == template.action
    {
        last.text.push_str(text);
        return;
    }

    current.push(DetailSegment {
        text: text.to_string(),
        style: template.style,
        link: template.link.clone(),
        action: template.action.clone(),
    });
}

fn trim_trailing_wrap_whitespace(
    current: &mut Vec<DetailSegment>,
    column: &mut usize,
    min_width: usize,
) {
    while *column > min_width {
        let Some(last) = current.last_mut() else {
            break;
        };
        let Some(ch) = last.text.chars().last() else {
            current.pop();
            continue;
        };
        if !ch.is_whitespace() || ch == '\n' {
            break;
        }

        last.text.pop();
        *column = column.saturating_sub(display_width_char(ch));
        if last.text.is_empty() {
            current.pop();
        }
    }
}

fn display_width_char(_ch: char) -> usize {
    1
}

fn reserved_width(width: usize, right_padding: usize) -> usize {
    width.saturating_sub(right_padding).max(1)
}

fn padding_prefix(width: usize) -> Vec<DetailSegment> {
    if width == 0 {
        Vec::new()
    } else {
        vec![DetailSegment::raw(" ".repeat(width))]
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

    builder.push_meta_line(vec![
        ("repo", vec![DetailSegment::raw(item.repo.clone())]),
        (
            "number",
            vec![DetailSegment::raw(
                item.number
                    .map(|number| format!("#{number}"))
                    .unwrap_or_else(|| "-".to_string()),
            )],
        ),
        (
            "state",
            vec![DetailSegment::raw(
                item.state.clone().unwrap_or_else(|| "-".to_string()),
            )],
        ),
        (
            "updated",
            vec![DetailSegment::raw(relative_time(item.updated_at))],
        ),
    ]);

    let mut secondary_meta = Vec::new();
    let mut action_note = None;
    if let Some(author) = useful_meta_value(item.author.as_deref()) {
        secondary_meta.push((
            "author",
            vec![DetailSegment::link(
                author.to_string(),
                github_profile_url(author),
            )],
        ));
    }
    if let Some(comments) = item.comments {
        secondary_meta.push(("comments", vec![DetailSegment::raw(comments.to_string())]));
    }
    if let Some(reason) = useful_meta_value(item.reason.as_deref()) {
        secondary_meta.push(("reason", vec![DetailSegment::raw(reason.to_string())]));
    }
    if matches!(item.kind, ItemKind::PullRequest) {
        let (action_text, note) = action_hint_text(app.action_hints.get(&item.id));
        secondary_meta.push(("action", vec![DetailSegment::raw(action_text)]));
        secondary_meta.push((
            "checks",
            check_hint_segments(app.action_hints.get(&item.id)),
        ));
        action_note = note;
    }
    if !secondary_meta.is_empty() {
        builder.push_meta_line(secondary_meta);
    }
    if let Some(note) = action_note {
        builder.push_key_value("action note", note);
    }
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
                    if index > 0 {
                        builder.push_blank();
                    }
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

fn useful_meta_value(value: Option<&str>) -> Option<&str> {
    let value = value?.trim();
    if value.is_empty() || value == "-" {
        None
    } else {
        Some(value)
    }
}

fn github_profile_url(author: &str) -> String {
    format!("https://github.com/{author}")
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
    builder.push_indented_wrapped_limited(header, COMMENT_LEFT_PADDING, COMMENT_RIGHT_PADDING, 2);
    builder.push_markdown_block_indented(
        &comment.body,
        "No comment body.",
        usize::MAX,
        usize::MAX,
        COMMENT_LEFT_PADDING,
        COMMENT_RIGHT_PADDING,
    );
    builder.document.comments.push(CommentRegion {
        index,
        start_line,
        end_line: builder.document.lines.len(),
    });
}

fn push_comment_separator(builder: &mut DetailsBuilder, selected: bool) {
    let width = builder
        .width
        .saturating_sub(COMMENT_LEFT_PADDING + COMMENT_RIGHT_PADDING)
        .max(12);
    builder.push_line(vec![
        DetailSegment::styled(" ".repeat(COMMENT_LEFT_PADDING), Style::default()),
        DetailSegment::styled("─".repeat(width.min(72)), comment_separator_style(selected)),
    ]);
}

fn action_hint_text(state: Option<&ActionHintState>) -> (String, Option<String>) {
    match state {
        Some(ActionHintState::Loaded(hints)) => {
            let text = if hints.labels.is_empty() {
                "-".to_string()
            } else {
                hints.labels.join(", ")
            };
            (text, hints.note.clone())
        }
        Some(ActionHintState::Loading) | None => ("loading...".to_string(), None),
        Some(ActionHintState::Error(error)) => (
            "unavailable".to_string(),
            Some(format!("Failed to load action hints: {error}")),
        ),
    }
}

fn check_hint_segments(state: Option<&ActionHintState>) -> Vec<DetailSegment> {
    match state {
        Some(ActionHintState::Loaded(hints)) => hints
            .checks
            .as_ref()
            .map(check_summary_segments)
            .unwrap_or_else(|| vec![DetailSegment::raw("-")]),
        Some(ActionHintState::Loading) | None => vec![DetailSegment::raw("loading...")],
        Some(ActionHintState::Error(_)) => vec![DetailSegment::raw("unavailable")],
    }
}

fn check_summary_segments(summary: &CheckSummary) -> Vec<DetailSegment> {
    let mut segments = Vec::new();
    push_check_part(
        &mut segments,
        format!("{} pass", summary.passed),
        Style::default().fg(Color::LightGreen),
    );
    push_check_part(
        &mut segments,
        format!("{} fail", summary.failed),
        if summary.failed > 0 {
            Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        },
    );
    if summary.pending > 0 {
        push_check_part(
            &mut segments,
            format!("{} pending", summary.pending),
            Style::default().fg(Color::Yellow),
        );
    }
    if summary.skipped > 0 {
        push_check_part(
            &mut segments,
            format!("{} skipped", summary.skipped),
            Style::default().fg(Color::DarkGray),
        );
    }
    if summary.incomplete {
        push_check_part(
            &mut segments,
            format!("{} total", summary.total),
            Style::default().fg(Color::Gray),
        );
    }
    segments
}

fn push_check_part(segments: &mut Vec<DetailSegment>, text: String, style: Style) {
    if !segments.is_empty() {
        segments.push(DetailSegment::raw(", "));
    }
    segments.push(DetailSegment::styled(text, style));
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

    if is_github_search_rate_limit(first_error) {
        return format!(
            "GitHub search rate limited; using cached data ({count} failed section(s))"
        );
    }

    format!("refresh complete with {count} failed section(s)")
}

fn compact_error_label(error: &str) -> String {
    if error.contains("GitHub CLI `gh` is required") {
        return "GitHub CLI missing".to_string();
    }

    if error.contains("Run `gh auth login`") {
        return "GitHub CLI auth required".to_string();
    }

    if is_github_search_rate_limit(error) {
        return "GitHub search rate limited".to_string();
    }

    let summary = error
        .split_once(" failed: ")
        .map(|(_, message)| message)
        .unwrap_or(error)
        .trim();
    truncate_inline(summary, 80)
}

fn is_github_search_rate_limit(error: &str) -> bool {
    error
        .to_ascii_lowercase()
        .contains("api rate limit exceeded")
}

fn truncate_inline(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    if max_chars <= 3 {
        return "...".chars().take(max_chars).collect();
    }

    let mut truncated = text
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    truncated.push_str("...");
    truncated
}

fn is_comment_submit_key(key: KeyEvent) -> bool {
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return false;
    }

    matches!(key.code, KeyCode::Enter | KeyCode::Char('\n'))
}

fn is_ctrl_c_key(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char(value) if value.eq_ignore_ascii_case(&'c'))
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

fn pr_action_error_title(action: PrAction) -> &'static str {
    match action {
        PrAction::Merge => "Merge Failed",
        PrAction::Close => "Close Failed",
        PrAction::Approve => "Approve Failed",
    }
}

fn pr_action_error_status(action: PrAction) -> &'static str {
    match action {
        PrAction::Merge => "pull request merge failed",
        PrAction::Close => "pull request close failed",
        PrAction::Approve => "pull request approval failed",
    }
}

fn pr_action_error_body(error: &str) -> String {
    operation_error_body(error)
}

fn operation_error_body(error: &str) -> String {
    let message = error
        .split_once(" failed: ")
        .map(|(_, message)| message)
        .unwrap_or(error)
        .trim();
    truncate_inline(message, 900)
}

fn comment_pending_dialog(mode: PendingCommentMode) -> MessageDialog {
    match mode {
        PendingCommentMode::Post => MessageDialog {
            title: "Posting Comment".to_string(),
            body: "Waiting for GitHub to accept the comment...".to_string(),
        },
        PendingCommentMode::Edit { .. } => MessageDialog {
            title: "Updating Comment".to_string(),
            body: "Waiting for GitHub to accept the update...".to_string(),
        },
    }
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
        let default_view = builtin_view_key(active_view);
        let active_view = if ui_state.active_view.trim().is_empty() {
            default_view
        } else {
            ui_state.active_view.clone()
        };
        let focus = FocusTarget::from_state_str(&ui_state.focus);
        let details_scroll = ui_state.details_scroll;
        let selected_comment_index = ui_state.selected_comment_index;
        let mut state = Self {
            active_view,
            sections,
            section_index: ui_state.section_index.clone(),
            selected_index: ui_state.selected_index.clone(),
            focus: FocusTarget::List,
            details_scroll,
            list_width_percent: ui_state.list_width_percent,
            dragging_split: false,
            split_drag_changed: false,
            search_active: false,
            search_query: String::new(),
            global_search_active: false,
            global_search_query: String::new(),
            global_search_running: false,
            status: "loading snapshot; background refresh started".to_string(),
            refreshing: false,
            last_refresh_request: Instant::now(),
            details: HashMap::new(),
            action_hints: HashMap::new(),
            details_stale: HashSet::new(),
            selected_comment_index: 0,
            comment_dialog: None,
            posting_comment: false,
            pr_action_dialog: None,
            pr_action_running: false,
            setup_dialog: None,
            message_dialog: None,
            mouse_capture_enabled: true,
            help_dialog: false,
        };
        state.clamp_positions();
        state.focus = if matches!(focus, FocusTarget::Details) && state.current_item().is_none() {
            FocusTarget::List
        } else {
            focus
        };
        state.details_scroll = details_scroll;
        state.selected_comment_index = selected_comment_index;
        state
    }

    fn ui_state(&self) -> UiState {
        UiState {
            list_width_percent: self.list_width_percent,
            active_view: self.active_view.clone(),
            section_index: self.section_index.clone(),
            selected_index: self.selected_index.clone(),
            focus: self.focus.as_state_str().to_string(),
            details_scroll: self.details_scroll,
            selected_comment_index: self.selected_comment_index,
        }
    }

    fn current_refresh_anchor(&self) -> RefreshAnchor {
        RefreshAnchor {
            active_view: self.active_view.clone(),
            section_key: self.current_section().map(|section| section.key.clone()),
            item_id: self.current_item().map(|item| item.id.clone()),
        }
    }

    fn restore_refresh_anchor(&mut self, anchor: &RefreshAnchor) -> bool {
        if let Some(view) = self.canonical_view_key(&anchor.active_view) {
            self.active_view = view;
        }

        self.clamp_positions();

        if same_view_key(&self.active_view, &anchor.active_view)
            && let Some(section_key) = &anchor.section_key
            && let Some(position) = self.section_position_by_key(&anchor.active_view, section_key)
        {
            self.set_current_section_position(position);
        }

        let restored = anchor.item_id.as_deref().is_some_and(|item_id| {
            self.select_current_item_by_id(item_id)
                || self.select_item_in_view(&anchor.active_view, item_id)
        });

        self.clamp_positions();
        if matches!(self.focus, FocusTarget::Details) && self.current_item().is_none() {
            self.focus = FocusTarget::List;
        }
        restored
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
                let anchor = self.current_refresh_anchor();
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
                let restored_item = self.restore_refresh_anchor(&anchor);
                if restored_item {
                    self.details_scroll = previous_details_scroll;
                    self.selected_comment_index = previous_comment_index;
                    if let Some(item_id) = anchor.item_id {
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
            AppMsg::DetailsLoaded {
                item_id,
                comments,
                actions,
            } => {
                match comments {
                    Ok(comments) => {
                        self.details_stale.remove(&item_id);
                        self.details
                            .insert(item_id.clone(), DetailState::Loaded(comments));
                        self.clamp_selected_comment();
                    }
                    Err(error) => {
                        if self.setup_dialog.is_none() {
                            self.setup_dialog = setup_dialog_from_error(&error);
                        }
                        self.details_stale.remove(&item_id);
                        self.details
                            .insert(item_id.clone(), DetailState::Error(error));
                    }
                }

                if let Some(actions) = actions {
                    match actions {
                        Ok(actions) => {
                            self.action_hints
                                .insert(item_id, ActionHintState::Loaded(actions));
                        }
                        Err(error) => {
                            self.action_hints
                                .insert(item_id, ActionHintState::Error(error));
                        }
                    }
                }
            }
            AppMsg::CommentPosted { item_id, result } => match result {
                Ok(comments) => {
                    self.selected_comment_index = comments.len().saturating_sub(1);
                    self.details_stale.remove(&item_id);
                    self.details.insert(item_id, DetailState::Loaded(comments));
                    self.clamp_selected_comment();
                    self.posting_comment = false;
                    self.status = "comment posted".to_string();
                    self.message_dialog = Some(MessageDialog {
                        title: "Comment Posted".to_string(),
                        body: "GitHub accepted the comment and comments were refreshed."
                            .to_string(),
                    });
                }
                Err(error) => {
                    let setup_dialog = setup_dialog_from_error(&error);
                    if self.setup_dialog.is_none() {
                        self.setup_dialog = setup_dialog;
                    }
                    if setup_dialog.is_none() {
                        self.message_dialog = Some(MessageDialog {
                            title: "Comment Failed".to_string(),
                            body: operation_error_body(&error),
                        });
                    }
                    self.posting_comment = false;
                    self.status = "comment post failed".to_string();
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
                    self.message_dialog = Some(MessageDialog {
                        title: "Comment Updated".to_string(),
                        body: "GitHub accepted the update and comments were refreshed.".to_string(),
                    });
                }
                Err(error) => {
                    let setup_dialog = setup_dialog_from_error(&error);
                    if self.setup_dialog.is_none() {
                        self.setup_dialog = setup_dialog;
                    }
                    if setup_dialog.is_none() {
                        self.message_dialog = Some(MessageDialog {
                            title: "Update Failed".to_string(),
                            body: operation_error_body(&error),
                        });
                    }
                    self.posting_comment = false;
                    self.status = "comment update failed".to_string();
                }
            },
            AppMsg::PrActionFinished {
                item_id,
                action,
                result,
            } => {
                self.pr_action_running = false;
                self.pr_action_dialog = None;
                match result {
                    Ok(()) => {
                        self.details_stale.insert(item_id.clone());
                        self.mark_item_after_pr_action(&item_id, action);
                        self.status = match action {
                            PrAction::Merge => "pull request merged; refreshing".to_string(),
                            PrAction::Close => "pull request closed; refreshing".to_string(),
                            PrAction::Approve => "pull request approved; refreshing".to_string(),
                        };
                    }
                    Err(error) => {
                        let setup_dialog = setup_dialog_from_error(&error);
                        if self.setup_dialog.is_none() {
                            self.setup_dialog = setup_dialog;
                        }
                        if setup_dialog.is_none() {
                            self.message_dialog = Some(MessageDialog {
                                title: pr_action_error_title(action).to_string(),
                                body: pr_action_error_body(&error),
                            });
                        }
                        self.status = pr_action_error_status(action).to_string();
                    }
                }
            }
            AppMsg::SectionPageLoaded {
                section_key,
                section,
                save_error,
            } => {
                let error = section.error.clone();
                let loaded_page_label = section_page_label(&section);
                if self.setup_dialog.is_none() {
                    self.setup_dialog = error.as_deref().and_then(setup_dialog_from_error);
                }
                self.replace_section_page(&section_key, section);
                self.refreshing = false;
                self.status = match (error.as_deref(), save_error) {
                    (None, None) => loaded_page_label
                        .map(|label| format!("loaded page {label}"))
                        .unwrap_or_else(|| "loaded result page".to_string()),
                    (Some(error), None) => refresh_error_status(1, Some(error)),
                    (_, Some(error)) => format!("snapshot save failed: {error}"),
                };
            }
            AppMsg::GlobalSearchFinished { query, sections } => {
                let errors = sections
                    .iter()
                    .filter(|section| section.error.is_some())
                    .count();
                let first_error = sections
                    .iter()
                    .find_map(|section| section.error.as_deref())
                    .map(str::to_string);
                if self.setup_dialog.is_none() {
                    self.setup_dialog = first_error.as_deref().and_then(setup_dialog_from_error);
                }

                let result_count = sections
                    .iter()
                    .map(|section| section.items.len())
                    .sum::<usize>();
                self.replace_global_search_sections(sections);
                self.global_search_running = false;
                self.global_search_active = false;
                self.global_search_query = query.clone();
                self.search_active = false;
                self.search_query.clear();
                self.active_view = global_search_view_key();
                self.focus = FocusTarget::List;
                self.details_scroll = 0;
                self.selected_comment_index = 0;
                self.comment_dialog = None;
                self.pr_action_dialog = None;
                self.clamp_positions();
                self.status = if errors == 0 {
                    format!("search complete: {result_count} result(s) for '{query}'")
                } else {
                    refresh_error_status(errors, first_error.as_deref())
                };
            }
        }
    }

    fn dismiss_setup_dialog(&mut self) {
        self.setup_dialog = None;
        self.status = "setup hint dismissed; cached data still available".to_string();
    }

    fn dismiss_message_dialog(&mut self) {
        self.message_dialog = None;
        self.status = "message dismissed".to_string();
    }

    fn toggle_mouse_capture(&mut self) {
        self.mouse_capture_enabled = !self.mouse_capture_enabled;
        self.status = if self.mouse_capture_enabled {
            "mouse controls enabled".to_string()
        } else {
            if self.current_item().is_some() {
                self.focus = FocusTarget::Details;
            }
            "text selection mode: drag terminal text; press m to restore mouse controls".to_string()
        };
    }

    fn show_help_dialog(&mut self) {
        self.help_dialog = true;
        self.search_active = false;
        self.global_search_active = false;
        self.status = "help".to_string();
    }

    fn dismiss_help_dialog(&mut self) {
        self.help_dialog = false;
        self.status = "help dismissed".to_string();
    }

    fn mark_item_after_pr_action(&mut self, item_id: &str, action: PrAction) {
        for section in &mut self.sections {
            for item in &mut section.items {
                if item.id != item_id {
                    continue;
                }
                match action {
                    PrAction::Merge => item.state = Some("merged".to_string()),
                    PrAction::Close => item.state = Some("closed".to_string()),
                    PrAction::Approve => {}
                }
            }
        }
    }

    fn replace_section_page(&mut self, section_key: &str, refreshed: SectionSnapshot) {
        let was_current = self
            .current_section()
            .is_some_and(|section| section.key == section_key);
        let Some(index) = self
            .sections
            .iter()
            .position(|section| section.key == section_key)
        else {
            return;
        };

        if refreshed.error.is_none() {
            self.sections[index] = refreshed;
            if was_current {
                self.set_current_selected_position(0);
                self.details_scroll = 0;
                self.selected_comment_index = 0;
                self.comment_dialog = None;
                self.pr_action_dialog = None;
            }
        } else {
            self.sections[index].error = refreshed.error;
        }
        self.clamp_positions();
    }

    fn section_page_request(
        &self,
        delta: isize,
        config: &Config,
    ) -> std::result::Result<SectionPageRequest, String> {
        let Some(section) = self.current_section() else {
            return Err("no section selected".to_string());
        };
        if !matches!(
            section.kind,
            SectionKind::PullRequests | SectionKind::Issues
        ) {
            return Err("notifications do not support result pagination".to_string());
        }
        if section.filters.contains(" | ") {
            return Err("combined sections cannot be paged yet".to_string());
        }
        let Some(total_count) = section.total_count else {
            return Err("total count unavailable; refresh first".to_string());
        };
        let page_size = section_page_size(section, config);
        let (total_pages, total_is_capped) = section_total_pages(total_count, page_size);
        if total_pages <= 1 {
            return Err("only one result page".to_string());
        }
        let current_page = section.page.max(1).min(total_pages);
        let next_page = if delta < 0 {
            current_page.saturating_sub(1).max(1)
        } else {
            current_page.saturating_add(1).min(total_pages)
        };
        if next_page == current_page {
            let edge = if delta < 0 { "first" } else { "last" };
            return Err(format!("already at {edge} result page"));
        }

        Ok(SectionPageRequest {
            section_key: section.key.clone(),
            view: section_view_key(section),
            kind: section.kind,
            title: section.title.clone(),
            filters: section.filters.clone(),
            page: next_page,
            page_size,
            total_pages,
            total_is_capped,
        })
    }

    fn replace_global_search_sections(&mut self, sections: Vec<SectionSnapshot>) {
        let search_view = global_search_view_key();
        self.sections
            .retain(|section| section_view_key(section) != search_view);
        self.sections.extend(sections);
    }

    fn ensure_current_details_loading(&mut self, tx: &UnboundedSender<AppMsg>) {
        let Some(item) = self.current_item().cloned() else {
            return;
        };
        if !matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest) || item.number.is_none() {
            return;
        }
        if !self.should_start_details_load(&item) {
            return;
        }

        if !self.details.contains_key(&item.id) {
            self.details.insert(item.id.clone(), DetailState::Loading);
        }
        if matches!(item.kind, ItemKind::PullRequest) && !self.action_hints.contains_key(&item.id) {
            self.action_hints
                .insert(item.id.clone(), ActionHintState::Loading);
        }
        start_details_load(item, tx.clone());
    }

    fn should_start_details_load(&mut self, item: &WorkItem) -> bool {
        let should_refresh = self.details_stale.remove(&item.id);
        !self.details.contains_key(&item.id)
            || should_refresh
            || (matches!(item.kind, ItemKind::PullRequest)
                && !self.action_hints.contains_key(&item.id))
    }

    fn switch_view(&mut self, view: impl Into<String>) {
        let view = view.into();
        let focus = self.focus;
        self.active_view = view;
        self.details_scroll = 0;
        self.selected_comment_index = 0;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.global_search_active = false;
        self.clamp_positions();
        self.focus = if matches!(focus, FocusTarget::Details) && self.current_item().is_none() {
            FocusTarget::List
        } else {
            focus
        };
    }

    fn move_view(&mut self, delta: isize) {
        let views = self.view_tabs();
        if views.is_empty() {
            return;
        }
        let current = views
            .iter()
            .position(|view| view.key == self.active_view)
            .unwrap_or(0);
        let next = move_wrapping(current, views.len(), delta);
        if let Some(view) = views.get(next) {
            self.switch_view(view.key.clone());
        }
    }

    fn focus_primary_list(&mut self) {
        self.focus = FocusTarget::List;
        self.details_scroll = 0;
        self.selected_comment_index = 0;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.search_active = false;
        self.global_search_active = false;
        self.status = "list focused".to_string();
        self.clamp_positions();
    }

    fn focus_ghr(&mut self) {
        self.focus = FocusTarget::Ghr;
        self.search_active = false;
        self.global_search_active = false;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.status = "ghr focused".to_string();
    }

    fn focus_sections(&mut self) {
        self.focus = FocusTarget::Sections;
        self.search_active = false;
        self.global_search_active = false;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.status = "Sections focused".to_string();
        self.clamp_positions();
    }

    fn move_section(&mut self, delta: isize) {
        let len = self.visible_sections().len();
        if len == 0 {
            return;
        }
        let current = self.current_section_position().min(len - 1);
        let next = move_wrapping(current, len, delta);
        self.set_current_section_position(next);
        self.set_current_selected_position(0);
        self.focus = FocusTarget::Sections;
        self.details_scroll = 0;
        self.selected_comment_index = 0;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.global_search_active = false;
    }

    fn select_section(&mut self, index: usize) {
        let len = self.visible_sections().len();
        if len == 0 {
            return;
        }
        self.set_current_section_position(index.min(len - 1));
        self.set_current_selected_position(0);
        self.focus = FocusTarget::Sections;
        self.details_scroll = 0;
        self.selected_comment_index = 0;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.search_active = false;
        self.global_search_active = false;
        self.status = "Sections focused".to_string();
    }

    fn move_selection(&mut self, delta: isize) {
        let Some(section) = self.current_section() else {
            return;
        };
        let len = self.filtered_indices(section).len();
        if len == 0 {
            return;
        }
        let current = self.current_selected_position().min(len - 1);
        self.set_current_selected_position(move_bounded(current, len, delta));
        self.details_scroll = 0;
        self.selected_comment_index = 0;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.global_search_active = false;
    }

    fn set_selection(&mut self, index: usize) {
        self.set_current_selected_position(index);
        self.details_scroll = 0;
        self.selected_comment_index = 0;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.global_search_active = false;
        self.clamp_positions();
    }

    fn select_last(&mut self) {
        let Some(section) = self.current_section() else {
            return;
        };
        let len = self.filtered_indices(section).len();
        if len > 0 {
            self.set_current_selected_position(len - 1);
            self.details_scroll = 0;
            self.selected_comment_index = 0;
            self.comment_dialog = None;
            self.pr_action_dialog = None;
            self.global_search_active = false;
        }
    }

    fn focus_details(&mut self) {
        if self.current_item().is_some() {
            self.focus = FocusTarget::Details;
            self.search_active = false;
            self.global_search_active = false;
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

    fn start_pr_action_dialog(&mut self, action: PrAction) {
        let Some(item) = self.current_item().cloned() else {
            self.status = "nothing selected".to_string();
            return;
        };
        if item.kind != ItemKind::PullRequest || item.number.is_none() {
            self.status = "selected item is not a pull request".to_string();
            return;
        }
        self.search_active = false;
        self.global_search_active = false;
        self.pr_action_dialog = Some(PrActionDialog { item, action });
        self.pr_action_running = false;
        self.status = match action {
            PrAction::Merge => "confirm pull request merge".to_string(),
            PrAction::Close => "confirm pull request close".to_string(),
            PrAction::Approve => "confirm pull request approval".to_string(),
        };
    }

    fn handle_pr_action_dialog_key(
        &mut self,
        key: KeyEvent,
        config: &Config,
        store: &SnapshotStore,
        tx: &UnboundedSender<AppMsg>,
    ) {
        self.handle_pr_action_dialog_key_with_submit(key, |item, action| {
            start_pr_action(item, action, config.clone(), store.clone(), tx.clone());
        });
    }

    fn handle_pr_action_dialog_key_with_submit<F>(&mut self, key: KeyEvent, mut submit: F)
    where
        F: FnMut(WorkItem, PrAction),
    {
        if self.pr_action_running {
            self.status = "pull request action already running".to_string();
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.pr_action_dialog = None;
                self.status = "pull request action cancelled".to_string();
            }
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                if let Some(action) = self.pr_action_dialog.as_ref().map(|dialog| dialog.action) {
                    self.submit_pr_action(action, &mut submit);
                }
            }
            _ => {}
        }
    }

    fn submit_pr_action<F>(&mut self, action: PrAction, submit: &mut F)
    where
        F: FnMut(WorkItem, PrAction),
    {
        let Some(dialog) = &self.pr_action_dialog else {
            return;
        };
        let item = dialog.item.clone();
        self.pr_action_running = true;
        self.status = match action {
            PrAction::Merge => "merging pull request".to_string(),
            PrAction::Close => "closing pull request".to_string(),
            PrAction::Approve => "approving pull request".to_string(),
        };
        submit(item, action);
    }

    fn start_new_comment_dialog(&mut self) {
        if !self.current_item_supports_comments() {
            self.status = "selected item cannot be commented on".to_string();
            return;
        }
        self.focus = FocusTarget::Details;
        self.search_active = false;
        self.global_search_active = false;
        self.pr_action_dialog = None;
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
        self.global_search_active = false;
        self.pr_action_dialog = None;
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
        self.global_search_active = false;
        self.pr_action_dialog = None;
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
        let dialog = self.comment_dialog.take()?;
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
        self.message_dialog = Some(comment_pending_dialog(mode));
        self.status = match mode {
            PendingCommentMode::Post => "posting comment".to_string(),
            PendingCommentMode::Edit { .. } => "updating comment".to_string(),
        };
        Some(PendingCommentSubmit { item, body, mode })
    }

    fn start_global_search_input(&mut self) {
        self.focus = FocusTarget::List;
        self.global_search_active = true;
        self.global_search_query.clear();
        self.search_active = false;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.status = match self.current_repo_scope() {
            Some(repo) => format!("repo search mode in {repo}"),
            None => "search mode".to_string(),
        };
    }

    fn handle_global_search_key(
        &mut self,
        key: KeyEvent,
        config: &Config,
        tx: &UnboundedSender<AppMsg>,
    ) {
        let repo_scope = self.current_repo_scope();
        self.handle_global_search_key_with_submit(key, |query| {
            start_global_search(query, repo_scope.clone(), config.clone(), tx.clone());
        });
    }

    fn handle_global_search_key_with_submit<F>(&mut self, key: KeyEvent, mut submit: F)
    where
        F: FnMut(String),
    {
        match key.code {
            KeyCode::Esc => {
                self.global_search_active = false;
                self.status = "search cancelled".to_string();
            }
            KeyCode::Enter => {
                let query = self.global_search_query.trim().to_string();
                if query.is_empty() {
                    self.status = "search query is empty".to_string();
                    return;
                }
                if self.global_search_running {
                    self.status = "search already running".to_string();
                    return;
                }
                self.global_search_active = false;
                self.global_search_running = true;
                self.search_active = false;
                self.search_query.clear();
                self.status = match self.current_repo_scope() {
                    Some(repo) => format!("searching {repo} for '{query}'"),
                    None => format!("searching GitHub for '{query}'"),
                };
                submit(query);
            }
            KeyCode::Backspace => {
                self.global_search_query.pop();
            }
            KeyCode::Char(value) => {
                self.global_search_query.push(value);
            }
            _ => {}
        }
    }

    fn start_search(&mut self) {
        self.focus = FocusTarget::List;
        self.search_active = true;
        self.global_search_active = false;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.status = "search mode".to_string();
        self.clamp_positions();
    }

    fn clear_search(&mut self) {
        self.search_active = false;
        self.global_search_active = false;
        self.search_query.clear();
        self.focus = FocusTarget::List;
        self.details_scroll = 0;
        self.status = "search cleared".to_string();
        self.clamp_positions();
    }

    fn push_search_char(&mut self, value: char) {
        self.search_query.push(value);
        self.set_current_selected_position(0);
        self.details_scroll = 0;
        self.clamp_positions();
    }

    fn pop_search_char(&mut self) {
        self.search_query.pop();
        self.set_current_selected_position(0);
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
            .filter(|section| section_view_key(section) == self.active_view)
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

    fn current_repo_scope(&self) -> Option<String> {
        self.current_item()
            .map(|item| item.repo.clone())
            .or_else(|| self.current_section().and_then(section_repo_scope))
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
        self.section_position(&self.active_view)
    }

    fn current_selected_position(&self) -> usize {
        self.selected_position(&self.active_view)
    }

    fn section_position(&self, view: &str) -> usize {
        self.section_index.get(view).copied().unwrap_or(0)
    }

    fn selected_position(&self, view: &str) -> usize {
        self.selected_index.get(view).copied().unwrap_or(0)
    }

    fn set_current_section_position(&mut self, position: usize) {
        self.section_index
            .insert(self.active_view.clone(), position);
    }

    fn set_current_selected_position(&mut self, position: usize) {
        self.selected_index
            .insert(self.active_view.clone(), position);
    }

    fn view_exists(&self, view: &str) -> bool {
        self.canonical_view_key(view).is_some()
    }

    fn canonical_view_key(&self, view: &str) -> Option<String> {
        self.view_tabs()
            .into_iter()
            .find(|tab| same_view_key(&tab.key, view))
            .map(|tab| tab.key)
    }

    fn section_position_by_key(&self, view: &str, section_key: &str) -> Option<usize> {
        self.sections
            .iter()
            .filter(|section| same_view_key(&section_view_key(section), view))
            .position(|section| section.key == section_key)
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

        self.set_current_selected_position(position);
        true
    }

    fn select_item_in_view(&mut self, view: &str, item_id: &str) -> bool {
        if !same_view_key(&self.active_view, view) || !self.view_exists(view) {
            return false;
        }

        let Some((section_position, selected_position)) = self.find_item_in_view(view, item_id)
        else {
            return false;
        };

        self.set_current_section_position(section_position);
        self.set_current_selected_position(selected_position);
        true
    }

    fn find_item_in_view(&self, view: &str, item_id: &str) -> Option<(usize, usize)> {
        self.sections
            .iter()
            .filter(|section| same_view_key(&section_view_key(section), view))
            .enumerate()
            .find_map(|(section_position, section)| {
                self.filtered_indices(section)
                    .into_iter()
                    .enumerate()
                    .find_map(|(selected_position, item_index)| {
                        section
                            .items
                            .get(item_index)
                            .is_some_and(|item| item.id == item_id)
                            .then_some((section_position, selected_position))
                    })
            })
    }

    fn clamp_positions(&mut self) {
        let views = self.view_tabs();
        if !views.iter().any(|view| view.key == self.active_view) {
            if let Some(view) = views
                .iter()
                .find(|view| same_view_key(&view.key, &self.active_view))
            {
                self.active_view = view.key.clone();
            } else if let Some(view) = views.first() {
                self.active_view = view.key.clone();
            }
        }

        for view in views {
            let section_count = self
                .sections
                .iter()
                .filter(|section| section_view_key(section) == view.key)
                .count();
            if section_count == 0 {
                self.section_index.insert(view.key.clone(), 0);
                self.selected_index.insert(view.key, 0);
                continue;
            }

            let section_index = self.section_position(&view.key).min(section_count - 1);
            self.section_index.insert(view.key.clone(), section_index);
            let item_count = self
                .sections
                .iter()
                .filter(|section| section_view_key(section) == view.key)
                .nth(section_index)
                .map(|section| self.filtered_indices(section).len())
                .unwrap_or(0);
            let selected_index = if item_count == 0 {
                0
            } else {
                self.selected_position(&view.key).min(item_count - 1)
            };
            self.selected_index.insert(view.key, selected_index);
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

    fn view_tabs(&self) -> Vec<ViewTab> {
        let mut tabs = Vec::new();
        for kind in [
            SectionKind::PullRequests,
            SectionKind::Issues,
            SectionKind::Notifications,
        ] {
            let key = builtin_view_key(kind);
            if self
                .sections
                .iter()
                .any(|section| section_view_key(section) == key)
            {
                tabs.push(ViewTab {
                    key,
                    label: kind.label().to_string(),
                });
            }
        }

        let search_key = global_search_view_key();
        if self
            .sections
            .iter()
            .any(|section| section_view_key(section) == search_key)
        {
            tabs.push(ViewTab {
                key: search_key,
                label: "Search".to_string(),
            });
        }

        for section in &self.sections {
            let key = section_view_key(section);
            if !key.starts_with("repo:") || tabs.iter().any(|view| view.key == key) {
                continue;
            }
            let label = key
                .strip_prefix("repo:")
                .unwrap_or(key.as_str())
                .to_string();
            tabs.push(ViewTab { key, label });
        }

        tabs
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

fn move_wrapping(current: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    (current as isize + delta).rem_euclid(len as isize) as usize
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
            refresh_error_status(3, Some("HTTP 403: API rate limit exceeded")),
            "GitHub search rate limited; using cached data (3 failed section(s))"
        );
    }

    #[test]
    fn compact_error_label_hides_long_gh_command_context() {
        let error = "gh search prs --json number,title,body,repository,author,updatedAt,url,state,isDraft,labels,commentsCount --limit 500 -- repo:rust-lang/rust is:open failed: HTTP 403: API rate limit exceeded for user ID 230646";

        assert_eq!(compact_error_label(error), "GitHub search rate limited");
        assert!(!compact_error_label(error).contains("--json"));
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
    fn ui_state_restores_view_selection_focus_and_scroll() {
        let mut first_issue = work_item("issue-1", "nervosnetwork/fiber", 1, "First issue", None);
        first_issue.kind = ItemKind::Issue;
        first_issue.url = "https://github.com/nervosnetwork/fiber/issues/1".to_string();
        let mut second_issue = work_item("issue-2", "nervosnetwork/fiber", 2, "Second issue", None);
        second_issue.kind = ItemKind::Issue;
        second_issue.url = "https://github.com/nervosnetwork/fiber/issues/2".to_string();
        let sections = vec![
            test_section(),
            SectionSnapshot {
                key: "issues:test".to_string(),
                kind: SectionKind::Issues,
                title: "Issues".to_string(),
                filters: String::new(),
                items: vec![first_issue, second_issue],
                total_count: None,
                page: 1,
                page_size: 0,
                refreshed_at: None,
                error: None,
            },
        ];
        let state = UiState {
            list_width_percent: 64,
            active_view: builtin_view_key(SectionKind::Issues),
            section_index: HashMap::from([(builtin_view_key(SectionKind::Issues), 0)]),
            selected_index: HashMap::from([(builtin_view_key(SectionKind::Issues), 1)]),
            focus: "details".to_string(),
            details_scroll: 7,
            selected_comment_index: 2,
        };

        let app = AppState::with_ui_state(SectionKind::PullRequests, sections, state);
        let saved = app.ui_state();

        assert_eq!(app.active_view, builtin_view_key(SectionKind::Issues));
        assert_eq!(app.current_selected_position(), 1);
        assert_eq!(
            app.current_item().map(|item| item.id.as_str()),
            Some("issue-2")
        );
        assert_eq!(app.focus, FocusTarget::Details);
        assert_eq!(app.details_scroll, 7);
        assert_eq!(app.selected_comment_index, 2);
        assert_eq!(app.list_width_percent, 64);
        assert_eq!(saved.active_view, builtin_view_key(SectionKind::Issues));
        assert_eq!(saved.selected_index.get("issues"), Some(&1));
        assert_eq!(saved.focus, "details");
        assert_eq!(saved.details_scroll, 7);
    }

    #[test]
    fn ui_state_restores_repo_view_case_insensitively() {
        let sections = vec![
            test_section(),
            SectionSnapshot {
                key: "repo:Fiber:pull_requests:Pull Requests".to_string(),
                kind: SectionKind::PullRequests,
                title: "Pull Requests".to_string(),
                filters: String::new(),
                items: vec![work_item(
                    "fiber-1",
                    "nervosnetwork/fiber",
                    1,
                    "Fiber PR",
                    None,
                )],
                total_count: None,
                page: 1,
                page_size: 0,
                refreshed_at: None,
                error: None,
            },
        ];
        let state = UiState {
            active_view: "repo:fiber".to_string(),
            focus: "details".to_string(),
            selected_index: HashMap::from([("repo:fiber".to_string(), 0)]),
            ..UiState::default()
        };

        let app = AppState::with_ui_state(SectionKind::PullRequests, sections, state);

        assert_eq!(app.active_view, "repo:Fiber");
        assert_eq!(app.focus, FocusTarget::Details);
        assert_eq!(
            app.current_item().map(|item| item.id.as_str()),
            Some("fiber-1")
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
            total_count: None,
            page: 1,
            page_size: 0,
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
    fn refresh_preserves_repo_anchor_over_builtin_duplicate_item() {
        let target = work_item(
            "fiber-1294",
            "nervosnetwork/fiber",
            1294,
            "Cannot pay invoice",
            Some("alice"),
        );
        let repo_view = "repo:Fiber";
        let sections = vec![
            SectionSnapshot {
                key: "pull_requests:Assigned to Me".to_string(),
                kind: SectionKind::PullRequests,
                title: "Assigned to Me".to_string(),
                filters: String::new(),
                items: vec![target.clone()],
                total_count: None,
                page: 1,
                page_size: 0,
                refreshed_at: None,
                error: None,
            },
            SectionSnapshot {
                key: "repo:Fiber:pull_requests:Pull Requests".to_string(),
                kind: SectionKind::PullRequests,
                title: "Pull Requests".to_string(),
                filters: String::new(),
                items: vec![
                    work_item("fiber-1201", "nervosnetwork/fiber", 1201, "Old PR", None),
                    target.clone(),
                ],
                total_count: None,
                page: 1,
                page_size: 0,
                refreshed_at: None,
                error: None,
            },
            SectionSnapshot {
                key: "repo:Fiber:issues:Issues".to_string(),
                kind: SectionKind::Issues,
                title: "Issues".to_string(),
                filters: String::new(),
                items: Vec::new(),
                total_count: None,
                page: 1,
                page_size: 0,
                refreshed_at: None,
                error: None,
            },
        ];
        let mut app = AppState::new(SectionKind::PullRequests, sections);
        app.switch_view(repo_view);
        app.set_selection(1);
        app.focus_details();
        app.details_scroll = 12;
        app.selected_comment_index = 1;
        app.details.insert(
            "fiber-1294".to_string(),
            DetailState::Loaded(vec![
                comment("alice", "first", None),
                comment("bob", "second", None),
            ]),
        );

        app.handle_msg(AppMsg::RefreshFinished {
            sections: vec![
                SectionSnapshot {
                    key: "pull_requests:Assigned to Me".to_string(),
                    kind: SectionKind::PullRequests,
                    title: "Assigned to Me".to_string(),
                    filters: String::new(),
                    items: vec![target.clone()],
                    total_count: None,
                    page: 1,
                    page_size: 0,
                    refreshed_at: None,
                    error: None,
                },
                SectionSnapshot {
                    key: "repo:Fiber:pull_requests:Pull Requests".to_string(),
                    kind: SectionKind::PullRequests,
                    title: "Pull Requests".to_string(),
                    filters: String::new(),
                    items: vec![target],
                    total_count: None,
                    page: 1,
                    page_size: 0,
                    refreshed_at: None,
                    error: None,
                },
                SectionSnapshot {
                    key: "repo:Fiber:issues:Issues".to_string(),
                    kind: SectionKind::Issues,
                    title: "Issues".to_string(),
                    filters: String::new(),
                    items: Vec::new(),
                    total_count: None,
                    page: 1,
                    page_size: 0,
                    refreshed_at: None,
                    error: None,
                },
            ],
            save_error: None,
        });

        assert_eq!(app.active_view, repo_view);
        assert_eq!(
            app.current_section().map(|section| section.title.as_str()),
            Some("Pull Requests")
        );
        assert_eq!(
            app.current_item().map(|item| item.id.as_str()),
            Some("fiber-1294")
        );
        assert_eq!(app.focus, FocusTarget::Details);
        assert_eq!(app.current_selected_position(), 0);
        assert_eq!(app.details_scroll, 12);
        assert_eq!(app.selected_comment_index, 1);
        assert!(app.details_stale.contains("fiber-1294"));
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
            total_count: None,
            page: 1,
            page_size: 0,
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
        let item = app.current_item().cloned().expect("selected item");

        assert!(app.should_start_details_load(&item));

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
    fn ctrl_c_exits_even_from_input_modes() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        assert!(handle_key(
            &mut app,
            ctrl_key(KeyCode::Char('c')),
            &config,
            &store,
            &tx
        ));

        app.start_new_comment_dialog();
        assert!(handle_key(
            &mut app,
            ctrl_key(KeyCode::Char('c')),
            &config,
            &store,
            &tx
        ));
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
        assert!(text.contains("open PR merge confirmation"));
        assert!(text.contains("open PR close confirmation"));
        assert!(text.contains("run the confirmed PR action"));
        assert!(text.contains("search PRs and issues in the current repo"));
        assert!(text.contains("terminal text selection"));
    }

    #[test]
    fn m_toggles_mouse_text_selection_mode_outside_inputs() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        assert!(app.mouse_capture_enabled);
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('m')),
            &config,
            &store,
            &tx
        ));
        assert!(!app.mouse_capture_enabled);
        assert_eq!(app.focus, FocusTarget::Details);
        assert!(app.status.contains("text selection mode"));

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('m')),
            &config,
            &store,
            &tx
        ));
        assert!(app.mouse_capture_enabled);
        assert_eq!(app.status, "mouse controls enabled");
    }

    #[test]
    fn text_selection_mode_renders_details_without_side_by_side_list() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.focus_details();
        app.mouse_capture_enabled = false;

        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let paths = test_paths();

        terminal
            .draw(|frame| draw(frame, &app, &paths))
            .expect("draw");

        let rendered = buffer_lines(terminal.backend().buffer()).join("\n");

        assert!(rendered.contains("Details:"));
        assert!(rendered.contains("Compiler diagnostics"));
        assert!(rendered.contains("A body with useful context"));
        assert!(!rendered.contains("Funding state"));
        assert!(!rendered.contains("Updated"));
    }

    #[test]
    fn list_table_renders_updated_next_to_meta() {
        let mut section = test_section();
        section.items[0].updated_at = Some(Utc::now() - chrono::Duration::days(2));
        let app = AppState::new(SectionKind::PullRequests, vec![section]);
        let backend = ratatui::backend::TestBackend::new(180, 30);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let paths = test_paths();

        terminal
            .draw(|frame| draw(frame, &app, &paths))
            .expect("draw");

        let lines = buffer_lines(terminal.backend().buffer());
        let header = lines
            .iter()
            .find(|line| {
                line.contains("Repo")
                    && line.contains("Title")
                    && line.contains("Updated")
                    && line.contains("Meta")
            })
            .expect("list header");
        let repo_pos = header.find("Repo").expect("repo column");
        let title_pos = header.find("Title").expect("title column");
        let updated_pos = header.find("Updated").expect("updated column");
        let meta_pos = header.find("Meta").expect("meta column");
        assert!(repo_pos < title_pos);
        assert!(title_pos < updated_pos);
        assert!(updated_pos < meta_pos);

        let row = lines
            .iter()
            .find(|line| line.contains("rust-lang/rust") && line.contains("Compiler diagnostics"))
            .expect("list row");
        let title_pos = row.find("Compiler diagnostics").expect("title cell");
        let updated_pos = row.find("2d").expect("updated cell");
        let meta_pos = row.find("open 0c").expect("meta cell");
        assert!(title_pos < updated_pos);
        assert!(updated_pos < meta_pos);
    }

    #[test]
    fn footer_uses_contextual_list_shortcuts() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.focus_list();
        let paths = test_paths();
        let text = footer_line(&app, &paths).to_string();

        assert!(
            text.contains(
                "List items  j/k move  pg d/u page  [ ] results  g/G ends  enter Details"
            )
        );
        assert!(text.contains("/ filter"));
        assert!(text.contains("M/C/A pr action"));
        assert!(
            text.contains(
                "| 1-4 focus  ? help  S repo  r refresh  o open  m text-select  q quit |"
            )
        );
        assert!(text.contains("| focus List  refresh idle  state list focused"));
        assert!(!text.contains("1 ghr  2 Sections  3 list  4 Details"));
        assert!(!text.contains("n/p comment"));
    }

    #[test]
    fn footer_switches_shortcuts_for_each_focus_region() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let paths = test_paths();

        app.focus_ghr();
        let ghr = footer_line(&app, &paths).to_string();
        assert!(ghr.contains("ghr tabs  h/l switch  j/enter Sections  esc List"));
        assert!(!ghr.contains("M/C/A pr action"));

        app.focus_sections();
        let sections = footer_line(&app, &paths).to_string();
        assert!(sections.contains("Sections tabs  h/l switch  k ghr  j/enter List"));
        assert!(!sections.contains("a comment"));

        app.focus_details();
        let details = footer_line(&app, &paths).to_string();
        assert!(details.contains("Details content  j/k scroll"));
        assert!(details.contains("n/p comment  a comment  R reply  e edit"));
        assert!(details.contains("esc List"));
        assert!(!details.contains("g/G ends"));
    }

    #[test]
    fn footer_context_label_uses_active_color() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let paths = test_paths();

        app.focus_details();
        let line = footer_line(&app, &paths);

        let details = line
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "Details")
            .expect("details context label");
        assert_eq!(details.style.fg, Some(Color::Cyan));
        assert!(details.style.add_modifier.contains(Modifier::BOLD));

        let content = line
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "content")
            .expect("details context value");
        assert_eq!(content.style.fg, Some(Color::Gray));
    }

    #[test]
    fn text_selection_mode_ignores_mouse_events() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.mouse_capture_enabled = false;
        app.focus_details();
        app.set_selection(1);

        let changed = handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 2,
                row: 8,
                modifiers: KeyModifiers::NONE,
            },
            Rect::new(0, 0, 120, 40),
        );

        assert!(!changed);
        assert_eq!(app.focus, FocusTarget::Details);
        assert_eq!(app.current_selected_position(), 1);
    }

    #[test]
    fn comment_editor_keeps_m_as_text() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.start_new_comment_dialog();

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('m')),
            &config,
            &store,
            &tx
        ));

        assert!(app.mouse_capture_enabled);
        assert_eq!(
            app.comment_dialog
                .as_ref()
                .map(|dialog| dialog.body.as_str()),
            Some("m")
        );
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
            total_count: None,
            page: 1,
            page_size: 0,
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
            total_count: None,
            page: 1,
            page_size: 0,
            refreshed_at: None,
            error: None,
        };
        let mut app = AppState::new(SectionKind::Issues, vec![section]);

        app.search_query = "fiber".to_string();
        app.clamp_positions();

        assert_eq!(app.current_item().map(|item| item.id.as_str()), Some("2"));
    }

    #[test]
    fn list_title_shows_filter_input_prompt() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.start_search();
        app.search_query = "borrow".to_string();
        let backend = ratatui::backend::TestBackend::new(180, 30);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let paths = test_paths();

        terminal
            .draw(|frame| draw(frame, &app, &paths))
            .expect("draw");

        let rendered = buffer_lines(terminal.backend().buffer()).join("\n");
        assert!(rendered.contains("Filter: /borrow_  Enter apply  Esc clear"));
    }

    #[test]
    fn list_title_shows_repo_search_input_prompt() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.start_global_search_input();
        app.global_search_query = "borrow".to_string();
        let backend = ratatui::backend::TestBackend::new(220, 30);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let paths = test_paths();

        terminal
            .draw(|frame| draw(frame, &app, &paths))
            .expect("draw");

        let rendered = buffer_lines(terminal.backend().buffer()).join("\n");
        assert!(
            rendered.contains("Repo Search in rust-lang/rust: Sborrow_  Enter search  Esc cancel")
        );
    }

    #[test]
    fn section_tab_label_shows_loaded_and_total_count() {
        let mut section = test_section();
        section.total_count = Some(120);
        let app = AppState::new(SectionKind::PullRequests, vec![section]);

        assert_eq!(
            section_tab_label(&app, app.visible_sections()[0]),
            "Test (2/120)"
        );
    }

    #[test]
    fn list_title_shows_visible_loaded_and_total_count() {
        let mut section = many_items_section(50);
        section.total_count = Some(120);
        let app = AppState::new(SectionKind::PullRequests, vec![section]);
        let backend = ratatui::backend::TestBackend::new(160, 30);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let paths = test_paths();

        terminal
            .draw(|frame| draw(frame, &app, &paths))
            .expect("draw");

        let rendered = buffer_lines(terminal.backend().buffer()).join("\n");
        assert!(rendered.contains("showing 1-18/120 | page 1/3"));
    }

    #[test]
    fn list_title_offsets_visible_range_for_result_page() {
        let mut section = many_items_section(50);
        section.total_count = Some(120);
        section.page = 2;
        section.page_size = 50;
        let app = AppState::new(SectionKind::PullRequests, vec![section]);
        let backend = ratatui::backend::TestBackend::new(160, 30);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let paths = test_paths();

        terminal
            .draw(|frame| draw(frame, &app, &paths))
            .expect("draw");

        let rendered = buffer_lines(terminal.backend().buffer()).join("\n");
        assert!(rendered.contains("showing 51-68/120 | page 2/3"));
    }

    #[test]
    fn section_page_request_uses_total_count_and_configured_page_size() {
        let mut section = many_items_section(100);
        section.total_count = Some(250);
        section.page = 1;
        section.page_size = 100;
        section.filters =
            "repo:rust-lang/rust is:open archived:false sort:updated-desc".to_string();
        let app = AppState::new(SectionKind::PullRequests, vec![section]);
        let request = app
            .section_page_request(1, &Config::default())
            .expect("next page request");

        assert_eq!(request.page, 2);
        assert_eq!(request.page_size, 100);
        assert_eq!(request.total_pages, 3);
        assert_eq!(
            request.filters,
            "repo:rust-lang/rust is:open archived:false sort:updated-desc"
        );
    }

    #[test]
    fn section_page_request_caps_at_github_result_window() {
        let mut section = many_items_section(100);
        section.total_count = Some(1073);
        section.page = 10;
        section.page_size = 100;
        let app = AppState::new(SectionKind::PullRequests, vec![section]);

        assert_eq!(
            app.section_page_request(1, &Config::default()).unwrap_err(),
            "already at last result page"
        );
        assert_eq!(
            section_page_label(app.current_section().unwrap()).as_deref(),
            Some("10/10+")
        );
    }

    #[test]
    fn capital_s_starts_global_search_input() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('S')),
            &config,
            &store,
            &tx
        ));

        assert!(app.global_search_active);
        assert!(!app.search_active);
        assert_eq!(app.global_search_query, "");
        assert_eq!(app.status, "repo search mode in rust-lang/rust");
    }

    #[test]
    fn global_search_enter_submits_query() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.start_global_search_input();
        let mut submitted = None;

        app.handle_global_search_key_with_submit(key(KeyCode::Char('f')), |query| {
            submitted = Some(query);
        });
        app.handle_global_search_key_with_submit(key(KeyCode::Char('i')), |query| {
            submitted = Some(query);
        });
        app.handle_global_search_key_with_submit(key(KeyCode::Char('b')), |query| {
            submitted = Some(query);
        });
        app.handle_global_search_key_with_submit(key(KeyCode::Enter), |query| {
            submitted = Some(query);
        });

        assert_eq!(submitted, Some("fib".to_string()));
        assert!(!app.global_search_active);
        assert!(app.global_search_running);
        assert_eq!(app.status, "searching rust-lang/rust for 'fib'");
    }

    #[test]
    fn global_search_scope_comes_from_current_item_or_repo_section() {
        let app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        assert_eq!(app.current_repo_scope().as_deref(), Some("rust-lang/rust"));

        let repo_section = SectionSnapshot {
            key: "repo:Fiber:pull_requests:Pull Requests".to_string(),
            kind: SectionKind::PullRequests,
            title: "Pull Requests".to_string(),
            filters: "repo:nervosnetwork/fiber is:open archived:false sort:updated-desc"
                .to_string(),
            items: Vec::new(),
            total_count: None,
            page: 1,
            page_size: 0,
            refreshed_at: None,
            error: None,
        };
        let app = AppState::new(SectionKind::PullRequests, vec![repo_section]);

        assert_eq!(
            app.current_repo_scope().as_deref(),
            Some("nervosnetwork/fiber")
        );
    }

    #[test]
    fn global_search_finished_switches_to_search_tab() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.global_search_running = true;
        let mut pr_section = SectionSnapshot::empty_for_view(
            global_search_view_key(),
            SectionKind::PullRequests,
            "Pull Requests",
            "fiber",
        );
        pr_section.items = vec![work_item(
            "pr-1",
            "nervosnetwork/fiber",
            1,
            "Fiber PR",
            None,
        )];
        let mut issue_section = SectionSnapshot::empty_for_view(
            global_search_view_key(),
            SectionKind::Issues,
            "Issues",
            "fiber",
        );
        let mut issue = work_item("issue-2", "nervosnetwork/fiber", 2, "Fiber issue", None);
        issue.kind = ItemKind::Issue;
        issue.url = "https://github.com/nervosnetwork/fiber/issues/2".to_string();
        issue_section.items = vec![issue];

        app.handle_msg(AppMsg::GlobalSearchFinished {
            query: "fiber".to_string(),
            sections: vec![pr_section, issue_section],
        });

        assert_eq!(app.active_view, global_search_view_key());
        assert!(app.view_tabs().iter().any(|view| view.label == "Search"));
        assert_eq!(app.visible_sections().len(), 2);
        assert_eq!(
            app.current_section().map(|section| section.title.as_str()),
            Some("Pull Requests")
        );
        assert_eq!(
            app.current_item().map(|item| item.id.as_str()),
            Some("pr-1")
        );
        assert!(!app.global_search_running);
        assert!(app.search_query.is_empty());
        assert_eq!(app.status, "search complete: 2 result(s) for 'fiber'");
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
    fn details_meta_is_compact_and_links_author() {
        let mut item = work_item("1", "chenyukang/ghr", 1, "More on tui", Some("chenyukang"));
        item.reason = Some("-".to_string());
        item.comments = Some(3);
        let section = SectionSnapshot {
            key: "pull_requests:test".to_string(),
            kind: SectionKind::PullRequests,
            title: "Test".to_string(),
            filters: String::new(),
            items: vec![item],
            total_count: None,
            page: 1,
            page_size: 0,
            refreshed_at: None,
            error: None,
        };
        let app = AppState::new(SectionKind::PullRequests, vec![section]);
        let document = build_details_document(&app, 120);
        let lines = document
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        let rendered = lines.join("\n");

        assert!(rendered.contains("repo: chenyukang/ghr"));
        assert!(rendered.contains("number: #1"));
        assert!(rendered.contains("state: open"));
        assert!(rendered.contains("author: chenyukang"));
        assert!(rendered.contains("comments: 3"));
        assert!(!rendered.contains("reason: -"));

        let author_line = lines
            .iter()
            .position(|line| line.contains("author: chenyukang"))
            .expect("author line");
        let author_column = lines[author_line].find("chenyukang").expect("author") as u16;
        assert_eq!(
            document.link_at(author_line, author_column),
            Some("https://github.com/chenyukang".to_string())
        );
    }

    #[test]
    fn details_meta_shows_pr_action_hints() {
        let section = SectionSnapshot {
            key: "pull_requests:test".to_string(),
            kind: SectionKind::PullRequests,
            title: "Test".to_string(),
            filters: String::new(),
            items: vec![work_item(
                "1",
                "chenyukang/ghr",
                1,
                "More on tui",
                Some("chenyukang"),
            )],
            total_count: None,
            page: 1,
            page_size: 0,
            refreshed_at: None,
            error: None,
        };
        let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
        app.action_hints.insert(
            "1".to_string(),
            ActionHintState::Loaded(ActionHints {
                labels: vec!["Approvable".to_string(), "Mergeable".to_string()],
                checks: Some(CheckSummary {
                    passed: 10,
                    failed: 2,
                    pending: 1,
                    skipped: 0,
                    total: 13,
                    incomplete: false,
                }),
                note: Some("Merge blocked: checks pending".to_string()),
            }),
        );

        let rendered = build_details_document(&app, 120)
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("action: Approvable, Mergeable"));
        assert!(rendered.contains("checks: 10 pass, 2 fail, 1 pending"));
        assert!(rendered.contains("action note: Merge blocked: checks pending"));
    }

    #[test]
    fn failed_check_count_is_rendered_red() {
        let segments = check_summary_segments(&CheckSummary {
            passed: 3,
            failed: 1,
            pending: 2,
            skipped: 0,
            total: 6,
            incomplete: false,
        });

        let failed = segments
            .iter()
            .find(|segment| segment.text == "1 fail")
            .expect("failed check segment");
        assert_eq!(failed.style.fg, Some(Color::LightRed));
        assert!(failed.style.add_modifier.contains(Modifier::BOLD));
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
            total_count: None,
            page: 1,
            page_size: 0,
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
    fn comment_markdown_prefers_word_boundary_wrapping() {
        let mut builder = DetailsBuilder::new(32);
        builder.push_markdown_block_indented(
            "I had a similar experience with you, the learning curve",
            "empty",
            usize::MAX,
            usize::MAX,
            COMMENT_LEFT_PADDING,
            COMMENT_RIGHT_PADDING,
        );
        let rendered = builder
            .finish()
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec![
                "  I had a similar experience",
                "  with you, the learning",
                "  curve"
            ]
        );
        assert!(rendered.iter().all(|line| !line.ends_with(' ')));
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
        let rendered_lines = document
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        let separator_indices = rendered_lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| line.trim_start().starts_with('─').then_some(index))
            .collect::<Vec<_>>();
        let separator_count = separator_indices.len();
        assert_eq!(separator_count, 2);
        assert!(
            separator_indices
                .iter()
                .all(|index| rendered_lines[*index].starts_with("  ")),
            "comment separators should be left padded: {rendered_lines:?}"
        );
        assert!(
            separator_indices
                .get(1)
                .and_then(|index| rendered_lines.get(index.saturating_sub(1)))
                .is_some_and(|line| line.is_empty()),
            "comments should be separated by one blank line: {rendered_lines:?}"
        );
        assert!(
            rendered_lines
                .iter()
                .any(|line| line.starts_with("  See https://example.com/one")),
            "comment body should be left padded: {rendered_lines:?}"
        );
        assert!(
            rendered_lines
                .iter()
                .filter(|line| !line.is_empty())
                .all(|line| display_width(line) <= 96),
            "comment lines should reserve right padding: {rendered_lines:?}"
        );

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
    fn m_key_opens_merge_confirmation_for_pull_request() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('M')),
            &config,
            &store,
            &tx
        ));

        let dialog = app.pr_action_dialog.as_ref().expect("merge dialog");
        assert_eq!(dialog.action, PrAction::Merge);
        assert_eq!(dialog.item.id, "1");
        assert_eq!(app.status, "confirm pull request merge");
    }

    #[test]
    fn capital_c_key_opens_close_confirmation_for_pull_request() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('C')),
            &config,
            &store,
            &tx
        ));

        let dialog = app.pr_action_dialog.as_ref().expect("close dialog");
        assert_eq!(dialog.action, PrAction::Close);
        assert_eq!(dialog.item.id, "1");
        assert_eq!(app.status, "confirm pull request close");
    }

    #[test]
    fn capital_a_key_opens_approve_confirmation_for_pull_request_details() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.focus_details();

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('A')),
            &config,
            &store,
            &tx
        ));

        let dialog = app.pr_action_dialog.as_ref().expect("approve dialog");
        assert_eq!(dialog.action, PrAction::Approve);
        assert_eq!(dialog.item.id, "1");
        assert_eq!(app.status, "confirm pull request approval");
    }

    #[test]
    fn pr_action_confirmation_submits_selected_action() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.start_pr_action_dialog(PrAction::Approve);
        let mut submitted = None;

        app.handle_pr_action_dialog_key_with_submit(key(KeyCode::Enter), |item, action| {
            submitted = Some((item.id, action));
        });

        assert!(app.pr_action_running);
        assert_eq!(app.status, "approving pull request");
        assert_eq!(submitted, Some(("1".to_string(), PrAction::Approve)));
    }

    #[test]
    fn pr_action_dialog_escape_cancels() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.start_pr_action_dialog(PrAction::Merge);

        app.handle_pr_action_dialog_key_with_submit(key(KeyCode::Esc), |_item, _action| {
            panic!("escape should not submit the action");
        });

        assert!(app.pr_action_dialog.is_none());
        assert!(!app.pr_action_running);
        assert_eq!(app.status, "pull request action cancelled");
    }

    #[test]
    fn pr_action_rejects_non_pull_request() {
        let mut item = work_item("1", "rust-lang/rust", 1, "Compiler diagnostics", None);
        item.kind = ItemKind::Issue;
        item.url = "https://github.com/rust-lang/rust/issues/1".to_string();
        let section = SectionSnapshot {
            key: "issues:test".to_string(),
            kind: SectionKind::Issues,
            title: "Test".to_string(),
            filters: String::new(),
            items: vec![item],
            total_count: None,
            page: 1,
            page_size: 0,
            refreshed_at: None,
            error: None,
        };
        let mut app = AppState::new(SectionKind::Issues, vec![section]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('M')),
            &config,
            &store,
            &tx
        ));

        assert!(app.pr_action_dialog.is_none());
        assert_eq!(app.status, "selected item is not a pull request");
    }

    #[test]
    fn pr_action_finished_marks_item_state_and_closes_dialog() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.start_pr_action_dialog(PrAction::Merge);
        app.pr_action_running = true;

        app.handle_msg(AppMsg::PrActionFinished {
            item_id: "1".to_string(),
            action: PrAction::Merge,
            result: Ok(()),
        });

        assert!(app.pr_action_dialog.is_none());
        assert!(!app.pr_action_running);
        assert_eq!(app.sections[0].items[0].state.as_deref(), Some("merged"));
        assert!(app.details_stale.contains("1"));
        assert_eq!(app.status, "pull request merged; refreshing");
    }

    #[test]
    fn approve_action_finished_keeps_item_open_and_refreshes_details() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.start_pr_action_dialog(PrAction::Approve);
        app.pr_action_running = true;

        app.handle_msg(AppMsg::PrActionFinished {
            item_id: "1".to_string(),
            action: PrAction::Approve,
            result: Ok(()),
        });

        assert!(app.pr_action_dialog.is_none());
        assert!(!app.pr_action_running);
        assert_eq!(app.sections[0].items[0].state.as_deref(), Some("open"));
        assert!(app.details_stale.contains("1"));
        assert_eq!(app.status, "pull request approved; refreshing");
    }

    #[test]
    fn pr_action_failure_opens_message_dialog() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.start_pr_action_dialog(PrAction::Merge);
        app.pr_action_running = true;

        app.handle_msg(AppMsg::PrActionFinished {
            item_id: "1".to_string(),
            action: PrAction::Merge,
            result: Err(
                "merge blocked for owner/repo#1: review approval required; 1 check(s) failing"
                    .to_string(),
            ),
        });

        assert!(app.pr_action_dialog.is_none());
        assert!(!app.pr_action_running);
        assert_eq!(app.status, "pull request merge failed");
        let dialog = app.message_dialog.as_ref().expect("message dialog");
        assert_eq!(dialog.title, "Merge Failed");
        assert!(dialog.body.contains("review approval required"));
    }

    #[test]
    fn message_dialog_enter_dismisses() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.message_dialog = Some(MessageDialog {
            title: "Merge Failed".to_string(),
            body: "review approval required".to_string(),
        });

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Enter),
            &config,
            &store,
            &tx
        ));

        assert!(app.message_dialog.is_none());
        assert_eq!(app.status, "message dismissed");
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
            app.message_dialog
                .as_ref()
                .map(|dialog| (dialog.title.as_str(), dialog.body.as_str())),
            Some((
                "Posting Comment",
                "Waiting for GitHub to accept the comment..."
            ))
        );
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
            app.message_dialog
                .as_ref()
                .map(|dialog| (dialog.title.as_str(), dialog.body.as_str())),
            Some((
                "Updating Comment",
                "Waiting for GitHub to accept the update..."
            ))
        );
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
    fn comment_post_success_opens_result_dialog() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.posting_comment = true;
        app.message_dialog = Some(comment_pending_dialog(PendingCommentMode::Post));

        app.handle_msg(AppMsg::CommentPosted {
            item_id: "1".to_string(),
            result: Ok(vec![comment("alice", "posted", None)]),
        });

        assert!(!app.posting_comment);
        assert_eq!(app.status, "comment posted");
        assert_eq!(
            app.message_dialog
                .as_ref()
                .map(|dialog| (dialog.title.as_str(), dialog.body.as_str())),
            Some((
                "Comment Posted",
                "GitHub accepted the comment and comments were refreshed."
            ))
        );
        assert_eq!(app.selected_comment_index, 0);
    }

    #[test]
    fn comment_post_failure_opens_result_dialog() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.posting_comment = true;
        app.message_dialog = Some(comment_pending_dialog(PendingCommentMode::Post));

        app.handle_msg(AppMsg::CommentPosted {
            item_id: "1".to_string(),
            result: Err("gh api repos/owner/repo/issues/1/comments failed: HTTP 403".to_string()),
        });

        assert!(!app.posting_comment);
        assert_eq!(app.status, "comment post failed");
        let dialog = app.message_dialog.as_ref().expect("failure dialog");
        assert_eq!(dialog.title, "Comment Failed");
        assert_eq!(dialog.body, "HTTP 403");
    }

    #[test]
    fn comment_update_success_opens_result_dialog() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.posting_comment = true;
        app.message_dialog = Some(comment_pending_dialog(PendingCommentMode::Edit {
            comment_index: 0,
            comment_id: 42,
        }));

        app.handle_msg(AppMsg::CommentUpdated {
            item_id: "1".to_string(),
            comment_index: 0,
            result: Ok(vec![own_comment(42, "chenyukang", "updated", None)]),
        });

        assert!(!app.posting_comment);
        assert_eq!(app.status, "comment updated");
        assert_eq!(
            app.message_dialog
                .as_ref()
                .map(|dialog| (dialog.title.as_str(), dialog.body.as_str())),
            Some((
                "Comment Updated",
                "GitHub accepted the update and comments were refreshed."
            ))
        );
    }

    #[test]
    fn comment_update_failure_opens_result_dialog() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.posting_comment = true;
        app.message_dialog = Some(comment_pending_dialog(PendingCommentMode::Edit {
            comment_index: 0,
            comment_id: 42,
        }));

        app.handle_msg(AppMsg::CommentUpdated {
            item_id: "1".to_string(),
            comment_index: 0,
            result: Err(
                "gh api repos/owner/repo/issues/comments/42 failed: validation failed".to_string(),
            ),
        });

        assert!(!app.posting_comment);
        assert_eq!(app.status, "comment update failed");
        let dialog = app.message_dialog.as_ref().expect("failure dialog");
        assert_eq!(dialog.title, "Update Failed");
        assert_eq!(dialog.body, "validation failed");
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
            Some(6 + MOUSE_SCROLL_LINES)
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
    fn mouse_clicking_view_tab_switches_view_and_focuses_ghr() {
        let sections = vec![
            test_section(),
            SectionSnapshot {
                key: "issues:test".to_string(),
                kind: SectionKind::Issues,
                title: "Issues".to_string(),
                filters: String::new(),
                items: vec![work_item("3", "nervosnetwork/fiber", 3, "Issue", None)],
                total_count: None,
                page: 1,
                page_size: 0,
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

        assert_eq!(app.active_view, builtin_view_key(SectionKind::Issues));
        assert_eq!(app.focus, FocusTarget::Ghr);
        assert!(!app.search_active);
        assert_eq!(app.status, "ghr focused");
    }

    #[test]
    fn repo_sections_create_top_level_repo_tab_with_generic_section_titles() {
        let sections = vec![
            test_section(),
            SectionSnapshot::empty_for_view(
                "repo:fiber",
                SectionKind::PullRequests,
                "Pull Requests",
                "repo:nervosnetwork/fiber is:open archived:false sort:updated-desc",
            ),
            SectionSnapshot::empty_for_view(
                "repo:fiber",
                SectionKind::Issues,
                "Issues",
                "repo:nervosnetwork/fiber is:open archived:false sort:updated-desc",
            ),
        ];
        let mut app = AppState::new(SectionKind::PullRequests, sections);

        app.switch_view("repo:fiber");

        assert_eq!(
            app.view_tabs()
                .iter()
                .map(|view| view.label.as_str())
                .collect::<Vec<_>>(),
            vec!["Pull Requests", "fiber"]
        );
        assert_eq!(
            app.visible_sections()
                .iter()
                .map(|section| section.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Pull Requests", "Issues"]
        );
    }

    #[test]
    fn mouse_clicking_section_tab_switches_section_and_focuses_sections() {
        let sections = vec![
            SectionSnapshot {
                key: "pull_requests:Mine".to_string(),
                kind: SectionKind::PullRequests,
                title: "Mine".to_string(),
                filters: String::new(),
                items: vec![work_item("1", "rust-lang/rust", 1, "Compiler", None)],
                total_count: None,
                page: 1,
                page_size: 0,
                refreshed_at: None,
                error: None,
            },
            SectionSnapshot {
                key: "pull_requests:Assigned".to_string(),
                kind: SectionKind::PullRequests,
                title: "Assigned".to_string(),
                filters: String::new(),
                items: vec![work_item("2", "nervosnetwork/fiber", 2, "Fiber", None)],
                total_count: None,
                page: 1,
                page_size: 0,
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
        assert_eq!(app.focus, FocusTarget::Sections);
        assert!(!app.search_active);
        assert_eq!(app.status, "Sections focused");
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
    fn focus_panel_title_marks_active_panel() {
        assert_eq!(
            focus_panel_title("Details", "Details:", true),
            "[FOCUS Details] Details:"
        );
        assert_eq!(focus_panel_title("Details", "Details:", false), "Details:");
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
            total_count: None,
            page: 1,
            page_size: 0,
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
    fn enter_and_four_focus_details_without_quitting() {
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
            key(KeyCode::Char('4')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.focus, FocusTarget::Details);
    }

    #[test]
    fn number_focus_keys_work_from_details() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        app.focus_details();
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('1')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.focus, FocusTarget::Ghr);

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('2')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.focus, FocusTarget::Sections);

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('3')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.focus, FocusTarget::List);

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('4')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.focus, FocusTarget::Details);
    }

    #[test]
    fn h_and_l_switch_only_the_focused_tab_group() {
        let mut issue = work_item("issue-1", "nervosnetwork/fiber", 1, "Issue", None);
        issue.kind = ItemKind::Issue;
        issue.url = "https://github.com/nervosnetwork/fiber/issues/1".to_string();
        let sections = vec![
            test_section(),
            SectionSnapshot {
                key: "pull_requests:assigned".to_string(),
                kind: SectionKind::PullRequests,
                title: "Assigned".to_string(),
                filters: String::new(),
                items: vec![work_item("2", "nervosnetwork/fiber", 2, "Fiber", None)],
                total_count: None,
                page: 1,
                page_size: 0,
                refreshed_at: None,
                error: None,
            },
            SectionSnapshot {
                key: "issues:test".to_string(),
                kind: SectionKind::Issues,
                title: "Issues".to_string(),
                filters: String::new(),
                items: vec![issue],
                total_count: None,
                page: 1,
                page_size: 0,
                refreshed_at: None,
                error: None,
            },
        ];
        let mut app = AppState::new(SectionKind::PullRequests, sections);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        app.focus_ghr();
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('l')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.active_view, builtin_view_key(SectionKind::Issues));
        assert_eq!(app.focus, FocusTarget::Ghr);

        app.focus_sections();
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('h')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.active_view, builtin_view_key(SectionKind::Issues));
        assert_eq!(app.current_section_position(), 0);
        assert_eq!(app.focus, FocusTarget::Sections);

        app.switch_view(builtin_view_key(SectionKind::PullRequests));
        app.focus_sections();
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('l')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.current_section_position(), 1);
        assert_eq!(app.focus, FocusTarget::Sections);

        app.focus_list();
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('h')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.current_section_position(), 1);
        assert_eq!(app.focus, FocusTarget::List);
    }

    #[test]
    fn h_and_l_wrap_focused_tab_groups_at_edges() {
        let mut issue = work_item("issue-1", "nervosnetwork/fiber", 1, "Issue", None);
        issue.kind = ItemKind::Issue;
        issue.url = "https://github.com/nervosnetwork/fiber/issues/1".to_string();
        let sections = vec![
            test_section(),
            SectionSnapshot {
                key: "pull_requests:assigned".to_string(),
                kind: SectionKind::PullRequests,
                title: "Assigned".to_string(),
                filters: String::new(),
                items: vec![work_item("2", "nervosnetwork/fiber", 2, "Fiber", None)],
                total_count: None,
                page: 1,
                page_size: 0,
                refreshed_at: None,
                error: None,
            },
            SectionSnapshot {
                key: "issues:test".to_string(),
                kind: SectionKind::Issues,
                title: "Issues".to_string(),
                filters: String::new(),
                items: vec![issue],
                total_count: None,
                page: 1,
                page_size: 0,
                refreshed_at: None,
                error: None,
            },
        ];
        let mut app = AppState::new(SectionKind::PullRequests, sections);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        app.focus_ghr();
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('h')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.active_view, builtin_view_key(SectionKind::Issues));
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('l')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.active_view, builtin_view_key(SectionKind::PullRequests));

        app.focus_sections();
        assert_eq!(app.current_section_position(), 0);
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('h')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.current_section_position(), 1);
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('l')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.current_section_position(), 0);
    }

    #[test]
    fn render_marks_only_one_focused_region() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let paths = test_paths();

        app.focus_sections();
        terminal
            .draw(|frame| draw(frame, &app, &paths))
            .expect("draw sections focus");
        let rendered = buffer_lines(terminal.backend().buffer()).join("\n");
        assert_eq!(rendered.matches("[FOCUS").count(), 1);
        assert!(rendered.contains("[FOCUS] Sections"));
        assert!(!rendered.contains("[FOCUS List]"));

        app.focus_list();
        terminal
            .draw(|frame| draw(frame, &app, &paths))
            .expect("draw list focus");
        let rendered = buffer_lines(terminal.backend().buffer()).join("\n");
        assert_eq!(rendered.matches("[FOCUS").count(), 1);
        assert!(rendered.contains("[FOCUS List]"));
        assert!(!rendered.contains("[FOCUS] Sections"));
    }

    #[test]
    fn search_input_keeps_number_keys_as_query_text() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        app.start_search();
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('1')),
            &config,
            &store,
            &tx
        ));

        assert_eq!(app.search_query, "1");
        assert_eq!(app.active_view, builtin_view_key(SectionKind::PullRequests));
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
    fn three_focuses_primary_list_without_changing_page() {
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
                total_count: None,
                page: 1,
                page_size: 0,
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
                total_count: None,
                page: 1,
                page_size: 0,
                refreshed_at: None,
                error: None,
            },
            SectionSnapshot {
                key: "issues:Test".to_string(),
                kind: SectionKind::Issues,
                title: "Test".to_string(),
                filters: String::new(),
                items: vec![work_item("3", "nervosnetwork/ckb", 3, "Issue", None)],
                total_count: None,
                page: 1,
                page_size: 0,
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
            key(KeyCode::Char('3')),
            &config,
            &store,
            &tx
        ));

        assert_eq!(app.active_view, builtin_view_key(SectionKind::Issues));
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

    fn ctrl_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, crossterm::event::KeyModifiers::CONTROL)
    }

    fn test_paths() -> Paths {
        let root = std::path::PathBuf::from("/tmp/ghr-test");
        Paths {
            config_path: root.join("config.toml"),
            db_path: root.join("ghr.db"),
            log_path: root.join("ghr.log"),
            state_path: root.join("state.toml"),
            root,
        }
    }

    fn buffer_lines(buffer: &ratatui::buffer::Buffer) -> Vec<String> {
        let width = buffer.area.width as usize;
        buffer
            .content()
            .chunks(width)
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
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
            total_count: None,
            page: 1,
            page_size: 0,
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
            total_count: None,
            page: 1,
            page_size: 0,
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
