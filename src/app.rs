use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
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
use pulldown_cmark::{CodeBlockKind, Event as MarkdownEvent, Options, Parser, Tag, TagEnd};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, HighlightSpacing, Paragraph, Row, Table, TableState, Tabs,
    Wrap,
};
use ratatui::{Frame, Terminal};
use tokio::process::Command as TokioCommand;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tracing::warn;

use crate::config::{Config, github_repo_from_remote_url};
use crate::dirs::Paths;
use crate::github::{
    PullRequestReviewCommentTarget, approve_pull_request, close_pull_request, edit_issue_comment,
    edit_pull_request_review_comment, fetch_comments, fetch_pull_request_action_hints,
    fetch_pull_request_diff, mark_notification_thread_read, merge_pull_request, post_issue_comment,
    post_pull_request_review_comment, post_pull_request_review_reply, refresh_dashboard,
    refresh_dashboard_with_progress, refresh_section_page, search_global,
    with_background_github_priority,
};
use crate::model::{
    ActionHints, CheckSummary, CommentPreview, ItemKind, PullRequestBranch, SectionKind,
    SectionSnapshot, WorkItem, builtin_view_key, configured_sections, global_search_view_key,
    mark_notification_read_in_section, merge_cached_sections, merge_refreshed_sections,
    section_counts, section_view_key,
};
use crate::snapshot::SnapshotStore;
use crate::state::UiState;

mod diff;
mod layout;
mod search;
mod status;
mod text;

use diff::{
    DiffFile, DiffLine, DiffLineKind, PullRequestDiff, parse_inline_diff_hunk,
    parse_pull_request_diff,
};
use layout::{
    block_inner, body_areas_with_ratio, centered_rect, centered_rect_width,
    centered_rect_with_size, details_area_for, page_areas, rect_contains,
    split_percent_from_column, splitter_contains,
};
#[cfg(test)]
use layout::{body_area, body_areas};
use search::{filtered_indices, fuzzy_score};
use status::{
    comment_pending_dialog, compact_error_label, message_dialog, operation_error_body,
    persistent_success_message_dialog, pr_action_error_body, pr_action_error_status,
    pr_action_error_title, pr_action_success_body, pr_action_success_title, refresh_error_status,
    setup_dialog_from_error, success_message_dialog,
};
use text::{display_width, normalize_text, truncate_inline, truncate_text};

enum AppMsg {
    RefreshStarted,
    RefreshFinished {
        sections: Vec<SectionSnapshot>,
        save_error: Option<String>,
    },
    RefreshSectionLoaded {
        section: SectionSnapshot,
        save_error: Option<String>,
    },
    CommentsLoaded {
        item_id: String,
        comments: std::result::Result<Vec<CommentPreview>, String>,
    },
    ActionHintsLoaded {
        item_id: String,
        actions: std::result::Result<ActionHints, String>,
    },
    DiffLoaded {
        item_id: String,
        diff: std::result::Result<PullRequestDiff, String>,
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
    ReviewCommentPosted {
        item_id: String,
        result: std::result::Result<(), String>,
    },
    PrActionFinished {
        item_id: String,
        action: PrAction,
        result: std::result::Result<(), String>,
    },
    PrCheckoutFinished {
        result: std::result::Result<PrCheckoutResult, String>,
    },
    NotificationReadFinished {
        thread_id: String,
        result: std::result::Result<Option<String>, String>,
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

#[derive(Debug, Clone)]
enum DiffState {
    Loading,
    Loaded(PullRequestDiff),
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
enum DetailsMode {
    Conversation,
    Diff,
}

impl DetailsMode {
    fn as_state_str(self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::Diff => "diff",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetupDialog {
    MissingGh,
    AuthRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupDialog {
    Initializing,
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CommentDialogMode {
    New,
    Reply {
        comment_index: usize,
        author: String,
        review_comment_id: Option<u64>,
    },
    Edit {
        comment_index: usize,
        comment_id: u64,
        is_review: bool,
    },
    Review {
        target: DiffReviewTarget,
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
    Checkout,
}

#[derive(Debug, Clone)]
struct PrActionDialog {
    item: WorkItem,
    action: PrAction,
    checkout: Option<PrCheckoutPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PrCheckoutResult {
    command: String,
    directory: PathBuf,
    output: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PrCheckoutPlan {
    directory: PathBuf,
    branch: Option<PullRequestBranch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MessageDialog {
    title: String,
    body: String,
    kind: MessageDialogKind,
    auto_close_at: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageDialogKind {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PendingCommentMode {
    Post,
    ReviewReply {
        comment_id: u64,
    },
    Edit {
        comment_index: usize,
        comment_id: u64,
        is_review: bool,
    },
    Review {
        target: DiffReviewTarget,
    },
}

const TABLE_HEADER_HEIGHT: u16 = 2;
const SUCCESS_DIALOG_AUTO_CLOSE: Duration = Duration::from_secs(1);
const TAB_DIVIDER_WIDTH: u16 = 3;
const MOUSE_LIST_SCROLL_LINES: u16 = 2;
const MOUSE_DIFF_FILE_SCROLL_LINES: u16 = 1;
const MOUSE_DETAILS_SCROLL_LINES: u16 = 1;
const MOUSE_COMMENT_SCROLL_LINES: u16 = 2;
const EVENT_BATCH_LIMIT: usize = 512;
const MAX_COALESCED_MOUSE_SCROLL_STEPS: i16 = 6;
const COMMENT_DIALOG_WIDTH_PERCENT: u16 = 72;
const COMMENT_DIALOG_MIN_HEIGHT: u16 = 10;
const COMMENT_DIALOG_VERTICAL_MARGIN: u16 = 4;
const COMMENT_DIALOG_FOOTER_HEIGHT: u16 = 2;
const COMMENT_DIALOG_MIN_EDITOR_HEIGHT: u16 = 4;
const COMMENT_DIALOG_EDITOR_PADDING_LINES: u16 = 1;
const COMMENT_DIALOG_FALLBACK_EDITOR_HEIGHT: u16 = 10;
const COMMENT_DIALOG_FALLBACK_EDITOR_WIDTH: u16 = 48;
const PR_ACTION_REMOTE_BRANCH_LINE: u16 = 6;
const COMMENT_LEFT_PADDING: usize = 2;
const COMMENT_RIGHT_PADDING: usize = 4;
const COMMENT_COLLAPSE_MIN_LINES: usize = 36;
const COMMENT_COLLAPSE_MIN_CHARS: usize = 2_800;
const COMMENT_COLLAPSED_PREVIEW_LINES: usize = 18;
const COMMENT_COLLAPSED_PREVIEW_CHARS: usize = 1_600;
const INLINE_COMMENT_CONTEXT_RADIUS: usize = 3;
const INLINE_COMMENT_CONTEXT_MAX_LINES: usize = INLINE_COMMENT_CONTEXT_RADIUS * 2 + 1;
const DIFF_INLINE_COMMENT_GUTTER_WIDTH: usize = 11;
const SEARCH_RESULT_WINDOW: usize = 1000;
const DIFF_DOUBLE_CLICK_MAX: Duration = Duration::from_millis(450);
const DETAILS_LOAD_DEBOUNCE: Duration = Duration::from_millis(350);

struct AppState {
    active_view: String,
    sections: Vec<SectionSnapshot>,
    section_index: HashMap<String, usize>,
    selected_index: HashMap<String, usize>,
    list_scroll_offset: HashMap<String, usize>,
    focus: FocusTarget,
    details_scroll: u16,
    details_mode: DetailsMode,
    list_width_percent: u16,
    dragging_split: bool,
    split_drag_changed: bool,
    search_active: bool,
    search_query: String,
    comment_search_active: bool,
    comment_search_query: String,
    global_search_active: bool,
    global_search_query: String,
    global_search_running: bool,
    global_search_return_view: Option<String>,
    global_search_scope: Option<String>,
    global_search_started_at: Option<Instant>,
    status: String,
    refreshing: bool,
    last_refresh_request: Instant,
    details: HashMap<String, DetailState>,
    diffs: HashMap<String, DiffState>,
    selected_diff_file: HashMap<String, usize>,
    selected_diff_line: HashMap<String, usize>,
    diff_mark: HashMap<String, DiffMarkState>,
    last_diff_click: Option<DiffClickState>,
    diff_mode_state: HashMap<String, DiffModeState>,
    conversation_details_state: HashMap<String, ConversationDetailsState>,
    viewed_details_snapshot: HashMap<String, String>,
    viewed_comments_snapshot: HashMap<String, String>,
    action_hints: HashMap<String, ActionHintState>,
    details_stale: HashSet<String>,
    details_refreshing: HashSet<String>,
    pending_details_load: Option<PendingDetailsLoad>,
    notification_read_pending: HashSet<String>,
    selected_comment_index: usize,
    expanded_comments: HashSet<String>,
    comment_dialog: Option<CommentDialog>,
    posting_comment: bool,
    pr_action_dialog: Option<PrActionDialog>,
    pr_action_running: bool,
    setup_dialog: Option<SetupDialog>,
    startup_dialog: Option<StartupDialog>,
    message_dialog: Option<MessageDialog>,
    mouse_capture_enabled: bool,
    help_dialog: bool,
    diff_return_state: Option<DiffReturnState>,
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
struct DiffReturnState {
    focus: FocusTarget,
    details_scroll: u16,
    selected_comment_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiffModeState {
    focus: FocusTarget,
    details_scroll: u16,
    selected_file: usize,
    selected_line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ConversationDetailsState {
    details_scroll: u16,
    selected_comment_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DiffMarkState {
    anchor: usize,
    focus: usize,
    complete: bool,
}

impl DiffMarkState {
    fn pending(anchor: usize) -> Self {
        Self {
            anchor,
            focus: anchor,
            complete: false,
        }
    }

    fn is_pending(self) -> bool {
        !self.complete
    }

    fn complete_at(&mut self, focus: usize) {
        self.focus = focus;
        self.complete = true;
    }

    fn range(self) -> (usize, usize) {
        ordered_range(self.anchor, self.focus)
    }
}

#[derive(Debug, Clone)]
struct DiffClickState {
    item_id: String,
    file_index: usize,
    review_index: usize,
    at: Instant,
}

#[derive(Debug, Clone)]
struct PendingDetailsLoad {
    item_id: String,
    ready_at: Instant,
}

impl DiffClickState {
    fn matches(&self, item_id: &str, file_index: usize, review_index: usize, now: Instant) -> bool {
        self.item_id == item_id
            && self.file_index == file_index
            && self.review_index == review_index
            && now.saturating_duration_since(self.at) <= DIFF_DOUBLE_CLICK_MAX
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefreshPriority {
    User,
    Background,
}

pub async fn run(config: Config, paths: Paths, store: SnapshotStore) -> Result<()> {
    let cached = store.load_all()?;
    let show_startup_dialog = should_show_startup_dialog(&cached);
    let sections = merge_cached_sections(configured_sections(&config), cached);
    let ui_state = UiState::load_or_default(&paths.state_path);
    let mut app = AppState::with_ui_state(config.defaults.view, sections, ui_state);
    let startup_setup_dialog = startup_setup_dialog();
    if let Some(dialog) = startup_setup_dialog {
        app.show_setup_dialog(dialog);
    } else if show_startup_dialog {
        app.show_startup_initializing();
    }

    let (tx, mut rx) = mpsc::unbounded_channel();
    if startup_setup_dialog.is_none() {
        start_refresh(
            config.clone(),
            store.clone(),
            tx.clone(),
            RefreshPriority::Background,
        );
    }

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

fn should_show_startup_dialog(cached: &HashMap<String, SectionSnapshot>) -> bool {
    cached.is_empty()
}

fn startup_setup_dialog() -> Option<SetupDialog> {
    startup_setup_dialog_from_gh_probe(
        Command::new("gh")
            .env("GH_PROMPT_DISABLED", "1")
            .arg("--version")
            .output()
            .map(|_| ()),
    )
}

fn startup_setup_dialog_from_gh_probe(result: io::Result<()>) -> Option<SetupDialog> {
    match result {
        Ok(()) => None,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Some(SetupDialog::MissingGh),
        Err(_) => None,
    }
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
        app.ensure_current_diff_loading(tx);
        app.dismiss_expired_message_dialog(Instant::now());

        if !app.refreshing
            && config.defaults.refetch_interval_seconds > 0
            && app.last_refresh_request.elapsed().as_secs()
                >= config.defaults.refetch_interval_seconds
        {
            start_refresh(
                config.clone(),
                store.clone(),
                tx.clone(),
                RefreshPriority::Background,
            );
        }

        terminal.draw(|frame| draw(frame, app, paths))?;

        let mut should_quit = false;
        if event::poll(Duration::from_millis(120))? {
            let events = read_event_batch(event::read()?)?;
            let size = terminal.size()?;
            let area = Rect::new(0, 0, size.width, size.height);
            should_quit = handle_event_batch(app, events, area, config, paths, store, tx);
        }
        sync_mouse_capture(terminal, app, &mut mouse_capture_enabled)?;
        if should_quit {
            break;
        }
    }

    save_ui_state(app, paths);
    Ok(())
}

fn read_event_batch(first: Event) -> Result<Vec<Event>> {
    let mut events = vec![first];
    while events.len() < EVENT_BATCH_LIMIT && event::poll(Duration::from_millis(0))? {
        events.push(event::read()?);
    }
    Ok(events)
}

fn handle_event_batch(
    app: &mut AppState,
    events: Vec<Event>,
    area: Rect,
    config: &Config,
    paths: &Paths,
    store: &SnapshotStore,
    tx: &UnboundedSender<AppMsg>,
) -> bool {
    let mut pending_scroll = None;

    for event in events {
        match event {
            Event::Key(key) => {
                flush_pending_mouse_scroll(app, &mut pending_scroll);
                if key.kind == KeyEventKind::Release {
                    continue;
                }
                if handle_key_in_area(app, key, config, store, tx, Some(area)) {
                    save_ui_state(app, paths);
                    return true;
                }
            }
            Event::Mouse(mouse) => {
                if try_accumulate_mouse_scroll(app, mouse, area, &mut pending_scroll) {
                    continue;
                }
                flush_pending_mouse_scroll(app, &mut pending_scroll);
                if handle_mouse_with_sync(app, mouse, area, Some(store), Some(tx)) {
                    save_ui_state(app, paths);
                }
            }
            _ => flush_pending_mouse_scroll(app, &mut pending_scroll),
        }
    }

    flush_pending_mouse_scroll(app, &mut pending_scroll);
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MouseWheelTargetKind {
    CommentDialog,
    List,
    Details,
}

#[derive(Debug, Clone, Copy)]
struct MouseWheelTarget {
    kind: MouseWheelTargetKind,
    area: Rect,
}

#[derive(Debug, Clone, Copy)]
struct PendingMouseScroll {
    target: MouseWheelTarget,
    steps: i16,
}

fn try_accumulate_mouse_scroll(
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

fn flush_pending_mouse_scroll(app: &mut AppState, pending: &mut Option<PendingMouseScroll>) {
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

fn mouse_scroll_step(kind: MouseEventKind) -> Option<i16> {
    match kind {
        MouseEventKind::ScrollDown => Some(1),
        MouseEventKind::ScrollUp => Some(-1),
        _ => None,
    }
}

fn mouse_wheel_target(app: &AppState, mouse: MouseEvent, area: Rect) -> Option<MouseWheelTarget> {
    if !app.mouse_capture_enabled
        || app.setup_dialog.is_some()
        || app.startup_dialog.is_some()
        || app.help_dialog
        || app.message_dialog.is_some()
        || app.pr_action_dialog.is_some()
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

fn start_refresh(
    config: Config,
    store: SnapshotStore,
    tx: UnboundedSender<AppMsg>,
    priority: RefreshPriority,
) {
    let _ = tx.send(AppMsg::RefreshStarted);
    tokio::spawn(async move {
        let mut save_error = None;
        let refresh = async {
            refresh_dashboard_with_progress(&config, |section| {
                if save_error.is_none()
                    && section.error.is_none()
                    && let Err(error) = store.save_section(section)
                {
                    let message = error.to_string();
                    warn!(error = %message, "failed to save refreshed snapshot");
                    save_error = Some(message);
                }

                let _ = tx.send(AppMsg::RefreshSectionLoaded {
                    section: section.clone(),
                    save_error: save_error.clone(),
                });
            })
            .await
        };
        let sections = match priority {
            RefreshPriority::User => refresh.await,
            RefreshPriority::Background => with_background_github_priority(refresh).await,
        };
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

fn start_notification_read_sync(
    thread_id: String,
    store: SnapshotStore,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let result = match mark_notification_thread_read(&thread_id).await {
            Ok(()) => match store.mark_notification_read(&thread_id) {
                Ok(_) => Ok(None),
                Err(error) => Ok(Some(error.to_string())),
            },
            Err(error) => Err(error.to_string()),
        };
        let _ = tx.send(AppMsg::NotificationReadFinished { thread_id, result });
    });
}

fn start_comments_load(item: WorkItem, tx: UnboundedSender<AppMsg>) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let comments = match item.number {
            Some(number) => fetch_comments(&item.repo, number, item.kind)
                .await
                .map_err(|error| error.to_string()),
            None => Ok(Vec::new()),
        };
        let _ = tx.send(AppMsg::CommentsLoaded { item_id, comments });
    });
}

fn start_action_hints_load(item: WorkItem, tx: UnboundedSender<AppMsg>) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let actions = match item.number {
            Some(number) => fetch_pull_request_action_hints(&item.repo, number)
                .await
                .map_err(|error| error.to_string()),
            None => Err("selected item has no pull request number".to_string()),
        };
        let _ = tx.send(AppMsg::ActionHintsLoaded { item_id, actions });
    });
}

fn start_diff_load(item: WorkItem, tx: UnboundedSender<AppMsg>) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let diff = match item.number {
            Some(number) => match fetch_pull_request_diff(&item.repo, number).await {
                Ok(diff) => parse_pull_request_diff(&diff),
                Err(error) => Err(error.to_string()),
            },
            None => Err("selected item has no pull request number".to_string()),
        };
        let _ = tx.send(AppMsg::DiffLoaded { item_id, diff });
    });
}

fn start_comment_submit(item: WorkItem, body: String, tx: UnboundedSender<AppMsg>) {
    tokio::spawn(async move {
        let result = match item.number {
            Some(number) => match post_issue_comment(&item.repo, number, &body).await {
                Ok(()) => fetch_comments(&item.repo, number, item.kind)
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

fn start_review_reply_submit(
    item: WorkItem,
    comment_id: u64,
    body: String,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let result = match item.number {
            Some(number) => {
                match post_pull_request_review_reply(&item.repo, number, comment_id, &body).await {
                    Ok(()) => fetch_comments(&item.repo, number, item.kind)
                        .await
                        .map_err(|error| error.to_string()),
                    Err(error) => Err(error.to_string()),
                }
            }
            None => Err("selected item has no pull request number".to_string()),
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
    is_review: bool,
    body: String,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let result = match item.number {
            Some(number) => match if is_review {
                edit_pull_request_review_comment(&item.repo, comment_id, &body).await
            } else {
                edit_issue_comment(&item.repo, comment_id, &body).await
            } {
                Ok(()) => fetch_comments(&item.repo, number, item.kind)
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

fn start_review_comment_submit(
    item: WorkItem,
    target: DiffReviewTarget,
    body: String,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        let result = match item.number {
            Some(number) => post_pull_request_review_comment(
                &item.repo,
                number,
                PullRequestReviewCommentTarget {
                    path: &target.path,
                    line: target.line,
                    side: target.side.as_api_value(),
                    start_line: target.start_line,
                    start_side: target.start_side.map(DiffReviewSide::as_api_value),
                },
                &body,
            )
            .await
            .map_err(|error| error.to_string()),
            None => Err("selected item has no pull request number".to_string()),
        };
        let _ = tx.send(AppMsg::ReviewCommentPosted { item_id, result });
    });
}

fn start_pr_action(
    item: WorkItem,
    action: PrAction,
    checkout: Option<PrCheckoutPlan>,
    config: Config,
    store: SnapshotStore,
    tx: UnboundedSender<AppMsg>,
) {
    tokio::spawn(async move {
        let item_id = item.id.clone();
        if action == PrAction::Checkout {
            let Some(checkout) = checkout else {
                let _ = tx.send(AppMsg::PrCheckoutFinished {
                    result: Err("missing local checkout target".to_string()),
                });
                return;
            };
            let result = run_pr_checkout(item, checkout.directory).await;
            let _ = tx.send(AppMsg::PrCheckoutFinished { result });
            return;
        }

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
                PrAction::Checkout => unreachable!("checkout is handled before remote PR actions"),
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
            let sections = with_background_github_priority(refresh_dashboard(&config)).await;
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

async fn run_pr_checkout(
    item: WorkItem,
    directory: PathBuf,
) -> std::result::Result<PrCheckoutResult, String> {
    let number = item
        .number
        .ok_or_else(|| "selected item has no pull request number".to_string())?;
    let args = pr_checkout_command_args(&item.repo, number);
    let command = pr_checkout_command_display(&args);
    let output = TokioCommand::new("gh")
        .env("GH_PROMPT_DISABLED", "1")
        .current_dir(&directory)
        .args(&args)
        .output()
        .await
        .map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                format!(
                    "GitHub CLI `gh` is required for local checkout. Install it, run `gh auth login`, then retry.\n\n{}\n\nTried: {command}",
                    checkout_directory_notice(&directory),
                )
            } else {
                format!(
                    "failed to run {command}: {error}\n\n{}",
                    checkout_directory_notice(&directory),
                )
            }
        })?;

    let output_text = command_output_text(&output.stdout, &output.stderr);
    if !output.status.success() {
        let detail = if output_text.is_empty() {
            "gh did not return any output".to_string()
        } else {
            output_text
        };
        return Err(format!(
            "{} failed.\n\n{}\n\n{}",
            command,
            checkout_directory_notice(&directory),
            truncate_text(&detail, 900),
        ));
    }

    let output = if output_text.is_empty() {
        "gh pr checkout completed successfully.".to_string()
    } else {
        truncate_text(&output_text, 900)
    };
    Ok(PrCheckoutResult {
        command,
        directory,
        output,
    })
}

fn pr_checkout_command_args(repository: &str, number: u64) -> Vec<String> {
    vec![
        "pr".to_string(),
        "checkout".to_string(),
        number.to_string(),
        "--repo".to_string(),
        repository.to_string(),
    ]
}

fn pr_checkout_command_display(args: &[String]) -> String {
    format!("gh {}", args.join(" "))
}

fn command_output_text(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout,
        (true, false) => stderr,
        (false, false) => format!("{stdout}\n{stderr}"),
    }
}

fn checkout_directory_notice(directory: &Path) -> String {
    format!("Checkout runs from {}.", directory.display())
}

fn resolve_pr_checkout_directory(
    config: &Config,
    repository: &str,
) -> std::result::Result<PathBuf, String> {
    if let Some(directory) = configured_local_dir_for_repo(config, repository) {
        ensure_directory_tracks_repo(&directory, repository).map_err(|error| {
            format!(
                "Configured local_dir for {repository} cannot be used.\n\n{error}\n\nSet [[repos]].local_dir to a checkout whose git remote points at {repository}."
            )
        })?;
        return Ok(directory);
    }

    let cwd = std::env::current_dir().map_err(|error| {
        format!(
            "Could not inspect the current working directory for {repository}: {error}\n\nSet [[repos]].local_dir for this repository."
        )
    })?;
    ensure_directory_tracks_repo(&cwd, repository).map_err(|error| {
        format!(
            "No local checkout found for {repository}.\n\n{error}\n\nLaunch ghr inside a checkout whose git remote points at {repository}, or set [[repos]].local_dir for this repository."
        )
    })?;
    Ok(cwd)
}

fn configured_local_dir_for_repo(config: &Config, repository: &str) -> Option<PathBuf> {
    config
        .repos
        .iter()
        .find(|repo| repo.repo.eq_ignore_ascii_case(repository))
        .and_then(|repo| repo.local_dir.as_deref())
        .map(str::trim)
        .filter(|local_dir| !local_dir.is_empty())
        .map(expand_user_path)
}

fn expand_user_path(value: &str) -> PathBuf {
    if value == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from(value));
    }
    if let Some(rest) = value.strip_prefix("~/")
        && let Some(home) = home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(value)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(::dirs::home_dir)
}

fn ensure_directory_tracks_repo(
    directory: &Path,
    repository: &str,
) -> std::result::Result<(), String> {
    if !directory.is_dir() {
        return Err(format!("{} is not a directory.", directory.display()));
    }
    let remotes = git_remotes_for_directory(directory)?;
    if remotes
        .iter()
        .any(|(_, repo)| repo.eq_ignore_ascii_case(repository))
    {
        return Ok(());
    }

    let remote_list = if remotes.is_empty() {
        "no GitHub remotes found".to_string()
    } else {
        remotes
            .iter()
            .map(|(remote, repo)| format!("{remote} -> {repo}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    Err(format!(
        "{} does not track {repository}; found {remote_list}.",
        directory.display()
    ))
}

fn git_remotes_for_directory(
    directory: &Path,
) -> std::result::Result<Vec<(String, String)>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .arg("remote")
        .output()
        .map_err(|error| {
            format!(
                "failed to run git remote in {}: {error}",
                directory.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "{} is not a usable git checkout: {}",
            directory.display(),
            command_output_text(&output.stdout, &output.stderr)
        ));
    }

    let mut remotes = Vec::new();
    let names = String::from_utf8_lossy(&output.stdout);
    for remote in names
        .lines()
        .map(str::trim)
        .filter(|remote| !remote.is_empty())
    {
        if let Some(repo) = git_remote_repo(directory, remote) {
            remotes.push((remote.to_string(), repo));
        }
    }
    Ok(remotes)
}

fn git_remote_repo(directory: &Path, remote: &str) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(["remote", "get-url", remote])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8(output.stdout).ok()?;
    github_repo_from_remote_url(url.trim())
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

    if app.comment_search_active {
        app.handle_comment_search_key(key, area);
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
    if handle_mouse_or_mark_key(app, key) {
        return false;
    }
    if is_diff_key(key) {
        app.show_diff();
        return false;
    }

    match key.code {
        KeyCode::Char('q') if app.details_mode == DetailsMode::Diff => {
            app.leave_diff();
            return false;
        }
        KeyCode::Char('q') => return true,
        KeyCode::Char('?') => app.show_help_dialog(),
        KeyCode::Char('r') => trigger_refresh(app, config, store, tx),
        KeyCode::Char('S') => app.start_global_search_input(),
        KeyCode::Tab => app.move_focused_tab_group(1),
        KeyCode::BackTab => app.move_focused_tab_group(-1),
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
        FocusTarget::List if app.details_mode == DetailsMode::Diff => {
            handle_diff_file_list_key(app, key, config, area)
        }
        FocusTarget::List => match key.code {
            KeyCode::Esc if !app.search_query.is_empty() => app.clear_search(),
            KeyCode::Esc if app.is_global_search_results_view() => {
                app.leave_global_search_results()
            }
            KeyCode::Esc => {}
            KeyCode::Char('/') => app.start_search(),
            KeyCode::Down | KeyCode::Char('j') => {
                app.move_selection(1);
                app.mark_current_notification_read(store, tx);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.move_selection(-1);
                app.mark_current_notification_read(store, tx);
            }
            KeyCode::PageDown | KeyCode::Char('d') => {
                app.move_selection(list_page_delta(app, area, 1));
                app.mark_current_notification_read(store, tx);
            }
            KeyCode::PageUp | KeyCode::Char('u') => {
                app.move_selection(list_page_delta(app, area, -1));
                app.mark_current_notification_read(store, tx);
            }
            KeyCode::Char('g') => {
                app.set_selection(0);
                app.mark_current_notification_read(store, tx);
            }
            KeyCode::Char('G') => {
                app.select_last();
                app.mark_current_notification_read(store, tx);
            }
            KeyCode::Char('[') => start_section_page_load(app, config, store, tx, -1),
            KeyCode::Char(']') => start_section_page_load(app, config, store, tx, 1),
            KeyCode::Char('M') => app.start_pr_action_dialog(PrAction::Merge),
            KeyCode::Char('C') => app.start_pr_action_dialog(PrAction::Close),
            KeyCode::Char('A') => app.start_pr_action_dialog(PrAction::Approve),
            KeyCode::Char('X') => app.start_pr_checkout_dialog(config),
            KeyCode::Char('a') => app.start_new_comment_dialog(),
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
            KeyCode::Char('C') => app.start_pr_action_dialog(PrAction::Close),
            KeyCode::Char('A') => app.start_pr_action_dialog(PrAction::Approve),
            KeyCode::Char('X') => app.start_pr_checkout_dialog(config),
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
            KeyCode::Char('e') if app.details_mode == DetailsMode::Conversation => {
                app.start_edit_selected_comment_dialog()
            }
            KeyCode::Char('n') if app.details_mode == DetailsMode::Diff => {
                app.page_diff_lines(1, area)
            }
            KeyCode::Char('p') if app.details_mode == DetailsMode::Diff => {
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
            KeyCode::Char('n') => app.move_comment_in_view(1, area),
            KeyCode::Char('p') => app.move_comment_in_view(-1, area),
            KeyCode::Enter if app.details_mode == DetailsMode::Conversation => {
                app.toggle_selected_comment_expanded()
            }
            KeyCode::Down | KeyCode::Char('j') => app.scroll_details(1),
            KeyCode::Up | KeyCode::Char('k') => app.scroll_details(-1),
            KeyCode::PageDown | KeyCode::Char('d') => app.scroll_details(8),
            KeyCode::PageUp | KeyCode::Char('u') => app.scroll_details(-8),
            KeyCode::Char('g') => app.details_scroll = 0,
            KeyCode::Char('G') => app.scroll_details_to_bottom(area),
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

fn handle_mouse_or_mark_key(app: &mut AppState, key: KeyEvent) -> bool {
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

fn handle_diff_file_list_key(
    app: &mut AppState,
    key: KeyEvent,
    config: &Config,
    area: Option<Rect>,
) {
    match key.code {
        KeyCode::Esc => app.focus_details(),
        KeyCode::Char('c') => app.start_review_comment_dialog(),
        KeyCode::Char('a') => app.start_new_comment_dialog(),
        KeyCode::Down | KeyCode::Char('j') => app.move_diff_file(1),
        KeyCode::Up | KeyCode::Char('k') => app.move_diff_file(-1),
        KeyCode::PageDown | KeyCode::Char('d') => {
            app.move_diff_file(diff_file_page_delta(app, area, 1));
        }
        KeyCode::PageUp | KeyCode::Char('u') => {
            app.move_diff_file(diff_file_page_delta(app, area, -1));
        }
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
        KeyCode::Char('C') => app.start_pr_action_dialog(PrAction::Close),
        KeyCode::Char('A') => app.start_pr_action_dialog(PrAction::Approve),
        KeyCode::Char('X') => app.start_pr_checkout_dialog(config),
        KeyCode::Enter => app.focus_details(),
        _ => {}
    }
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
        start_refresh(
            config.clone(),
            store.clone(),
            tx.clone(),
            RefreshPriority::User,
        );
    }
}

fn save_ui_state(app: &mut AppState, paths: &Paths) {
    if let Err(error) = app.ui_state().save(&paths.state_path) {
        let message = error.to_string();
        warn!(error = %message, "failed to save ui state");
        app.status = format!("layout save failed: {message}");
    }
}

#[cfg(test)]
fn handle_mouse(app: &mut AppState, mouse: MouseEvent, area: Rect) -> bool {
    handle_mouse_with_sync(app, mouse, area, None, None)
}

fn handle_mouse_with_sync(
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
    if let Some(dialog) = &app.pr_action_dialog {
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && let Some(url) = pr_action_dialog_link_at(dialog, area, mouse.column, mouse.row)
        {
            app.open_url(&url);
        }
        return false;
    }
    if let Some(dialog) = &app.comment_dialog {
        let dialog_area = comment_dialog_area(dialog, area);
        if rect_contains(dialog_area, mouse.column, mouse.row) {
            match mouse.kind {
                MouseEventKind::ScrollDown => {
                    app.scroll_comment_dialog(MOUSE_COMMENT_SCROLL_LINES as i16, Some(area))
                }
                MouseEventKind::ScrollUp => {
                    app.scroll_comment_dialog(-(MOUSE_COMMENT_SCROLL_LINES as i16), Some(area))
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
    let layout = MouseLayout {
        view_tabs: page[0],
        section_tabs: page[1],
        table: body[0],
        details: details_area,
    };

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if splitter_contains(
                body_area,
                layout.table,
                layout.details,
                mouse.column,
                mouse.row,
            ) {
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
        MouseEventKind::Moved if rect_contains(layout.table, mouse.column, mouse.row) => {
            handle_table_hover(app, mouse, layout.table, store, tx);
        }
        _ => {}
    }

    false
}

fn handle_startup_dialog_mouse(
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

#[derive(Debug, Clone, Copy)]
struct MouseLayout {
    view_tabs: Rect,
    section_tabs: Rect,
    table: Rect,
    details: Rect,
}

fn handle_left_click(
    app: &mut AppState,
    mouse: MouseEvent,
    layout: MouseLayout,
    store: Option<&SnapshotStore>,
    tx: Option<&UnboundedSender<AppMsg>>,
) {
    if let Some(view) = view_tab_at(app, layout.view_tabs, mouse.column, mouse.row) {
        app.switch_view(view);
        app.focus = FocusTarget::Ghr;
        app.search_active = false;
        app.comment_search_active = false;
        app.global_search_active = false;
        app.status = "ghr focused".to_string();
        return;
    }

    if let Some(section_index) = section_tab_at(app, layout.section_tabs, mouse.column, mouse.row) {
        app.select_section(section_index);
        return;
    }

    if rect_contains(layout.table, mouse.column, mouse.row) {
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
    app.focus = FocusTarget::Details;
    app.search_active = false;
    app.comment_search_active = false;
    app.global_search_active = false;

    if app.details_mode == DetailsMode::Diff
        && let Some(diff_line) = document.diff_line_at(line_index)
    {
        app.handle_diff_line_click(diff_line, None);
        return;
    }
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
    app.comment_search_active = false;
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

fn handle_list_scroll(app: &mut AppState, area: Rect, delta: isize) {
    app.focus = FocusTarget::List;
    app.search_active = false;
    app.comment_search_active = false;
    app.global_search_active = false;
    if app.details_mode == DetailsMode::Diff {
        app.move_diff_file(delta);
    } else {
        app.scroll_list_viewport(area, delta);
    }
}

fn mouse_list_scroll_delta(app: &AppState, direction: isize) -> isize {
    if app.details_mode == DetailsMode::Diff {
        return direction
            .signum()
            .saturating_mul(MOUSE_DIFF_FILE_SCROLL_LINES as isize);
    }

    direction.saturating_mul(MOUSE_LIST_SCROLL_LINES as isize)
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

fn diff_file_page_delta(app: &AppState, area: Option<Rect>, direction: isize) -> isize {
    let rows = area
        .map(|area| {
            let body = body_areas_with_ratio(page_areas(area)[2], app.list_width_percent);
            usize::from(block_inner(body[0]).height.max(1))
        })
        .unwrap_or(10);
    direction.saturating_mul(rows as isize)
}

fn diff_line_page_delta(app: &AppState, area: Option<Rect>, direction: isize) -> isize {
    let rows = area
        .map(|area| usize::from(block_inner(details_area_for(app, area)).height.max(1)))
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

fn handle_table_click(
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

fn handle_table_hover(
    app: &mut AppState,
    mouse: MouseEvent,
    area: Rect,
    store: Option<&SnapshotStore>,
    tx: Option<&UnboundedSender<AppMsg>>,
) {
    if app.details_mode == DetailsMode::Diff {
        return;
    }

    let Some(position) = table_row_at(app, area, mouse.row) else {
        return;
    };

    if let Some(thread_id) = app.notification_thread_id_at_position(position) {
        mark_notification_read_if_possible(app, thread_id, store, tx);
    }
}

fn mark_current_notification_read_if_possible(
    app: &mut AppState,
    store: Option<&SnapshotStore>,
    tx: Option<&UnboundedSender<AppMsg>>,
) {
    if let (Some(store), Some(tx)) = (store, tx) {
        app.mark_current_notification_read(store, tx);
    }
}

fn mark_notification_read_if_possible(
    app: &mut AppState,
    thread_id: String,
    store: Option<&SnapshotStore>,
    tx: Option<&UnboundedSender<AppMsg>>,
) {
    if let (Some(store), Some(tx)) = (store, tx) {
        app.mark_notification_read(thread_id, store, tx);
    }
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

    let offset = app.current_list_scroll_offset(filtered_len, usize::from(visible_rows));
    let position = offset + usize::from(row - data_start);
    (position < filtered_len).then_some(position)
}

fn diff_file_row_at(app: &AppState, area: Rect, row: u16) -> Option<usize> {
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

fn table_visible_rows(area: Rect) -> u16 {
    block_inner(area).height.saturating_sub(TABLE_HEADER_HEIGHT)
}

fn table_visible_range(offset: usize, visible_rows: usize, len: usize) -> Option<(usize, usize)> {
    if visible_rows == 0 || len == 0 {
        return None;
    }

    let offset = offset.min(max_table_viewport_offset(len, visible_rows));
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

fn max_table_viewport_offset(len: usize, visible_rows: usize) -> usize {
    if visible_rows == 0 {
        0
    } else {
        len.saturating_sub(visible_rows)
    }
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
        draw_help_dialog(frame, area);
    } else if let Some(dialog) = &app.pr_action_dialog {
        draw_pr_action_dialog(frame, dialog, app.pr_action_running, area);
    } else if let Some(dialog) = &app.comment_dialog {
        draw_comment_dialog(frame, dialog, area);
    } else if app.global_search_running {
        draw_global_search_loading_dialog(frame, app, area);
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
    let title = if ghr_focused { "[Focus] ghr" } else { "ghr" };

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
        .highlight_style(active_view_tab_style());
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
                .title(Span::styled(title, border_style)),
        )
        .style(Style::default().fg(Color::Gray))
        .highlight_style(active_section_tab_style());
    frame.render_widget(tabs, area);
}

fn active_view_tab_style() -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(Color::LightCyan)
        .add_modifier(Modifier::BOLD)
}

fn active_section_tab_style() -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(Color::LightYellow)
        .add_modifier(Modifier::BOLD)
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
            let row_style = if app.item_has_unseen_details(item) {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Row::new(vec![
                item.repo.clone(),
                item.number
                    .map(|number| format!("#{number}"))
                    .unwrap_or_default(),
                item.title.clone(),
                relative_time(item.updated_at),
                item_meta(item),
            ])
            .style(row_style)
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
    let visible_rows = usize::from(table_visible_rows(area));
    let table_offset = app.current_list_scroll_offset(filtered_indices.len(), visible_rows);
    if let Some((start, end)) =
        table_visible_range(table_offset, visible_rows, filtered_indices.len())
    {
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
}

fn draw_diff_files(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let files_focused = app.focus == FocusTarget::List;
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
    } else if files_focused {
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
            Style::default().fg(Color::Green),
            BorderType::Plain,
            Style::default().fg(Color::White).bg(Color::DarkGray),
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
            let entries = diff_tree_entries(diff);
            if let Some(item_id) = app.current_item().map(|item| item.id.as_str()) {
                let selected_file = app.selected_diff_file_index_for(item_id, diff);
                selected_row = diff_tree_row_index_for_file(&entries, selected_file);
            }
            entries
                .into_iter()
                .map(|entry| {
                    let indent = "  ".repeat(entry.depth);
                    let (marker, style) = if entry.file_index.is_some() {
                        (" ", Style::default().fg(Color::White))
                    } else {
                        ("▾ ", Style::default().fg(Color::Green))
                    };
                    Row::new(vec![
                        format!("{indent}{marker}{}", entry.label),
                        entry.stats,
                    ])
                    .style(style)
                })
                .collect::<Vec<_>>()
        }
        Some(DiffState::Error(error)) => {
            title = "Files | error".to_string();
            vec![
                Row::new(vec![compact_error_label(error), String::new()])
                    .style(Style::default().fg(Color::LightRed)),
            ]
        }
        Some(DiffState::Loading) | None => {
            title = "Files | loading".to_string();
            vec![
                Row::new(vec!["loading diff...".to_string(), String::new()])
                    .style(Style::default().fg(Color::Gray)),
            ]
        }
    };

    let title = focus_panel_title("List", &title, files_focused);
    let table = Table::new(rows, [Constraint::Min(12), Constraint::Length(12)])
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

fn active_list_input_prompt(app: &AppState) -> Option<(String, Color)> {
    if app.global_search_active {
        let scope = app
            .global_search_scope
            .clone()
            .or_else(|| app.current_repo_scope())
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

    if app.is_global_search_results_view() {
        return Some((
            format!("Search results: {}  Esc back", app.global_search_query),
            Color::LightMagenta,
        ));
    }

    if app.search_active {
        return Some((
            format!(
                "Local Search: /{}_  Enter search  Esc clear",
                app.search_query
            ),
            Color::Yellow,
        ));
    }

    None
}

fn active_details_input_prompt(app: &AppState) -> Option<(String, Color)> {
    if app.comment_search_active {
        return Some((
            format!(
                "Comment Search: /{}_  Enter keep  Esc clear",
                app.comment_search_query
            ),
            Color::Yellow,
        ));
    }

    None
}

fn draw_details(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let details_focused = app.focus == FocusTarget::Details;
    let raw_title = active_details_input_prompt(app)
        .map(|(prompt, _)| format!("{} {prompt}", details_title()))
        .unwrap_or_else(|| details_title().to_string());
    let title = focus_panel_title("Details", &raw_title, details_focused);
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
        .scroll((app.details_scroll, 0));
    frame.render_widget(details, area);
}

fn details_title() -> &'static str {
    "Details:"
}

fn focus_panel_title(_label: &str, title: &str, focused: bool) -> String {
    if focused {
        format!("[Focus] {title}")
    } else {
        title.to_string()
    }
}

fn pull_request_changes_url(item: &WorkItem) -> String {
    match item.number {
        Some(number) if !item.repo.trim().is_empty() => {
            format!("https://github.com/{}/pull/{number}/changes", item.repo)
        }
        _ => format!("{}/changes", item.url.trim_end_matches('/')),
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
    let search = if app.comment_search_active {
        Some(format!("comment-search: /{}_", app.comment_search_query))
    } else if app.focus == FocusTarget::Details
        && app.details_mode == DetailsMode::Conversation
        && !app.comment_search_query.is_empty()
    {
        Some(format!("comment-search: /{}", app.comment_search_query))
    } else if app.global_search_active {
        Some(format!("repo-search: S{}_", app.global_search_query))
    } else if app.global_search_running {
        Some("repo search running".to_string())
    } else if app.search_active {
        Some(format!("local-search: /{}_", app.search_query))
    } else if app.search_query.is_empty() {
        None
    } else {
        Some(format!("local-search: /{}", app.search_query))
    };
    let (mouse, text_selection_state) = footer_mouse_shortcut(app);

    let mut spans = Vec::new();
    push_footer_focus_shortcuts(&mut spans, app);

    push_footer_separator(&mut spans);
    push_footer_pair(&mut spans, "1-4", "focus", Color::Cyan);
    push_footer_pair(&mut spans, "?", "help", Color::Yellow);
    push_footer_pair(&mut spans, "S", "repo", Color::Yellow);
    push_footer_pair(&mut spans, "r", "refresh", Color::Yellow);
    push_footer_pair(&mut spans, "o", "open", Color::Yellow);
    if let Some(mouse) = mouse {
        push_footer_pair(&mut spans, "m", mouse, Color::LightBlue);
    }
    let q_action = if app.details_mode == DetailsMode::Diff {
        "back"
    } else {
        "quit"
    };
    push_footer_pair(&mut spans, "q", q_action, Color::Yellow);

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

fn footer_mouse_shortcut(app: &AppState) -> (Option<&'static str>, Option<&'static str>) {
    if !app.mouse_capture_enabled {
        return (Some("restore mouse"), Some("text-select: drag copy"));
    }
    if app.details_mode == DetailsMode::Diff {
        return (None, None);
    }
    (Some("text-select"), None)
}

fn push_footer_focus_shortcuts(spans: &mut Vec<Span<'static>>, app: &AppState) {
    match app.focus {
        FocusTarget::Ghr => {
            push_footer_context(spans, "ghr", "tabs");
            push_footer_pair(spans, "tab/h/l", "switch", Color::Cyan);
            push_footer_pair(spans, "j/enter", "Sections", Color::Cyan);
            push_footer_pair(spans, "esc", "List", Color::Cyan);
        }
        FocusTarget::Sections => {
            push_footer_context(spans, "Sections", "tabs");
            push_footer_pair(spans, "tab/h/l", "switch", Color::Cyan);
            push_footer_pair(spans, "k", "ghr", Color::Cyan);
            push_footer_pair(spans, "j/enter", "List", Color::Cyan);
            push_footer_pair(spans, "esc", "List", Color::Cyan);
        }
        FocusTarget::List => {
            if app.details_mode == DetailsMode::Diff {
                push_footer_context(spans, "List", "files");
                push_footer_pair(spans, "j/k", "file", Color::Cyan);
                push_footer_pair(spans, "pg", "page", Color::Cyan);
                push_footer_pair(spans, "[ ]", "file", Color::Cyan);
                push_footer_pair(spans, "g/G", "ends", Color::Cyan);
                push_footer_pair(spans, "enter", "diff", Color::Cyan);
                push_footer_pair(spans, "c", "inline", Color::LightBlue);
                push_footer_pair(spans, "a", "comment", Color::LightBlue);
                push_footer_pair(spans, "M/C/A/X", "pr action", Color::LightMagenta);
            } else {
                push_footer_context(spans, "List", "items");
                push_footer_pair(spans, "j/k", "move", Color::Cyan);
                push_footer_pair(spans, "pg", "page", Color::Cyan);
                push_footer_pair(spans, "[ ]", "results", Color::Cyan);
                push_footer_pair(spans, "g/G", "ends", Color::Cyan);
                push_footer_pair(spans, "enter", "Details", Color::Cyan);
                push_footer_pair(spans, "/", "search", Color::Yellow);
                if app.is_global_search_results_view() {
                    push_footer_pair(spans, "esc", "back", Color::Cyan);
                }
                push_footer_pair(spans, "v", "diff", Color::LightMagenta);
                push_footer_pair(spans, "a", "comment", Color::LightBlue);
                push_footer_pair(spans, "M/C/A/X", "pr action", Color::LightMagenta);
            }
        }
        FocusTarget::Details => {
            let context = if app.details_mode == DetailsMode::Diff {
                "diff"
            } else {
                "content"
            };
            push_footer_context(spans, "Details", context);
            if app.details_mode == DetailsMode::Diff {
                push_footer_pair(spans, "j/k", "line", Color::Cyan);
                push_footer_pair(spans, "n/p", "page", Color::Cyan);
            } else {
                push_footer_pair(spans, "j/k", "scroll", Color::Cyan);
                push_footer_pair(spans, "pg", "page", Color::Cyan);
            }
            push_footer_pair(spans, "g/G", "top/bottom", Color::Cyan);
            if app.details_mode == DetailsMode::Diff {
                push_footer_pair(spans, "[ ]", "file", Color::LightBlue);
                push_footer_pair(spans, "m", "begin", Color::Yellow);
                push_footer_pair(spans, "e", "end", Color::Yellow);
                push_footer_pair(spans, "c", "inline", Color::LightBlue);
                push_footer_pair(spans, "a", "comment", Color::LightBlue);
                push_footer_pair(spans, "M/C/A/X", "pr action", Color::LightMagenta);
            } else {
                push_footer_pair(spans, "v", "diff", Color::LightMagenta);
                push_footer_pair(spans, "/", "search", Color::Yellow);
                push_footer_pair(spans, "n/p", "comment", Color::LightBlue);
                push_footer_pair(spans, "enter", "expand", Color::Yellow);
                push_footer_pair(spans, "c/a", "comment", Color::LightBlue);
                push_footer_pair(spans, "R", "reply", Color::LightBlue);
                push_footer_pair(spans, "e", "edit", Color::LightBlue);
                push_footer_pair(spans, "M/C/A/X", "pr action", Color::LightMagenta);
            }
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

fn modal_surface_style() -> Style {
    Style::default()
}

fn modal_text_style() -> Style {
    Style::default().fg(Color::White)
}

fn draw_startup_dialog(
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
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent))
        .style(modal_surface_style())
        .title(Span::styled(
            title,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);

    if show_ok {
        let ok = Paragraph::new("[ OK ]").alignment(Alignment::Center).style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
        );
        frame.render_widget(ok, startup_dialog_ok_area(dialog_area));
    }
}

fn startup_dialog_area(dialog: StartupDialog, area: Rect) -> Rect {
    let height = match dialog {
        StartupDialog::Initializing => 11,
        StartupDialog::Ready => 13,
    };
    centered_rect(72, height, area)
}

fn startup_dialog_ok_area(dialog_area: Rect) -> Rect {
    let width = 8.min(dialog_area.width.saturating_sub(4)).max(1);
    let x = dialog_area.x + dialog_area.width.saturating_sub(width) / 2;
    let y = dialog_area.y + dialog_area.height.saturating_sub(2);
    Rect::new(x, y, width, 1)
}

fn startup_dialog_content(
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
                    Span::styled(
                        "?",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" anytime for the shortcut reference."),
                ]),
                Line::from("Click OK or press Enter/Esc to close this dialog."),
            ],
            true,
        ),
    }
}

fn startup_loading_line(elapsed_secs: u64) -> Line<'static> {
    let dots = match elapsed_secs % 4 {
        0 => "",
        1 => ".",
        2 => "..",
        _ => "...",
    };
    Line::from(vec![
        Span::styled(
            format!("Loading{dots:<3}"),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {elapsed_secs}s"),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}

fn startup_progress_line(elapsed_secs: u64) -> Line<'static> {
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
        Span::styled(bar, Style::default().fg(Color::Cyan)),
    ])
}

fn startup_loaded_summary(app: &AppState) -> String {
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

fn draw_setup_dialog(frame: &mut Frame<'_>, dialog: SetupDialog, area: Rect) {
    let (title, lines) = setup_dialog_content(dialog);
    let dialog_area = centered_rect(90, 17, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .style(modal_surface_style())
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
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
        .style(modal_surface_style())
        .title(Span::styled(
            "Help",
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
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
    let dialog_area = pr_action_dialog_area(dialog, area);
    let number = dialog
        .item
        .number
        .map(|number| format!("#{number}"))
        .unwrap_or_else(|| "-".to_string());
    let action_label = match dialog.action {
        PrAction::Merge => "merge",
        PrAction::Close => "close",
        PrAction::Approve => "approve",
        PrAction::Checkout => "checkout",
    };
    let prompt = match dialog.action {
        PrAction::Merge => "Merge this pull request on GitHub?",
        PrAction::Close => "Close this pull request on GitHub?",
        PrAction::Approve => "Approve this pull request on GitHub?",
        PrAction::Checkout => "Checkout this pull request locally?",
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
        if dialog.action == PrAction::Checkout {
            key_value_line(
                "local dir",
                dialog
                    .checkout
                    .as_ref()
                    .map(|checkout| checkout.directory.display().to_string())
                    .unwrap_or_else(|| "-".to_string()),
            )
        } else {
            Line::from("")
        },
        if dialog.action == PrAction::Checkout {
            remote_branch_line(
                dialog
                    .checkout
                    .as_ref()
                    .and_then(|checkout| checkout.branch.as_ref()),
            )
        } else {
            Line::from("")
        },
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
        .style(modal_surface_style())
        .title(Span::styled(
            match dialog.action {
                PrAction::Merge => "Merge Pull Request",
                PrAction::Close => "Close Pull Request",
                PrAction::Approve => "Approve Pull Request",
                PrAction::Checkout => "Checkout Pull Request Locally",
            },
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
}

fn pr_action_dialog_area(dialog: &PrActionDialog, area: Rect) -> Rect {
    let dialog_height = if dialog.action == PrAction::Checkout {
        14
    } else {
        12
    };
    centered_rect(66, dialog_height, area)
}

fn remote_branch_line(branch: Option<&PullRequestBranch>) -> Line<'static> {
    let Some(branch) = branch else {
        return key_value_line("remote branch", "unavailable".to_string());
    };
    Line::from(vec![
        Span::styled("remote branch: ", Style::default().fg(Color::Gray)),
        Span::styled(pull_request_branch_label(branch), link_style()),
    ])
}

fn pr_action_dialog_link_at(
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

fn draw_message_dialog(frame: &mut Frame<'_>, dialog: &MessageDialog, area: Rect) {
    let dialog_area = centered_rect(78, 14, area);
    let footer = if dialog.auto_close_at.is_some() {
        "Auto closes shortly | Enter/Esc: close"
    } else {
        "Enter/Esc: close"
    };
    let text = format!("{}\n\n{footer}", dialog.body);
    let accent = message_dialog_accent(dialog);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent))
        .style(modal_surface_style())
        .title(Span::styled(
            dialog.title.clone(),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(text)
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
}

fn message_dialog_accent(dialog: &MessageDialog) -> Color {
    match dialog.kind {
        MessageDialogKind::Info => Color::Yellow,
        MessageDialogKind::Success => Color::LightGreen,
        MessageDialogKind::Error => Color::LightRed,
    }
}

fn draw_global_search_loading_dialog(frame: &mut Frame<'_>, app: &AppState, area: Rect) {
    let dialog_area = centered_rect(58, 9, area);
    let elapsed_secs = app
        .global_search_started_at
        .map(|started| started.elapsed().as_secs())
        .unwrap_or(0);
    let lines = global_search_loading_content(app, elapsed_secs);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::LightMagenta))
        .style(modal_surface_style())
        .title(Span::styled(
            "Searching",
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, dialog_area);
    frame.render_widget(paragraph, dialog_area);
}

fn global_search_loading_content(app: &AppState, elapsed_secs: u64) -> Vec<Line<'static>> {
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
        CommentDialogMode::New => "New Comment".to_string(),
        CommentDialogMode::Reply { author, .. } => {
            return draw_reply_dialog(frame, dialog, author, area);
        }
        CommentDialogMode::Edit { .. } => "Edit Comment".to_string(),
        CommentDialogMode::Review { target } => {
            format!("Review {}", target.location_label())
        }
    };
    draw_comment_editor(frame, &title, dialog, area);
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
        .style(modal_surface_style())
        .title(Span::styled(
            title.to_string(),
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .style(modal_text_style());

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
                Line::from("Install GitHub CLI: https://cli.github.com/"),
                command_line("macOS: brew install gh"),
                command_line("Debian/Ubuntu: sudo apt install gh"),
                Line::from("Linux package details:"),
                Line::from("https://github.com/cli/cli/blob/trunk/docs/install_linux.md"),
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
        help_key_line("q in diff", "return to the state before opening diff"),
        help_key_line("r", "refresh from GitHub"),
        help_key_line("Tab / Shift+Tab", "switch the focused tab group"),
        help_key_line("1 / 2 / 3 / 4", "focus ghr / Sections / List / Details"),
        help_key_line("/", "search the current list or Details comments"),
        help_key_line("S", "search PRs and issues in the current repo"),
        help_key_line(
            "Esc in Search results",
            "return to the previous default list",
        ),
        help_key_line("m", "toggle mouse text selection mode"),
        help_key_line("Esc", "leave details or clear search"),
        Line::from(""),
        help_heading("ghr and Sections"),
        help_key_line(
            "Tab / Shift+Tab / h/l or Left/Right",
            "switch the focused tab group",
        ),
        help_key_line(
            "j/k or Up/Down",
            "move focus between ghr, Sections, and List",
        ),
        Line::from(""),
        help_heading("List"),
        help_key_line("j/k or Up/Down", "move selection"),
        help_key_line("PgDown/PgUp", "move by visible page"),
        help_key_line("[ / ]", "load previous / next GitHub result page"),
        help_key_line("g / G", "first / last item"),
        help_key_line("Enter or 4", "focus Details"),
        help_key_line("o", "open selected item in browser"),
        help_key_line("S", "search PRs and issues in the current repo"),
        help_key_line("v", "show pull request diff"),
        help_key_line("M", "open PR merge confirmation"),
        help_key_line("C", "open PR close confirmation"),
        help_key_line("A", "open PR approve confirmation"),
        help_key_line("X", "open local PR checkout confirmation"),
        help_key_line("a", "add a new issue or PR comment"),
        Line::from(""),
        help_heading("Diff Files"),
        help_key_line("3", "focus the changed-file list"),
        help_key_line("j/k or Up/Down", "choose a changed file"),
        help_key_line("PgDown/PgUp", "move by visible file page"),
        help_key_line("[ / ]", "previous / next changed file"),
        help_key_line("Enter or 4", "focus the file diff"),
        help_key_line("c", "add review comment on selected diff line"),
        help_key_line("a", "add a normal PR comment"),
        Line::from(""),
        help_heading("Details"),
        help_key_line("j/k or Up/Down", "scroll details or select diff line"),
        help_key_line("/", "search loaded comments by keyword"),
        help_key_line("n / p in conversation", "focus next / previous comment"),
        help_key_line("n / p in diff", "page down / page up"),
        help_key_line(
            "Enter in conversation",
            "expand or collapse a long focused comment",
        ),
        help_key_line("PgDown/PgUp or d/u", "scroll details by page"),
        help_key_line("g / G", "scroll details to top / bottom"),
        help_key_line("v", "show PR diff"),
        help_key_line("[ / ]", "jump previous / next diff file"),
        help_key_line("m in diff", "begin a review range"),
        help_key_line("e in diff", "end the review range"),
        help_key_line("single click in diff", "begin or move a review range"),
        help_key_line("double click in diff", "end the review range"),
        help_key_line("c in diff", "add review comment on selected diff line"),
        help_key_line("a in diff", "add a normal PR comment"),
        help_key_line("c / a", "add a new comment"),
        help_key_line("R", "reply to focused comment"),
        help_key_line("e", "edit focused comment when it is yours"),
        help_key_line("S", "search PRs and issues in the current repo"),
        help_key_line("M", "open PR merge confirmation"),
        help_key_line("C", "open PR close confirmation"),
        help_key_line("A", "open PR approve confirmation"),
        help_key_line("X", "open local PR checkout confirmation"),
        help_key_line("o", "open selected item in browser"),
        Line::from(""),
        help_heading("Pull Request Confirmation"),
        help_key_line("y / Enter", "run the confirmed PR action"),
        help_key_line(
            "X action",
            "runs gh pr checkout from the matching local checkout",
        ),
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
        help_key_line("click list row", "select item or diff file"),
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

#[derive(Debug, Clone)]
struct DetailsDocument {
    lines: Vec<Line<'static>>,
    links: Vec<LinkRegion>,
    actions: Vec<ActionRegion>,
    comments: Vec<CommentRegion>,
    diff_files: Vec<usize>,
    diff_lines: Vec<DiffLineRegion>,
    selected_diff_line: Option<usize>,
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

    fn comment_region(&self, index: usize) -> Option<&CommentRegion> {
        self.comments.iter().find(|comment| comment.index == index)
    }

    fn diff_line_at(&self, line: usize) -> Option<usize> {
        self.diff_lines
            .iter()
            .find(|diff_line| diff_line.line == line)
            .map(|diff_line| diff_line.review_index)
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

impl CommentRegion {
    fn focus_line(&self) -> usize {
        self.start_line
            .saturating_add(1)
            .min(self.end_line.saturating_sub(1))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CommentDisplayEntry {
    index: usize,
    depth: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CommentCollapseState {
    long: bool,
    collapsed: bool,
    line_count: usize,
    char_count: usize,
}

#[derive(Debug, Clone)]
struct DiffLineRegion {
    line: usize,
    review_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiffTreeEntry {
    file_index: Option<usize>,
    label: String,
    stats: String,
    depth: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum DiffReviewSide {
    Left,
    Right,
}

impl DiffReviewSide {
    fn as_api_value(self) -> &'static str {
        match self {
            Self::Left => "LEFT",
            Self::Right => "RIGHT",
        }
    }

    fn short_label(self) -> &'static str {
        match self {
            Self::Left => "L",
            Self::Right => "R",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiffReviewTarget {
    path: String,
    line: usize,
    side: DiffReviewSide,
    start_line: Option<usize>,
    start_side: Option<DiffReviewSide>,
    preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DiffInlineCommentKey {
    path: String,
    line: usize,
    side: DiffReviewSide,
}

impl From<&DiffReviewTarget> for DiffInlineCommentKey {
    fn from(target: &DiffReviewTarget) -> Self {
        Self {
            path: target.path.clone(),
            line: target.line,
            side: target.side,
        }
    }
}

struct DiffRenderContext<'a> {
    item_id: &'a str,
    comments: Option<&'a [CommentPreview]>,
    expanded_comments: &'a HashSet<String>,
    details_focused: bool,
    selected_comment_index: usize,
    selected_file: usize,
    selected_line: usize,
    selected_range: Option<(usize, usize)>,
}

impl DiffReviewTarget {
    fn location_label(&self) -> String {
        match (self.start_line, self.start_side) {
            (Some(start_line), Some(start_side)) if start_side != self.side => {
                format!(
                    "{}:{start_line}{}-{}{}",
                    self.path,
                    start_side.short_label(),
                    self.line,
                    self.side.short_label()
                )
            }
            (Some(start_line), _) if start_line != self.line => {
                format!("{}:{start_line}-{}", self.path, self.line)
            }
            _ => format!("{}:{}", self.path, self.line),
        }
    }

    fn is_range(&self) -> bool {
        self.start_line.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DetailAction {
    ReplyComment(usize),
    EditComment(usize),
    ToggleCommentExpanded(usize),
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
    kind: MarkdownBlockKind,
    gap_before: bool,
    segments: Vec<DetailSegment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkdownBlockKind {
    Text,
    ListItem,
    TableRow,
    Code { language: CodeLanguage },
}

#[derive(Debug, Clone, Default)]
struct MarkdownTable {
    rows: Vec<MarkdownTableRow>,
}

#[derive(Debug, Clone)]
struct MarkdownTableRow {
    cells: Vec<Vec<DetailSegment>>,
    header: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodeLanguage {
    Rust,
    Plain,
    Other,
}

impl CodeLanguage {
    fn from_code_block(kind: &CodeBlockKind<'_>) -> Self {
        match kind {
            CodeBlockKind::Fenced(info) if is_rust_code_info(info) => Self::Rust,
            CodeBlockKind::Fenced(info) if is_plain_code_info(info) => Self::Plain,
            CodeBlockKind::Indented => Self::Plain,
            _ => Self::Other,
        }
    }
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

struct MarkdownRenderOptions {
    prefix: Vec<DetailSegment>,
    right_padding: usize,
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
                diff_files: Vec::new(),
                diff_lines: Vec::new(),
                selected_diff_line: None,
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

    fn mark_diff_file(&mut self) {
        self.document.diff_files.push(self.document.lines.len());
    }

    fn mark_diff_line(&mut self, review_index: usize, selected: bool) {
        let line = self.document.lines.len();
        self.document
            .diff_lines
            .push(DiffLineRegion { line, review_index });
        if selected {
            self.document.selected_diff_line = Some(line);
        }
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
        self.push_styled_key_value(key, vec![DetailSegment::raw(value.into())]);
    }

    fn push_styled_key_value(&mut self, key: &str, value: Vec<DetailSegment>) {
        let mut segments = vec![DetailSegment::styled(
            format!("{key}: "),
            Style::default().fg(Color::Gray),
        )];
        segments.extend(value);
        self.push_wrapped_limited(segments, 1);
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
            if block.gap_before
                && !self.push_markdown_gap(prefix.as_slice(), &mut emitted, max_lines)
            {
                return;
            }
            match block.kind {
                MarkdownBlockKind::Text | MarkdownBlockKind::ListItem => {
                    if !self.push_wrapped_prefixed(
                        &block.segments,
                        prefix.as_slice(),
                        &mut emitted,
                        max_lines,
                    ) {
                        return;
                    }
                }
                MarkdownBlockKind::Code { .. } | MarkdownBlockKind::TableRow => {
                    if !self.push_preformatted_prefixed(
                        &block.segments,
                        prefix.as_slice(),
                        &mut emitted,
                        max_lines,
                    ) {
                        return;
                    }
                }
            }
        }
    }

    #[cfg(test)]
    fn push_markdown_block_indented(
        &mut self,
        text: &str,
        empty_message: &str,
        max_lines: usize,
        max_chars: usize,
        left_padding: usize,
        right_padding: usize,
    ) {
        self.push_markdown_block_prefixed(
            text,
            empty_message,
            max_lines,
            max_chars,
            MarkdownRenderOptions {
                prefix: padding_prefix(left_padding),
                right_padding,
            },
        );
    }

    fn push_markdown_block_prefixed(
        &mut self,
        text: &str,
        empty_message: &str,
        max_lines: usize,
        max_chars: usize,
        options: MarkdownRenderOptions,
    ) {
        let text = truncate_text(&normalize_text(text), max_chars);
        if text.trim().is_empty() {
            let segments = vec![DetailSegment::raw(empty_message.to_string())];
            self.push_prefixed_wrapped_limited(segments, options.prefix, options.right_padding, 1);
            return;
        }

        let blocks = markdown_blocks(&text);
        let original_width = self.width;
        self.width = reserved_width(self.width, options.right_padding);
        let mut emitted = 0;
        for block in blocks {
            let mut line_prefix = options.prefix.clone();
            line_prefix.extend(quote_prefix(block.quote_depth));
            if block.gap_before
                && !self.push_markdown_gap(line_prefix.as_slice(), &mut emitted, max_lines)
            {
                break;
            }
            match block.kind {
                MarkdownBlockKind::Text | MarkdownBlockKind::ListItem => {
                    if !self.push_wrapped_prefixed(
                        &block.segments,
                        line_prefix.as_slice(),
                        &mut emitted,
                        max_lines,
                    ) {
                        break;
                    }
                }
                MarkdownBlockKind::Code { .. } | MarkdownBlockKind::TableRow => {
                    if !self.push_preformatted_prefixed(
                        &block.segments,
                        line_prefix.as_slice(),
                        &mut emitted,
                        max_lines,
                    ) {
                        break;
                    }
                }
            }
        }
        self.width = original_width;
    }

    fn push_markdown_gap(
        &mut self,
        prefix: &[DetailSegment],
        emitted: &mut usize,
        max_lines: usize,
    ) -> bool {
        if *emitted >= max_lines {
            self.push_plain("...");
            return false;
        }
        self.push_line(prefix.to_vec());
        *emitted += 1;
        true
    }

    fn push_wrapped_limited(&mut self, segments: Vec<DetailSegment>, max_lines: usize) {
        let mut emitted = 0;
        let _ = self.push_wrapped(&segments, &mut emitted, max_lines);
    }

    fn push_prefixed_wrapped_limited(
        &mut self,
        segments: Vec<DetailSegment>,
        prefix: Vec<DetailSegment>,
        right_padding: usize,
        max_lines: usize,
    ) {
        let original_width = self.width;
        self.width = reserved_width(self.width, right_padding);
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

    fn push_preformatted_prefixed(
        &mut self,
        segments: &[DetailSegment],
        prefix: &[DetailSegment],
        emitted: &mut usize,
        max_lines: usize,
    ) -> bool {
        let prefix_width: usize = prefix
            .iter()
            .map(|segment| display_width(&segment.text))
            .sum();
        let prefix = if prefix_width < self.width {
            prefix
        } else {
            &[]
        };
        let prefix_width = if prefix.is_empty() { 0 } else { prefix_width };
        let mut current = prefix.to_vec();
        let mut column = prefix_width;

        for segment in segments {
            for ch in segment.text.chars() {
                if column >= self.width {
                    if !self.flush_wrapped_line(&mut current, emitted, max_lines) {
                        return false;
                    }
                    current = prefix.to_vec();
                    column = prefix_width;
                }
                push_char_segment(&mut current, segment, ch);
                column += display_width_char(ch);
            }
        }

        self.flush_wrapped_line(&mut current, emitted, max_lines)
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
    if app.details_mode == DetailsMode::Diff {
        return build_diff_document(app, width);
    }
    build_conversation_document(app, width)
}

fn build_conversation_document(app: &AppState, width: u16) -> DetailsDocument {
    let mut builder = DetailsBuilder::new(width);
    let Some(item) = app.current_item() else {
        builder.push_plain("No item selected");
        return builder.finish();
    };

    if matches!(item.kind, ItemKind::PullRequest) {
        push_details_mode_tabs(&mut builder, DetailsMode::Conversation);
        builder.push_blank();
    }

    builder.push_wrapped_limited(
        vec![DetailSegment::styled(
            item.title.clone(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )],
        3,
    );

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
    let mut action_meta = Vec::new();
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
    if let Some(comments) = details_comment_count(app, item) {
        secondary_meta.push(("comments", vec![DetailSegment::raw(comments.to_string())]));
    }
    if let Some(reason) = useful_meta_value(item.reason.as_deref()) {
        secondary_meta.push(("reason", vec![DetailSegment::raw(reason.to_string())]));
    }
    if matches!(item.kind, ItemKind::PullRequest) {
        let (action_segments, note) = action_hint_segments(app.action_hints.get(&item.id));
        secondary_meta.push((
            "branch",
            branch_hint_segments(app.action_hints.get(&item.id)),
        ));
        action_meta.push(("action", action_segments));
        action_meta.push((
            "checks",
            check_hint_segments(app.action_hints.get(&item.id)),
        ));
        action_note = note;
    }
    if !secondary_meta.is_empty() {
        builder.push_meta_line(secondary_meta);
    }
    if !action_meta.is_empty() {
        builder.push_meta_line(action_meta);
    }
    if let Some(note) = action_note {
        builder.push_styled_key_value("action note", action_note_segments(&note));
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
    builder.push_blank();
    builder.push_markdown_block(
        item.body.as_deref().unwrap_or(""),
        "No description.",
        22,
        2_400,
    );

    if matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest) {
        builder.push_blank();
        builder.push_heading("Recent Comments");
        builder.push_blank();
        match app.details.get(&item.id) {
            Some(DetailState::Loading) => {
                builder.push_plain("loading comments...");
            }
            Some(DetailState::Loaded(comments)) if comments.is_empty() => {
                builder.push_plain("No comments.");
            }
            Some(DetailState::Loaded(comments)) => {
                let comment_search_query = app.comment_search_query.trim();
                let search_matches = (!comment_search_query.is_empty())
                    .then(|| comment_search_matches(comments, comment_search_query));
                if let Some(matches) = &search_matches {
                    builder.push_plain(format!(
                        "Comment search: {}/{} matches for /{}",
                        matches.len(),
                        comments.len(),
                        comment_search_query
                    ));
                    builder.push_blank();
                }
                for (position, entry) in comment_display_entries(comments).iter().enumerate() {
                    if position > 0 {
                        builder.push_blank();
                    }
                    let index = entry.index;
                    let comment = &comments[index];
                    let search_match = search_matches
                        .as_ref()
                        .is_some_and(|matches| matches.contains(&index));
                    let collapse = app.comment_collapse_state(&item.id, index, comment);
                    push_comment(
                        &mut builder,
                        index,
                        comment,
                        app.focus == FocusTarget::Details && index == app.selected_comment_index,
                        search_match,
                        entry.depth,
                        collapse,
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

fn build_diff_document(app: &AppState, width: u16) -> DetailsDocument {
    let mut builder = DetailsBuilder::new(width);
    let Some(item) = app.current_item() else {
        builder.push_plain("No item selected");
        return builder.finish();
    };

    if !matches!(item.kind, ItemKind::PullRequest) {
        builder.push_plain("Diff is available for pull requests only.");
        return builder.finish();
    }

    push_details_mode_tabs(&mut builder, DetailsMode::Diff);
    builder.push_blank();
    builder.push_wrapped_limited(
        vec![DetailSegment::styled(
            item.title.clone(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )],
        3,
    );
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
    ]);
    builder.push_blank();

    match app.diffs.get(&item.id) {
        Some(DiffState::Loaded(diff)) => {
            let selected_file = app.selected_diff_file_index_for(&item.id, diff);
            let selected_line = diff
                .files
                .get(selected_file)
                .map(|file| app.selected_diff_line_index_for(&item.id, file))
                .unwrap_or(0);
            let inline_comments = match app.details.get(&item.id) {
                Some(DetailState::Loaded(comments)) => Some(comments.as_slice()),
                _ => None,
            };
            push_diff(
                &mut builder,
                diff,
                DiffRenderContext {
                    item_id: &item.id,
                    comments: inline_comments,
                    expanded_comments: &app.expanded_comments,
                    details_focused: app.focus == FocusTarget::Details,
                    selected_comment_index: app.selected_comment_index,
                    selected_file,
                    selected_line,
                    selected_range: app.diff_mark_range_for(&item.id),
                },
            );
        }
        Some(DiffState::Error(error)) => {
            builder.push_heading("Diff");
            builder.push_plain(format!("Failed to load diff: {error}"));
        }
        Some(DiffState::Loading) | None => {
            builder.push_heading("Diff");
            builder.push_plain("loading diff...");
        }
    }

    builder.finish()
}

fn push_details_mode_tabs(builder: &mut DetailsBuilder, active: DetailsMode) {
    let tab = |label: &'static str, mode: DetailsMode| {
        let style = if active == mode {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        DetailSegment::styled(label, style)
    };
    builder.push_line(vec![
        tab("Conversation", DetailsMode::Conversation),
        DetailSegment::styled(" | ", Style::default().fg(Color::DarkGray)),
        tab("Diff", DetailsMode::Diff),
    ]);
}

fn push_diff(builder: &mut DetailsBuilder, diff: &PullRequestDiff, context: DiffRenderContext<'_>) {
    builder.push_line(vec![
        DetailSegment::styled("Diff", heading_style()),
        DetailSegment::raw(format!("  files: {}  ", diff.files.len())),
        DetailSegment::styled(format!("+{}", diff.additions), diff_added_style()),
        DetailSegment::raw(" "),
        DetailSegment::styled(format!("-{}", diff.deletions), diff_removed_style()),
    ]);

    if diff.files.is_empty() {
        builder.push_plain("No diff.");
        return;
    }

    let selected_file = context
        .selected_file
        .min(diff.files.len().saturating_sub(1));
    let file = &diff.files[selected_file];
    let inline_comment_map = context
        .comments
        .map(diff_inline_comment_map)
        .unwrap_or_default();
    let empty_entries: Vec<CommentDisplayEntry> = Vec::new();

    builder.push_line(vec![DetailSegment::styled(
        format!("file {}/{}", selected_file + 1, diff.files.len()),
        diff_metadata_style(),
    )]);

    builder.push_blank();
    builder.mark_diff_file();
    push_diff_file_header(builder, file);
    for metadata in &file.metadata {
        builder.push_line(vec![DetailSegment::styled(
            truncate_inline(metadata, builder.width),
            diff_metadata_style(),
        )]);
    }
    let mut review_index = 0;
    for hunk in &file.hunks {
        builder.push_line(vec![DetailSegment::styled(
            truncate_inline(&hunk.header, builder.width),
            diff_hunk_style(),
        )]);
        for line in &hunk.lines {
            let target = diff_review_target(file, line);
            let inline_entries = target
                .as_ref()
                .and_then(|target| inline_comment_map.get(&DiffInlineCommentKey::from(target)))
                .unwrap_or(&empty_entries);
            let line_review_index = target.as_ref().map(|_| {
                let index = review_index;
                review_index += 1;
                index
            });
            push_diff_line(
                builder,
                line,
                line_review_index,
                line_review_index.is_some_and(|index| {
                    index == context.selected_line || index_in_range(index, context.selected_range)
                }),
                inline_entries.len(),
            );
            if let Some(comments) = context.comments {
                push_diff_inline_comments(
                    builder,
                    context.item_id,
                    comments,
                    inline_entries,
                    context.expanded_comments,
                    context.details_focused,
                    context.selected_comment_index,
                );
            }
        }
    }
}

fn push_diff_file_header(builder: &mut DetailsBuilder, file: &DiffFile) {
    let path = if file.old_path == file.new_path {
        file.new_path.clone()
    } else {
        format!("{} -> {}", file.old_path, file.new_path)
    };
    builder.push_line(vec![
        DetailSegment::styled("▾ ", diff_file_style()),
        DetailSegment::styled(
            truncate_inline(&path, builder.width.saturating_sub(16).max(1)),
            diff_file_style(),
        ),
        DetailSegment::raw("  "),
        DetailSegment::styled(format!("+{}", file.additions), diff_added_style()),
        DetailSegment::raw(" "),
        DetailSegment::styled(format!("-{}", file.deletions), diff_removed_style()),
    ]);
}

fn push_diff_line(
    builder: &mut DetailsBuilder,
    line: &DiffLine,
    review_index: Option<usize>,
    selected: bool,
    inline_comment_count: usize,
) {
    if let Some(review_index) = review_index {
        builder.mark_diff_line(review_index, selected);
    }
    let gutter = diff_gutter(line.old_line, line.new_line);
    let (marker, mut style) = match line.kind {
        DiffLineKind::Context => (" ", diff_context_style()),
        DiffLineKind::Added => ("+", diff_added_style()),
        DiffLineKind::Removed => ("-", diff_removed_style()),
        DiffLineKind::Metadata => ("\\", diff_metadata_style()),
    };
    let gutter_style = if selected {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        diff_gutter_style()
    };
    if selected {
        style = style.bg(Color::DarkGray).add_modifier(Modifier::BOLD);
    }
    let inline_comment_marker = if inline_comment_count > 0 {
        Some(if inline_comment_count > 9 {
            "●* "
        } else {
            "● "
        })
    } else {
        None
    };
    let comment_marker_width = inline_comment_marker.map(display_width).unwrap_or(0);
    let prefix_width = display_width(&gutter) + comment_marker_width + display_width(marker) + 1;
    let content_width = builder.width.saturating_sub(prefix_width).max(1);
    let mut segments = vec![DetailSegment::styled(gutter, gutter_style)];
    if let Some(marker) = inline_comment_marker {
        segments.push(DetailSegment::styled(
            marker,
            diff_inline_comment_marker_style(),
        ));
    }
    segments.extend([
        DetailSegment::styled(marker, style),
        DetailSegment::styled(" ", style),
        DetailSegment::styled(truncate_inline(&line.text, content_width), style),
    ]);
    builder.push_line(segments);
}

fn push_diff_inline_comments(
    builder: &mut DetailsBuilder,
    item_id: &str,
    comments: &[CommentPreview],
    entries: &[CommentDisplayEntry],
    expanded_comments: &HashSet<String>,
    details_focused: bool,
    selected_comment_index: usize,
) {
    for entry in entries {
        let Some(comment) = comments.get(entry.index) else {
            continue;
        };
        let selected = details_focused && entry.index == selected_comment_index;
        let collapse = comment_collapse_state_for(item_id, entry.index, comment, expanded_comments);
        push_diff_inline_comment(
            builder,
            entry.index,
            comment,
            selected,
            entry.depth,
            collapse,
        );
    }
}

fn push_diff_inline_comment(
    builder: &mut DetailsBuilder,
    index: usize,
    comment: &CommentPreview,
    selected: bool,
    depth: usize,
    collapse: CommentCollapseState,
) {
    let timestamp = comment
        .updated_at
        .as_ref()
        .or(comment.created_at.as_ref())
        .cloned();
    let start_line = builder.document.lines.len();
    push_diff_inline_comment_separator(builder, selected, depth);
    let content_start_line = builder.document.lines.len();

    let mut header = vec![
        DetailSegment::styled(
            if selected { "▸ " } else { "● " },
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
    if collapse.long {
        header.push(DetailSegment::raw("  "));
        header.push(DetailSegment::action(
            if collapse.collapsed {
                "▸ collapsed"
            } else {
                "▾ expanded"
            },
            DetailAction::ToggleCommentExpanded(index),
        ));
    }

    let prefix = diff_inline_comment_prefix(selected, depth);
    builder.push_prefixed_wrapped_limited(header, prefix.clone(), COMMENT_RIGHT_PADDING, 2);
    let collapsed_body;
    let body = if collapse.collapsed {
        collapsed_body = collapsed_comment_body(&comment.body);
        collapsed_body.as_str()
    } else {
        comment.body.as_str()
    };
    builder.push_markdown_block_prefixed(
        body,
        "No comment body.",
        usize::MAX,
        usize::MAX,
        MarkdownRenderOptions {
            prefix,
            right_padding: COMMENT_RIGHT_PADDING,
        },
    );
    if collapse.collapsed {
        push_diff_inline_comment_expand_line(builder, index, selected, depth, collapse);
    }
    if selected {
        add_selected_comment_text_weight(builder, content_start_line, builder.document.lines.len());
        push_diff_inline_comment_separator(builder, true, depth);
        add_comment_right_border(builder, start_line, builder.document.lines.len());
    }
    builder.document.comments.push(CommentRegion {
        index,
        start_line,
        end_line: builder.document.lines.len(),
    });
}

fn push_diff_inline_comment_separator(builder: &mut DetailsBuilder, selected: bool, depth: usize) {
    let mut segments = diff_inline_comment_prefix(selected, depth);
    let prefix_width = segments_width(&segments);
    let width = builder
        .width
        .saturating_sub(prefix_width + COMMENT_RIGHT_PADDING)
        .max(12);
    let line = if selected { "━" } else { "─" };
    segments.push(DetailSegment::styled(
        line.repeat(width),
        comment_separator_style(selected),
    ));
    builder.push_line(segments);
}

fn push_diff_inline_comment_expand_line(
    builder: &mut DetailsBuilder,
    index: usize,
    selected: bool,
    depth: usize,
    collapse: CommentCollapseState,
) {
    builder.push_prefixed_wrapped_limited(
        vec![
            DetailSegment::styled("⋯ ", diff_metadata_style()),
            DetailSegment::action(
                "▸ show full comment",
                DetailAction::ToggleCommentExpanded(index),
            ),
            DetailSegment::styled(
                format!(
                    " ({} lines, {} chars)",
                    collapse.line_count, collapse.char_count
                ),
                diff_metadata_style(),
            ),
        ],
        diff_inline_comment_prefix(selected, depth),
        COMMENT_RIGHT_PADDING,
        2,
    );
}

fn diff_inline_comment_prefix(selected: bool, depth: usize) -> Vec<DetailSegment> {
    let depth = depth.min(4);
    let mut prefix = " ".repeat(DIFF_INLINE_COMMENT_GUTTER_WIDTH);
    prefix.push_str(if selected { "┃ " } else { "│ " });
    if depth > 0 {
        prefix.push_str(&"  ".repeat(depth.saturating_sub(1)));
        prefix.push_str("↳ ");
    }
    vec![DetailSegment::styled(
        prefix,
        if selected {
            comment_selected_rail_style()
        } else {
            comment_thread_style()
        },
    )]
}

fn diff_tree_entries(diff: &PullRequestDiff) -> Vec<DiffTreeEntry> {
    let mut files = diff
        .files
        .iter()
        .enumerate()
        .map(|(index, file)| (index, diff_display_path(file)))
        .collect::<Vec<_>>();
    files.sort_by(|(_, left), (_, right)| left.cmp(right));

    let mut entries = Vec::new();
    let mut seen_dirs = HashSet::new();
    for (file_index, path) in files {
        let parts = path
            .split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        if parts.is_empty() {
            entries.push(DiffTreeEntry {
                file_index: Some(file_index),
                label: path,
                stats: diff_file_stats(&diff.files[file_index]),
                depth: 0,
            });
            continue;
        }

        let mut prefix = String::new();
        for (depth, directory) in parts.iter().take(parts.len().saturating_sub(1)).enumerate() {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(directory);
            if seen_dirs.insert(prefix.clone()) {
                entries.push(DiffTreeEntry {
                    file_index: None,
                    label: (*directory).to_string(),
                    stats: String::new(),
                    depth,
                });
            }
        }

        entries.push(DiffTreeEntry {
            file_index: Some(file_index),
            label: parts
                .last()
                .map(|part| (*part).to_string())
                .unwrap_or_else(|| path.clone()),
            stats: diff_file_stats(&diff.files[file_index]),
            depth: parts.len().saturating_sub(1),
        });
    }

    entries
}

fn diff_tree_row_index_for_file(entries: &[DiffTreeEntry], file_index: usize) -> Option<usize> {
    entries
        .iter()
        .position(|entry| entry.file_index == Some(file_index))
}

fn diff_display_path(file: &DiffFile) -> String {
    if file.new_path != "/dev/null" {
        file.new_path.clone()
    } else {
        file.old_path.clone()
    }
}

fn diff_file_stats(file: &DiffFile) -> String {
    format!(
        "{} +{} -{}",
        diff_file_status(file),
        file.additions,
        file.deletions
    )
}

fn diff_file_status(file: &DiffFile) -> &'static str {
    match (file.old_path.as_str(), file.new_path.as_str()) {
        ("/dev/null", _) => "A",
        (_, "/dev/null") => "D",
        (old, new) if old != new => "R",
        _ => "M",
    }
}

fn diff_review_targets(file: &DiffFile) -> Vec<DiffReviewTarget> {
    file.hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .filter_map(|line| diff_review_target(file, line))
        .collect()
}

fn ordered_range(left: usize, right: usize) -> (usize, usize) {
    if left <= right {
        (left, right)
    } else {
        (right, left)
    }
}

fn index_in_range(index: usize, range: Option<(usize, usize)>) -> bool {
    let Some((start, end)) = range.map(|(start, end)| ordered_range(start, end)) else {
        return false;
    };
    index >= start && index <= end
}

fn diff_review_target_from_range(
    targets: &[DiffReviewTarget],
    start: usize,
    end: usize,
) -> Result<DiffReviewTarget, String> {
    if targets.is_empty() {
        return Err("no reviewable diff lines".to_string());
    }
    let (start, end) = ordered_range(start, end);
    let start = start.min(targets.len() - 1);
    let end = end.min(targets.len() - 1);
    let first = &targets[start];
    let last = &targets[end];
    if targets[start..=end]
        .iter()
        .any(|target| target.path != first.path)
    {
        return Err("range must stay in one file".to_string());
    }
    let mut target = last.clone();
    if start != end {
        target.start_line = Some(first.line);
        target.start_side = Some(first.side);
        target.preview = format!("{} lines selected", end.saturating_sub(start) + 1);
    }
    Ok(target)
}

fn diff_review_target(file: &DiffFile, line: &DiffLine) -> Option<DiffReviewTarget> {
    let (line_number, side) = match line.kind {
        DiffLineKind::Removed => (line.old_line?, DiffReviewSide::Left),
        DiffLineKind::Context | DiffLineKind::Added => (line.new_line?, DiffReviewSide::Right),
        DiffLineKind::Metadata => return None,
    };

    Some(DiffReviewTarget {
        path: diff_review_path(file, side),
        line: line_number,
        side,
        start_line: None,
        start_side: None,
        preview: truncate_inline(&line.text, 80),
    })
}

fn diff_review_path(file: &DiffFile, side: DiffReviewSide) -> String {
    match side {
        DiffReviewSide::Left if file.new_path == "/dev/null" => file.old_path.clone(),
        _ if file.new_path != "/dev/null" => file.new_path.clone(),
        _ => file.old_path.clone(),
    }
}

fn diff_inline_comment_map(
    comments: &[CommentPreview],
) -> HashMap<DiffInlineCommentKey, Vec<CommentDisplayEntry>> {
    let mut id_to_index = HashMap::new();
    for (index, comment) in comments.iter().enumerate() {
        if let Some(id) = comment.id {
            id_to_index.insert(id, index);
        }
    }

    let direct_keys = comments
        .iter()
        .map(diff_inline_comment_key_for_comment)
        .collect::<Vec<_>>();
    let mut index_keys = direct_keys.clone();
    for (index, comment) in comments.iter().enumerate() {
        if index_keys[index].is_some() {
            continue;
        }

        let mut parent_id = comment.parent_id;
        let mut seen = HashSet::new();
        while let Some(id) = parent_id {
            if !seen.insert(id) {
                break;
            }
            let Some(parent_index) = id_to_index.get(&id).copied() else {
                break;
            };
            if let Some(key) = index_keys[parent_index]
                .clone()
                .or_else(|| direct_keys[parent_index].clone())
            {
                index_keys[index] = Some(key);
                break;
            }
            parent_id = comments[parent_index].parent_id;
        }
    }

    let mut map: HashMap<DiffInlineCommentKey, Vec<CommentDisplayEntry>> = HashMap::new();
    for entry in comment_display_entries(comments) {
        if let Some(key) = index_keys[entry.index].clone() {
            map.entry(key).or_default().push(entry);
        }
    }
    map
}

fn diff_inline_comment_key_for_comment(comment: &CommentPreview) -> Option<DiffInlineCommentKey> {
    let review = comment.review.as_ref()?;
    let line = usize::try_from(review_display_line(review)?).ok()?;
    let side = diff_review_side_from_label(review_display_side(review)?)?;
    Some(DiffInlineCommentKey {
        path: review.path.clone(),
        line,
        side,
    })
}

fn diff_review_side_from_label(label: &str) -> Option<DiffReviewSide> {
    match label {
        label if label.eq_ignore_ascii_case("LEFT") => Some(DiffReviewSide::Left),
        label if label.eq_ignore_ascii_case("RIGHT") => Some(DiffReviewSide::Right),
        _ => None,
    }
}

fn diff_gutter(old_line: Option<usize>, new_line: Option<usize>) -> String {
    let old = old_line
        .map(|line| format!("{line:>4}"))
        .unwrap_or_else(|| "    ".to_string());
    let new = new_line
        .map(|line| format!("{line:>4}"))
        .unwrap_or_else(|| "    ".to_string());
    format!("{old} {new} │ ")
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
    search_match: bool,
    depth: usize,
    collapse: CommentCollapseState,
) {
    let timestamp = comment
        .updated_at
        .as_ref()
        .or(comment.created_at.as_ref())
        .cloned();
    let start_line = builder.document.lines.len();
    push_comment_separator(builder, selected, depth);
    let content_start_line = builder.document.lines.len();

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
    if let Some(review) = &comment.review {
        header.push(DetailSegment::raw("  "));
        header.push(DetailSegment::styled(
            review_comment_label(review),
            diff_metadata_style(),
        ));
    }
    if search_match {
        header.push(DetailSegment::styled(
            "  match",
            comment_search_match_style(),
        ));
    }
    if collapse.long {
        header.push(DetailSegment::raw("  "));
        header.push(DetailSegment::action(
            if collapse.collapsed {
                "▸ collapsed"
            } else {
                "▾ expanded"
            },
            DetailAction::ToggleCommentExpanded(index),
        ));
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
    let prefix = comment_line_prefix(selected, depth);
    builder.push_prefixed_wrapped_limited(
        header,
        prefix.clone(),
        comment_right_padding(selected),
        2,
    );
    if selected && let Some(review) = &comment.review {
        push_inline_review_context(builder, review, selected, depth);
    }
    let collapsed_body;
    let body = if collapse.collapsed {
        collapsed_body = collapsed_comment_body(&comment.body);
        collapsed_body.as_str()
    } else {
        comment.body.as_str()
    };
    builder.push_markdown_block_prefixed(
        body,
        "No comment body.",
        usize::MAX,
        usize::MAX,
        MarkdownRenderOptions {
            prefix,
            right_padding: comment_right_padding(selected),
        },
    );
    if collapse.collapsed {
        push_comment_expand_line(builder, index, selected, depth, collapse);
    }
    if selected {
        add_selected_comment_text_weight(builder, content_start_line, builder.document.lines.len());
        push_comment_separator(builder, true, depth);
        add_comment_right_border(builder, start_line, builder.document.lines.len());
    }
    builder.document.comments.push(CommentRegion {
        index,
        start_line,
        end_line: builder.document.lines.len(),
    });
}

fn add_selected_comment_text_weight(
    builder: &mut DetailsBuilder,
    start_line: usize,
    end_line: usize,
) {
    for line in builder
        .document
        .lines
        .iter_mut()
        .take(end_line)
        .skip(start_line)
    {
        for span in &mut line.spans {
            if !span.content.trim().is_empty() {
                span.style = span.style.add_modifier(Modifier::BOLD);
            }
        }
    }
}

fn long_comment_metrics(comment: &CommentPreview) -> (usize, usize) {
    let text = normalize_text(&comment.body);
    let line_count = if text.is_empty() {
        0
    } else {
        text.lines().count()
    };
    (line_count, text.chars().count())
}

fn is_long_comment(comment: &CommentPreview) -> bool {
    let (line_count, char_count) = long_comment_metrics(comment);
    line_count > COMMENT_COLLAPSE_MIN_LINES || char_count > COMMENT_COLLAPSE_MIN_CHARS
}

fn comment_collapse_state_for(
    item_id: &str,
    index: usize,
    comment: &CommentPreview,
    expanded_comments: &HashSet<String>,
) -> CommentCollapseState {
    let (line_count, char_count) = long_comment_metrics(comment);
    let long = line_count > COMMENT_COLLAPSE_MIN_LINES || char_count > COMMENT_COLLAPSE_MIN_CHARS;
    let key = comment_expand_key(item_id, index, comment);
    CommentCollapseState {
        long,
        collapsed: long && !expanded_comments.contains(&key),
        line_count,
        char_count,
    }
}

fn comment_expand_key(item_id: &str, index: usize, comment: &CommentPreview) -> String {
    match comment.id {
        Some(id) => format!("{item_id}:comment:{id}"),
        None => format!("{item_id}:comment-index:{index}"),
    }
}

fn collapsed_comment_body(body: &str) -> String {
    let text = normalize_text(body);
    let mut output = String::new();
    let mut chars = 0_usize;
    for (index, line) in text.lines().enumerate() {
        if index >= COMMENT_COLLAPSED_PREVIEW_LINES {
            break;
        }
        let line_chars = line.chars().count();
        if chars.saturating_add(line_chars) > COMMENT_COLLAPSED_PREVIEW_CHARS {
            let remaining = COMMENT_COLLAPSED_PREVIEW_CHARS.saturating_sub(chars);
            if remaining > 0 {
                output.push_str(&line.chars().take(remaining).collect::<String>());
            }
            break;
        }
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(line);
        chars = chars.saturating_add(line_chars).saturating_add(1);
    }
    output
}

fn push_comment_expand_line(
    builder: &mut DetailsBuilder,
    index: usize,
    selected: bool,
    depth: usize,
    collapse: CommentCollapseState,
) {
    let prefix = comment_line_prefix(selected, depth);
    builder.push_prefixed_wrapped_limited(
        vec![
            DetailSegment::styled("⋯ ", diff_metadata_style()),
            DetailSegment::action(
                "▸ show full comment",
                DetailAction::ToggleCommentExpanded(index),
            ),
            DetailSegment::styled(
                format!(
                    " ({} lines, {} chars)",
                    collapse.line_count, collapse.char_count
                ),
                diff_metadata_style(),
            ),
        ],
        prefix,
        comment_right_padding(selected),
        2,
    );
}

fn push_inline_review_context(
    builder: &mut DetailsBuilder,
    review: &crate::model::ReviewCommentPreview,
    selected: bool,
    depth: usize,
) {
    let Some(raw_hunk) = review
        .diff_hunk
        .as_deref()
        .filter(|hunk| !hunk.trim().is_empty())
    else {
        return;
    };
    let Some(hunk) = parse_inline_diff_hunk(raw_hunk) else {
        return;
    };

    let focus_span = inline_diff_focus_span(&hunk.lines, review);
    let (start, end) = inline_diff_context_range(hunk.lines.len(), focus_span);
    let prefix = comment_line_prefix(selected, depth);
    let right_padding = comment_right_padding(selected);
    let original_width = builder.width;
    builder.width = reserved_width(builder.width, right_padding);

    builder.push_prefixed_wrapped_limited(
        vec![DetailSegment::styled(
            truncate_inline(
                &hunk.header,
                builder.width.saturating_sub(segments_width(&prefix)),
            ),
            diff_hunk_style(),
        )],
        prefix.clone(),
        0,
        1,
    );
    if start > 0 {
        push_inline_diff_ellipsis(builder, prefix.as_slice());
    }
    for (offset, line) in hunk.lines[start..end].iter().enumerate() {
        let index = start + offset;
        let focused = focus_span.is_some_and(|(start, end)| (start..=end).contains(&index));
        push_inline_diff_line(builder, line, prefix.as_slice(), focused);
    }
    if end < hunk.lines.len() {
        push_inline_diff_ellipsis(builder, prefix.as_slice());
    }

    builder.width = original_width;
}

type InlineDiffTarget<'a> = (usize, Option<&'a str>);
type InlineDiffRangeTarget<'a> = (InlineDiffTarget<'a>, InlineDiffTarget<'a>);

fn inline_diff_focus_span(
    lines: &[DiffLine],
    review: &crate::model::ReviewCommentPreview,
) -> Option<(usize, usize)> {
    inline_diff_focus_range_candidates(review)
        .into_iter()
        .find_map(|(start, end)| {
            let start_index = find_inline_diff_line(lines, start.0, start.1)?;
            let end_index = find_inline_diff_line(lines, end.0, end.1)?;
            Some(if start_index <= end_index {
                (start_index, end_index)
            } else {
                (end_index, start_index)
            })
        })
        .or_else(|| inline_diff_focus_line(lines, review).map(|index| (index, index)))
}

fn inline_diff_focus_range_candidates(
    review: &crate::model::ReviewCommentPreview,
) -> Vec<InlineDiffRangeTarget<'_>> {
    let mut candidates = Vec::new();
    push_inline_diff_focus_range_candidate(
        &mut candidates,
        review.start_line,
        review.line,
        review.start_side.as_deref().or(review.side.as_deref()),
        review.side.as_deref(),
    );
    push_inline_diff_focus_range_candidate(
        &mut candidates,
        review.original_start_line,
        review.original_line,
        review.start_side.as_deref().or(review.side.as_deref()),
        review.side.as_deref(),
    );
    candidates
}

fn push_inline_diff_focus_range_candidate<'a>(
    candidates: &mut Vec<InlineDiffRangeTarget<'a>>,
    start: Option<u64>,
    end: Option<u64>,
    start_side: Option<&'a str>,
    end_side: Option<&'a str>,
) {
    let Some(start) = start.and_then(|line| usize::try_from(line).ok()) else {
        return;
    };
    let Some(end) = end.and_then(|line| usize::try_from(line).ok()) else {
        return;
    };
    if candidates.iter().any(|(existing_start, existing_end)| {
        *existing_start == (start, start_side) && *existing_end == (end, end_side)
    }) {
        return;
    }
    candidates.push(((start, start_side), (end, end_side)));
}

fn inline_diff_focus_line(
    lines: &[DiffLine],
    review: &crate::model::ReviewCommentPreview,
) -> Option<usize> {
    inline_diff_focus_candidates(review)
        .into_iter()
        .find_map(|(target, side)| find_inline_diff_line(lines, target, side))
}

fn inline_diff_focus_candidates(
    review: &crate::model::ReviewCommentPreview,
) -> Vec<(usize, Option<&str>)> {
    let mut candidates = Vec::new();
    push_inline_diff_focus_candidate(&mut candidates, review.line, review.side.as_deref());
    push_inline_diff_focus_candidate(
        &mut candidates,
        review.original_line,
        review.side.as_deref(),
    );
    push_inline_diff_focus_candidate(
        &mut candidates,
        review.start_line,
        review.start_side.as_deref().or(review.side.as_deref()),
    );
    push_inline_diff_focus_candidate(
        &mut candidates,
        review.original_start_line,
        review.start_side.as_deref().or(review.side.as_deref()),
    );
    candidates
}

fn push_inline_diff_focus_candidate<'a>(
    candidates: &mut Vec<(usize, Option<&'a str>)>,
    line: Option<u64>,
    side: Option<&'a str>,
) {
    let Some(line) = line.and_then(|line| usize::try_from(line).ok()) else {
        return;
    };
    if candidates
        .iter()
        .any(|(existing_line, existing_side)| *existing_line == line && *existing_side == side)
    {
        return;
    }
    candidates.push((line, side));
}

fn find_inline_diff_line(lines: &[DiffLine], target: usize, side: Option<&str>) -> Option<usize> {
    lines
        .iter()
        .position(|line| diff_line_matches_side(line, target, side))
}

fn diff_line_matches_side(line: &DiffLine, target: usize, side: Option<&str>) -> bool {
    match side.unwrap_or_default() {
        side if side.eq_ignore_ascii_case("LEFT") => line.old_line == Some(target),
        side if side.eq_ignore_ascii_case("RIGHT") => line.new_line == Some(target),
        _ => line.new_line == Some(target) || line.old_line == Some(target),
    }
}

fn review_display_line(review: &crate::model::ReviewCommentPreview) -> Option<u64> {
    review.line.or(review.original_line)
}

fn review_display_start_line(review: &crate::model::ReviewCommentPreview) -> Option<u64> {
    review.start_line.or(review.original_start_line)
}

fn review_display_side(review: &crate::model::ReviewCommentPreview) -> Option<&str> {
    review.side.as_deref().or(review.start_side.as_deref())
}

fn review_line_label(review: &crate::model::ReviewCommentPreview) -> String {
    let end = review_display_line(review);
    let start = review_display_start_line(review);
    match (start, end) {
        (Some(start), Some(end)) if start != end => format!("{start}-{end}"),
        (_, Some(end)) => end.to_string(),
        (Some(start), None) => start.to_string(),
        (None, None) => "-".to_string(),
    }
}

fn inline_diff_context_range(len: usize, focus_span: Option<(usize, usize)>) -> (usize, usize) {
    if len == 0 {
        return (0, 0);
    }
    let Some((span_start, span_end)) = focus_span else {
        return (0, len.min(INLINE_COMMENT_CONTEXT_MAX_LINES));
    };

    let span_start = span_start.min(len - 1);
    let span_end = span_end.min(len - 1).max(span_start);
    let span_len = span_end - span_start + 1;
    let max_lines = span_len
        .saturating_add(INLINE_COMMENT_CONTEXT_RADIUS)
        .saturating_add(INLINE_COMMENT_CONTEXT_RADIUS)
        .clamp(INLINE_COMMENT_CONTEXT_MAX_LINES, 18)
        .min(len);
    let mut start = span_start.saturating_sub(INLINE_COMMENT_CONTEXT_RADIUS);
    let mut end = span_end
        .saturating_add(INLINE_COMMENT_CONTEXT_RADIUS)
        .saturating_add(1)
        .min(len);

    if end.saturating_sub(start) > max_lines {
        if span_len >= max_lines {
            start = span_start;
            end = start.saturating_add(max_lines).min(len);
        } else {
            let extra = max_lines - span_len;
            let before = extra / 2;
            let after = extra - before;
            start = span_start.saturating_sub(before);
            end = span_end.saturating_add(after).saturating_add(1).min(len);
            if end.saturating_sub(start) < max_lines {
                start = start.saturating_sub(max_lines - (end - start));
            }
        }
    }
    if end.saturating_sub(start) < max_lines {
        end = (start + max_lines).min(len);
    }
    (start, end)
}

fn push_inline_diff_ellipsis(builder: &mut DetailsBuilder, prefix: &[DetailSegment]) {
    let mut segments = prefix.to_vec();
    segments.push(DetailSegment::styled("      ⋮", diff_metadata_style()));
    builder.push_line(segments);
}

fn push_inline_diff_line(
    builder: &mut DetailsBuilder,
    line: &DiffLine,
    prefix: &[DetailSegment],
    focused: bool,
) {
    let marker = match line.kind {
        DiffLineKind::Context => " ",
        DiffLineKind::Added => "+",
        DiffLineKind::Removed => "-",
        DiffLineKind::Metadata => "\\",
    };
    let mut style = match line.kind {
        DiffLineKind::Context => diff_context_style(),
        DiffLineKind::Added => diff_added_style(),
        DiffLineKind::Removed => diff_removed_style(),
        DiffLineKind::Metadata => diff_metadata_style(),
    };
    if focused {
        style = style.fg(Color::Yellow).add_modifier(Modifier::BOLD);
    }
    let gutter_style = if focused {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        diff_gutter_style()
    };
    let focus_marker = if focused { ">" } else { " " };
    let gutter = format!(
        "{focus_marker}{}",
        compact_diff_gutter(line.old_line, line.new_line)
    );
    let prefix_width = prefix
        .iter()
        .map(|segment| display_width(&segment.text))
        .sum::<usize>();
    let content_width = builder
        .width
        .saturating_sub(prefix_width + display_width(&gutter) + display_width(marker) + 1)
        .max(1);
    let mut segments = prefix.to_vec();
    segments.push(DetailSegment::styled(gutter, gutter_style));
    segments.push(DetailSegment::styled(marker, style));
    segments.push(DetailSegment::styled(" ", style));
    segments.push(DetailSegment::styled(
        truncate_inline(&line.text, content_width),
        style,
    ));
    builder.push_line(segments);
}

fn compact_diff_gutter(old_line: Option<usize>, new_line: Option<usize>) -> String {
    let old = old_line
        .map(|line| format!("{line:>4}"))
        .unwrap_or_else(|| "    ".to_string());
    let new = new_line
        .map(|line| format!("{line:>4}"))
        .unwrap_or_else(|| "    ".to_string());
    format!("{old} {new} │ ")
}

fn details_comment_count(app: &AppState, item: &WorkItem) -> Option<usize> {
    match app.details.get(&item.id) {
        Some(DetailState::Loaded(comments)) => Some(comments.len()),
        _ => item.comments.map(|comments| comments as usize),
    }
}

fn review_comment_label(review: &crate::model::ReviewCommentPreview) -> String {
    let line = review_line_label(review);
    let side = review_display_side(review)
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| "line".to_string());
    format!("inline {}:{line} {side}", review.path)
}

fn push_comment_separator(builder: &mut DetailsBuilder, selected: bool, depth: usize) {
    let prefix = comment_line_prefix(selected, depth);
    let prefix_width = segments_width(&prefix);
    let width = builder
        .width
        .saturating_sub(prefix_width + comment_right_padding(selected))
        .max(12);
    let line = if selected { "━" } else { "─" };
    let mut segments = prefix;
    segments.push(DetailSegment::styled(
        line.repeat(width),
        comment_separator_style(selected),
    ));
    builder.push_line(segments);
}

fn comment_line_prefix(selected: bool, depth: usize) -> Vec<DetailSegment> {
    let depth = depth.min(4);
    if selected {
        let mut prefix = "┃ ".to_string();
        if depth > 0 {
            prefix.push_str("  ");
            prefix.push_str(&"  ".repeat(depth.saturating_sub(1)));
            prefix.push_str("↳ ");
        }
        vec![DetailSegment::styled(prefix, comment_selected_rail_style())]
    } else if depth > 0 {
        let mut prefix = "    ".to_string();
        prefix.push_str(&"  ".repeat(depth.saturating_sub(1)));
        prefix.push_str("↳ ");
        vec![DetailSegment::styled(prefix, comment_thread_style())]
    } else {
        padding_prefix(COMMENT_LEFT_PADDING)
    }
}

fn add_comment_right_border(builder: &mut DetailsBuilder, start_line: usize, end_line: usize) {
    let border_column = comment_right_border_column(builder.width);
    for line in builder
        .document
        .lines
        .iter_mut()
        .take(end_line)
        .skip(start_line)
    {
        let width = display_width(&line.to_string());
        if width < border_column {
            line.spans
                .push(Span::raw(" ".repeat(border_column - width)));
        }
        line.spans
            .push(Span::styled("┃", comment_selected_rail_style()));
    }
}

fn comment_right_border_column(width: usize) -> usize {
    width
        .saturating_sub(COMMENT_RIGHT_PADDING + 1)
        .max(COMMENT_LEFT_PADDING)
}

fn comment_right_padding(selected: bool) -> usize {
    COMMENT_RIGHT_PADDING + usize::from(selected)
}

fn action_hint_segments(state: Option<&ActionHintState>) -> (Vec<DetailSegment>, Option<String>) {
    match state {
        Some(ActionHintState::Loaded(hints)) => {
            let segments = if hints.labels.is_empty() {
                vec![DetailSegment::raw("-")]
            } else {
                action_label_segments(&hints.labels)
            };
            (segments, hints.note.clone())
        }
        Some(ActionHintState::Loading) | None => (vec![DetailSegment::raw("loading...")], None),
        Some(ActionHintState::Error(error)) => (
            vec![DetailSegment::raw("unavailable")],
            Some(format!("Failed to load action hints: {error}")),
        ),
    }
}

fn action_label_segments(labels: &[String]) -> Vec<DetailSegment> {
    let mut segments = Vec::new();
    for label in labels {
        if !segments.is_empty() {
            segments.push(DetailSegment::raw(", "));
        }
        let style = if label == "Mergeable" {
            Style::default().fg(Color::LightGreen)
        } else {
            Style::default()
        };
        segments.push(DetailSegment::styled(label, style));
    }
    segments
}

fn action_note_segments(note: &str) -> Vec<DetailSegment> {
    const CONFLICTS: &str = "merge conflicts must be resolved";
    let mut segments = Vec::new();
    let mut rest = note;
    while let Some(index) = rest.find(CONFLICTS) {
        if index > 0 {
            segments.push(DetailSegment::raw(rest[..index].to_string()));
        }
        segments.push(DetailSegment::styled(
            CONFLICTS,
            log_error_style().add_modifier(Modifier::BOLD),
        ));
        rest = &rest[index + CONFLICTS.len()..];
    }
    if !rest.is_empty() {
        segments.push(DetailSegment::raw(rest.to_string()));
    }
    if segments.is_empty() {
        segments.push(DetailSegment::raw(note.to_string()));
    }
    segments
}

fn branch_hint_segments(state: Option<&ActionHintState>) -> Vec<DetailSegment> {
    match state {
        Some(ActionHintState::Loaded(hints)) => hints
            .head
            .as_ref()
            .map(|branch| vec![DetailSegment::raw(pull_request_branch_label(branch))])
            .unwrap_or_else(|| vec![DetailSegment::raw("unavailable")]),
        Some(ActionHintState::Loading) | None => vec![DetailSegment::raw("loading...")],
        Some(ActionHintState::Error(_)) => vec![DetailSegment::raw("unavailable")],
    }
}

fn pull_request_branch_label(branch: &PullRequestBranch) -> String {
    format!("{}:{}", branch.repository, branch.branch)
}

fn pull_request_branch_url(branch: &PullRequestBranch) -> String {
    format!(
        "https://github.com/{}/tree/{}",
        branch.repository, branch.branch
    )
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
    let mut code_language = CodeLanguage::Other;
    let mut strong_depth = 0_u8;
    let mut emphasis_depth = 0_u8;
    let mut quote_depth = 0_u8;
    let mut table: Option<MarkdownTable> = None;
    let mut table_row: Option<MarkdownTableRow> = None;
    let mut in_table_head = false;
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;

    for event in Parser::new_ext(text, options) {
        match event {
            MarkdownEvent::Start(Tag::Paragraph) => {}
            MarkdownEvent::End(TagEnd::Paragraph) if table.is_none() => flush_markdown_block(
                &mut blocks,
                &mut current,
                quote_depth,
                MarkdownBlockKind::Text,
            ),
            MarkdownEvent::End(TagEnd::Paragraph) => {}
            MarkdownEvent::Start(Tag::Heading { .. }) => {}
            MarkdownEvent::End(TagEnd::Heading(_)) if table.is_none() => flush_markdown_block(
                &mut blocks,
                &mut current,
                quote_depth,
                MarkdownBlockKind::Text,
            ),
            MarkdownEvent::End(TagEnd::Heading(_)) => {}
            MarkdownEvent::Start(Tag::Table(_)) => {
                flush_markdown_block(
                    &mut blocks,
                    &mut current,
                    quote_depth,
                    MarkdownBlockKind::Text,
                );
                table = Some(MarkdownTable::default());
            }
            MarkdownEvent::End(TagEnd::Table) => {
                flush_table_row(&mut table, &mut table_row);
                if let Some(table) = table.take() {
                    push_markdown_table(&mut blocks, quote_depth, table);
                }
                in_table_head = false;
            }
            MarkdownEvent::Start(Tag::TableHead) => {
                in_table_head = true;
                table_row = Some(MarkdownTableRow {
                    cells: Vec::new(),
                    header: true,
                });
            }
            MarkdownEvent::End(TagEnd::TableHead) => {
                flush_table_row(&mut table, &mut table_row);
                in_table_head = false;
            }
            MarkdownEvent::Start(Tag::TableRow) => {
                flush_table_row(&mut table, &mut table_row);
                table_row = Some(MarkdownTableRow {
                    cells: Vec::new(),
                    header: in_table_head,
                });
            }
            MarkdownEvent::End(TagEnd::TableRow) => {
                flush_table_row(&mut table, &mut table_row);
            }
            MarkdownEvent::Start(Tag::TableCell) => {
                if table.is_some() && table_row.is_none() {
                    table_row = Some(MarkdownTableRow {
                        cells: Vec::new(),
                        header: in_table_head,
                    });
                }
                current.clear();
            }
            MarkdownEvent::End(TagEnd::TableCell) => {
                flush_table_cell(&mut table_row, &mut current);
            }
            MarkdownEvent::Start(Tag::BlockQuote(_)) => {
                flush_markdown_block(
                    &mut blocks,
                    &mut current,
                    quote_depth,
                    MarkdownBlockKind::Text,
                );
                quote_depth = quote_depth.saturating_add(1);
            }
            MarkdownEvent::End(TagEnd::BlockQuote(_)) => {
                flush_markdown_block(
                    &mut blocks,
                    &mut current,
                    quote_depth,
                    MarkdownBlockKind::Text,
                );
                quote_depth = quote_depth.saturating_sub(1);
            }
            MarkdownEvent::Start(Tag::Item) => {
                current.push(DetailSegment::styled(
                    "- ",
                    Style::default().fg(Color::Gray),
                ));
            }
            MarkdownEvent::End(TagEnd::Item) => flush_markdown_block(
                &mut blocks,
                &mut current,
                quote_depth,
                MarkdownBlockKind::ListItem,
            ),
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
            MarkdownEvent::Start(Tag::CodeBlock(kind)) => {
                flush_markdown_block(
                    &mut blocks,
                    &mut current,
                    quote_depth,
                    MarkdownBlockKind::Text,
                );
                in_code_block = true;
                code_language = CodeLanguage::from_code_block(&kind);
                code_block.clear();
            }
            MarkdownEvent::End(TagEnd::CodeBlock) => {
                let mut lines = code_block.split('\n').collect::<Vec<_>>();
                if lines.last() == Some(&"") {
                    lines.pop();
                }
                if lines.is_empty() {
                    lines.push("");
                }
                for line in lines {
                    push_markdown_block(
                        &mut blocks,
                        quote_depth,
                        MarkdownBlockKind::Code {
                            language: code_language,
                        },
                        highlight_code_line(line, code_language),
                    );
                }
                in_code_block = false;
                code_language = CodeLanguage::Other;
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
            MarkdownEvent::HardBreak => flush_markdown_block(
                &mut blocks,
                &mut current,
                quote_depth,
                MarkdownBlockKind::Text,
            ),
            MarkdownEvent::Rule => push_markdown_block(
                &mut blocks,
                quote_depth,
                MarkdownBlockKind::Text,
                vec![DetailSegment::styled(
                    "─".repeat(24),
                    Style::default().fg(Color::DarkGray),
                )],
            ),
            MarkdownEvent::TaskListMarker(checked) => {
                current.push(DetailSegment::raw(if checked { "[x] " } else { "[ ] " }));
            }
            _ => {}
        }
    }
    flush_markdown_block(
        &mut blocks,
        &mut current,
        quote_depth,
        MarkdownBlockKind::Text,
    );
    blocks
}

fn is_rust_code_info(info: &str) -> bool {
    matches!(
        info.split_whitespace().next().map(str::to_ascii_lowercase),
        Some(language) if matches!(language.as_str(), "rust" | "rs")
    )
}

fn is_plain_code_info(info: &str) -> bool {
    matches!(
        info.split_whitespace().next().map(str::to_ascii_lowercase),
        Some(language) if matches!(
            language.as_str(),
            "plain" | "text" | "txt" | "log" | "console" | "output"
        )
    )
}

fn highlight_code_line(line: &str, language: CodeLanguage) -> Vec<DetailSegment> {
    match language {
        CodeLanguage::Rust => highlight_rust_code_line(line),
        CodeLanguage::Plain => highlight_plain_code_line(line),
        CodeLanguage::Other => vec![DetailSegment::styled(line.to_string(), code_plain_style())],
    }
}

fn highlight_plain_code_line(line: &str) -> Vec<DetailSegment> {
    let content_start = line
        .char_indices()
        .find_map(|(index, ch)| (!ch.is_whitespace()).then_some(index))
        .unwrap_or(line.len());
    let (prefix, content) = line.split_at(content_start);
    let mut segments = Vec::new();
    push_highlighted_text(&mut segments, prefix, code_plain_style());
    push_highlighted_text(&mut segments, content, plain_code_content_style(content));
    segments
}

fn plain_code_content_style(content: &str) -> Style {
    let lower = content.to_ascii_lowercase();
    if lower.starts_with("error") || lower.contains(" failed") || lower.contains(": fail") {
        log_error_style()
    } else if lower.starts_with("warning") {
        log_warning_style()
    } else if lower.contains(" info ") || lower.contains(" info  ") {
        log_info_style()
    } else if content == "---" || content.starts_with("##[") || content.starts_with("[TIMING:") {
        log_meta_style()
    } else {
        code_plain_style()
    }
}

fn highlight_rust_code_line(line: &str) -> Vec<DetailSegment> {
    let chars = line.chars().collect::<Vec<_>>();
    let mut segments = Vec::new();
    let mut index = 0;

    while index < chars.len() {
        let ch = chars[index];
        if ch == '/' && chars.get(index + 1) == Some(&'/') {
            push_highlighted_text(
                &mut segments,
                &chars[index..].iter().collect::<String>(),
                rust_comment_style(),
            );
            break;
        }

        if ch == '"' {
            let end = quoted_end(&chars, index, '"');
            push_highlighted_text(
                &mut segments,
                &chars[index..end].iter().collect::<String>(),
                rust_string_style(),
            );
            index = end;
            continue;
        }

        if ch == '\''
            && let Some(end) = rust_char_literal_end(&chars, index)
        {
            push_highlighted_text(
                &mut segments,
                &chars[index..end].iter().collect::<String>(),
                rust_string_style(),
            );
            index = end;
            continue;
        }

        if is_rust_ident_start(ch) {
            let start = index;
            index += 1;
            while chars
                .get(index)
                .is_some_and(|candidate| is_rust_ident_continue(*candidate))
            {
                index += 1;
            }
            let mut end = index;
            let text = chars[start..index].iter().collect::<String>();
            let style = if is_rust_keyword(&text) {
                rust_keyword_style()
            } else if is_rust_primitive_type(&text) {
                rust_type_style()
            } else if chars.get(index) == Some(&'!') {
                end = index + 1;
                rust_macro_style()
            } else {
                code_plain_style()
            };
            push_highlighted_text(
                &mut segments,
                &chars[start..end].iter().collect::<String>(),
                style,
            );
            index = end;
            continue;
        }

        if ch.is_ascii_digit() {
            let start = index;
            index += 1;
            while chars.get(index).is_some_and(|candidate| {
                candidate.is_ascii_alphanumeric() || matches!(candidate, '_' | '.')
            }) {
                index += 1;
            }
            push_highlighted_text(
                &mut segments,
                &chars[start..index].iter().collect::<String>(),
                rust_number_style(),
            );
            continue;
        }

        push_highlighted_text(&mut segments, &ch.to_string(), code_plain_style());
        index += 1;
    }

    segments
}

fn quoted_end(chars: &[char], start: usize, quote: char) -> usize {
    let mut index = start + 1;
    let mut escaped = false;
    while index < chars.len() {
        let ch = chars[index];
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == quote {
            return index + 1;
        }
        index += 1;
    }
    chars.len()
}

fn rust_char_literal_end(chars: &[char], start: usize) -> Option<usize> {
    let end = quoted_end(chars, start, '\'');
    if end <= start + 1 || end > chars.len() {
        return None;
    }
    let body_len = chars[start + 1..end - 1].len();
    if (1..=6).contains(&body_len) {
        Some(end)
    } else {
        None
    }
}

fn is_rust_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_rust_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn is_rust_keyword(text: &str) -> bool {
    matches!(
        text,
        "as" | "async"
            | "await"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "Self"
            | "self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
    )
}

fn is_rust_primitive_type(text: &str) -> bool {
    matches!(
        text,
        "bool"
            | "char"
            | "f32"
            | "f64"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "str"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
    )
}

fn push_highlighted_text(segments: &mut Vec<DetailSegment>, text: &str, style: Style) {
    let template = DetailSegment::styled("", style);
    push_text_segment(segments, &template, text);
}

fn flush_table_cell(row: &mut Option<MarkdownTableRow>, current: &mut Vec<DetailSegment>) {
    if let Some(row) = row.as_mut() {
        row.cells.push(trim_table_cell(std::mem::take(current)));
        return;
    }
    current.clear();
}

fn flush_table_row(table: &mut Option<MarkdownTable>, row: &mut Option<MarkdownTableRow>) {
    let Some(row) = row.take() else {
        return;
    };
    if row
        .cells
        .iter()
        .any(|cell| cell.iter().any(|segment| !segment.text.is_empty()))
        && let Some(table) = table.as_mut()
    {
        table.rows.push(row);
    }
}

fn push_markdown_table(blocks: &mut Vec<MarkdownBlock>, quote_depth: u8, table: MarkdownTable) {
    let column_count = table
        .rows
        .iter()
        .map(|row| row.cells.len())
        .max()
        .unwrap_or(0);
    if column_count == 0 {
        return;
    }

    let mut widths = vec![0_usize; column_count];
    for row in &table.rows {
        for (index, cell) in row.cells.iter().enumerate() {
            widths[index] = widths[index].max(segments_width(cell));
        }
    }

    for row in table.rows {
        let is_header = row.header;
        push_markdown_block(
            blocks,
            quote_depth,
            MarkdownBlockKind::TableRow,
            table_row_segments(&row, &widths),
        );
        if is_header {
            push_markdown_block(
                blocks,
                quote_depth,
                MarkdownBlockKind::TableRow,
                table_separator_segments(&widths),
            );
        }
    }
}

fn table_row_segments(row: &MarkdownTableRow, widths: &[usize]) -> Vec<DetailSegment> {
    let mut segments = Vec::new();
    for (index, width) in widths.iter().copied().enumerate() {
        if index > 0 {
            segments.push(DetailSegment::styled(" │ ", table_separator_style()));
        }
        let mut cell = row.cells.get(index).cloned().unwrap_or_default();
        if row.header {
            for segment in &mut cell {
                segment.style = segment.style.add_modifier(Modifier::BOLD);
            }
        }
        let cell_width = segments_width(&cell);
        segments.extend(cell);
        if width > cell_width {
            segments.push(DetailSegment::raw(" ".repeat(width - cell_width)));
        }
    }
    segments
}

fn table_separator_segments(widths: &[usize]) -> Vec<DetailSegment> {
    let mut segments = Vec::new();
    for (index, width) in widths.iter().copied().enumerate() {
        if index > 0 {
            segments.push(DetailSegment::styled("─┼─", table_separator_style()));
        }
        segments.push(DetailSegment::styled(
            "─".repeat(width.max(1)),
            table_separator_style(),
        ));
    }
    segments
}

fn trim_table_cell(mut segments: Vec<DetailSegment>) -> Vec<DetailSegment> {
    while let Some(first) = segments.first_mut() {
        let trimmed = first.text.trim_start().to_string();
        if trimmed.is_empty() {
            segments.remove(0);
        } else {
            first.text = trimmed;
            break;
        }
    }
    while let Some(last) = segments.last_mut() {
        let trimmed = last.text.trim_end().to_string();
        if trimmed.is_empty() {
            segments.pop();
        } else {
            last.text = trimmed;
            break;
        }
    }
    segments
}

fn segments_width(segments: &[DetailSegment]) -> usize {
    segments
        .iter()
        .map(|segment| display_width(&segment.text))
        .sum()
}

fn flush_markdown_block(
    blocks: &mut Vec<MarkdownBlock>,
    current: &mut Vec<DetailSegment>,
    quote_depth: u8,
    kind: MarkdownBlockKind,
) {
    if current.iter().any(|segment| !segment.text.is_empty()) {
        push_markdown_block(blocks, quote_depth, kind, std::mem::take(current));
    }
}

fn push_markdown_block(
    blocks: &mut Vec<MarkdownBlock>,
    quote_depth: u8,
    kind: MarkdownBlockKind,
    segments: Vec<DetailSegment>,
) {
    let gap_before = markdown_gap_before(blocks.last(), quote_depth, kind);
    let segments = if quote_depth > 0 {
        muted_quote_segments(segments)
    } else {
        segments
    };
    blocks.push(MarkdownBlock {
        quote_depth,
        kind,
        gap_before,
        segments,
    });
}

fn markdown_gap_before(
    previous: Option<&MarkdownBlock>,
    quote_depth: u8,
    kind: MarkdownBlockKind,
) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    if previous.quote_depth != quote_depth {
        return previous.quote_depth > quote_depth;
    }
    !matches!(
        (previous.kind, kind),
        (MarkdownBlockKind::ListItem, MarkdownBlockKind::ListItem)
            | (MarkdownBlockKind::TableRow, MarkdownBlockKind::TableRow)
            | (
                MarkdownBlockKind::Code { .. },
                MarkdownBlockKind::Code { .. }
            )
    )
}

fn muted_quote_segments(mut segments: Vec<DetailSegment>) -> Vec<DetailSegment> {
    for segment in &mut segments {
        segment.style = segment.style.fg(Color::Gray);
    }
    segments
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
            None => append_mention_segments(current, &part, style),
        }
    }
}

fn append_mention_segments(current: &mut Vec<DetailSegment>, text: &str, style: Style) {
    let mut rest = text;
    while let Some(start) = find_github_mention_start(rest) {
        if start > 0 {
            current.push(DetailSegment::styled(rest[..start].to_string(), style));
        }

        let after_start = &rest[start + 1..];
        let username_len = after_start
            .char_indices()
            .take_while(|(_, ch)| is_github_username_char(*ch))
            .map(|(index, ch)| index + ch.len_utf8())
            .last()
            .unwrap_or(0);
        if username_len == 0 {
            current.push(DetailSegment::styled("@", style));
            rest = after_start;
            continue;
        }

        let username = &after_start[..username_len];
        current.push(DetailSegment::link(
            format!("@{username}"),
            github_profile_url(username),
        ));
        rest = &after_start[username_len..];
    }

    if !rest.is_empty() {
        current.push(DetailSegment::styled(rest.to_string(), style));
    }
}

fn find_github_mention_start(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    for (index, ch) in text.char_indices() {
        if ch != '@' {
            continue;
        }
        if index > 0 {
            let previous = text[..index].chars().next_back();
            if previous.is_some_and(|ch| {
                is_github_username_char(ch) || matches!(ch, '.' | '_' | '+' | '/')
            }) {
                continue;
            }
        }
        let next_index = index + 1;
        if next_index >= bytes.len() {
            continue;
        }
        let Some(next) = text[next_index..].chars().next() else {
            continue;
        };
        if next.is_ascii_alphanumeric() {
            return Some(index);
        }
    }
    None
}

fn is_github_username_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '-'
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

fn table_separator_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn action_style() -> Style {
    Style::default()
        .fg(Color::LightMagenta)
        .add_modifier(Modifier::UNDERLINED)
}

fn quote_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn code_plain_style() -> Style {
    Style::default().fg(Color::Gray)
}

fn rust_keyword_style() -> Style {
    Style::default()
        .fg(Color::LightMagenta)
        .add_modifier(Modifier::BOLD)
}

fn rust_type_style() -> Style {
    Style::default().fg(Color::Cyan)
}

fn rust_string_style() -> Style {
    Style::default().fg(Color::Yellow)
}

fn rust_comment_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn rust_macro_style() -> Style {
    Style::default().fg(Color::LightBlue)
}

fn rust_number_style() -> Style {
    Style::default().fg(Color::Yellow)
}

fn log_error_style() -> Style {
    Style::default()
        .fg(Color::LightRed)
        .add_modifier(Modifier::BOLD)
}

fn log_warning_style() -> Style {
    Style::default().fg(Color::Yellow)
}

fn log_info_style() -> Style {
    Style::default().fg(Color::LightBlue)
}

fn log_meta_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn diff_file_style() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

fn diff_hunk_style() -> Style {
    Style::default().fg(Color::LightBlue)
}

fn diff_gutter_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn diff_added_style() -> Style {
    Style::default().fg(Color::LightGreen)
}

fn diff_removed_style() -> Style {
    Style::default().fg(Color::LightRed)
}

fn diff_context_style() -> Style {
    Style::default().fg(Color::Gray)
}

fn diff_metadata_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn diff_inline_comment_marker_style() -> Style {
    Style::default()
        .fg(Color::LightMagenta)
        .add_modifier(Modifier::BOLD)
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
        Style::default()
            .fg(Color::LightYellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn comment_search_match_style() -> Style {
    Style::default()
        .fg(Color::LightMagenta)
        .add_modifier(Modifier::BOLD)
}

fn comment_selected_rail_style() -> Style {
    Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD)
}

fn comment_thread_style() -> Style {
    Style::default().fg(Color::DarkGray)
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

fn is_diff_key(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char(value) if value.eq_ignore_ascii_case(&'v'))
}

fn sorted_strings(values: &HashSet<String>) -> Vec<String> {
    let mut values = values.iter().cloned().collect::<Vec<_>>();
    values.sort();
    values
}

fn item_supports_details_memory(item: &WorkItem) -> bool {
    matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest) && item.number.is_some()
}

fn details_snapshot_hash(item: &WorkItem) -> String {
    let value = serde_json::json!({
        "kind": item.kind,
        "repo": &item.repo,
        "number": item.number,
        "title": &item.title,
        "body": &item.body,
        "author": &item.author,
        "state": &item.state,
        "url": &item.url,
        "updated_at": &item.updated_at,
        "labels": &item.labels,
        "comments_count": item.comments,
    });
    let bytes = serde_json::to_vec(&value).unwrap_or_default();
    format!("{:x}", md5::compute(bytes))
}

fn comments_snapshot_hash(comments: &[CommentPreview]) -> String {
    let bytes = serde_json::to_vec(comments).unwrap_or_default();
    format!("{:x}", md5::compute(bytes))
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
        let conversation_details_state = ui_state
            .details_scroll_by_item
            .iter()
            .map(|(item_id, details_scroll)| {
                (
                    item_id.clone(),
                    ConversationDetailsState {
                        details_scroll: *details_scroll,
                        selected_comment_index: ui_state
                            .selected_comment_index_by_item
                            .get(item_id)
                            .copied()
                            .unwrap_or(0),
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        let mut state = Self {
            active_view,
            sections,
            section_index: ui_state.section_index.clone(),
            selected_index: ui_state.selected_index.clone(),
            list_scroll_offset: HashMap::new(),
            focus: FocusTarget::List,
            details_scroll,
            details_mode: DetailsMode::Conversation,
            list_width_percent: ui_state.list_width_percent,
            dragging_split: false,
            split_drag_changed: false,
            search_active: false,
            search_query: String::new(),
            comment_search_active: false,
            comment_search_query: String::new(),
            global_search_active: false,
            global_search_query: String::new(),
            global_search_running: false,
            global_search_return_view: None,
            global_search_scope: None,
            global_search_started_at: None,
            status: "loading snapshot; background refresh started".to_string(),
            refreshing: false,
            last_refresh_request: Instant::now(),
            details: HashMap::new(),
            diffs: HashMap::new(),
            selected_diff_file: ui_state.selected_diff_file.clone(),
            selected_diff_line: ui_state.selected_diff_line.clone(),
            diff_mark: HashMap::new(),
            last_diff_click: None,
            diff_mode_state: HashMap::new(),
            conversation_details_state,
            viewed_details_snapshot: ui_state.viewed_details_snapshot.clone(),
            viewed_comments_snapshot: ui_state.viewed_comments_snapshot.clone(),
            action_hints: HashMap::new(),
            details_stale: HashSet::new(),
            details_refreshing: HashSet::new(),
            pending_details_load: None,
            notification_read_pending: HashSet::new(),
            selected_comment_index: 0,
            expanded_comments: ui_state.expanded_comments.iter().cloned().collect(),
            comment_dialog: None,
            posting_comment: false,
            pr_action_dialog: None,
            pr_action_running: false,
            setup_dialog: None,
            startup_dialog: None,
            message_dialog: None,
            mouse_capture_enabled: true,
            help_dialog: false,
            diff_return_state: None,
        };
        state.clamp_positions();
        state.focus = if matches!(focus, FocusTarget::Details) && state.current_item().is_none() {
            FocusTarget::List
        } else {
            focus
        };
        state.details_scroll = details_scroll;
        state.selected_comment_index = selected_comment_index;
        if ui_state.details_mode == "diff" {
            state.restore_saved_details_mode();
        }
        state
    }

    fn ui_state(&self) -> UiState {
        let mut conversation_details_state = self.conversation_details_state.clone();
        if self.focus == FocusTarget::Details
            && self.details_mode == DetailsMode::Conversation
            && let Some(item) = self.current_item()
            && item_supports_details_memory(item)
        {
            conversation_details_state.insert(
                item.id.clone(),
                ConversationDetailsState {
                    details_scroll: self.details_scroll,
                    selected_comment_index: self.selected_comment_index,
                },
            );
        }

        UiState {
            list_width_percent: self.list_width_percent,
            active_view: self.active_view.clone(),
            section_index: self.section_index.clone(),
            selected_index: self.selected_index.clone(),
            focus: self.focus.as_state_str().to_string(),
            details_mode: self.details_mode.as_state_str().to_string(),
            details_scroll: self.details_scroll,
            selected_comment_index: self.selected_comment_index,
            expanded_comments: sorted_strings(&self.expanded_comments),
            details_scroll_by_item: conversation_details_state
                .iter()
                .map(|(item_id, state)| (item_id.clone(), state.details_scroll))
                .collect(),
            selected_comment_index_by_item: conversation_details_state
                .iter()
                .map(|(item_id, state)| (item_id.clone(), state.selected_comment_index))
                .collect(),
            viewed_details_snapshot: self.viewed_details_snapshot.clone(),
            viewed_comments_snapshot: self.viewed_comments_snapshot.clone(),
            selected_diff_file: self.selected_diff_file.clone(),
            selected_diff_line: self.selected_diff_line.clone(),
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

    fn show_startup_initializing(&mut self) {
        self.startup_dialog = Some(StartupDialog::Initializing);
        self.status = "initializing; refreshing from GitHub".to_string();
    }

    fn show_setup_dialog(&mut self, dialog: SetupDialog) {
        self.setup_dialog = Some(dialog);
        self.startup_dialog = None;
        self.status = match dialog {
            SetupDialog::MissingGh => {
                "GitHub CLI missing: install `gh`, then run `gh auth login`".to_string()
            }
            SetupDialog::AuthRequired => {
                "GitHub CLI auth required: run `gh auth login`".to_string()
            }
        };
    }

    fn apply_refreshed_section(&mut self, section: SectionSnapshot, save_error: Option<String>) {
        let anchor = self.current_refresh_anchor();
        let previous_details_scroll = self.details_scroll;
        let previous_comment_index = self.selected_comment_index;
        let title = section.title.clone();
        let section_error = section.error.clone();
        let setup_dialog = section_error.as_deref().and_then(setup_dialog_from_error);

        let current = std::mem::take(&mut self.sections);
        self.sections = merge_refreshed_sections(current, vec![section]);

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

        if self.setup_dialog.is_none() {
            self.setup_dialog = setup_dialog;
        }
        if matches!(self.startup_dialog, Some(StartupDialog::Initializing)) {
            self.startup_dialog = if self.setup_dialog.is_some() {
                None
            } else {
                Some(StartupDialog::Ready)
            };
        }

        self.status = match (section_error.as_deref(), save_error) {
            (None, None) => format!("loaded {title}; still refreshing"),
            (Some(error), None) => refresh_error_status(1, Some(error)),
            (_, Some(error)) => format!("snapshot save failed: {error}"),
        };
    }

    fn handle_msg(&mut self, message: AppMsg) {
        match message {
            AppMsg::RefreshStarted => {
                self.refreshing = true;
                self.last_refresh_request = Instant::now();
                self.status = "refreshing from GitHub".to_string();
            }
            AppMsg::RefreshSectionLoaded {
                section,
                save_error,
            } => {
                self.apply_refreshed_section(section, save_error);
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
                if matches!(self.startup_dialog, Some(StartupDialog::Initializing)) {
                    self.startup_dialog = if setup_dialog.is_some() {
                        None
                    } else {
                        Some(StartupDialog::Ready)
                    };
                }
                self.setup_dialog = setup_dialog;
                self.status = match (errors, save_error) {
                    (0, None) => "refresh complete".to_string(),
                    (count, None) => refresh_error_status(count, first_error.as_deref()),
                    (_, Some(error)) => format!("snapshot save failed: {error}"),
                };
            }
            AppMsg::CommentsLoaded { item_id, comments } => match comments {
                Ok(comments) => {
                    self.details_stale.remove(&item_id);
                    self.details_refreshing.remove(&item_id);
                    self.details
                        .insert(item_id.clone(), DetailState::Loaded(comments));
                    self.clamp_selected_comment();
                    self.mark_current_details_viewed_if_current(&item_id);
                }
                Err(error) => {
                    if self.setup_dialog.is_none() {
                        self.setup_dialog = setup_dialog_from_error(&error);
                    }
                    self.details_stale.remove(&item_id);
                    self.details_refreshing.remove(&item_id);
                    self.details
                        .insert(item_id.clone(), DetailState::Error(error));
                }
            },
            AppMsg::ActionHintsLoaded { item_id, actions } => match actions {
                Ok(actions) => {
                    self.action_hints
                        .insert(item_id, ActionHintState::Loaded(actions));
                }
                Err(error) => {
                    self.action_hints
                        .insert(item_id, ActionHintState::Error(error));
                }
            },
            AppMsg::DiffLoaded { item_id, diff } => match diff {
                Ok(diff) => {
                    let file_count = diff.files.len();
                    if file_count == 0 {
                        self.selected_diff_file.insert(item_id.clone(), 0);
                        self.selected_diff_line.insert(item_id.clone(), 0);
                    } else {
                        let selected = self.selected_diff_file.entry(item_id.clone()).or_insert(0);
                        *selected = (*selected).min(file_count - 1);
                        let selected_line =
                            self.selected_diff_line.entry(item_id.clone()).or_insert(0);
                        let line_count = diff_review_targets(&diff.files[*selected]).len();
                        if line_count == 0 {
                            *selected_line = 0;
                        } else {
                            *selected_line = (*selected_line).min(line_count - 1);
                        }
                    }
                    self.diffs.insert(item_id, DiffState::Loaded(diff));
                    self.status = format!("diff loaded: {file_count} file(s)");
                }
                Err(error) => {
                    if self.setup_dialog.is_none() {
                        self.setup_dialog = setup_dialog_from_error(&error);
                    }
                    self.diffs.insert(item_id, DiffState::Error(error));
                    self.status = "diff load failed".to_string();
                }
            },
            AppMsg::CommentPosted { item_id, result } => match result {
                Ok(comments) => {
                    self.selected_comment_index = comments.len().saturating_sub(1);
                    self.details_stale.remove(&item_id);
                    self.details_refreshing.remove(&item_id);
                    self.details
                        .insert(item_id.clone(), DetailState::Loaded(comments));
                    self.clamp_selected_comment();
                    self.mark_current_details_viewed_if_current(&item_id);
                    self.posting_comment = false;
                    self.status = "comment posted".to_string();
                    self.message_dialog = Some(success_message_dialog(
                        "Comment Posted",
                        "GitHub accepted the comment and comments were refreshed.",
                    ));
                }
                Err(error) => {
                    let setup_dialog = setup_dialog_from_error(&error);
                    if self.setup_dialog.is_none() {
                        self.setup_dialog = setup_dialog;
                    }
                    if setup_dialog.is_none() {
                        self.message_dialog = Some(message_dialog(
                            "Comment Failed",
                            operation_error_body(&error),
                        ));
                    } else {
                        self.message_dialog = None;
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
                    self.details_refreshing.remove(&item_id);
                    self.details
                        .insert(item_id.clone(), DetailState::Loaded(comments));
                    self.clamp_selected_comment();
                    self.mark_current_details_viewed_if_current(&item_id);
                    self.posting_comment = false;
                    self.status = "comment updated".to_string();
                    self.message_dialog = Some(success_message_dialog(
                        "Comment Updated",
                        "GitHub accepted the update and comments were refreshed.",
                    ));
                }
                Err(error) => {
                    let setup_dialog = setup_dialog_from_error(&error);
                    if self.setup_dialog.is_none() {
                        self.setup_dialog = setup_dialog;
                    }
                    if setup_dialog.is_none() {
                        self.message_dialog = Some(message_dialog(
                            "Update Failed",
                            operation_error_body(&error),
                        ));
                    } else {
                        self.message_dialog = None;
                    }
                    self.posting_comment = false;
                    self.status = "comment update failed".to_string();
                }
            },
            AppMsg::ReviewCommentPosted { item_id, result } => match result {
                Ok(()) => {
                    self.details_stale.insert(item_id);
                    self.posting_comment = false;
                    self.status = "review comment posted".to_string();
                    self.message_dialog = Some(success_message_dialog(
                        "Review Comment Posted",
                        "GitHub accepted the review comment.",
                    ));
                }
                Err(error) => {
                    let setup_dialog = setup_dialog_from_error(&error);
                    if self.setup_dialog.is_none() {
                        self.setup_dialog = setup_dialog;
                    }
                    if setup_dialog.is_none() {
                        self.message_dialog = Some(message_dialog(
                            "Review Comment Failed",
                            operation_error_body(&error),
                        ));
                    } else {
                        self.message_dialog = None;
                    }
                    self.posting_comment = false;
                    self.status = "review comment failed".to_string();
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
                            PrAction::Checkout => "pull request checked out locally".to_string(),
                        };
                        self.message_dialog = Some(success_message_dialog(
                            pr_action_success_title(action),
                            pr_action_success_body(action),
                        ));
                    }
                    Err(error) => {
                        let setup_dialog = setup_dialog_from_error(&error);
                        if self.setup_dialog.is_none() {
                            self.setup_dialog = setup_dialog;
                        }
                        if setup_dialog.is_none() {
                            self.message_dialog = Some(message_dialog(
                                pr_action_error_title(action),
                                pr_action_error_body(&error),
                            ));
                        } else {
                            self.message_dialog = None;
                        }
                        self.status = pr_action_error_status(action).to_string();
                    }
                }
            }
            AppMsg::PrCheckoutFinished { result } => {
                self.pr_action_running = false;
                self.pr_action_dialog = None;
                match result {
                    Ok(result) => {
                        self.status = "pull request checked out locally".to_string();
                        self.message_dialog = Some(persistent_success_message_dialog(
                            pr_action_success_title(PrAction::Checkout),
                            format!(
                                "{}\n\n{}\n\n{}",
                                result.command,
                                checkout_directory_notice(&result.directory),
                                result.output
                            ),
                        ));
                    }
                    Err(error) => {
                        let setup_dialog = setup_dialog_from_error(&error);
                        if self.setup_dialog.is_none() {
                            self.setup_dialog = setup_dialog;
                        }
                        if setup_dialog.is_none() {
                            self.message_dialog = Some(message_dialog(
                                pr_action_error_title(PrAction::Checkout),
                                pr_action_error_body(&error),
                            ));
                        } else {
                            self.message_dialog = None;
                        }
                        self.status = pr_action_error_status(PrAction::Checkout).to_string();
                    }
                }
            }
            AppMsg::NotificationReadFinished { thread_id, result } => {
                self.notification_read_pending.remove(&thread_id);
                match result {
                    Ok(save_error) => {
                        let changed = self.apply_notification_read_local(&thread_id);
                        self.status = match (changed, save_error) {
                            (_, Some(error)) => {
                                format!("notification marked read; snapshot save failed: {error}")
                            }
                            (true, None) => "notification marked read".to_string(),
                            (false, None) => "notification read synced".to_string(),
                        };
                    }
                    Err(error) => {
                        let setup_dialog = setup_dialog_from_error(&error);
                        if self.setup_dialog.is_none() {
                            self.setup_dialog = setup_dialog;
                        }
                        self.status = format!(
                            "notification read sync failed: {}",
                            operation_error_body(&error)
                        );
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
                self.global_search_started_at = None;
                self.global_search_scope = None;
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

    fn dismiss_startup_dialog(&mut self) {
        self.startup_dialog = None;
        self.status = "startup hint dismissed".to_string();
    }

    fn dismiss_message_dialog(&mut self) {
        self.message_dialog = None;
        self.status = "message dismissed".to_string();
    }

    fn dismiss_expired_message_dialog(&mut self, now: Instant) {
        if self
            .message_dialog
            .as_ref()
            .and_then(|dialog| dialog.auto_close_at)
            .is_some_and(|deadline| now >= deadline)
        {
            self.message_dialog = None;
        }
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
        self.comment_search_active = false;
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
                    PrAction::Checkout => {}
                }
            }
        }
    }

    fn mark_current_notification_read(
        &mut self,
        store: &SnapshotStore,
        tx: &UnboundedSender<AppMsg>,
    ) {
        let Some(item) = self.current_item() else {
            return;
        };
        if !item.unread.unwrap_or(false) {
            return;
        }

        let thread_id = item.id.clone();
        self.mark_notification_read(thread_id, store, tx);
    }

    fn mark_notification_read(
        &mut self,
        thread_id: String,
        store: &SnapshotStore,
        tx: &UnboundedSender<AppMsg>,
    ) {
        if !self.notification_read_pending.insert(thread_id.clone()) {
            return;
        }

        self.status = "marking notification read".to_string();
        start_notification_read_sync(thread_id, store.clone(), tx.clone());
    }

    fn notification_thread_id_at_position(&self, position: usize) -> Option<String> {
        let section = self.current_section()?;
        let filtered_indices = self.filtered_indices(section);
        let item_index = filtered_indices.get(position)?;
        let item = section.items.get(*item_index)?;
        item.unread.unwrap_or(false).then(|| item.id.clone())
    }

    fn apply_notification_read_local(&mut self, thread_id: &str) -> bool {
        let mut changed = false;
        for section in &mut self.sections {
            changed |= mark_notification_read_in_section(section, thread_id);
        }
        if changed {
            self.clamp_positions();
        }
        changed
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

    fn is_global_search_results_view(&self) -> bool {
        same_view_key(&self.active_view, &global_search_view_key())
    }

    fn leave_global_search_results(&mut self) {
        if !self.is_global_search_results_view() {
            return;
        }

        let search_view = global_search_view_key();
        self.sections
            .retain(|section| section_view_key(section) != search_view);
        let return_view = self
            .global_search_return_view
            .take()
            .filter(|view| self.section_view_exists(view))
            .or_else(|| {
                let builtin = builtin_view_key(SectionKind::PullRequests);
                self.section_view_exists(&builtin).then_some(builtin)
            })
            .or_else(|| self.sections.first().map(section_view_key));
        if let Some(view) = return_view {
            self.active_view = view;
        }
        self.global_search_active = false;
        self.global_search_query.clear();
        self.search_active = false;
        self.search_query.clear();
        self.comment_search_active = false;
        self.details_scroll = 0;
        self.selected_comment_index = 0;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.focus = FocusTarget::List;
        self.status = "search results cleared".to_string();
        self.clamp_positions();
    }

    fn section_view_exists(&self, view: &str) -> bool {
        self.sections
            .iter()
            .any(|section| same_view_key(&section_view_key(section), view))
    }

    fn ensure_current_details_loading(&mut self, tx: &UnboundedSender<AppMsg>) {
        let Some(item) = self.current_item().cloned() else {
            self.pending_details_load = None;
            return;
        };
        if !matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest) || item.number.is_none() {
            self.pending_details_load = None;
            return;
        }
        if !self.details_load_needed(&item) {
            self.pending_details_load = None;
            return;
        }
        if !self.details_load_ready(&item.id) {
            return;
        }

        self.pending_details_load = None;
        if self.start_comments_load_if_needed(&item) {
            start_comments_load(item.clone(), tx.clone());
        }
        if self.start_action_hints_load_if_needed(&item) {
            start_action_hints_load(item, tx.clone());
        }
    }

    fn details_load_needed(&self, item: &WorkItem) -> bool {
        self.comments_load_needed(item) || self.action_hints_load_needed(item)
    }

    fn comments_load_needed(&self, item: &WorkItem) -> bool {
        !self.details.contains_key(&item.id) || self.details_stale.contains(&item.id)
    }

    fn action_hints_load_needed(&self, item: &WorkItem) -> bool {
        matches!(item.kind, ItemKind::PullRequest) && !self.action_hints.contains_key(&item.id)
    }

    fn details_load_ready(&mut self, item_id: &str) -> bool {
        if self.focus == FocusTarget::Details {
            self.pending_details_load = None;
            return true;
        }

        let now = Instant::now();
        match self.pending_details_load.as_ref() {
            Some(pending) if pending.item_id == item_id && now >= pending.ready_at => true,
            Some(pending) if pending.item_id == item_id => false,
            _ => {
                self.pending_details_load = Some(PendingDetailsLoad {
                    item_id: item_id.to_string(),
                    ready_at: now + DETAILS_LOAD_DEBOUNCE,
                });
                false
            }
        }
    }

    fn ensure_current_diff_loading(&mut self, tx: &UnboundedSender<AppMsg>) {
        if self.details_mode != DetailsMode::Diff {
            return;
        }
        let Some(item) = self.current_item().cloned() else {
            return;
        };
        if !matches!(item.kind, ItemKind::PullRequest) || item.number.is_none() {
            return;
        }
        if self.diffs.contains_key(&item.id) {
            return;
        }

        self.diffs.insert(item.id.clone(), DiffState::Loading);
        start_diff_load(item, tx.clone());
    }

    fn current_diff(&self) -> Option<&DiffState> {
        self.current_item()
            .and_then(|item| self.diffs.get(&item.id))
    }

    fn selected_diff_file_index_for(&self, item_id: &str, diff: &PullRequestDiff) -> usize {
        if diff.files.is_empty() {
            return 0;
        }
        self.selected_diff_file
            .get(item_id)
            .copied()
            .unwrap_or(0)
            .min(diff.files.len() - 1)
    }

    fn selected_diff_line_index_for(&self, item_id: &str, file: &DiffFile) -> usize {
        let count = diff_review_targets(file).len();
        if count == 0 {
            return 0;
        }
        self.selected_diff_line
            .get(item_id)
            .copied()
            .unwrap_or(0)
            .min(count - 1)
    }

    fn selected_diff_range_for(&self, item_id: &str) -> Option<(usize, usize)> {
        let mark = self.diff_mark.get(item_id)?;
        if !mark.complete {
            return None;
        }
        Some(mark.range())
    }

    fn diff_mark_range_for(&self, item_id: &str) -> Option<(usize, usize)> {
        let mark = self.diff_mark.get(item_id)?;
        Some(mark.range())
    }

    fn current_diff_review_targets(&self) -> Option<Vec<DiffReviewTarget>> {
        let item = self.current_item()?;
        let diff = match self.diffs.get(&item.id)? {
            DiffState::Loaded(diff) => diff,
            _ => return None,
        };
        if diff.files.is_empty() {
            return Some(Vec::new());
        }
        let file_index = self.selected_diff_file_index_for(&item.id, diff);
        Some(diff_review_targets(&diff.files[file_index]))
    }

    #[cfg(test)]
    fn current_diff_review_target(&self) -> Option<DiffReviewTarget> {
        self.current_diff_review_target_result().ok().flatten()
    }

    fn current_diff_review_target_result(&self) -> Result<Option<DiffReviewTarget>, String> {
        let Some(item) = self.current_item() else {
            return Ok(None);
        };
        let Some(targets) = self.current_diff_review_targets() else {
            return Ok(None);
        };
        if targets.is_empty() {
            return Ok(None);
        }
        if let Some((start, end)) = self.selected_diff_range_for(&item.id) {
            return diff_review_target_from_range(&targets, start, end).map(Some);
        }
        let index = self
            .selected_diff_line
            .get(&item.id)
            .copied()
            .unwrap_or(0)
            .min(targets.len() - 1);
        Ok(targets.get(index).cloned())
    }

    fn current_diff_file_count(&self) -> Option<usize> {
        self.current_item()
            .and_then(|item| match self.diffs.get(&item.id) {
                Some(DiffState::Loaded(diff)) => Some(diff.files.len()),
                _ => None,
            })
    }

    fn current_diff_file_order(&self) -> Option<Vec<usize>> {
        self.current_item()
            .and_then(|item| match self.diffs.get(&item.id) {
                Some(DiffState::Loaded(diff)) => Some(
                    diff_tree_entries(diff)
                        .into_iter()
                        .filter_map(|entry| entry.file_index)
                        .collect(),
                ),
                _ => None,
            })
    }

    fn start_comments_load_if_needed(&mut self, item: &WorkItem) -> bool {
        let should_refresh = self.details_stale.remove(&item.id);
        if self.details.contains_key(&item.id) && !should_refresh {
            return false;
        }
        if should_refresh {
            self.details_refreshing.insert(item.id.clone());
        }
        if !self.details.contains_key(&item.id) {
            self.details.insert(item.id.clone(), DetailState::Loading);
        }
        true
    }

    fn start_action_hints_load_if_needed(&mut self, item: &WorkItem) -> bool {
        if !self.action_hints_load_needed(item) {
            return false;
        }
        self.action_hints
            .insert(item.id.clone(), ActionHintState::Loading);
        true
    }

    fn switch_view(&mut self, view: impl Into<String>) {
        self.save_current_conversation_details_state();
        let view = view.into();
        let focus = self.focus;
        self.active_view = view;
        self.details_scroll = 0;
        self.selected_comment_index = 0;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.global_search_active = false;
        self.comment_search_active = false;
        self.comment_search_query.clear();
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

    fn move_focused_tab_group(&mut self, delta: isize) {
        if self.focus == FocusTarget::Sections {
            self.move_section(delta);
        } else {
            self.move_view(delta);
        }
    }

    fn focus_primary_list(&mut self) {
        self.save_current_conversation_details_state();
        self.focus = FocusTarget::List;
        if self.details_mode != DetailsMode::Diff {
            self.details_scroll = 0;
            self.selected_comment_index = 0;
        }
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.search_active = false;
        self.comment_search_active = false;
        self.global_search_active = false;
        self.status = if self.details_mode == DetailsMode::Diff {
            "files focused".to_string()
        } else {
            "list focused".to_string()
        };
        self.clamp_positions();
    }

    fn focus_ghr(&mut self) {
        self.save_current_conversation_details_state();
        self.focus = FocusTarget::Ghr;
        self.search_active = false;
        self.comment_search_active = false;
        self.global_search_active = false;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.status = "ghr focused".to_string();
    }

    fn focus_sections(&mut self) {
        self.save_current_conversation_details_state();
        self.focus = FocusTarget::Sections;
        self.search_active = false;
        self.comment_search_active = false;
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
        self.save_current_conversation_details_state();
        let current = self.current_section_position().min(len - 1);
        let next = move_wrapping(current, len, delta);
        self.set_current_section_position(next);
        self.set_current_selected_position(0);
        self.clear_current_list_scroll_offset();
        self.focus = FocusTarget::Sections;
        self.details_scroll = 0;
        self.selected_comment_index = 0;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.global_search_active = false;
        self.comment_search_active = false;
        self.comment_search_query.clear();
    }

    fn select_section(&mut self, index: usize) {
        let len = self.visible_sections().len();
        if len == 0 {
            return;
        }
        self.save_current_conversation_details_state();
        self.set_current_section_position(index.min(len - 1));
        self.set_current_selected_position(0);
        self.clear_current_list_scroll_offset();
        self.focus = FocusTarget::Sections;
        self.details_scroll = 0;
        self.selected_comment_index = 0;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.search_active = false;
        self.comment_search_active = false;
        self.comment_search_query.clear();
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
        self.save_current_conversation_details_state();
        let current = self.current_selected_position().min(len - 1);
        self.set_current_selected_position(move_bounded(current, len, delta));
        self.clear_current_list_scroll_offset();
        self.details_scroll = 0;
        self.selected_comment_index = 0;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.global_search_active = false;
        self.comment_search_active = false;
        self.comment_search_query.clear();
    }

    fn set_selection(&mut self, index: usize) {
        self.save_current_conversation_details_state();
        self.set_current_selected_position(index);
        self.clear_current_list_scroll_offset();
        self.details_scroll = 0;
        self.selected_comment_index = 0;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.global_search_active = false;
        self.comment_search_active = false;
        self.comment_search_query.clear();
        self.clamp_positions();
    }

    fn scroll_list_viewport(&mut self, area: Rect, delta: isize) {
        let Some(section) = self.current_section() else {
            return;
        };
        let len = self.filtered_indices(section).len();
        if len == 0 {
            return;
        }

        let visible_rows = usize::from(table_visible_rows(area).max(1));
        let current = self.current_list_scroll_offset(len, visible_rows);
        let max_offset = max_table_viewport_offset(len, visible_rows);
        let next = move_bounded(current, max_offset.saturating_add(1), delta);
        self.set_current_list_scroll_offset(next);
        self.status = "list scrolled".to_string();
    }

    fn select_diff_file(&mut self, index: usize) {
        let Some(item_id) = self.current_item().map(|item| item.id.clone()) else {
            self.status = "nothing to diff".to_string();
            return;
        };
        let Some(count) = self.current_diff_file_count() else {
            self.status = "diff still loading".to_string();
            return;
        };
        if count == 0 {
            self.selected_diff_file.insert(item_id.clone(), 0);
            self.selected_diff_line.insert(item_id.clone(), 0);
            self.diff_mark.remove(&item_id);
            self.details_scroll = 0;
            self.status = "no diff files".to_string();
            return;
        }

        let next = index.min(count - 1);
        self.selected_diff_file.insert(item_id.clone(), next);
        self.selected_diff_line.insert(item_id.clone(), 0);
        self.diff_mark.remove(&item_id);
        self.details_scroll = 0;
        let position = self
            .current_diff_file_order()
            .and_then(|order| {
                order
                    .iter()
                    .position(|file_index| *file_index == next)
                    .map(|position| position + 1)
            })
            .unwrap_or(next + 1);
        self.status = format!("file {position}/{count} focused");
    }

    fn select_diff_line(&mut self, index: usize, area: Option<Rect>) {
        let Some(item_id) = self.current_item().map(|item| item.id.clone()) else {
            self.status = "nothing to diff".to_string();
            return;
        };
        let Some(targets) = self.current_diff_review_targets() else {
            self.status = "diff still loading".to_string();
            return;
        };
        if targets.is_empty() {
            self.selected_diff_line.insert(item_id.clone(), 0);
            self.diff_mark.remove(&item_id);
            self.status = "no reviewable diff lines".to_string();
            return;
        }

        let next = index.min(targets.len() - 1);
        self.selected_diff_line.insert(item_id.clone(), next);
        self.update_diff_mark_after_line_select(next);
        self.ensure_selected_diff_line_visible(area);
        if let Some(target) = targets.get(next) {
            self.status = format!("line {}:{} selected", target.path, target.line);
        }
    }

    fn handle_diff_line_click(&mut self, index: usize, area: Option<Rect>) {
        let Some(item_id) = self.current_item().map(|item| item.id.clone()) else {
            self.status = "nothing to diff".to_string();
            return;
        };
        let file_index = self.selected_diff_file.get(&item_id).copied().unwrap_or(0);
        let is_double_click = self.is_diff_double_click(&item_id, file_index, index);
        let had_pending_mark = self
            .diff_mark
            .get(&item_id)
            .is_some_and(|mark| mark.is_pending());
        self.select_diff_line(index, area);

        if is_double_click {
            self.end_diff_mark();
        } else if had_pending_mark {
            self.update_diff_mark_status(&item_id);
        } else {
            self.begin_diff_mark();
        }
    }

    fn is_diff_double_click(
        &mut self,
        item_id: &str,
        file_index: usize,
        review_index: usize,
    ) -> bool {
        let now = Instant::now();
        let is_double_click = self
            .last_diff_click
            .as_ref()
            .is_some_and(|last| last.matches(item_id, file_index, review_index, now));
        self.last_diff_click = Some(DiffClickState {
            item_id: item_id.to_string(),
            file_index,
            review_index,
            at: now,
        });
        is_double_click
    }

    fn begin_diff_mark(&mut self) {
        let Some((item_id, targets)) = self.diff_review_context("nothing to mark") else {
            return;
        };
        let current = self.selected_diff_review_index(&item_id, targets.len());

        self.diff_mark
            .insert(item_id.clone(), DiffMarkState::pending(current));
        self.update_diff_mark_status(&item_id);
    }

    fn end_diff_mark(&mut self) {
        let Some((item_id, targets)) = self.diff_review_context("nothing to mark") else {
            return;
        };
        let current = self.selected_diff_review_index(&item_id, targets.len());

        let Some(mark) = self.diff_mark.get_mut(&item_id) else {
            self.status = "press m to begin mark first".to_string();
            return;
        };
        mark.complete_at(current);
        self.update_diff_mark_status(&item_id);
    }

    fn diff_review_context(
        &mut self,
        missing_item_status: &'static str,
    ) -> Option<(String, Vec<DiffReviewTarget>)> {
        let Some(item_id) = self.current_item().map(|item| item.id.clone()) else {
            self.status = missing_item_status.to_string();
            return None;
        };
        let Some(targets) = self.current_diff_review_targets() else {
            self.status = "diff still loading".to_string();
            return None;
        };
        if targets.is_empty() {
            self.status = "no reviewable diff lines".to_string();
            return None;
        }
        Some((item_id, targets))
    }

    fn selected_diff_review_index(&self, item_id: &str, target_count: usize) -> usize {
        self.selected_diff_line
            .get(item_id)
            .copied()
            .unwrap_or(0)
            .min(target_count.saturating_sub(1))
    }

    fn update_diff_mark_after_line_select(&mut self, selected: usize) {
        let Some(item_id) = self.current_item().map(|item| item.id.clone()) else {
            return;
        };
        if let Some(mark) = self.diff_mark.get_mut(&item_id) {
            if mark.complete {
                self.diff_mark.remove(&item_id);
            } else {
                mark.focus = selected;
            }
        }
    }

    fn update_diff_mark_status(&mut self, item_id: &str) {
        let Some(mark) = self.diff_mark.get(item_id).copied() else {
            return;
        };
        let Some(targets) = self.current_diff_review_targets() else {
            self.status = "diff still loading".to_string();
            return;
        };
        if targets.is_empty() {
            self.status = "no reviewable diff lines".to_string();
            return;
        }
        let (start, end) = mark.range();
        match diff_review_target_from_range(&targets, start, end) {
            Ok(target) => {
                if mark.complete {
                    self.status = format!("selected {}", target.location_label());
                } else if target.is_range() {
                    self.status =
                        format!("marking {}; press e to end mark", target.location_label());
                } else {
                    self.status = format!(
                        "mark started at {}; move highlight, then press e",
                        target.location_label()
                    );
                }
            }
            Err(error) => {
                self.status = error;
            }
        }
    }

    fn move_diff_line(&mut self, delta: isize, area: Option<Rect>) {
        let Some(item_id) = self.current_item().map(|item| item.id.clone()) else {
            self.status = "nothing to diff".to_string();
            return;
        };
        let Some(targets) = self.current_diff_review_targets() else {
            self.status = "diff still loading".to_string();
            return;
        };
        if targets.is_empty() {
            self.selected_diff_line.insert(item_id, 0);
            self.status = "no reviewable diff lines".to_string();
            return;
        }

        let current = self
            .selected_diff_line
            .get(&item_id)
            .copied()
            .unwrap_or(0)
            .min(targets.len() - 1);
        self.select_diff_line(move_bounded(current, targets.len(), delta), area);
    }

    fn page_diff_lines(&mut self, direction: isize, area: Option<Rect>) {
        let Some(area) = area else {
            self.move_diff_line(diff_line_page_delta(self, None, direction), None);
            return;
        };

        let details_area = details_area_for(self, area);
        let inner = block_inner(details_area);
        let page_height = usize::from(inner.height.max(1));
        let max_scroll = usize::from(max_details_scroll(self, details_area));
        let current_scroll = usize::from(self.details_scroll);
        let next_scroll = if direction < 0 {
            current_scroll.saturating_sub(page_height)
        } else {
            current_scroll.saturating_add(page_height).min(max_scroll)
        };
        self.details_scroll = next_scroll.min(usize::from(u16::MAX)) as u16;
        self.select_first_visible_diff_line(area);
    }

    fn scroll_diff_details_to_top(&mut self, area: Option<Rect>) {
        self.details_scroll = 0;
        if let Some(area) = area {
            self.select_first_visible_diff_line(area);
        } else {
            self.select_diff_line(0, None);
        }
    }

    fn scroll_diff_details_to_bottom(&mut self, area: Option<Rect>) {
        let Some(area) = area else {
            self.select_last_diff_line(None);
            return;
        };
        let details_area = details_area_for(self, area);
        self.details_scroll = max_details_scroll(self, details_area);
        if !self.select_first_visible_diff_line(area) {
            self.select_last_diff_line(None);
        }
    }

    fn select_last_diff_line(&mut self, area: Option<Rect>) {
        let Some(targets) = self.current_diff_review_targets() else {
            self.status = "diff still loading".to_string();
            return;
        };
        if targets.is_empty() {
            self.status = "no reviewable diff lines".to_string();
            return;
        }
        self.select_diff_line(targets.len() - 1, area);
    }

    fn select_first_visible_diff_line(&mut self, area: Rect) -> bool {
        let details_area = details_area_for(self, area);
        let inner = block_inner(details_area);
        let start = usize::from(self.details_scroll);
        let end = start.saturating_add(usize::from(inner.height.max(1)));
        let next = {
            let document = build_details_document(self, inner.width);
            document
                .diff_lines
                .iter()
                .find(|line| line.line >= start && line.line < end)
                .map(|line| line.review_index)
        };

        if let Some(index) = next {
            self.select_diff_line(index, Some(area));
            true
        } else {
            false
        }
    }

    fn scroll_details_to_bottom(&mut self, area: Option<Rect>) {
        let Some(area) = area else {
            self.details_scroll = u16::MAX;
            return;
        };
        let details_area = details_area_for(self, area);
        self.details_scroll = max_details_scroll(self, details_area);
    }

    fn ensure_selected_diff_line_visible(&mut self, area: Option<Rect>) {
        let Some(area) = area else {
            return;
        };
        let details_area = details_area_for(self, area);
        let inner = block_inner(details_area);
        let document = build_details_document(self, inner.width);
        let Some(selected_line) = document.selected_diff_line else {
            return;
        };

        let visible_height = usize::from(inner.height.max(1));
        let current_scroll = usize::from(self.details_scroll);
        let next_scroll = if selected_line < current_scroll {
            selected_line
        } else if selected_line >= current_scroll.saturating_add(visible_height) {
            selected_line.saturating_sub(visible_height.saturating_sub(1))
        } else {
            current_scroll
        };
        self.details_scroll = next_scroll
            .min(usize::from(max_details_scroll(self, details_area)))
            .min(usize::from(u16::MAX)) as u16;
    }

    fn move_diff_file(&mut self, delta: isize) {
        let Some(item_id) = self.current_item().map(|item| item.id.clone()) else {
            self.status = "nothing to diff".to_string();
            return;
        };
        let Some(order) = self.current_diff_file_order() else {
            self.status = "diff still loading".to_string();
            return;
        };
        if order.is_empty() {
            self.selected_diff_file.insert(item_id.clone(), 0);
            self.selected_diff_line.insert(item_id, 0);
            self.details_scroll = 0;
            self.status = "no diff files".to_string();
            return;
        }

        let count = order.len();
        let current = self
            .selected_diff_file
            .get(&item_id)
            .copied()
            .unwrap_or_else(|| order[0]);
        let current_position = order
            .iter()
            .position(|file_index| *file_index == current)
            .unwrap_or(0);
        self.select_diff_file(order[move_wrapping(current_position, count, delta)]);
    }

    fn select_last(&mut self) {
        let Some(section) = self.current_section() else {
            return;
        };
        let len = self.filtered_indices(section).len();
        if len > 0 {
            self.save_current_conversation_details_state();
            self.set_current_selected_position(len - 1);
            self.details_scroll = 0;
            self.selected_comment_index = 0;
            self.comment_dialog = None;
            self.pr_action_dialog = None;
            self.global_search_active = false;
            self.comment_search_active = false;
            self.comment_search_query.clear();
        }
    }

    fn focus_details(&mut self) {
        if self.current_item().is_some() {
            if self.details_mode == DetailsMode::Conversation {
                self.restore_current_conversation_details_state();
            }
            self.focus = FocusTarget::Details;
            self.search_active = false;
            self.comment_search_active = false;
            self.global_search_active = false;
            if self.current_comments().is_some() {
                self.clamp_selected_comment();
            }
            if self.details_mode == DetailsMode::Conversation {
                self.mark_current_details_viewed();
            }
            self.status = "details focused".to_string();
        } else {
            self.status = "nothing to focus".to_string();
        }
    }

    fn restore_saved_details_mode(&mut self) {
        let Some(item) = self.current_item() else {
            return;
        };
        if !matches!(item.kind, ItemKind::PullRequest) {
            return;
        }

        let item_id = item.id.clone();
        self.details_mode = DetailsMode::Diff;
        self.selected_diff_file.entry(item_id.clone()).or_insert(0);
        self.selected_diff_line.entry(item_id).or_insert(0);
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.diff_return_state = Some(DiffReturnState {
            focus: FocusTarget::Details,
            details_scroll: 0,
            selected_comment_index: 0,
        });
    }

    fn show_diff(&mut self) {
        let Some(item) = self.current_item() else {
            self.status = "nothing to diff".to_string();
            return;
        };
        if !matches!(item.kind, ItemKind::PullRequest) {
            self.status = "diff only available for pull requests".to_string();
            return;
        }

        let item_id = item.id.clone();
        self.save_current_conversation_details_state();
        let loading = !self.diffs.contains_key(&item_id);
        let saved_diff_state = self.diff_mode_state.get(&item_id).cloned();
        if let Some(saved) = saved_diff_state.as_ref() {
            self.restore_diff_mode_state_for(&item_id, saved);
        } else {
            self.selected_diff_file.entry(item_id.clone()).or_insert(0);
            self.selected_diff_line.entry(item_id.clone()).or_insert(0);
        }
        if self.details_mode != DetailsMode::Diff {
            self.diff_return_state = Some(DiffReturnState {
                focus: self.focus,
                details_scroll: self.details_scroll,
                selected_comment_index: self.selected_comment_index,
            });
        }
        self.details_mode = DetailsMode::Diff;
        self.focus = saved_diff_state
            .as_ref()
            .map(|saved| saved.focus)
            .unwrap_or(FocusTarget::List);
        self.details_scroll = saved_diff_state
            .as_ref()
            .map(|saved| saved.details_scroll)
            .unwrap_or(0);
        self.search_active = false;
        self.comment_search_active = false;
        self.comment_search_query.clear();
        self.global_search_active = false;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.status = if loading {
            "loading diff".to_string()
        } else {
            "files focused".to_string()
        };
    }

    fn leave_diff(&mut self) {
        self.save_current_diff_mode_state();
        self.details_mode = DetailsMode::Conversation;
        self.search_active = false;
        self.comment_search_active = false;
        self.global_search_active = false;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        if let Some(previous) = self.diff_return_state.take() {
            self.focus = previous.focus;
            self.details_scroll = previous.details_scroll;
            self.selected_comment_index = previous.selected_comment_index;
            self.clamp_selected_comment();
            if self.focus == FocusTarget::Details && self.current_item().is_none() {
                self.focus = FocusTarget::List;
            }
        } else {
            self.focus = FocusTarget::Details;
            self.details_scroll = 0;
        }
        self.status = "returned from diff".to_string();
    }

    fn save_current_diff_mode_state(&mut self) {
        if self.details_mode != DetailsMode::Diff {
            return;
        }
        let Some(item_id) = self.current_item().map(|item| item.id.clone()) else {
            return;
        };
        self.diff_mode_state.insert(
            item_id.clone(),
            DiffModeState {
                focus: self.focus,
                details_scroll: self.details_scroll,
                selected_file: self.selected_diff_file.get(&item_id).copied().unwrap_or(0),
                selected_line: self.selected_diff_line.get(&item_id).copied().unwrap_or(0),
            },
        );
    }

    fn restore_diff_mode_state_for(&mut self, item_id: &str, saved: &DiffModeState) {
        let (selected_file, selected_line) = match self.diffs.get(item_id) {
            Some(DiffState::Loaded(diff)) if !diff.files.is_empty() => {
                let selected_file = saved.selected_file.min(diff.files.len() - 1);
                let line_count = diff_review_targets(&diff.files[selected_file]).len();
                let selected_line = if line_count == 0 {
                    0
                } else {
                    saved.selected_line.min(line_count - 1)
                };
                (selected_file, selected_line)
            }
            _ => (saved.selected_file, saved.selected_line),
        };
        self.selected_diff_file
            .insert(item_id.to_string(), selected_file);
        self.selected_diff_line
            .insert(item_id.to_string(), selected_line);
    }

    #[cfg(test)]
    fn show_conversation(&mut self) {
        self.details_mode = DetailsMode::Conversation;
        self.diff_return_state = None;
        self.focus = FocusTarget::Details;
        self.details_scroll = 0;
        self.status = "conversation focused".to_string();
    }

    fn focus_list(&mut self) {
        self.save_current_conversation_details_state();
        self.focus = FocusTarget::List;
        self.status = if self.details_mode == DetailsMode::Diff {
            "files focused".to_string()
        } else {
            "list focused".to_string()
        };
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
        let Some(comments) = self.current_comments() else {
            self.status = "no comments".to_string();
            return;
        };
        let order = comment_display_entries(comments)
            .into_iter()
            .map(|entry| entry.index)
            .collect::<Vec<_>>();
        if order.is_empty() {
            self.status = "no comments".to_string();
            return;
        }
        let current_position = order
            .iter()
            .position(|index| *index == self.selected_comment_index)
            .unwrap_or(0);
        let next_position = move_bounded(current_position, order.len(), delta);
        self.selected_comment_index = order[next_position];
        self.status = format!("comment {} focused", next_position + 1);
    }

    fn move_comment_in_view(&mut self, delta: isize, area: Option<Rect>) {
        if self.move_comment_search_match(delta, area) {
            return;
        }

        let before = self.selected_comment_index;
        self.move_comment(delta);
        if before == self.selected_comment_index {
            self.scroll_details_past_comment_edge(delta, area);
        } else {
            self.scroll_selected_comment_into_view(area);
        }
    }

    fn move_comment_search_match(&mut self, delta: isize, area: Option<Rect>) -> bool {
        if self.comment_search_query.trim().is_empty() {
            return false;
        }

        let matches = self.current_comment_search_matches();
        if matches.is_empty() {
            self.status = format!("no comments match '{}'", self.comment_search_query.trim());
            return true;
        }

        let current = self.selected_comment_index;
        let next_position = match matches.iter().position(|index| *index == current) {
            Some(position) => move_wrapping(position, matches.len(), delta),
            None if delta < 0 => matches
                .iter()
                .rposition(|index| *index < current)
                .unwrap_or(matches.len() - 1),
            None => matches
                .iter()
                .position(|index| *index > current)
                .unwrap_or(0),
        };
        self.selected_comment_index = matches[next_position];
        self.scroll_selected_comment_into_view(area);
        self.status = self.comment_search_status(&matches);
        true
    }

    fn current_comment_search_matches(&self) -> Vec<usize> {
        let Some(comments) = self.current_comments() else {
            return Vec::new();
        };
        comment_search_matches(comments, &self.comment_search_query)
    }

    fn comment_search_status(&self, matches: &[usize]) -> String {
        let position = matches
            .iter()
            .position(|index| *index == self.selected_comment_index)
            .map(|position| position + 1)
            .unwrap_or(0);
        format!(
            "comment search: {position}/{} for '{}'",
            matches.len(),
            self.comment_search_query.trim()
        )
    }

    fn scroll_details_past_comment_edge(&mut self, delta: isize, area: Option<Rect>) {
        let amount = area
            .map(|area| {
                usize::from(block_inner(details_area_for(self, area)).height)
                    .saturating_div(2)
                    .max(3)
            })
            .unwrap_or(6)
            .min(i16::MAX as usize) as i16;
        let signed = if delta < 0 { -amount } else { amount };
        self.scroll_details(signed);
        if let Some(area) = area {
            let details_area = details_area_for(self, area);
            self.details_scroll = self
                .details_scroll
                .min(max_details_scroll(self, details_area));
        }
    }

    fn scroll_selected_comment_into_view(&mut self, area: Option<Rect>) {
        if self.details_mode != DetailsMode::Conversation {
            return;
        }
        let Some(area) = area else {
            return;
        };
        let details_area = details_area_for(self, area);
        let inner = block_inner(details_area);
        if inner.height == 0 {
            return;
        }
        let document = build_details_document(self, inner.width);
        let Some(region) = document.comment_region(self.selected_comment_index) else {
            return;
        };
        let viewport_start = usize::from(self.details_scroll);
        let viewport_height = usize::from(inner.height);
        let viewport_end = viewport_start.saturating_add(viewport_height);
        let focus_line = region.focus_line();
        let lower_margin = viewport_start.saturating_add(1);
        let upper_margin = viewport_end.saturating_sub(2);
        if focus_line < lower_margin {
            self.details_scroll = focus_line.saturating_sub(1).min(usize::from(u16::MAX)) as u16;
        } else if focus_line >= upper_margin {
            let next_scroll = focus_line.saturating_sub(viewport_height / 3);
            self.details_scroll = next_scroll.min(usize::from(u16::MAX)) as u16;
        }
        self.details_scroll = self
            .details_scroll
            .min(max_details_scroll(self, details_area));
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
            DetailAction::ToggleCommentExpanded(index) => {
                self.select_comment(index);
                self.toggle_selected_comment_expanded();
            }
        }
    }

    fn toggle_selected_comment_expanded(&mut self) {
        let Some(item_id) = self.current_item().map(|item| item.id.clone()) else {
            self.status = "nothing selected".to_string();
            return;
        };
        let Some(comment) = self.current_selected_comment().cloned() else {
            self.status = "no comment selected".to_string();
            return;
        };
        if !is_long_comment(&comment) {
            self.status = "comment is already fully visible".to_string();
            return;
        }
        let key = comment_expand_key(&item_id, self.selected_comment_index, &comment);
        if self.expanded_comments.remove(&key) {
            self.status = "comment collapsed".to_string();
        } else {
            self.expanded_comments.insert(key);
            self.status = "comment expanded".to_string();
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
        self.comment_search_active = false;
        self.pr_action_dialog = Some(PrActionDialog {
            item,
            action,
            checkout: None,
        });
        self.pr_action_running = false;
        self.status = match action {
            PrAction::Merge => "confirm pull request merge".to_string(),
            PrAction::Close => "confirm pull request close".to_string(),
            PrAction::Approve => "confirm pull request approval".to_string(),
            PrAction::Checkout => "confirm local pull request checkout".to_string(),
        };
    }

    fn start_pr_checkout_dialog(&mut self, config: &Config) {
        let Some(item) = self.current_item().cloned() else {
            self.status = "nothing selected".to_string();
            return;
        };
        if item.kind != ItemKind::PullRequest || item.number.is_none() {
            self.status = "selected item is not a pull request".to_string();
            return;
        }

        let directory = match resolve_pr_checkout_directory(config, &item.repo) {
            Ok(directory) => directory,
            Err(error) => {
                self.message_dialog = Some(message_dialog("Checkout Unavailable", error));
                self.status = "pull request checkout unavailable".to_string();
                return;
            }
        };
        let branch = self
            .action_hints
            .get(&item.id)
            .and_then(|state| match state {
                ActionHintState::Loaded(hints) => hints.head.clone(),
                _ => None,
            });

        self.search_active = false;
        self.global_search_active = false;
        self.comment_search_active = false;
        self.pr_action_dialog = Some(PrActionDialog {
            item,
            action: PrAction::Checkout,
            checkout: Some(PrCheckoutPlan { directory, branch }),
        });
        self.pr_action_running = false;
        self.status = "confirm local pull request checkout".to_string();
    }

    fn handle_pr_action_dialog_key(
        &mut self,
        key: KeyEvent,
        config: &Config,
        store: &SnapshotStore,
        tx: &UnboundedSender<AppMsg>,
    ) {
        self.handle_pr_action_dialog_key_with_submit(key, |item, action, checkout| {
            start_pr_action(
                item,
                action,
                checkout,
                config.clone(),
                store.clone(),
                tx.clone(),
            );
        });
    }

    fn handle_pr_action_dialog_key_with_submit<F>(&mut self, key: KeyEvent, mut submit: F)
    where
        F: FnMut(WorkItem, PrAction, Option<PrCheckoutPlan>),
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
        F: FnMut(WorkItem, PrAction, Option<PrCheckoutPlan>),
    {
        let Some(dialog) = &self.pr_action_dialog else {
            return;
        };
        let item = dialog.item.clone();
        let checkout = dialog.checkout.clone();
        self.pr_action_running = true;
        self.status = match action {
            PrAction::Merge => "merging pull request".to_string(),
            PrAction::Close => "closing pull request".to_string(),
            PrAction::Approve => "approving pull request".to_string(),
            PrAction::Checkout => "checking out pull request locally".to_string(),
        };
        submit(item, action, checkout);
    }

    fn start_new_comment_dialog(&mut self) {
        if !self.current_item_supports_comments() {
            self.status = "selected item cannot be commented on".to_string();
            return;
        }
        self.focus = FocusTarget::Details;
        self.search_active = false;
        self.comment_search_active = false;
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
        let review_comment_id = comment.review.as_ref().and(comment.id);
        self.focus = FocusTarget::Details;
        self.search_active = false;
        self.comment_search_active = false;
        self.global_search_active = false;
        self.pr_action_dialog = None;
        self.comment_dialog = Some(CommentDialog {
            mode: CommentDialogMode::Reply {
                comment_index: self.selected_comment_index,
                author: author.clone(),
                review_comment_id,
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
        self.comment_search_active = false;
        self.global_search_active = false;
        self.pr_action_dialog = None;
        self.comment_dialog = Some(CommentDialog {
            mode: CommentDialogMode::Edit {
                comment_index: self.selected_comment_index,
                comment_id,
                is_review: comment.review.is_some(),
            },
            body: comment.body,
            scroll: 0,
        });
        self.scroll_comment_dialog_to_cursor();
        self.status = "editing comment".to_string();
    }

    fn start_review_comment_dialog(&mut self) {
        if self.details_mode != DetailsMode::Diff {
            self.status = "review comments are available in diff mode".to_string();
            return;
        }
        let Some(item) = self.current_item() else {
            self.status = "nothing selected".to_string();
            return;
        };
        if item.kind != ItemKind::PullRequest || item.number.is_none() {
            self.status = "selected item is not a pull request".to_string();
            return;
        }
        let target = match self.current_diff_review_target_result() {
            Ok(Some(target)) => target,
            Ok(None) => {
                self.status = "no diff line selected".to_string();
                return;
            }
            Err(error) => {
                self.status = error;
                return;
            }
        };

        self.focus = FocusTarget::Details;
        self.search_active = false;
        self.comment_search_active = false;
        self.global_search_active = false;
        self.pr_action_dialog = None;
        self.comment_dialog = Some(CommentDialog {
            mode: CommentDialogMode::Review {
                target: target.clone(),
            },
            body: String::new(),
            scroll: 0,
        });
        self.scroll_comment_dialog_to_cursor();
        self.status = format!("reviewing {}", target.location_label());
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
            PendingCommentMode::ReviewReply { comment_id } => {
                start_review_reply_submit(submit.item, comment_id, submit.body, tx.clone());
            }
            PendingCommentMode::Edit {
                comment_index,
                comment_id,
                is_review,
            } => {
                start_comment_edit(
                    submit.item,
                    comment_index,
                    comment_id,
                    is_review,
                    submit.body,
                    tx.clone(),
                );
            }
            PendingCommentMode::Review { target } => {
                start_review_comment_submit(submit.item, target, submit.body, tx.clone());
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
            CommentDialogMode::New => PendingCommentMode::Post,
            CommentDialogMode::Reply {
                review_comment_id: Some(comment_id),
                ..
            } => PendingCommentMode::ReviewReply { comment_id },
            CommentDialogMode::Reply { .. } => PendingCommentMode::Post,
            CommentDialogMode::Edit {
                comment_index,
                comment_id,
                is_review,
            } => PendingCommentMode::Edit {
                comment_index,
                comment_id,
                is_review,
            },
            CommentDialogMode::Review { target } => PendingCommentMode::Review { target },
        };
        self.posting_comment = true;
        self.message_dialog = Some(comment_pending_dialog(&mode));
        self.status = match &mode {
            PendingCommentMode::Post => "posting comment".to_string(),
            PendingCommentMode::ReviewReply { .. } => "posting review reply".to_string(),
            PendingCommentMode::Edit { .. } => "updating comment".to_string(),
            PendingCommentMode::Review { .. } => "posting review comment".to_string(),
        };
        Some(PendingCommentSubmit { item, body, mode })
    }

    fn start_global_search_input(&mut self) {
        self.save_current_conversation_details_state();
        if !self.is_global_search_results_view() {
            self.global_search_return_view = Some(self.active_view.clone());
        }
        self.focus = FocusTarget::List;
        self.global_search_active = true;
        self.global_search_scope = self.current_repo_scope();
        self.global_search_query.clear();
        self.search_active = false;
        self.comment_search_active = false;
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
                let repo_scope = self.current_repo_scope();
                self.global_search_active = false;
                self.global_search_running = true;
                self.global_search_started_at = Some(Instant::now());
                self.global_search_scope = repo_scope.clone();
                self.search_active = false;
                self.search_query.clear();
                self.comment_search_active = false;
                self.status = match repo_scope {
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
        self.save_current_conversation_details_state();
        self.focus = FocusTarget::List;
        self.search_active = true;
        self.global_search_active = false;
        self.comment_search_active = false;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.status = "search mode".to_string();
        self.clamp_positions();
    }

    fn clear_search(&mut self) {
        self.save_current_conversation_details_state();
        self.search_active = false;
        self.global_search_active = false;
        self.search_query.clear();
        self.focus = FocusTarget::List;
        self.details_scroll = 0;
        self.status = "search cleared".to_string();
        self.clamp_positions();
    }

    fn push_search_char(&mut self, value: char) {
        self.save_current_conversation_details_state();
        self.search_query.push(value);
        self.set_current_selected_position(0);
        self.details_scroll = 0;
        self.clamp_positions();
    }

    fn pop_search_char(&mut self) {
        self.save_current_conversation_details_state();
        self.search_query.pop();
        self.set_current_selected_position(0);
        self.details_scroll = 0;
        self.clamp_positions();
    }

    fn start_comment_search(&mut self) {
        if self.details_mode != DetailsMode::Conversation {
            self.status = "comment search is available in conversation details".to_string();
            return;
        }
        if self.current_comments().is_none() {
            self.status = "comments are still loading".to_string();
        } else {
            self.status = "comment search mode".to_string();
        }
        self.focus = FocusTarget::Details;
        self.comment_search_active = true;
        self.search_active = false;
        self.global_search_active = false;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
    }

    fn handle_comment_search_key(&mut self, key: KeyEvent, area: Option<Rect>) {
        match key.code {
            KeyCode::Esc => self.clear_comment_search(),
            KeyCode::Enter => {
                self.comment_search_active = false;
                self.update_comment_search_selection(area);
            }
            KeyCode::Backspace => {
                self.comment_search_query.pop();
                self.update_comment_search_selection(area);
            }
            KeyCode::Char(value) => {
                self.comment_search_query.push(value);
                self.update_comment_search_selection(area);
            }
            _ => {}
        }
    }

    fn clear_comment_search(&mut self) {
        self.comment_search_active = false;
        self.comment_search_query.clear();
        self.focus = FocusTarget::Details;
        self.status = "comment search cleared".to_string();
    }

    fn update_comment_search_selection(&mut self, area: Option<Rect>) {
        let query = self.comment_search_query.trim();
        if query.is_empty() {
            self.status = "comment search mode".to_string();
            return;
        }
        let matches = self.current_comment_search_matches();
        if matches.is_empty() {
            self.status = format!("no comments match '{query}'");
            return;
        }

        if !matches.contains(&self.selected_comment_index) {
            self.selected_comment_index = matches[0];
        }
        self.scroll_selected_comment_into_view(area);
        self.status = self.comment_search_status(&matches);
    }

    fn open_selected(&mut self) {
        let Some(url) = self.selected_open_url() else {
            self.status = "nothing to open".to_string();
            return;
        };

        self.open_url(&url);
    }

    fn selected_open_url(&self) -> Option<String> {
        let item = self.current_item()?;
        if self.details_mode == DetailsMode::Diff && item.kind == ItemKind::PullRequest {
            return Some(pull_request_changes_url(item));
        }
        Some(item.url.clone())
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

    fn loaded_comments_for_item(&self, item_id: &str) -> Option<&[CommentPreview]> {
        match self.details.get(item_id) {
            Some(DetailState::Loaded(comments)) => Some(comments.as_slice()),
            _ => None,
        }
    }

    fn details_snapshot_hash_for_item(&self, item: &WorkItem) -> String {
        details_snapshot_hash(item)
    }

    fn item_has_unseen_details(&self, item: &WorkItem) -> bool {
        if !item_supports_details_memory(item) {
            return false;
        }
        if self
            .viewed_details_snapshot
            .get(&item.id)
            .is_some_and(|viewed| viewed != &self.details_snapshot_hash_for_item(item))
        {
            return true;
        }
        self.loaded_comments_for_item(&item.id)
            .is_some_and(|comments| {
                self.viewed_comments_snapshot
                    .get(&item.id)
                    .is_some_and(|viewed| viewed != &comments_snapshot_hash(comments))
            })
    }

    fn mark_current_details_viewed(&mut self) {
        let Some(item) = self.current_item().cloned() else {
            return;
        };
        if !item_supports_details_memory(&item)
            || self.details_stale.contains(&item.id)
            || self.details_refreshing.contains(&item.id)
        {
            return;
        }
        self.viewed_details_snapshot
            .insert(item.id.clone(), self.details_snapshot_hash_for_item(&item));
        let comments_hash = self
            .loaded_comments_for_item(&item.id)
            .map(comments_snapshot_hash);
        if let Some(hash) = comments_hash {
            self.viewed_comments_snapshot.insert(item.id.clone(), hash);
        }
    }

    fn mark_current_details_viewed_if_current(&mut self, item_id: &str) {
        if self.focus != FocusTarget::Details || self.details_mode != DetailsMode::Conversation {
            return;
        }
        if self
            .current_item()
            .is_some_and(|item| item.id.as_str() == item_id)
        {
            self.mark_current_details_viewed();
        }
    }

    fn save_current_conversation_details_state(&mut self) {
        if self.details_mode != DetailsMode::Conversation {
            return;
        }
        let Some(item) = self.current_item() else {
            return;
        };
        if !item_supports_details_memory(item) {
            return;
        }
        let item_id = item.id.clone();
        self.conversation_details_state.insert(
            item_id,
            ConversationDetailsState {
                details_scroll: self.details_scroll,
                selected_comment_index: self.selected_comment_index,
            },
        );
    }

    fn restore_current_conversation_details_state(&mut self) {
        if self.details_mode != DetailsMode::Conversation {
            return;
        }
        let Some(item) = self.current_item() else {
            return;
        };
        if let Some(state) = self.conversation_details_state.get(&item.id).copied() {
            self.details_scroll = state.details_scroll;
            self.selected_comment_index = state.selected_comment_index;
            if self.current_comments().is_some() {
                self.clamp_selected_comment();
            }
        }
    }

    fn current_selected_comment(&self) -> Option<&CommentPreview> {
        self.current_comments()?.get(self.selected_comment_index)
    }

    fn comment_collapse_state(
        &self,
        item_id: &str,
        index: usize,
        comment: &CommentPreview,
    ) -> CommentCollapseState {
        comment_collapse_state_for(item_id, index, comment, &self.expanded_comments)
    }

    fn current_section_position(&self) -> usize {
        self.section_position(&self.active_view)
    }

    fn current_selected_position(&self) -> usize {
        self.selected_position(&self.active_view)
    }

    fn current_list_scroll_offset(&self, len: usize, visible_rows: usize) -> usize {
        if len == 0 {
            return 0;
        }

        let default =
            table_viewport_offset(self.current_selected_position().min(len - 1), visible_rows);
        let Some(section) = self.current_section() else {
            return default;
        };
        self.list_scroll_offset
            .get(&section.key)
            .copied()
            .unwrap_or(default)
            .min(max_table_viewport_offset(len, visible_rows))
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

    fn set_current_list_scroll_offset(&mut self, offset: usize) {
        if let Some(section_key) = self.current_section().map(|section| section.key.clone()) {
            self.list_scroll_offset.insert(section_key, offset);
        }
    }

    fn clear_current_list_scroll_offset(&mut self) {
        if let Some(section_key) = self.current_section().map(|section| section.key.clone()) {
            self.list_scroll_offset.remove(&section_key);
        }
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

fn comment_search_matches(comments: &[CommentPreview], query: &str) -> Vec<usize> {
    let query = query.trim();
    if query.is_empty() {
        return comment_display_entries(comments)
            .into_iter()
            .map(|entry| entry.index)
            .collect();
    }

    comment_display_entries(comments)
        .into_iter()
        .filter_map(|entry| {
            comment_matches_search(&comments[entry.index], query).then_some(entry.index)
        })
        .collect()
}

fn comment_display_entries(comments: &[CommentPreview]) -> Vec<CommentDisplayEntry> {
    let mut id_to_index = HashMap::new();
    for (index, comment) in comments.iter().enumerate() {
        if let Some(id) = comment.id {
            id_to_index.insert(id, index);
        }
    }

    let mut children = vec![Vec::new(); comments.len()];
    let mut roots = Vec::new();
    for (index, comment) in comments.iter().enumerate() {
        let parent_index = comment
            .parent_id
            .and_then(|parent_id| id_to_index.get(&parent_id).copied())
            .filter(|parent_index| *parent_index != index)
            .filter(|parent_index| {
                comments[index].review.is_some() && comments[*parent_index].review.is_some()
            });
        if let Some(parent_index) = parent_index {
            children[parent_index].push(index);
        } else {
            roots.push(index);
        }
    }

    let mut entries = Vec::with_capacity(comments.len());
    let mut visited = vec![false; comments.len()];
    for index in roots {
        push_comment_display_entry(index, 0, &children, &mut visited, &mut entries);
    }
    for index in 0..comments.len() {
        if !visited[index] {
            push_comment_display_entry(index, 0, &children, &mut visited, &mut entries);
        }
    }
    entries
}

fn push_comment_display_entry(
    index: usize,
    depth: usize,
    children: &[Vec<usize>],
    visited: &mut [bool],
    entries: &mut Vec<CommentDisplayEntry>,
) {
    if index >= visited.len() || visited[index] {
        return;
    }
    visited[index] = true;
    entries.push(CommentDisplayEntry {
        index,
        depth: depth.min(4),
    });
    for child_index in &children[index] {
        push_comment_display_entry(
            *child_index,
            depth.saturating_add(1),
            children,
            visited,
            entries,
        );
    }
}

fn comment_matches_search(comment: &CommentPreview, query: &str) -> bool {
    let haystack = searchable_comment_text(comment);
    query
        .split_whitespace()
        .all(|token| fuzzy_score(token, &haystack).is_some())
}

fn searchable_comment_text(comment: &CommentPreview) -> String {
    let mut parts = vec![comment.author.clone(), comment.body.clone()];
    if let Some(url) = &comment.url {
        parts.push(url.clone());
    }
    if let Some(review) = &comment.review {
        parts.push(review.path.clone());
        if let Some(line) = review.line {
            parts.push(line.to_string());
        }
        if let Some(diff_hunk) = &review.diff_hunk {
            parts.push(diff_hunk.clone());
        }
    }
    parts.join(" ").to_lowercase()
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
    fn parse_pull_request_diff_tracks_files_hunks_and_lines() {
        let diff = parse_pull_request_diff(
            r#"diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 pub fn old() {
-    println!("old");
+    println!("new");
+    println!("extra");
 }
diff --git a/README.md b/README.md
deleted file mode 100644
--- a/README.md
+++ /dev/null
@@ -1 +0,0 @@
-hello
"#,
        )
        .expect("parse diff");

        assert_eq!(diff.files.len(), 2);
        assert_eq!(diff.additions, 2);
        assert_eq!(diff.deletions, 2);
        assert_eq!(diff.files[0].new_path, "src/lib.rs");
        assert_eq!(diff.files[0].hunks.len(), 1);
        assert_eq!(diff.files[0].hunks[0].lines[1].kind, DiffLineKind::Removed);
        assert_eq!(diff.files[0].hunks[0].lines[2].new_line, Some(2));
        assert_eq!(diff.files[1].new_path, "/dev/null");
    }

    #[test]
    fn diff_document_renders_lazygit_style_gutter() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.show_diff();
        app.diffs.insert(
            "1".to_string(),
            DiffState::Loaded(
                parse_pull_request_diff(
                    r#"diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,2 @@
-old
+new
 context
"#,
                )
                .expect("parse diff"),
            ),
        );

        let document = build_details_document(&app, 96);
        let rendered = document
            .lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Conversation | Diff"));
        assert!(rendered.contains("▾ src/lib.rs"));
        assert!(rendered.contains("@@ -1,2 +1,2 @@"));
        assert!(rendered.contains("   1      │ - old"));
        assert!(rendered.contains("        1 │ + new"));
        assert!(!rendered.contains("selected:"));
        assert!(!rendered.contains("selected range:"));
        assert_eq!(document.diff_files, vec![8]);
        assert_eq!(document.diff_line_at(10), Some(0));
        assert_eq!(document.diff_line_at(11), Some(1));
    }

    #[test]
    fn diff_review_targets_map_sides_and_paths() {
        let diff = parse_pull_request_diff(
            r#"diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,2 @@
-old
+new
 context
"#,
        )
        .expect("parse diff");

        let targets = diff_review_targets(&diff.files[0]);

        assert_eq!(
            targets,
            vec![
                DiffReviewTarget {
                    path: "src/lib.rs".to_string(),
                    line: 1,
                    side: DiffReviewSide::Left,
                    start_line: None,
                    start_side: None,
                    preview: "old".to_string(),
                },
                DiffReviewTarget {
                    path: "src/lib.rs".to_string(),
                    line: 1,
                    side: DiffReviewSide::Right,
                    start_line: None,
                    start_side: None,
                    preview: "new".to_string(),
                },
                DiffReviewTarget {
                    path: "src/lib.rs".to_string(),
                    line: 2,
                    side: DiffReviewSide::Right,
                    start_line: None,
                    start_side: None,
                    preview: "context".to_string(),
                },
            ]
        );
    }

    #[test]
    fn diff_details_render_inline_review_comments_below_target_line() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.show_diff();
        app.focus_details();
        app.diffs.insert(
            "1".to_string(),
            DiffState::Loaded(
                parse_pull_request_diff(
                    r#"diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,2 @@
-old
+new
 context
"#,
                )
                .expect("parse diff"),
            ),
        );

        let review = crate::model::ReviewCommentPreview {
            path: "src/lib.rs".to_string(),
            line: Some(1),
            original_line: None,
            start_line: None,
            original_start_line: None,
            side: Some("RIGHT".to_string()),
            start_side: None,
            diff_hunk: None,
        };
        let mut parent = comment(
            "alice",
            "Please keep this inline.",
            Some("https://github.com/rust-lang/rust/pull/1#discussion_r1"),
        );
        parent.id = Some(1);
        parent.review = Some(review.clone());
        let mut child = comment("bob", "Thread reply", None);
        child.id = Some(2);
        child.parent_id = Some(1);
        child.review = Some(review);
        app.details
            .insert("1".to_string(), DetailState::Loaded(vec![parent, child]));

        let document = build_details_document(&app, 120);
        let rendered = document
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert!(
            rendered.iter().any(|line| line.contains("│ ● + new")),
            "target diff line should show an inline comment marker: {rendered:?}"
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("alice") && line.contains("reply")),
            "inline comment header should render under the diff line: {rendered:?}"
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.contains('↳') && line.contains("bob")),
            "review replies should stay nested in the inline thread: {rendered:?}"
        );

        let body_line = rendered
            .iter()
            .position(|line| line.contains("Please keep this inline."))
            .expect("parent inline body");
        assert_eq!(document.comment_at(body_line), Some(0));

        let header_line = rendered
            .iter()
            .position(|line| line.contains("alice") && line.contains("reply"))
            .expect("parent inline header");
        let open_column =
            display_width(&rendered[header_line][..rendered[header_line].find("open").unwrap()])
                as u16;
        let reply_column =
            display_width(&rendered[header_line][..rendered[header_line].find("reply").unwrap()])
                as u16;
        assert_eq!(
            document.link_at(header_line, open_column),
            Some("https://github.com/rust-lang/rust/pull/1#discussion_r1".to_string())
        );
        assert_eq!(
            document.action_at(header_line, reply_column),
            Some(DetailAction::ReplyComment(0))
        );
    }

    #[test]
    fn diff_details_selects_review_line_with_keyboard_and_mouse() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.show_diff();
        app.focus_details();
        app.diffs.insert(
            "1".to_string(),
            DiffState::Loaded(
                parse_pull_request_diff(
                    r#"diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,2 @@
-old
+new
 context
"#,
                )
                .expect("parse diff"),
            ),
        );
        let area = Rect::new(0, 0, 120, 34);

        assert!(!handle_key_in_area(
            &mut app,
            key(KeyCode::Char('j')),
            &config,
            &store,
            &tx,
            Some(area)
        ));
        assert_eq!(app.selected_diff_line.get("1"), Some(&1));
        assert_eq!(
            app.current_diff_review_target().map(|target| target.side),
            Some(DiffReviewSide::Right)
        );

        let details_area = details_area_for(&app, area);
        let inner = block_inner(details_area);
        let document = build_details_document(&app, inner.width);
        let line = document
            .diff_lines
            .iter()
            .find(|line| line.review_index == 2)
            .expect("third review line")
            .line;
        let page = page_areas(area);
        let body = body_areas_with_ratio(body_area(area), app.list_width_percent);
        handle_left_click(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: inner.x + 2,
                row: inner.y + line as u16,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
            MouseLayout {
                view_tabs: page[0],
                section_tabs: page[1],
                table: body[0],
                details: details_area,
            },
            None,
            None,
        );

        assert_eq!(app.selected_diff_line.get("1"), Some(&2));
    }

    #[test]
    fn diff_details_page_keys_select_first_visible_review_line() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        let mut diff_lines = String::new();
        for line in 1..=40 {
            diff_lines.push_str(&format!(" line {line}\n"));
        }
        app.show_diff();
        app.focus_details();
        app.diffs.insert(
            "1".to_string(),
            DiffState::Loaded(
                parse_pull_request_diff(&format!(
                    "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,40 +1,40 @@\n{diff_lines}"
                ))
                .expect("parse diff"),
            ),
        );
        let area = Rect::new(0, 0, 120, 28);

        assert!(!handle_key_in_area(
            &mut app,
            key(KeyCode::PageDown),
            &config,
            &store,
            &tx,
            Some(area)
        ));
        assert!(app.details_scroll > 0);
        let (expected_index, expected_line) = first_visible_diff_line(&app, area);
        assert_eq!(app.selected_diff_line.get("1"), Some(&expected_index));
        assert_eq!(selected_diff_document_line(&app, area), Some(expected_line));

        assert!(!handle_key_in_area(
            &mut app,
            key(KeyCode::PageUp),
            &config,
            &store,
            &tx,
            Some(area)
        ));
        let (expected_index, expected_line) = first_visible_diff_line(&app, area);
        assert_eq!(app.selected_diff_line.get("1"), Some(&expected_index));
        assert_eq!(selected_diff_document_line(&app, area), Some(expected_line));
    }

    #[test]
    fn n_and_p_page_diff_details_instead_of_jumping_hunks() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        let mut diff_lines = String::new();
        for line in 1..=40 {
            diff_lines.push_str(&format!(" line {line}\n"));
        }
        app.show_diff();
        app.focus_details();
        app.diffs.insert(
            "1".to_string(),
            DiffState::Loaded(
                parse_pull_request_diff(&format!(
                    "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,40 +1,40 @@\n{diff_lines}"
                ))
                .expect("parse diff"),
            ),
        );
        let area = Rect::new(0, 0, 120, 28);

        assert!(!handle_key_in_area(
            &mut app,
            key(KeyCode::Char('n')),
            &config,
            &store,
            &tx,
            Some(area)
        ));
        assert!(app.details_scroll > 0);
        let after_next = app.details_scroll;
        let (expected_index, expected_line) = first_visible_diff_line(&app, area);
        assert_eq!(app.selected_diff_line.get("1"), Some(&expected_index));
        assert_eq!(selected_diff_document_line(&app, area), Some(expected_line));

        assert!(!handle_key_in_area(
            &mut app,
            key(KeyCode::Char('p')),
            &config,
            &store,
            &tx,
            Some(area)
        ));
        assert!(app.details_scroll < after_next);
        let (expected_index, expected_line) = first_visible_diff_line(&app, area);
        assert_eq!(app.selected_diff_line.get("1"), Some(&expected_index));
        assert_eq!(selected_diff_document_line(&app, area), Some(expected_line));
    }

    #[test]
    fn g_and_upper_g_jump_diff_details_top_and_bottom() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        let mut diff_lines = String::new();
        for line in 1..=40 {
            diff_lines.push_str(&format!(" line {line}\n"));
        }
        app.show_diff();
        app.focus_details();
        app.diffs.insert(
            "1".to_string(),
            DiffState::Loaded(
                parse_pull_request_diff(&format!(
                    "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,40 +1,40 @@\n{diff_lines}"
                ))
                .expect("parse diff"),
            ),
        );
        let area = Rect::new(0, 0, 120, 28);
        let max_scroll = max_details_scroll(&app, details_area_for(&app, area));

        assert!(!handle_key_in_area(
            &mut app,
            key(KeyCode::Char('G')),
            &config,
            &store,
            &tx,
            Some(area)
        ));
        assert_eq!(app.details_scroll, max_scroll);
        let (expected_index, expected_line) = first_visible_diff_line(&app, area);
        assert_eq!(app.selected_diff_line.get("1"), Some(&expected_index));
        assert_eq!(selected_diff_document_line(&app, area), Some(expected_line));

        assert!(!handle_key_in_area(
            &mut app,
            key(KeyCode::Char('g')),
            &config,
            &store,
            &tx,
            Some(area)
        ));
        assert_eq!(app.details_scroll, 0);
    }

    #[test]
    fn m_marks_diff_range_for_review_comment() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.show_diff();
        app.focus_details();
        app.diffs.insert(
            "1".to_string(),
            DiffState::Loaded(
                parse_pull_request_diff(
                    r#"diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -0,0 +1,3 @@
+one
+two
+three
"#,
                )
                .expect("parse diff"),
            ),
        );
        let area = Rect::new(0, 0, 120, 24);

        assert!(!handle_key_in_area(
            &mut app,
            key(KeyCode::Char('m')),
            &config,
            &store,
            &tx,
            Some(area)
        ));
        assert!(app.mouse_capture_enabled);
        assert!(!handle_key_in_area(
            &mut app,
            key(KeyCode::Char('j')),
            &config,
            &store,
            &tx,
            Some(area)
        ));
        assert!(!handle_key_in_area(
            &mut app,
            key(KeyCode::Char('j')),
            &config,
            &store,
            &tx,
            Some(area)
        ));
        assert!(!handle_key_in_area(
            &mut app,
            key(KeyCode::Char('e')),
            &config,
            &store,
            &tx,
            Some(area)
        ));

        assert!(!handle_key_in_area(
            &mut app,
            key(KeyCode::Char('c')),
            &config,
            &store,
            &tx,
            Some(area)
        ));

        assert!(matches!(
            app.comment_dialog.as_ref().map(|dialog| &dialog.mode),
            Some(CommentDialogMode::Review { target })
                if target.path == "src/lib.rs"
                    && target.start_line == Some(1)
                    && target.start_side == Some(DiffReviewSide::Right)
                    && target.line == 3
                    && target.side == DiffReviewSide::Right
        ));
        assert_eq!(app.status, "reviewing src/lib.rs:1-3");
    }

    #[test]
    fn mixed_side_diff_range_creates_review_comment_target() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.show_diff();
        app.focus_details();
        app.diffs.insert(
            "1".to_string(),
            DiffState::Loaded(
                parse_pull_request_diff(
                    r#"diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1 @@
-old
+new
"#,
                )
                .expect("parse diff"),
            ),
        );
        let area = Rect::new(0, 0, 120, 24);

        for code in [
            KeyCode::Char('m'),
            KeyCode::Char('j'),
            KeyCode::Char('e'),
            KeyCode::Char('c'),
        ] {
            assert!(!handle_key_in_area(
                &mut app,
                key(code),
                &config,
                &store,
                &tx,
                Some(area)
            ));
        }

        assert!(matches!(
            app.comment_dialog.as_ref().map(|dialog| &dialog.mode),
            Some(CommentDialogMode::Review { target })
                if target.path == "src/lib.rs"
                    && target.start_line == Some(1)
                    && target.start_side == Some(DiffReviewSide::Left)
                    && target.line == 1
                    && target.side == DiffReviewSide::Right
        ));
        assert_eq!(app.status, "reviewing src/lib.rs:1L-1R");
    }

    #[test]
    fn single_click_begins_and_double_click_ends_diff_range() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.show_diff();
        app.diffs.insert(
            "1".to_string(),
            DiffState::Loaded(
                parse_pull_request_diff(
                    r#"diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -0,0 +1,3 @@
+one
+two
+three
"#,
                )
                .expect("parse diff"),
            ),
        );
        let area = Rect::new(0, 0, 120, 24);
        let page = page_areas(area);
        let body = body_areas_with_ratio(body_area(area), app.list_width_percent);
        let details_area = details_area_for(&app, area);
        let inner = block_inner(details_area);
        let document = build_details_document(&app, inner.width);
        let line_for = |review_index| {
            document
                .diff_lines
                .iter()
                .find(|line| line.review_index == review_index)
                .expect("review line")
                .line as u16
        };

        for review_index in [0, 2] {
            handle_left_click(
                &mut app,
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: inner.x + 2,
                    row: inner.y + line_for(review_index),
                    modifiers: crossterm::event::KeyModifiers::NONE,
                },
                MouseLayout {
                    view_tabs: page[0],
                    section_tabs: page[1],
                    table: body[0],
                    details: details_area,
                },
                None,
                None,
            );
        }

        let target = app.current_diff_review_target().expect("current line");
        assert_eq!(target.start_line, None);
        assert_eq!(target.line, 3);

        handle_left_click(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: inner.x + 2,
                row: inner.y + line_for(2),
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
            MouseLayout {
                view_tabs: page[0],
                section_tabs: page[1],
                table: body[0],
                details: details_area,
            },
            None,
            None,
        );

        let target = app.current_diff_review_target().expect("review range");
        assert_eq!(target.start_line, Some(1));
        assert_eq!(target.line, 3);
        assert_eq!(target.side, DiffReviewSide::Right);
    }

    #[test]
    fn show_diff_focuses_diff_files_for_pull_requests() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);

        app.show_diff();

        assert_eq!(app.focus, FocusTarget::List);
        assert_eq!(app.details_mode, DetailsMode::Diff);
        assert_eq!(app.details_scroll, 0);
        assert_eq!(app.status, "loading diff");

        app.show_conversation();
        assert_eq!(app.details_mode, DetailsMode::Conversation);
        assert_eq!(app.status, "conversation focused");
    }

    #[test]
    fn q_in_diff_returns_to_state_before_diff_instead_of_quitting() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.focus_list();
        app.details.insert(
            "1".to_string(),
            DetailState::Loaded(vec![
                comment("alice", "first", None),
                comment("bob", "second", None),
            ]),
        );
        app.details_scroll = 7;
        app.selected_comment_index = 1;

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('v')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.details_mode, DetailsMode::Diff);
        assert_eq!(app.focus, FocusTarget::List);
        app.diffs.insert(
            "1".to_string(),
            DiffState::Loaded(
                parse_pull_request_diff(
                    r#"diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1 @@
-old
+new
diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -8 +8 @@
-main_old
+main_new
"#,
                )
                .expect("parse diff"),
            ),
        );
        app.focus_list();
        app.details_scroll = 21;
        app.selected_comment_index = 0;
        app.selected_diff_file.insert("1".to_string(), 1);
        app.selected_diff_line.insert("1".to_string(), 1);

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('q')),
            &config,
            &store,
            &tx
        ));

        assert_eq!(app.details_mode, DetailsMode::Conversation);
        assert_eq!(app.focus, FocusTarget::List);
        assert_eq!(app.details_scroll, 7);
        assert_eq!(app.selected_comment_index, 1);
        assert_eq!(app.status, "returned from diff");

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('v')),
            &config,
            &store,
            &tx
        ));

        assert_eq!(app.details_mode, DetailsMode::Diff);
        assert_eq!(app.focus, FocusTarget::List);
        assert_eq!(app.details_scroll, 21);
        assert_eq!(app.selected_diff_file.get("1"), Some(&1));
        assert_eq!(app.selected_diff_line.get("1"), Some(&1));
    }

    #[test]
    fn esc_in_diff_details_refocuses_diff_files() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        app.focus_sections();
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('v')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.details_mode, DetailsMode::Diff);
        assert_eq!(app.focus, FocusTarget::List);

        app.focus_details();
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Esc),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.details_mode, DetailsMode::Diff);
        assert_eq!(app.focus, FocusTarget::List);
        assert_eq!(app.status, "files focused");
    }

    #[test]
    fn v_opens_diff_from_any_focus_region() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        app.focus_ghr();
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('v')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.focus, FocusTarget::List);
        assert_eq!(app.details_mode, DetailsMode::Diff);

        app.show_conversation();
        app.focus_list();
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('V')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.focus, FocusTarget::List);
        assert_eq!(app.details_mode, DetailsMode::Diff);

        app.show_conversation();
        app.focus_sections();
        assert!(!handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::SHIFT),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.focus, FocusTarget::List);
        assert_eq!(app.details_mode, DetailsMode::Diff);
    }

    #[test]
    fn diff_file_navigation_switches_between_files() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.show_diff();
        app.diffs.insert(
            "1".to_string(),
            DiffState::Loaded(
                parse_pull_request_diff(
                    r#"diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1 @@
-old
+new
diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -8 +8 @@
-main_old
+main_new
"#,
                )
                .expect("parse diff"),
            ),
        );

        app.move_diff_file(1);
        assert_eq!(app.selected_diff_file.get("1"), Some(&1));
        assert_eq!(app.details_scroll, 0);

        app.move_diff_file(1);
        assert_eq!(app.selected_diff_file.get("1"), Some(&0));
    }

    #[test]
    fn diff_tree_groups_changed_files_by_directory() {
        let diff = parse_pull_request_diff(
            r#"diff --git a/src/app.rs b/src/app.rs
--- a/src/app.rs
+++ b/src/app.rs
@@ -1 +1 @@
-old
+new
diff --git a/src/github.rs b/src/github.rs
--- a/src/github.rs
+++ b/src/github.rs
@@ -1 +1 @@
-old
+new
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@ -1 +1 @@
-old
+new
"#,
        )
        .expect("parse diff");

        let entries = diff_tree_entries(&diff);
        let labels = entries
            .iter()
            .map(|entry| {
                (
                    entry.depth,
                    entry.file_index,
                    entry.label.as_str(),
                    entry.stats.as_str(),
                )
            })
            .collect::<Vec<_>>();

        assert!(labels.contains(&(0, None, "src", "")));
        assert!(labels.contains(&(1, Some(0), "app.rs", "M +1 -1")));
        assert!(labels.contains(&(1, Some(1), "github.rs", "M +1 -1")));
        assert!(labels.contains(&(0, Some(2), "README.md", "M +1 -1")));
    }

    #[test]
    fn diff_file_list_keys_select_files_and_keep_right_side_diff() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.show_diff();
        app.focus_list();
        app.diffs.insert(
            "1".to_string(),
            DiffState::Loaded(
                parse_pull_request_diff(
                    r#"diff --git a/src/app.rs b/src/app.rs
--- a/src/app.rs
+++ b/src/app.rs
@@ -1 +1 @@
-old
+app
diff --git a/src/github.rs b/src/github.rs
--- a/src/github.rs
+++ b/src/github.rs
@@ -1 +1 @@
-old
+github
"#,
                )
                .expect("parse diff"),
            ),
        );

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('j')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.selected_diff_file.get("1"), Some(&1));
        assert_eq!(app.focus, FocusTarget::List);

        let rendered = build_details_document(&app, 120)
            .lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("▾ src/github.rs"));
        assert!(rendered.contains("+ github"));
        assert!(!rendered.contains("+ app"));

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Enter),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.focus, FocusTarget::Details);
    }

    #[test]
    fn m_in_diff_file_list_keeps_files_open_instead_of_text_selection_mode() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.show_diff();
        app.focus_list();

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('m')),
            &config,
            &store,
            &tx
        ));

        assert!(app.mouse_capture_enabled);
        assert_eq!(app.details_mode, DetailsMode::Diff);
        assert_eq!(app.focus, FocusTarget::List);
        assert!(app.status.contains("mark diff lines from Details"));
    }

    #[test]
    fn m_restores_mouse_capture_even_in_diff_mode() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.show_diff();
        app.focus_details();
        app.mouse_capture_enabled = false;

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('m')),
            &config,
            &store,
            &tx
        ));

        assert!(app.mouse_capture_enabled);
        assert_eq!(app.details_mode, DetailsMode::Diff);
        assert_eq!(app.status, "mouse controls enabled");
    }

    #[test]
    fn c_in_diff_file_list_opens_review_comment_dialog() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.show_diff();
        app.focus_list();
        app.diffs.insert(
            "1".to_string(),
            DiffState::Loaded(
                parse_pull_request_diff(
                    r#"diff --git a/src/app.rs b/src/app.rs
--- a/src/app.rs
+++ b/src/app.rs
@@ -1 +1 @@
-old
+app
"#,
                )
                .expect("parse diff"),
            ),
        );

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('c')),
            &config,
            &store,
            &tx
        ));

        assert_eq!(app.focus, FocusTarget::Details);
        assert!(matches!(
            app.comment_dialog.as_ref().map(|dialog| &dialog.mode),
            Some(CommentDialogMode::Review { target })
                if target.path == "src/app.rs" && target.line == 1
        ));
    }

    #[test]
    fn a_in_diff_file_list_opens_normal_comment_dialog() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.show_diff();
        app.focus_list();

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('a')),
            &config,
            &store,
            &tx
        ));

        assert_eq!(app.focus, FocusTarget::Details);
        assert_eq!(app.details_mode, DetailsMode::Diff);
        assert_eq!(
            app.comment_dialog.as_ref().map(|dialog| &dialog.mode),
            Some(&CommentDialogMode::New)
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
    fn startup_setup_dialog_detects_missing_gh_before_refresh() {
        assert_eq!(
            startup_setup_dialog_from_gh_probe(Err(io::Error::new(io::ErrorKind::NotFound, "gh"))),
            Some(SetupDialog::MissingGh)
        );
        assert_eq!(
            startup_setup_dialog_from_gh_probe(Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "gh"
            ))),
            None
        );
        assert_eq!(startup_setup_dialog_from_gh_probe(Ok(())), None);
    }

    #[test]
    fn startup_setup_dialog_sets_status_without_initializing_dialog() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.show_startup_initializing();

        app.show_setup_dialog(SetupDialog::MissingGh);

        assert_eq!(app.setup_dialog, Some(SetupDialog::MissingGh));
        assert_eq!(app.startup_dialog, None);
        assert_eq!(
            app.status,
            "GitHub CLI missing: install `gh`, then run `gh auth login`"
        );
    }

    #[test]
    fn refresh_failure_opens_setup_dialog() {
        let mut failed_section = test_section();
        failed_section.items.clear();
        failed_section.error = Some("GitHub CLI `gh` is required but was not found.".to_string());
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.show_startup_initializing();

        app.handle_msg(AppMsg::RefreshFinished {
            sections: vec![failed_section],
            save_error: None,
        });

        assert_eq!(app.setup_dialog, Some(SetupDialog::MissingGh));
        assert_eq!(app.startup_dialog, None);
        assert_eq!(
            app.status,
            "GitHub CLI missing: install `gh`, then run `gh auth login`"
        );
    }

    #[test]
    fn startup_refresh_finishes_with_ready_dialog() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let paths = test_paths();

        assert_eq!(app.startup_dialog, None);
        app.show_startup_initializing();
        assert_eq!(app.startup_dialog, Some(StartupDialog::Initializing));

        app.handle_msg(AppMsg::RefreshFinished {
            sections: vec![test_section()],
            save_error: None,
        });

        assert_eq!(app.startup_dialog, Some(StartupDialog::Ready));
        assert_eq!(app.setup_dialog, None);
        assert_eq!(app.status, "refresh complete");

        let (_title, lines, show_ok) =
            startup_dialog_content(StartupDialog::Ready, &app, &paths, 0);
        let text = lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(show_ok);
        assert!(text.contains("2 item(s) across 1 section(s)"));
        assert!(text.contains("/tmp/ghr-test/config.toml"));
        assert!(text.contains("? anytime"));
    }

    #[test]
    fn startup_dialog_is_only_needed_without_cached_snapshots() {
        assert!(should_show_startup_dialog(&HashMap::new()));

        let cached = HashMap::from([("pull_requests:test".to_string(), test_section())]);
        assert!(!should_show_startup_dialog(&cached));
    }

    #[test]
    fn refresh_finished_does_not_open_ready_dialog_without_startup_dialog() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);

        app.handle_msg(AppMsg::RefreshFinished {
            sections: vec![test_section()],
            save_error: None,
        });

        assert_eq!(app.startup_dialog, None);
        assert_eq!(app.status, "refresh complete");
    }

    #[test]
    fn progressive_refresh_section_renders_before_full_refresh_finishes() {
        let mut empty = test_section();
        empty.items.clear();
        let mut app = AppState::new(SectionKind::PullRequests, vec![empty]);
        app.show_startup_initializing();
        app.refreshing = true;

        app.handle_msg(AppMsg::RefreshSectionLoaded {
            section: test_section(),
            save_error: None,
        });

        assert_eq!(app.startup_dialog, Some(StartupDialog::Ready));
        assert!(app.refreshing);
        assert_eq!(app.sections[0].items.len(), 2);
        assert_eq!(
            app.current_item().map(|item| item.title.as_str()),
            Some("Compiler diagnostics")
        );
        assert_eq!(app.status, "loaded Test; still refreshing");
    }

    #[test]
    fn full_refresh_after_progress_does_not_reopen_ready_dialog() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.show_startup_initializing();

        app.handle_msg(AppMsg::RefreshSectionLoaded {
            section: test_section(),
            save_error: None,
        });
        app.handle_msg(AppMsg::RefreshFinished {
            sections: vec![test_section()],
            save_error: None,
        });

        assert_eq!(app.startup_dialog, Some(StartupDialog::Ready));
        assert_eq!(app.status, "refresh complete");
    }

    #[test]
    fn dismissed_startup_ready_dialog_stays_closed_after_full_refresh() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.show_startup_initializing();

        app.handle_msg(AppMsg::RefreshSectionLoaded {
            section: test_section(),
            save_error: None,
        });
        app.dismiss_startup_dialog();
        app.handle_msg(AppMsg::RefreshFinished {
            sections: vec![test_section()],
            save_error: None,
        });

        assert_eq!(app.startup_dialog, None);
        assert_eq!(app.status, "refresh complete");
    }

    #[test]
    fn startup_initializing_dialog_asks_user_to_wait() {
        let app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let paths = test_paths();
        let (_title, lines, show_ok) =
            startup_dialog_content(StartupDialog::Initializing, &app, &paths, 2);
        let text = lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!show_ok);
        assert!(text.contains("Loading.."));
        assert!(text.contains("["));
        assert!(text.contains("Please wait"));
        assert!(!text.contains("quit"));
        assert!(!text.contains("Ctrl+C"));
    }

    #[test]
    fn startup_initializing_q_does_not_quit() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        app.startup_dialog = Some(StartupDialog::Initializing);
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('q')),
            &config,
            &store,
            &tx
        ));

        assert_eq!(app.startup_dialog, Some(StartupDialog::Initializing));
    }

    #[test]
    fn startup_ready_dialog_dismisses_before_regular_input() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        app.startup_dialog = Some(StartupDialog::Ready);
        app.search_active = true;
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Enter),
            &config,
            &store,
            &tx
        ));

        assert_eq!(app.startup_dialog, None);
        assert!(app.search_active);
        assert_eq!(app.status, "startup hint dismissed");
    }

    #[test]
    fn startup_ready_ok_click_dismisses_dialog() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let area = Rect::new(0, 0, 120, 40);
        app.startup_dialog = Some(StartupDialog::Ready);
        let ok_area = startup_dialog_ok_area(startup_dialog_area(StartupDialog::Ready, area));

        assert!(handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: ok_area.x,
                row: ok_area.y,
                modifiers: KeyModifiers::NONE,
            },
            area,
        ));

        assert_eq!(app.startup_dialog, None);
        assert_eq!(app.status, "startup hint dismissed");
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
            details_mode: "conversation".to_string(),
            details_scroll: 7,
            selected_comment_index: 2,
            expanded_comments: Vec::new(),
            details_scroll_by_item: HashMap::new(),
            selected_comment_index_by_item: HashMap::new(),
            viewed_details_snapshot: HashMap::new(),
            viewed_comments_snapshot: HashMap::new(),
            selected_diff_file: HashMap::new(),
            selected_diff_line: HashMap::new(),
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
        assert_eq!(saved.details_mode, "conversation");
        assert_eq!(saved.details_scroll, 7);
        assert_eq!(saved.details_scroll_by_item.get("issue-2"), Some(&7));
        assert_eq!(
            saved.selected_comment_index_by_item.get("issue-2"),
            Some(&2)
        );
    }

    #[test]
    fn ui_state_restores_diff_mode_for_selected_pull_request() {
        let state = UiState {
            active_view: builtin_view_key(SectionKind::PullRequests),
            selected_index: HashMap::from([(builtin_view_key(SectionKind::PullRequests), 0)]),
            focus: "details".to_string(),
            details_mode: "diff".to_string(),
            details_scroll: 19,
            selected_diff_file: HashMap::from([("1".to_string(), 2)]),
            selected_diff_line: HashMap::from([("1".to_string(), 7)]),
            ..UiState::default()
        };

        let app = AppState::with_ui_state(SectionKind::PullRequests, vec![test_section()], state);
        let saved = app.ui_state();

        assert_eq!(app.details_mode, DetailsMode::Diff);
        assert_eq!(app.focus, FocusTarget::Details);
        assert_eq!(app.details_scroll, 19);
        assert_eq!(app.selected_diff_file.get("1"), Some(&2));
        assert_eq!(app.selected_diff_line.get("1"), Some(&7));
        assert_eq!(saved.details_mode, "diff");
        assert_eq!(saved.selected_diff_file.get("1"), Some(&2));
        assert_eq!(saved.selected_diff_line.get("1"), Some(&7));
    }

    #[test]
    fn conversation_details_position_is_restored_per_item() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.details.insert(
            "1".to_string(),
            DetailState::Loaded(vec![
                comment("alice", "first", None),
                comment("bob", "second", None),
            ]),
        );
        app.focus_details();
        app.details_scroll = 9;
        app.selected_comment_index = 1;

        app.move_selection(1);
        assert_eq!(app.current_item().map(|item| item.id.as_str()), Some("2"));
        assert_eq!(app.details_scroll, 0);
        assert_eq!(app.selected_comment_index, 0);

        app.move_selection(-1);
        app.focus_details();

        assert_eq!(app.current_item().map(|item| item.id.as_str()), Some("1"));
        assert_eq!(app.details_scroll, 9);
        assert_eq!(app.selected_comment_index, 1);
    }

    #[test]
    fn viewed_details_snapshot_marks_unseen_comment_updates() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.details.insert(
            "1".to_string(),
            DetailState::Loaded(vec![comment("alice", "old", None)]),
        );
        app.focus_details();
        assert!(!app.item_has_unseen_details(app.current_item().expect("item")));

        app.focus_list();
        app.handle_msg(AppMsg::CommentsLoaded {
            item_id: "1".to_string(),
            comments: Ok(vec![
                comment("alice", "old", None),
                comment("bob", "new", None),
            ]),
        });
        assert!(app.item_has_unseen_details(app.current_item().expect("item")));

        app.focus_details();
        assert!(!app.item_has_unseen_details(app.current_item().expect("item")));
    }

    #[test]
    fn viewed_details_snapshot_marks_unseen_list_metadata_updates() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.focus_details();
        assert!(!app.item_has_unseen_details(app.current_item().expect("item")));

        app.sections[0].items[0].comments = Some(3);

        assert!(app.item_has_unseen_details(app.current_item().expect("item")));
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

        assert!(app.start_comments_load_if_needed(&item));

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
    fn pr_action_hints_can_load_without_reloading_cached_comments() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.details.insert(
            "1".to_string(),
            DetailState::Loaded(vec![comment("alice", "cached comment", None)]),
        );
        let item = app.current_item().cloned().expect("selected item");

        assert!(app.details_load_needed(&item));
        assert!(!app.start_comments_load_if_needed(&item));
        assert!(app.start_action_hints_load_if_needed(&item));

        assert!(
            matches!(
                app.details.get("1"),
                Some(DetailState::Loaded(comments)) if comments[0].body == "cached comment"
            ),
            "loading PR metadata should not hide already loaded comments"
        );
        assert!(matches!(
            app.action_hints.get("1"),
            Some(ActionHintState::Loading)
        ));
    }

    #[test]
    fn list_focused_details_load_is_debounced_but_details_focus_is_immediate() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.focus_list();
        let item_id = app.current_item().expect("item").id.clone();

        assert!(app.details_load_needed(app.current_item().unwrap()));
        assert!(!app.details_load_ready(&item_id));
        assert!(app.pending_details_load.is_some());

        app.pending_details_load
            .as_mut()
            .expect("pending load")
            .ready_at = Instant::now() - Duration::from_millis(1);
        assert!(app.details_load_ready(&item_id));

        app.pending_details_load = Some(PendingDetailsLoad {
            item_id: item_id.clone(),
            ready_at: Instant::now() + Duration::from_secs(60),
        });
        app.focus_details();
        assert!(app.details_load_ready(&item_id));
        assert!(app.pending_details_load.is_none());
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
    fn ctrl_c_in_diff_keeps_diff_state_available_for_save() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.show_diff();
        app.focus_details();
        app.details_scroll = 11;
        app.selected_diff_file.insert("1".to_string(), 3);
        app.selected_diff_line.insert("1".to_string(), 5);

        assert!(handle_key(
            &mut app,
            ctrl_key(KeyCode::Char('c')),
            &config,
            &store,
            &tx
        ));

        let saved = app.ui_state();
        assert_eq!(saved.details_mode, "diff");
        assert_eq!(saved.focus, "details");
        assert_eq!(saved.details_scroll, 11);
        assert_eq!(saved.selected_diff_file.get("1"), Some(&3));
        assert_eq!(saved.selected_diff_line.get("1"), Some(&5));
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
            text.contains("List items  j/k move  pg page  [ ] results  g/G ends  enter Details")
        );
        assert!(text.contains("/ search"));
        assert!(text.contains("v diff"));
        assert!(text.contains("M/C/A/X pr action"));
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
        assert!(ghr.contains("ghr tabs  tab/h/l switch  j/enter Sections  esc List"));
        assert!(!ghr.contains("M/C/A/X pr action"));

        app.focus_sections();
        let sections = footer_line(&app, &paths).to_string();
        assert!(sections.contains("Sections tabs  tab/h/l switch  k ghr  j/enter List"));
        assert!(!sections.contains("a comment"));

        app.focus_details();
        let details = footer_line(&app, &paths).to_string();
        assert!(details.contains("Details content  j/k scroll"));
        assert!(details.contains("n/p comment  enter expand  c/a comment  R reply  e edit"));
        assert!(details.contains("esc List"));
        assert!(!details.contains("g/G ends"));

        app.show_diff();
        app.focus_details();
        let diff = footer_line(&app, &paths).to_string();
        assert!(diff.contains("Details diff  j/k line  n/p page  g/G top/bottom"));
        assert!(diff.contains("[ ] file  m begin  e end  c inline  a comment  M/C/A/X pr action"));
        assert!(!diff.contains("m text-select"));
        assert!(diff.contains("q back"));
        assert!(!diff.contains("q quit"));
        assert!(!diff.contains("R reply"));

        app.focus_list();
        let diff_list = footer_line(&app, &paths).to_string();
        assert!(!diff_list.contains("m text-select"));
    }

    #[test]
    fn conversation_details_n_and_p_focus_comments_and_keep_d_u_as_page_scroll() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.focus_details();
        app.details.insert(
            "1".to_string(),
            DetailState::Loaded(vec![
                comment("alice", "first comment", None),
                comment("bob", "second comment", None),
                comment("carol", "third comment", None),
            ]),
        );
        let area = Rect::new(0, 0, 100, 12);
        let details_area = details_area_for(&app, area);
        let inner = block_inner(details_area);
        let document = build_details_document(&app, inner.width);
        let second = document.comment_region(1).expect("second comment");
        assert!(
            second.start_line >= usize::from(inner.height),
            "small test viewport should start before the second comment"
        );
        app.details_scroll = second
            .start_line
            .saturating_sub(usize::from(inner.height).saturating_sub(1))
            .min(usize::from(u16::MAX)) as u16;
        assert!(
            second.start_line < usize::from(app.details_scroll) + usize::from(inner.height),
            "the separator is visible before jumping"
        );
        assert!(
            second.focus_line() >= usize::from(app.details_scroll) + usize::from(inner.height),
            "the actual comment header is still below the viewport before jumping"
        );

        assert!(!handle_key_in_area(
            &mut app,
            key(KeyCode::Char('n')),
            &config,
            &store,
            &tx,
            Some(area)
        ));
        assert_eq!(app.selected_comment_index, 1);
        let scroll_start = usize::from(app.details_scroll);
        let scroll_end = scroll_start + usize::from(inner.height);
        assert!(
            second.focus_line() >= scroll_start && second.focus_line() < scroll_end,
            "n should scroll the focused comment header into view"
        );

        assert!(!handle_key_in_area(
            &mut app,
            key(KeyCode::Char('d')),
            &config,
            &store,
            &tx,
            Some(area)
        ));
        assert_eq!(app.selected_comment_index, 1);
        assert!(usize::from(app.details_scroll) >= scroll_start);
    }

    #[test]
    fn conversation_details_p_at_first_comment_scrolls_toward_metadata() {
        let mut item = work_item("1", "rust-lang/rust", 1, "Compiler diagnostics", None);
        item.body = Some(
            (1..=24)
                .map(|index| format!("description paragraph {index}"))
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
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.focus_details();
        app.details.insert(
            "1".to_string(),
            DetailState::Loaded(vec![comment("alice", "first comment", None)]),
        );
        let area = Rect::new(0, 0, 100, 14);
        let details_area = details_area_for(&app, area);
        let inner = block_inner(details_area);
        let document = build_details_document(&app, inner.width);
        let first = document.comment_region(0).expect("first comment");
        app.details_scroll = first.focus_line().min(usize::from(u16::MAX)) as u16;
        app.selected_comment_index = 0;
        let before = app.details_scroll;

        assert!(!handle_key_in_area(
            &mut app,
            key(KeyCode::Char('p')),
            &config,
            &store,
            &tx,
            Some(area)
        ));

        assert_eq!(app.selected_comment_index, 0);
        assert!(app.details_scroll < before);
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
        assert!(missing_text.contains("https://cli.github.com/"));
        assert!(missing_text.contains("install_linux.md"));
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
        assert!(rendered.contains("Local Search: /borrow_  Enter search  Esc clear"));
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
    fn details_comment_search_input_focuses_matching_comment() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let item_id = app.current_item().expect("item").id.clone();
        app.details.insert(
            item_id,
            DetailState::Loaded(vec![
                comment("alice", "first compiler note", None),
                comment("bob", "fiber routing regression", None),
                comment("carol", "another compiler note", None),
            ]),
        );
        app.focus_details();

        app.start_comment_search();
        for value in "routing".chars() {
            app.handle_comment_search_key(key(KeyCode::Char(value)), None);
        }

        assert_eq!(app.focus, FocusTarget::Details);
        assert!(app.comment_search_active);
        assert_eq!(app.comment_search_query, "routing");
        assert_eq!(app.selected_comment_index, 1);
        assert_eq!(app.status, "comment search: 1/1 for 'routing'");
        assert!(app.search_query.is_empty());
    }

    #[test]
    fn comment_search_n_and_p_cycle_matching_comments() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let item_id = app.current_item().expect("item").id.clone();
        app.details.insert(
            item_id,
            DetailState::Loaded(vec![
                comment("alice", "compiler first", None),
                comment("bob", "fiber only", None),
                comment("carol", "compiler second", None),
            ]),
        );
        app.focus_details();
        app.comment_search_query = "compiler".to_string();
        app.selected_comment_index = 0;

        app.move_comment_in_view(1, None);
        assert_eq!(app.selected_comment_index, 2);
        assert_eq!(app.status, "comment search: 2/2 for 'compiler'");

        app.move_comment_in_view(1, None);
        assert_eq!(app.selected_comment_index, 0);
        assert_eq!(app.status, "comment search: 1/2 for 'compiler'");
    }

    #[test]
    fn details_title_shows_comment_search_input_prompt() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let item_id = app.current_item().expect("item").id.clone();
        app.details.insert(
            item_id,
            DetailState::Loaded(vec![comment("alice", "borrow checker", None)]),
        );
        app.focus_details();
        app.start_comment_search();
        app.comment_search_query = "borrow".to_string();
        let backend = ratatui::backend::TestBackend::new(220, 30);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let paths = test_paths();

        terminal
            .draw(|frame| draw(frame, &app, &paths))
            .expect("draw");

        let rendered = buffer_lines(terminal.backend().buffer()).join("\n");
        assert!(rendered.contains("Comment Search: /borrow_  Enter keep  Esc clear"));
        assert!(rendered.contains("Comment search: 1/1 matches for /borrow"));
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
    fn top_tab_highlights_use_high_contrast_blocks() {
        let view = active_view_tab_style();
        assert_eq!(view.fg, Some(Color::Black));
        assert_eq!(view.bg, Some(Color::LightCyan));
        assert!(view.add_modifier.contains(Modifier::BOLD));

        let section = active_section_tab_style();
        assert_eq!(section.fg, Some(Color::Black));
        assert_eq!(section.bg, Some(Color::LightYellow));
        assert!(section.add_modifier.contains(Modifier::BOLD));
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
        assert!(app.global_search_started_at.is_some());
        assert_eq!(app.global_search_scope.as_deref(), Some("rust-lang/rust"));
        assert_eq!(app.status, "searching rust-lang/rust for 'fib'");

        let rendered = global_search_loading_content(&app, 1)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Searching pull requests and issues."));
        assert!(rendered.contains("query: fib"));
        assert!(rendered.contains("scope: rust-lang/rust"));
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
        app.global_search_started_at = Some(Instant::now());
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
        assert!(app.global_search_started_at.is_none());
    }

    #[test]
    fn escape_on_search_results_returns_to_previous_list() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.start_global_search_input();
        app.global_search_query = "fiber".to_string();
        app.handle_global_search_key_with_submit(key(KeyCode::Enter), |_| {});

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
        app.handle_msg(AppMsg::GlobalSearchFinished {
            query: "fiber".to_string(),
            sections: vec![pr_section],
        });

        assert_eq!(app.active_view, global_search_view_key());
        assert!(app.view_tabs().iter().any(|view| view.label == "Search"));
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Esc),
            &config,
            &store,
            &tx
        ));

        assert_eq!(app.active_view, builtin_view_key(SectionKind::PullRequests));
        assert!(!app.view_tabs().iter().any(|view| view.label == "Search"));
        assert!(app.global_search_query.is_empty());
        assert_eq!(
            app.current_section().map(|section| section.title.as_str()),
            Some("Test")
        );
        assert_eq!(app.status, "search results cleared");
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
                head: Some(PullRequestBranch {
                    repository: "chenyukang/ghr".to_string(),
                    branch: "feature/checks".to_string(),
                }),
            }),
        );

        let lines = build_details_document(&app, 120)
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        let rendered = lines.join("\n");

        assert!(rendered.contains("action: Approvable, Mergeable"));
        assert!(rendered.contains("checks:"));
        assert!(rendered.contains("10 pass"));
        assert!(rendered.contains("2 fail"));
        assert!(rendered.contains("1 pending"));
        assert!(rendered.contains("branch: chenyukang/ghr:feature/checks"));
        assert!(rendered.contains("action note: Merge blocked: checks pending"));
        let branch_line = lines
            .iter()
            .position(|line| line.contains("branch: chenyukang/ghr:feature/checks"))
            .expect("branch line");
        let action_line = lines
            .iter()
            .position(|line| line.contains("action: Approvable, Mergeable"))
            .expect("action line");
        assert!(
            action_line > branch_line,
            "action/checks should start on a new line"
        );
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
    fn mergeable_action_label_is_rendered_green() {
        let segments = action_label_segments(&["Approvable".to_string(), "Mergeable".to_string()]);

        let approvable = segments
            .iter()
            .find(|segment| segment.text == "Approvable")
            .expect("approvable segment");
        assert_eq!(approvable.style, Style::default());

        let mergeable = segments
            .iter()
            .find(|segment| segment.text == "Mergeable")
            .expect("mergeable segment");
        assert_eq!(mergeable.style.fg, Some(Color::LightGreen));
    }

    #[test]
    fn merge_conflict_action_note_is_rendered_red() {
        let segments =
            action_note_segments("Merge blocked: draft; merge conflicts must be resolved");

        let conflict = segments
            .iter()
            .find(|segment| segment.text == "merge conflicts must be resolved")
            .expect("conflict segment");
        assert_eq!(conflict.style.fg, Some(Color::LightRed));
        assert!(conflict.style.add_modifier.contains(Modifier::BOLD));

        let rendered = segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<String>();
        assert_eq!(
            rendered,
            "Merge blocked: draft; merge conflicts must be resolved"
        );
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
    fn markdown_tables_render_as_separated_rows() {
        let mut builder = DetailsBuilder::new(100);
        builder.push_markdown_block(
            "| x | not const | const |\n| --- | ---- | --- |\n| not comptime | fn | const fn |\n| comptime | comptime fn | ??? |",
            "empty",
            usize::MAX,
            usize::MAX,
        );
        let rendered = builder
            .finish()
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        let joined = rendered.join("\n");

        assert!(
            rendered.iter().any(|line| line.contains("x")
                && line.contains("│ not const")
                && line.contains("│ const")),
            "table header should keep visible column separators: {rendered:?}"
        );
        assert!(
            rendered.iter().any(|line| line.contains("not comptime")
                && line.contains("│ fn")
                && line.contains("│ const fn")),
            "table row should keep visible column separators: {rendered:?}"
        );
        assert!(
            rendered.iter().any(|line| line.contains("comptime")
                && line.contains("│ comptime fn")
                && line.contains("│ ???")),
            "table row should keep visible column separators: {rendered:?}"
        );
        assert!(!joined.contains("xnot constconst"));
        assert!(!joined.contains("| ---"));
    }

    #[test]
    fn details_headings_have_body_gap() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.details.insert(
            "1".to_string(),
            DetailState::Loaded(vec![comment("alice", "First comment", None)]),
        );

        let rendered = build_details_document(&app, 100)
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        let description_index = rendered
            .iter()
            .position(|line| line == "Description")
            .expect("description heading");
        assert_eq!(rendered.get(description_index + 1), Some(&String::new()));
        assert_eq!(
            rendered.get(description_index + 2),
            Some(&"A body with useful context".to_string())
        );

        let comments_index = rendered
            .iter()
            .position(|line| line == "Recent Comments")
            .expect("comments heading");
        assert_eq!(rendered.get(comments_index + 1), Some(&String::new()));
        assert!(
            rendered
                .get(comments_index + 2)
                .is_some_and(|line| line.trim_start().starts_with('─')),
            "comment separator should start after the heading gap: {rendered:?}"
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
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();

        assert!(
            quote_lines.len() > 1,
            "quote should wrap in the narrow details pane"
        );
        assert!(
            quote_lines.iter().all(|line| line.starts_with("│ ")),
            "each wrapped quote line should keep the quote marker: {quote_lines:?}"
        );
        let quoted_text = document.lines[0]
            .spans
            .iter()
            .find(|span| span.content.contains("quoted"))
            .expect("quoted text span");
        assert_eq!(quoted_text.style.fg, Some(Color::Gray));
        let normal_index = rendered
            .iter()
            .position(|line| line == "normal reply")
            .expect("normal reply line");
        assert_eq!(
            rendered
                .get(normal_index.saturating_sub(1))
                .map(String::as_str),
            Some(""),
            "normal paragraph should be separated from the quoted block: {rendered:?}"
        );
        assert!(
            document.lines[normal_index]
                .spans
                .iter()
                .all(|span| span.style.fg != Some(Color::Gray)),
            "normal paragraph should keep the regular text color"
        );
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
    fn markdown_preserves_block_spacing_without_splitting_lists_or_code() {
        let mut builder = DetailsBuilder::new(72);
        builder.push_markdown_block_indented(
            "Feature gate: `#![feature(split_as_slice)]`\n\nThis is a tracking issue for:\n\n### Public API\n\n```rust\nlet a = [1,2,3];\nlet mut iter = a.split(|i| i == 2);\n```\n\n- [x] Implementation: #92287\n- [ ] Final comment period (FCP)",
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

        assert_eq!(rendered[0], "  Feature gate: #![feature(split_as_slice)]");
        assert!(rendered[1].trim().is_empty());
        assert_eq!(rendered[2], "  This is a tracking issue for:");
        assert!(rendered[3].trim().is_empty());
        assert_eq!(rendered[4], "  Public API");
        assert!(rendered[5].trim().is_empty());
        assert_eq!(rendered[6], "  let a = [1,2,3];");
        assert_eq!(rendered[7], "  let mut iter = a.split(|i| i == 2);");
        assert!(rendered[8].trim().is_empty());
        assert_eq!(rendered[9], "  - [x] Implementation: #92287");
        assert_eq!(rendered[10], "  - [ ] Final comment period (FCP)");
    }

    #[test]
    fn fenced_rust_code_preserves_indentation_and_highlights() {
        let mut builder = DetailsBuilder::new(64);
        builder.push_markdown_block_indented(
            "```rust\nfn places_alias<'tcx>(\n    tcx: TyCtxt<'tcx>,\n) -> bool {\n    return false; // conservative\n}\n```",
            "empty",
            usize::MAX,
            usize::MAX,
            COMMENT_LEFT_PADDING,
            COMMENT_RIGHT_PADDING,
        );
        let document = builder.finish();
        let rendered = document
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert_eq!(rendered[0], "  fn places_alias<'tcx>(");
        assert_eq!(rendered[1], "      tcx: TyCtxt<'tcx>,");
        assert_eq!(rendered[3], "      return false; // conservative");

        let keyword = document.lines[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "fn")
            .expect("highlighted fn keyword");
        assert_eq!(keyword.style.fg, Some(Color::LightMagenta));
        assert!(keyword.style.add_modifier.contains(Modifier::BOLD));

        let comment = document.lines[3]
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "// conservative")
            .expect("highlighted comment");
        assert_eq!(comment.style.fg, Some(Color::DarkGray));
    }

    #[test]
    fn plain_fenced_code_preserves_log_indentation_and_highlights_failures() {
        let mut builder = DetailsBuilder::new(88);
        builder.push_markdown_block_indented(
            "```plain\n---\n    Finished `dev` profile [unoptimized + debuginfo] target(s) in 31.62s\nerror[E0308]: mismatched types\n   --> /rust/deps/zerovec/src/yoke_impls.rs:164:19\n[2026-04-30T21:58:21.028Z INFO  opt_dist::timer] Section ended: FAIL\n```",
            "empty",
            usize::MAX,
            usize::MAX,
            COMMENT_LEFT_PADDING,
            COMMENT_RIGHT_PADDING,
        );
        let document = builder.finish();
        let rendered = document
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert_eq!(rendered[0], "  ---");
        assert_eq!(
            rendered[1],
            "      Finished `dev` profile [unoptimized + debuginfo] target(s) in 31.62s"
        );
        assert_eq!(rendered[2], "  error[E0308]: mismatched types");

        let separator = document.lines[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "---")
            .expect("highlighted separator");
        assert_eq!(separator.style.fg, Some(Color::DarkGray));

        let error = document.lines[2]
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "error[E0308]: mismatched types")
            .expect("highlighted error");
        assert_eq!(error.style.fg, Some(Color::LightRed));
        assert!(error.style.add_modifier.contains(Modifier::BOLD));
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
                    parent_id: None,
                    is_mine: false,
                    review: None,
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
                    parent_id: None,
                    is_mine: false,
                    review: None,
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
    fn github_mentions_in_comments_are_clickable() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.details.insert(
            "1".to_string(),
            DetailState::Loaded(vec![comment(
                "npmccallum",
                "@clarfonthey Apologies. See email test@example.com and https://github.com/rust-lang/rust.",
                None,
            )]),
        );

        let document = build_details_document(&app, 120);
        let rendered = document
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        let mention_line = rendered
            .iter()
            .position(|line| line.contains("@clarfonthey"))
            .expect("mention line");
        let mention_column = rendered[mention_line].find("@clarfonthey").unwrap() as u16;
        let email_column = rendered[mention_line].find("@example").unwrap() as u16;
        let github_column = rendered[mention_line].find("https://github.com").unwrap() as u16;

        assert_eq!(
            document.link_at(mention_line, mention_column),
            Some("https://github.com/clarfonthey".to_string())
        );
        assert_eq!(document.link_at(mention_line, email_column), None);
        assert_eq!(
            document.link_at(mention_line, github_column),
            Some("https://github.com/rust-lang/rust".to_string())
        );
    }

    #[test]
    fn review_replies_render_under_parent_comment() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let review = crate::model::ReviewCommentPreview {
            path: "src/lib.rs".to_string(),
            line: Some(10),
            original_line: None,
            start_line: None,
            original_start_line: None,
            side: Some("RIGHT".to_string()),
            start_side: None,
            diff_hunk: None,
        };
        let mut parent = comment("alice", "parent review", None);
        parent.id = Some(1);
        parent.review = Some(review.clone());
        let mut standalone = comment("carol", "standalone comment", None);
        standalone.id = Some(2);
        let mut child = comment("bob", "child reply", None);
        child.id = Some(3);
        child.parent_id = Some(1);
        child.review = Some(review);
        app.details.insert(
            "1".to_string(),
            DetailState::Loaded(vec![parent, standalone, child]),
        );

        let document = build_details_document(&app, 120);
        let rendered = document
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        let parent_line = rendered
            .iter()
            .position(|line| line.contains("parent review"))
            .expect("parent body");
        let child_line = rendered
            .iter()
            .position(|line| line.contains("child reply"))
            .expect("child body");
        let standalone_line = rendered
            .iter()
            .position(|line| line.contains("standalone comment"))
            .expect("standalone body");

        assert!(parent_line < child_line);
        assert!(child_line < standalone_line);
        assert!(
            rendered
                .iter()
                .any(|line| line.contains('↳') && line.contains("bob")),
            "child reply should show a nested connector: {rendered:?}"
        );
        assert!(
            rendered.iter().any(|line| line.starts_with("    ↳ bob")),
            "child reply should be indented away from parent comments: {rendered:?}"
        );
        assert_eq!(document.comment_at(child_line), Some(2));

        app.focus_details();
        app.selected_comment_index = 0;
        app.move_comment(1);
        assert_eq!(app.selected_comment_index, 2);
        app.move_comment(1);
        assert_eq!(app.selected_comment_index, 1);
    }

    #[test]
    fn long_comments_are_collapsed_until_toggled() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let mut long_comment = comment(
            "rust-log-analyzer",
            &(1..=45)
                .map(|line| {
                    if line == 45 {
                        "final unique tail marker".to_string()
                    } else {
                        format!("line {line}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Some("https://github.com/rust-lang/rust/pull/146097#issuecomment-3261964634"),
        );
        long_comment.id = Some(3_261_964_634);
        app.details
            .insert("1".to_string(), DetailState::Loaded(vec![long_comment]));

        let document = build_details_document(&app, 120);
        let rendered = document
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        let joined = rendered.join("\n");

        assert!(joined.contains("▸ collapsed"));
        assert!(joined.contains("▸ show full comment"));
        assert!(joined.contains("line 1"));
        assert!(!joined.contains("final unique tail marker"));

        let expand_line = rendered
            .iter()
            .position(|line| line.contains("show full comment"))
            .expect("expand action line");
        let expand_column = rendered[expand_line].find("show full comment").unwrap() as u16;
        assert_eq!(
            document.action_at(expand_line, expand_column),
            Some(DetailAction::ToggleCommentExpanded(0))
        );

        app.handle_detail_action(DetailAction::ToggleCommentExpanded(0));
        let expanded = build_details_document(&app, 120)
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(expanded.contains("▾ expanded"));
        assert!(expanded.contains("final unique tail marker"));
        assert!(!expanded.contains("show full comment"));
        assert!(
            app.ui_state()
                .expanded_comments
                .contains(&"1:comment:3261964634".to_string())
        );
    }

    #[test]
    fn details_comments_show_inline_review_location() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let mut inline = own_comment(
            88,
            "chenyukang",
            "This is a review comment?",
            Some("https://github.com/chenyukang/ghr/pull/8#discussion_r88"),
        );
        inline.review = Some(crate::model::ReviewCommentPreview {
            path: "src/github.rs".to_string(),
            line: Some(876),
            original_line: None,
            start_line: None,
            original_start_line: None,
            side: Some("RIGHT".to_string()),
            start_side: None,
            diff_hunk: Some(
                "@@ -873,6 +873,7 @@ fn run_gh_json(args: &[String]) -> Result<String> {\n \n     let output = Command::new(\"gh\")\n         .env(\"GH_PROMPT_DISABLED\", \"1\")\n+       .env(\"GH_NO_UPDATE_NOTIFIER\", \"1\")\n         .args(args)\n         .output()\n         .await".to_string(),
            ),
        });
        app.details
            .insert("1".to_string(), DetailState::Loaded(vec![inline]));
        app.focus_details();

        let rendered = build_details_document(&app, 120)
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("inline src/github.rs:876 right"));
        assert!(rendered.contains("GH_NO_UPDATE_NOTIFIER"));
        assert!(rendered.lines().any(|line| {
            line.contains('>') && line.contains('+') && line.contains("GH_NO_UPDATE_NOTIFIER")
        }));
        assert!(rendered.contains("This is a review comment?"));
        assert!(rendered.contains("comments: 1"));
    }

    #[test]
    fn inline_review_context_can_fall_back_to_original_line() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let mut inline = comment(
            "Copilot",
            "This deployScript example omits capacity on the output Cell.",
            Some("https://github.com/nervosnetwork/ckb-dev-skills/pull/2#discussion_r3014289695"),
        );
        let mut diff_hunk = "@@ -0,0 +1,240 @@\n".to_string();
        for line in 1..=49 {
            diff_hunk.push_str(&format!("+line {line}\n"));
        }
        diff_hunk.push_str("+outputsData: [ccc.bytesFrom(scriptBinary)],");
        inline.review = Some(crate::model::ReviewCommentPreview {
            path: "skill/deployment.md".to_string(),
            line: Some(56),
            original_line: Some(50),
            start_line: Some(44),
            original_start_line: Some(42),
            side: Some("RIGHT".to_string()),
            start_side: Some("RIGHT".to_string()),
            diff_hunk: Some(diff_hunk),
        });
        app.details
            .insert("1".to_string(), DetailState::Loaded(vec![inline]));
        app.focus_details();

        let rendered = build_details_document(&app, 140)
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("inline skill/deployment.md:44-56 right"));
        assert!(
            rendered
                .lines()
                .any(|line| line.contains('+') && line.contains("line 42")),
            "range comments should keep the selected range in view"
        );
        assert!(rendered.lines().any(|line| {
            line.contains('>') && line.contains('+') && line.contains("outputsData")
        }));
        assert!(
            !rendered.contains("+line 1"),
            "context should not fall back to the start of the hunk"
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
            .position(|line| line.to_string().contains("┃ ▸ bob"))
            .expect("selected comment header");
        let reply_column = document.lines[bob_line_index]
            .to_string()
            .find("reply")
            .expect("reply button") as u16;

        let selected_border_count = document
            .lines
            .iter()
            .filter(|line| line.to_string().contains("┃ ━"))
            .count();
        assert_eq!(selected_border_count, 2);
        assert!(
            document
                .lines
                .iter()
                .any(|line| line.to_string().contains("┃ Second comment"))
        );
        assert!(document.lines[bob_line_index].to_string().ends_with('┃'));
        let selected_body_line = document
            .lines
            .iter()
            .find(|line| line.to_string().contains("┃ Second comment"))
            .expect("selected comment body");
        assert!(selected_body_line.to_string().ends_with('┃'));
        assert!(
            selected_body_line
                .spans
                .iter()
                .any(|span| span.content.contains("Second")
                    && span.style.add_modifier.contains(Modifier::BOLD))
        );
        assert!(
            document
                .lines
                .iter()
                .all(|line| line.spans.iter().all(|span| span.style.bg.is_none()))
        );
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
                comment_id: 42,
                is_review: false,
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
                author: "alice".to_string(),
                review_comment_id: None,
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
                comment_id: 42,
                is_review: false,
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
    fn open_url_targets_changes_page_in_diff_mode() {
        let section = SectionSnapshot {
            key: "pull_requests:test".to_string(),
            kind: SectionKind::PullRequests,
            title: "Test".to_string(),
            filters: String::new(),
            items: vec![work_item("8", "chenyukang/ghr", 8, "Add diff UI", None)],
            total_count: None,
            page: 1,
            page_size: 0,
            refreshed_at: None,
            error: None,
        };
        let mut app = AppState::new(SectionKind::PullRequests, vec![section]);

        assert_eq!(
            app.selected_open_url().as_deref(),
            Some("https://github.com/chenyukang/ghr/pull/8")
        );

        app.show_diff();
        assert_eq!(
            app.selected_open_url().as_deref(),
            Some("https://github.com/chenyukang/ghr/pull/8/changes")
        );
    }

    #[test]
    fn c_key_in_details_opens_new_comment_dialog() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.focus_details();

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('c')),
            &config,
            &store,
            &tx
        ));

        assert_eq!(
            app.comment_dialog.map(|dialog| dialog.mode),
            Some(CommentDialogMode::New)
        );
        assert!(app.pr_action_dialog.is_none());
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
    fn capital_x_key_opens_checkout_confirmation_for_pull_request_list() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![checkout_test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = checkout_test_config();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('X')),
            &config,
            &store,
            &tx
        ));

        let dialog = app.pr_action_dialog.as_ref().expect("checkout dialog");
        assert_eq!(dialog.action, PrAction::Checkout);
        assert_eq!(dialog.item.id, "checkout-pr");
        assert!(dialog.checkout.is_some());
        assert_eq!(app.status, "confirm local pull request checkout");
    }

    #[test]
    fn capital_x_key_opens_checkout_confirmation_for_pull_request_details() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![checkout_test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = checkout_test_config();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.focus_details();
        app.action_hints.insert(
            "checkout-pr".to_string(),
            ActionHintState::Loaded(ActionHints {
                labels: Vec::new(),
                checks: None,
                note: None,
                head: Some(PullRequestBranch {
                    repository: "chenyukang/ghr".to_string(),
                    branch: "codex/pr-checkout-local".to_string(),
                }),
            }),
        );

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('X')),
            &config,
            &store,
            &tx
        ));

        let dialog = app.pr_action_dialog.as_ref().expect("checkout dialog");
        assert_eq!(dialog.action, PrAction::Checkout);
        assert_eq!(dialog.item.id, "checkout-pr");
        assert_eq!(
            dialog
                .checkout
                .as_ref()
                .and_then(|checkout| checkout.branch.as_ref())
                .map(pull_request_branch_label)
                .as_deref(),
            Some("chenyukang/ghr:codex/pr-checkout-local")
        );
        assert_eq!(app.status, "confirm local pull request checkout");
    }

    #[test]
    fn checkout_confirmation_remote_branch_is_clickable() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![checkout_test_section()]);
        let config = checkout_test_config();
        app.action_hints.insert(
            "checkout-pr".to_string(),
            ActionHintState::Loaded(ActionHints {
                labels: Vec::new(),
                checks: None,
                note: None,
                head: Some(PullRequestBranch {
                    repository: "chenyukang/ghr".to_string(),
                    branch: "codex/pr-checkout-local".to_string(),
                }),
            }),
        );
        app.start_pr_checkout_dialog(&config);
        let dialog = app.pr_action_dialog.as_ref().expect("checkout dialog");
        let area = Rect::new(0, 0, 120, 40);
        let inner = block_inner(pr_action_dialog_area(dialog, area));
        let branch_column = inner
            .x
            .saturating_add(display_width("remote branch: ") as u16)
            .saturating_add(1);
        let branch_row = inner.y.saturating_add(PR_ACTION_REMOTE_BRANCH_LINE);

        assert_eq!(
            pr_action_dialog_link_at(dialog, area, branch_column, branch_row).as_deref(),
            Some("https://github.com/chenyukang/ghr/tree/codex/pr-checkout-local")
        );
        assert_eq!(
            pr_action_dialog_link_at(dialog, area, branch_column.saturating_sub(2), branch_row),
            None
        );
    }

    #[test]
    fn checkout_confirmation_submits_selected_action() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![checkout_test_section()]);
        let config = checkout_test_config();
        app.start_pr_checkout_dialog(&config);
        let mut submitted = None;

        app.handle_pr_action_dialog_key_with_submit(
            key(KeyCode::Enter),
            |item, action, checkout| {
                submitted = Some((item.id, action, checkout));
            },
        );

        assert!(app.pr_action_running);
        assert_eq!(app.status, "checking out pull request locally");
        assert!(matches!(
            submitted,
            Some((id, PrAction::Checkout, Some(_))) if id == "checkout-pr"
        ));
    }

    #[test]
    fn pr_checkout_command_construction_uses_repo_and_number() {
        let args = pr_checkout_command_args("rust-lang/rust", 123);

        assert_eq!(
            args,
            vec![
                "pr".to_string(),
                "checkout".to_string(),
                "123".to_string(),
                "--repo".to_string(),
                "rust-lang/rust".to_string(),
            ]
        );
        assert_eq!(
            pr_checkout_command_display(&args),
            "gh pr checkout 123 --repo rust-lang/rust"
        );
    }

    #[test]
    fn pr_action_confirmation_submits_selected_action() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.start_pr_action_dialog(PrAction::Approve);
        let mut submitted = None;

        app.handle_pr_action_dialog_key_with_submit(key(KeyCode::Enter), |item, action, _| {
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

        app.handle_pr_action_dialog_key_with_submit(key(KeyCode::Esc), |_item, _action, _| {
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
    fn checkout_rejects_non_pull_request() {
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
            key(KeyCode::Char('X')),
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
        let dialog = app.message_dialog.as_ref().expect("success dialog");
        assert_eq!(dialog.title, "Pull Request Merged");
        assert_eq!(dialog.kind, MessageDialogKind::Success);
        assert_eq!(message_dialog_accent(dialog), Color::LightGreen);
        assert!(dialog.auto_close_at.is_some());
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
        let dialog = app.message_dialog.as_ref().expect("success dialog");
        assert_eq!(dialog.title, "Pull Request Approved");
        assert_eq!(dialog.kind, MessageDialogKind::Success);
        assert_eq!(message_dialog_accent(dialog), Color::LightGreen);
        assert!(dialog.auto_close_at.is_some());
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
        assert_eq!(dialog.kind, MessageDialogKind::Error);
        assert_eq!(message_dialog_accent(dialog), Color::LightRed);
        assert!(dialog.auto_close_at.is_none());
    }

    #[test]
    fn pr_checkout_finished_shows_success_output() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.start_pr_action_dialog(PrAction::Checkout);
        app.pr_action_running = true;

        app.handle_msg(AppMsg::PrCheckoutFinished {
            result: Ok(PrCheckoutResult {
                command: "gh pr checkout 1 --repo rust-lang/rust".to_string(),
                directory: PathBuf::from("/tmp/rust"),
                output: "Switched to branch 'diagnostics'".to_string(),
            }),
        });

        assert!(app.pr_action_dialog.is_none());
        assert!(!app.pr_action_running);
        assert_eq!(app.status, "pull request checked out locally");
        let dialog = app.message_dialog.as_ref().expect("success dialog");
        assert_eq!(dialog.title, "Pull Request Checked Out");
        assert!(
            dialog
                .body
                .contains("gh pr checkout 1 --repo rust-lang/rust")
        );
        assert!(dialog.body.contains("Checkout runs from /tmp/rust."));
        assert!(dialog.body.contains("Switched to branch"));
        assert_eq!(dialog.kind, MessageDialogKind::Success);
        assert_eq!(message_dialog_accent(dialog), Color::LightGreen);
        assert!(dialog.auto_close_at.is_none());
    }

    #[test]
    fn pr_checkout_failure_opens_message_dialog() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.start_pr_action_dialog(PrAction::Checkout);
        app.pr_action_running = true;

        app.handle_msg(AppMsg::PrCheckoutFinished {
            result: Err(
                "gh pr checkout 1 --repo rust-lang/rust failed.\n\nCheckout runs from /tmp.\n\nfatal: not a git repository"
                    .to_string(),
            ),
        });

        assert!(app.pr_action_dialog.is_none());
        assert!(!app.pr_action_running);
        assert_eq!(app.status, "pull request checkout failed");
        let dialog = app.message_dialog.as_ref().expect("message dialog");
        assert_eq!(dialog.title, "Checkout Failed");
        assert!(dialog.body.contains("Checkout runs from /tmp."));
        assert!(dialog.body.contains("fatal: not a git repository"));
        assert_eq!(dialog.kind, MessageDialogKind::Error);
        assert_eq!(message_dialog_accent(dialog), Color::LightRed);
        assert!(dialog.auto_close_at.is_none());
    }

    #[test]
    fn message_dialog_enter_dismisses() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.message_dialog = Some(message_dialog("Merge Failed", "review approval required"));

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
    fn success_message_dialog_auto_dismisses_after_deadline() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let deadline = Instant::now() + Duration::from_millis(5);
        app.message_dialog = Some(MessageDialog {
            title: "Comment Posted".to_string(),
            body: "ok".to_string(),
            kind: MessageDialogKind::Success,
            auto_close_at: Some(deadline),
        });

        app.dismiss_expired_message_dialog(deadline - Duration::from_millis(1));
        assert!(app.message_dialog.is_some());

        app.dismiss_expired_message_dialog(deadline);
        assert!(app.message_dialog.is_none());
        assert_eq!(app.status, "loading snapshot; background refresh started");
    }

    #[test]
    fn blocking_message_dialog_does_not_auto_dismiss() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.message_dialog = Some(message_dialog("Comment Failed", "HTTP 403"));

        app.dismiss_expired_message_dialog(Instant::now() + Duration::from_secs(60));

        let dialog = app.message_dialog.as_ref().expect("blocking dialog");
        assert_eq!(dialog.title, "Comment Failed");
        assert!(dialog.auto_close_at.is_none());
    }

    #[test]
    fn pending_message_dialog_is_not_error_colored() {
        let dialog = comment_pending_dialog(&PendingCommentMode::Post);

        assert_eq!(dialog.kind, MessageDialogKind::Info);
        assert_ne!(message_dialog_accent(&dialog), Color::LightRed);
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
                    comment_id: 42,
                    is_review: false,
                }
            ))
        );
    }

    #[test]
    fn inline_review_comment_edit_uses_review_edit_mode() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let mut inline = own_comment(88, "chenyukang", "old inline", None);
        inline.review = Some(crate::model::ReviewCommentPreview {
            path: "src/github.rs".to_string(),
            line: Some(876),
            original_line: None,
            start_line: None,
            original_start_line: None,
            side: Some("RIGHT".to_string()),
            start_side: None,
            diff_hunk: None,
        });
        app.details
            .insert("1".to_string(), DetailState::Loaded(vec![inline]));

        app.start_edit_selected_comment_dialog();

        assert_eq!(
            app.comment_dialog.as_ref().map(|dialog| &dialog.mode),
            Some(&CommentDialogMode::Edit {
                comment_index: 0,
                comment_id: 88,
                is_review: true,
            })
        );
    }

    #[test]
    fn ctrl_enter_in_inline_review_reply_submits_review_reply() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let mut inline = comment(
            "alice",
            "inline question",
            Some("https://github.com/owner/repo/pull/1#discussion_r88"),
        );
        inline.id = Some(88);
        inline.review = Some(crate::model::ReviewCommentPreview {
            path: "src/github.rs".to_string(),
            line: Some(876),
            original_line: None,
            start_line: None,
            original_start_line: None,
            side: Some("RIGHT".to_string()),
            start_side: None,
            diff_hunk: None,
        });
        app.details
            .insert("1".to_string(), DetailState::Loaded(vec![inline]));
        app.start_reply_to_selected_comment();
        app.comment_dialog.as_mut().unwrap().body = "reply inline".to_string();
        let mut submitted = None;

        app.handle_comment_dialog_key_with_submit(
            KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::CONTROL),
            None,
            |pending| submitted = Some((pending.item.id, pending.body, pending.mode)),
        );

        assert!(app.comment_dialog.is_none());
        assert!(app.posting_comment);
        assert_eq!(app.status, "posting review reply");
        assert_eq!(
            app.message_dialog
                .as_ref()
                .map(|dialog| (dialog.title.as_str(), dialog.body.as_str())),
            Some((
                "Posting Review Reply",
                "Waiting for GitHub to accept the review reply..."
            ))
        );
        assert_eq!(
            submitted,
            Some((
                "1".to_string(),
                "reply inline".to_string(),
                PendingCommentMode::ReviewReply { comment_id: 88 }
            ))
        );
    }

    #[test]
    fn c_in_diff_details_opens_review_comment_dialog_and_a_opens_normal_comment() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        app.show_diff();
        app.diffs.insert(
            "1".to_string(),
            DiffState::Loaded(
                parse_pull_request_diff(
                    r#"diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1 @@
-old
+new
"#,
                )
                .expect("parse diff"),
            ),
        );

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('a')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(
            app.comment_dialog.as_ref().map(|dialog| &dialog.mode),
            Some(&CommentDialogMode::New)
        );
        assert!(app.pr_action_dialog.is_none());
        assert_eq!(app.details_mode, DetailsMode::Diff);

        app.comment_dialog = None;
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('c')),
            &config,
            &store,
            &tx
        ));
        assert!(matches!(
            app.comment_dialog.as_ref().map(|dialog| &dialog.mode),
            Some(CommentDialogMode::Review { target })
                if target.path == "src/lib.rs"
                    && target.line == 1
                    && target.side == DiffReviewSide::Left
        ));
        assert_eq!(app.status, "reviewing src/lib.rs:1");

        app.comment_dialog = None;
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('A')),
            &config,
            &store,
            &tx
        ));
        assert!(app.comment_dialog.is_none());
        assert!(matches!(
            app.pr_action_dialog.as_ref().map(|dialog| dialog.action),
            Some(PrAction::Approve)
        ));

        app.pr_action_dialog = None;
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('M')),
            &config,
            &store,
            &tx
        ));
        assert!(matches!(
            app.pr_action_dialog.as_ref().map(|dialog| dialog.action),
            Some(PrAction::Merge)
        ));
    }

    #[test]
    fn ctrl_enter_in_review_dialog_submits_review_comment() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.show_diff();
        app.diffs.insert(
            "1".to_string(),
            DiffState::Loaded(
                parse_pull_request_diff(
                    r#"diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1 @@
-old
+new
"#,
                )
                .expect("parse diff"),
            ),
        );
        app.move_diff_line(1, None);
        app.start_review_comment_dialog();
        app.comment_dialog.as_mut().unwrap().body = "please tighten this".to_string();
        let mut submitted = None;

        app.handle_comment_dialog_key_with_submit(
            KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::CONTROL),
            None,
            |pending| submitted = Some((pending.item.id, pending.body, pending.mode)),
        );

        assert!(app.comment_dialog.is_none());
        assert!(app.posting_comment);
        assert_eq!(app.status, "posting review comment");
        assert_eq!(
            app.message_dialog
                .as_ref()
                .map(|dialog| (dialog.title.as_str(), dialog.body.as_str())),
            Some((
                "Posting Review Comment",
                "Waiting for GitHub to accept the review comment..."
            ))
        );
        assert_eq!(
            submitted,
            Some((
                "1".to_string(),
                "please tighten this".to_string(),
                PendingCommentMode::Review {
                    target: DiffReviewTarget {
                        path: "src/lib.rs".to_string(),
                        line: 1,
                        side: DiffReviewSide::Right,
                        start_line: None,
                        start_side: None,
                        preview: "new".to_string(),
                    }
                }
            ))
        );
    }

    #[test]
    fn comment_post_success_opens_result_dialog() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.posting_comment = true;
        app.message_dialog = Some(comment_pending_dialog(&PendingCommentMode::Post));

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
        assert!(
            app.message_dialog
                .as_ref()
                .and_then(|dialog| dialog.auto_close_at)
                .is_some()
        );
        assert_eq!(app.selected_comment_index, 0);
    }

    #[test]
    fn comment_post_failure_opens_result_dialog() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.posting_comment = true;
        app.message_dialog = Some(comment_pending_dialog(&PendingCommentMode::Post));

        app.handle_msg(AppMsg::CommentPosted {
            item_id: "1".to_string(),
            result: Err("gh api repos/owner/repo/issues/1/comments failed: HTTP 403".to_string()),
        });

        assert!(!app.posting_comment);
        assert_eq!(app.status, "comment post failed");
        let dialog = app.message_dialog.as_ref().expect("failure dialog");
        assert_eq!(dialog.title, "Comment Failed");
        assert_eq!(dialog.body, "HTTP 403");
        assert!(dialog.auto_close_at.is_none());
    }

    #[test]
    fn comment_update_success_opens_result_dialog() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.posting_comment = true;
        app.message_dialog = Some(comment_pending_dialog(&PendingCommentMode::Edit {
            comment_index: 0,
            comment_id: 42,
            is_review: false,
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
        assert!(
            app.message_dialog
                .as_ref()
                .and_then(|dialog| dialog.auto_close_at)
                .is_some()
        );
    }

    #[test]
    fn comment_update_failure_opens_result_dialog() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.posting_comment = true;
        app.message_dialog = Some(comment_pending_dialog(&PendingCommentMode::Edit {
            comment_index: 0,
            comment_id: 42,
            is_review: false,
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
        assert!(dialog.auto_close_at.is_none());
    }

    #[test]
    fn review_comment_result_dialog_reports_success_and_failure() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.posting_comment = true;

        app.handle_msg(AppMsg::ReviewCommentPosted {
            item_id: "1".to_string(),
            result: Ok(()),
        });

        assert!(!app.posting_comment);
        assert!(app.details_stale.contains("1"));
        assert_eq!(app.status, "review comment posted");
        assert_eq!(
            app.message_dialog
                .as_ref()
                .map(|dialog| (dialog.title.as_str(), dialog.body.as_str())),
            Some((
                "Review Comment Posted",
                "GitHub accepted the review comment."
            ))
        );
        assert!(
            app.message_dialog
                .as_ref()
                .and_then(|dialog| dialog.auto_close_at)
                .is_some()
        );

        app.posting_comment = true;
        app.handle_msg(AppMsg::ReviewCommentPosted {
            item_id: "1".to_string(),
            result: Err(
                "gh api repos/owner/repo/pulls/1/comments failed: validation failed".to_string(),
            ),
        });

        assert!(!app.posting_comment);
        assert_eq!(app.status, "review comment failed");
        let dialog = app.message_dialog.as_ref().expect("failure dialog");
        assert_eq!(dialog.title, "Review Comment Failed");
        assert_eq!(dialog.body, "validation failed");
        assert!(dialog.auto_close_at.is_none());
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
            Some(6 + MOUSE_COMMENT_SCROLL_LINES)
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
                parent_id: None,
                is_mine: false,
                review: None,
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
    fn current_repo_tab_can_lead_configured_repo_tabs() {
        let mut config = Config::default();
        config.repos.push(crate::config::RepoConfig {
            name: "runnel".to_string(),
            repo: "chenyukang/runnel".to_string(),
            local_dir: None,
            show_prs: true,
            show_issues: true,
        });
        config.repos.push(crate::config::RepoConfig {
            name: "Fiber".to_string(),
            repo: "nervosnetwork/fiber".to_string(),
            local_dir: None,
            show_prs: true,
            show_issues: true,
        });
        let app = AppState::new(SectionKind::PullRequests, configured_sections(&config));

        let repo_tabs = app
            .view_tabs()
            .into_iter()
            .filter(|view| view.key.starts_with("repo:"))
            .map(|view| view.label)
            .collect::<Vec<_>>();

        assert_eq!(repo_tabs, vec!["runnel", "Fiber"]);
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
        let visible_rows = usize::from(table_visible_rows(table));

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
        assert_eq!(app.current_selected_position(), 0);
        assert_eq!(
            app.current_list_scroll_offset(30, visible_rows),
            usize::from(MOUSE_LIST_SCROLL_LINES)
        );
        assert_eq!(app.current_item().map(|item| item.id.as_str()), Some("1"));
    }

    #[test]
    fn mouse_move_over_list_does_not_select_item() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![many_items_section(30)]);
        app.focus_details();
        let area = Rect::new(0, 0, 120, 40);
        let table = body_areas_with_ratio(body_area(area), app.list_width_percent)[0];
        let inner = block_inner(table);

        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Moved,
                column: inner.x + 2,
                row: inner.y + TABLE_HEADER_HEIGHT + 3,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
            area,
        );

        assert_eq!(app.focus, FocusTarget::Details);
        assert_eq!(app.current_selected_position(), 0);
        assert_eq!(app.current_item().map(|item| item.id.as_str()), Some("1"));
    }

    #[test]
    fn mouse_click_details_action_uses_current_rendered_rows() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.focus_list();
        app.selected_comment_index = 0;
        app.details.insert(
            "1".to_string(),
            DetailState::Loaded(vec![
                comment("alice", "First comment", None),
                comment(
                    "bob",
                    "Second comment",
                    Some("https://github.com/rust-lang/rust/pull/1#issuecomment-2"),
                ),
            ]),
        );
        let area = Rect::new(0, 0, 140, 36);
        let details = details_area_for(&app, area);
        let inner = block_inner(details);
        let document = build_details_document(&app, inner.width);
        let bob_line_index = document
            .lines
            .iter()
            .position(|line| line.to_string().contains("bob"))
            .expect("bob comment line");
        let reply_column = document.lines[bob_line_index]
            .to_string()
            .find("reply")
            .expect("reply action") as u16;
        let row = inner.y + bob_line_index as u16 - app.details_scroll;

        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: inner.x + reply_column,
                row,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
            area,
        );

        assert_eq!(app.selected_comment_index, 1);
        assert!(matches!(
            app.comment_dialog.as_ref().map(|dialog| &dialog.mode),
            Some(CommentDialogMode::Reply {
                comment_index: 1,
                author,
                ..
            }) if author == "bob"
        ));
    }

    #[test]
    fn mouse_wheel_scrolls_diff_file_list_one_file_at_a_time() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.show_diff();
        app.diffs.insert(
            "1".to_string(),
            DiffState::Loaded(
                parse_pull_request_diff(
                    r#"diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+new
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1 +1 @@
-old
+new
diff --git a/c.rs b/c.rs
--- a/c.rs
+++ b/c.rs
@@ -1 +1 @@
-old
+new
"#,
                )
                .expect("parse diff"),
            ),
        );
        let area = Rect::new(0, 0, 120, 40);
        let files_area = body_areas_with_ratio(body_area(area), app.list_width_percent)[0];
        let inner = block_inner(files_area);

        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: inner.x + 2,
                row: inner.y + 1,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
            area,
        );

        assert_eq!(app.focus, FocusTarget::List);
        assert_eq!(
            app.selected_diff_file.get("1"),
            Some(&(MOUSE_DIFF_FILE_SCROLL_LINES as usize))
        );
        assert_eq!(app.details_scroll, 0);
    }

    #[test]
    fn event_batch_coalesces_diff_file_list_wheel_to_one_file() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.show_diff();
        app.diffs.insert(
            "1".to_string(),
            DiffState::Loaded(
                parse_pull_request_diff(
                    r#"diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+new
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1 +1 @@
-old
+new
diff --git a/c.rs b/c.rs
--- a/c.rs
+++ b/c.rs
@@ -1 +1 @@
-old
+new
diff --git a/d.rs b/d.rs
--- a/d.rs
+++ b/d.rs
@@ -1 +1 @@
-old
+new
"#,
                )
                .expect("parse diff"),
            ),
        );
        let config = Config::default();
        let paths = test_paths();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        let (tx, _rx) = mpsc::unbounded_channel();
        let area = Rect::new(0, 0, 120, 40);
        let files_area = body_areas_with_ratio(body_area(area), app.list_width_percent)[0];
        let inner = block_inner(files_area);
        let events = (0..100)
            .map(|_| {
                Event::Mouse(MouseEvent {
                    kind: MouseEventKind::ScrollDown,
                    column: inner.x + 2,
                    row: inner.y + 1,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                })
            })
            .collect();

        assert!(!handle_event_batch(
            &mut app, events, area, &config, &paths, &store, &tx
        ));

        assert_eq!(app.focus, FocusTarget::List);
        assert_eq!(app.selected_diff_file.get("1"), Some(&1));
        assert_eq!(app.details_scroll, 0);
    }

    #[test]
    fn table_visible_range_tracks_current_page() {
        assert_eq!(table_visible_range(0, 10, 25), Some((1, 10)));
        assert_eq!(table_visible_range(10, 10, 25), Some((11, 20)));
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
            "[Focus] Details:"
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
        assert_eq!(app.details_scroll, MOUSE_DETAILS_SCROLL_LINES);

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
    fn mouse_wheel_can_reach_last_comment() {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.details.insert(
            "1".to_string(),
            DetailState::Loaded(
                (1..=12)
                    .map(|index| {
                        comment(&format!("user{index}"), &format!("comment {index}"), None)
                    })
                    .collect(),
            ),
        );
        let area = Rect::new(0, 0, 100, 16);
        let details = details_area_for(&app, area);
        let inner = block_inner(details);

        for _ in 0..80 {
            handle_mouse(
                &mut app,
                MouseEvent {
                    kind: MouseEventKind::ScrollDown,
                    column: inner.x + 2,
                    row: inner.y + inner.height.saturating_sub(1),
                    modifiers: crossterm::event::KeyModifiers::NONE,
                },
                area,
            );
        }

        let max_scroll = max_details_scroll(&app, details_area_for(&app, area));
        assert_eq!(app.details_scroll, max_scroll);
        let document = build_details_document(&app, inner.width);
        let last = document.comment_region(11).expect("last comment");
        let viewport_start = usize::from(app.details_scroll);
        let viewport_end = viewport_start + usize::from(inner.height);
        assert!(
            last.focus_line() >= viewport_start && last.focus_line() < viewport_end,
            "last comment header should be visible at bottom: scroll={} height={} last={:?}",
            app.details_scroll,
            inner.height,
            last
        );
    }

    #[test]
    fn event_batch_coalesces_excessive_details_wheel_events() {
        let mut item = work_item("1", "rust-lang/rust", 1, "Compiler diagnostics", None);
        item.body = Some(
            (1..=120)
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
        app.focus_details();
        app.search_active = true;
        let config = Config::default();
        let paths = test_paths();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
        let (tx, _rx) = mpsc::unbounded_channel();
        let area = Rect::new(0, 0, 100, 20);
        let details = body_areas(body_area(area))[1];
        let inner = block_inner(details);
        let events = (0..100)
            .map(|_| {
                Event::Mouse(MouseEvent {
                    kind: MouseEventKind::ScrollDown,
                    column: inner.x + 2,
                    row: inner.y + 2,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                })
            })
            .collect();

        assert!(!handle_event_batch(
            &mut app, events, area, &config, &paths, &store, &tx
        ));

        assert_eq!(app.focus, FocusTarget::Details);
        assert!(!app.search_active);
        assert_eq!(
            app.details_scroll,
            (MAX_COALESCED_MOUSE_SCROLL_STEPS as u16) * MOUSE_DETAILS_SCROLL_LINES
        );
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
    fn tab_switches_the_current_focused_tab_group() {
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
            key(KeyCode::Tab),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.active_view, builtin_view_key(SectionKind::Issues));
        assert_eq!(app.focus, FocusTarget::Ghr);

        app.switch_view(builtin_view_key(SectionKind::PullRequests));
        app.focus_sections();
        assert_eq!(app.current_section_position(), 0);
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Tab),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.active_view, builtin_view_key(SectionKind::PullRequests));
        assert_eq!(app.current_section_position(), 1);
        assert_eq!(app.focus, FocusTarget::Sections);

        assert!(!handle_key(
            &mut app,
            key(KeyCode::BackTab),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.active_view, builtin_view_key(SectionKind::PullRequests));
        assert_eq!(app.current_section_position(), 0);
        assert_eq!(app.focus, FocusTarget::Sections);
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
        assert_eq!(rendered.matches("[Focus").count(), 1);
        assert!(rendered.contains("[Focus] Sections"));
        assert!(!rendered.contains("[Focus] List"));

        app.focus_list();
        terminal
            .draw(|frame| draw(frame, &app, &paths))
            .expect("draw list focus");
        let rendered = buffer_lines(terminal.backend().buffer()).join("\n");
        assert_eq!(rendered.matches("[Focus").count(), 1);
        assert!(rendered.contains("[Focus] Test"));
        assert!(!rendered.contains("[Focus] Sections"));
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
    fn notification_read_local_removes_unread_and_marks_all_items_read() {
        let unread_item = notification_item("thread-1", true);
        let other_item = notification_item("thread-2", true);
        let sections = vec![
            SectionSnapshot {
                key: "notifications:unread".to_string(),
                kind: SectionKind::Notifications,
                title: "Unread".to_string(),
                filters: "is:unread".to_string(),
                items: vec![unread_item.clone(), other_item.clone()],
                total_count: None,
                page: 1,
                page_size: 0,
                refreshed_at: None,
                error: None,
            },
            SectionSnapshot {
                key: "notifications:all".to_string(),
                kind: SectionKind::Notifications,
                title: "All".to_string(),
                filters: "is:all".to_string(),
                items: vec![unread_item],
                total_count: None,
                page: 1,
                page_size: 0,
                refreshed_at: None,
                error: None,
            },
        ];
        let mut app = AppState::new(SectionKind::Notifications, sections);

        assert!(app.apply_notification_read_local("thread-1"));

        assert_eq!(app.sections[0].items.len(), 1);
        assert_eq!(app.sections[0].items[0].id, "thread-2");
        assert_eq!(app.sections[1].items[0].unread, Some(false));
        assert_eq!(
            app.current_item().map(|item| item.id.as_str()),
            Some("thread-2")
        );
    }

    #[test]
    fn notification_read_finished_updates_local_state_and_clears_pending() {
        let sections = vec![SectionSnapshot {
            key: "notifications:unread".to_string(),
            kind: SectionKind::Notifications,
            title: "Unread".to_string(),
            filters: "is:unread".to_string(),
            items: vec![notification_item("thread-1", true)],
            total_count: None,
            page: 1,
            page_size: 0,
            refreshed_at: None,
            error: None,
        }];
        let mut app = AppState::new(SectionKind::Notifications, sections);
        app.notification_read_pending.insert("thread-1".to_string());

        app.handle_msg(AppMsg::NotificationReadFinished {
            thread_id: "thread-1".to_string(),
            result: Ok(None),
        });

        assert!(!app.notification_read_pending.contains("thread-1"));
        assert!(app.sections[0].items.is_empty());
        assert_eq!(app.status, "notification marked read");
    }

    #[test]
    fn pull_request_notification_enter_and_v_open_diff_context() {
        let sections = vec![SectionSnapshot {
            key: "notifications:reviews".to_string(),
            kind: SectionKind::Notifications,
            title: "Review requested".to_string(),
            filters: "reason:review-requested".to_string(),
            items: vec![notification_item("thread-1", false)],
            total_count: None,
            page: 1,
            page_size: 0,
            refreshed_at: None,
            error: None,
        }];
        let mut app = AppState::new(SectionKind::Notifications, sections);
        let (tx, _rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

        assert_eq!(
            app.current_item().map(|item| item.kind),
            Some(ItemKind::PullRequest)
        );
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Enter),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.focus, FocusTarget::Details);

        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('v')),
            &config,
            &store,
            &tx
        ));

        assert_eq!(app.details_mode, DetailsMode::Diff);
        assert_eq!(app.focus, FocusTarget::List);
        assert_eq!(app.status, "loading diff");
    }

    #[test]
    fn pull_request_notification_click_focuses_pr_details() {
        let sections = vec![SectionSnapshot {
            key: "notifications:reviews".to_string(),
            kind: SectionKind::Notifications,
            title: "Review requested".to_string(),
            filters: "reason:review-requested".to_string(),
            items: vec![notification_item("thread-1", false)],
            total_count: None,
            page: 1,
            page_size: 0,
            refreshed_at: None,
            error: None,
        }];
        let mut app = AppState::new(SectionKind::Notifications, sections);
        let area = Rect::new(0, 0, 100, 40);
        let table = body_areas(body_area(area))[0];
        let inner = block_inner(table);

        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: inner.x + 4,
                row: inner.y + TABLE_HEADER_HEIGHT,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
            area,
        );

        assert_eq!(app.focus, FocusTarget::Details);
        assert_eq!(
            app.current_item().map(|item| item.kind),
            Some(ItemKind::PullRequest)
        );
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

    fn checkout_test_section() -> SectionSnapshot {
        SectionSnapshot {
            key: "pull_requests:checkout".to_string(),
            kind: SectionKind::PullRequests,
            title: "Checkout".to_string(),
            filters: String::new(),
            items: vec![work_item(
                "checkout-pr",
                "chenyukang/ghr",
                19,
                "Add local PR checkout",
                None,
            )],
            total_count: None,
            page: 1,
            page_size: 0,
            refreshed_at: None,
            error: None,
        }
    }

    fn checkout_test_config() -> Config {
        let local_dir = checkout_test_repo_dir();
        let mut config = Config::default();
        config.repos.push(crate::config::RepoConfig {
            name: "ghr".to_string(),
            repo: "chenyukang/ghr".to_string(),
            local_dir: Some(local_dir.display().to_string()),
            show_prs: true,
            show_issues: true,
        });
        config
    }

    fn checkout_test_repo_dir() -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("ghr-checkout-test-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create checkout test dir");

        let init = Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["init", "-q"])
            .output()
            .expect("run git init");
        assert!(
            init.status.success(),
            "git init failed: {}",
            command_output_text(&init.stdout, &init.stderr)
        );

        let remote = Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args([
                "remote",
                "add",
                "origin",
                "https://github.com/chenyukang/ghr.git",
            ])
            .output()
            .expect("run git remote add");
        assert!(
            remote.status.success(),
            "git remote add failed: {}",
            command_output_text(&remote.stdout, &remote.stderr)
        );

        dir
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

    fn first_visible_diff_line(app: &AppState, area: Rect) -> (usize, usize) {
        let details_area = details_area_for(app, area);
        let inner = block_inner(details_area);
        let document = build_details_document(app, inner.width);
        let start = usize::from(app.details_scroll);
        let end = start.saturating_add(usize::from(inner.height.max(1)));
        document
            .diff_lines
            .iter()
            .find(|line| line.line >= start && line.line < end)
            .map(|line| (line.review_index, line.line))
            .expect("visible reviewable diff line")
    }

    fn selected_diff_document_line(app: &AppState, area: Rect) -> Option<usize> {
        let details_area = details_area_for(app, area);
        let inner = block_inner(details_area);
        build_details_document(app, inner.width).selected_diff_line
    }

    fn comment(author: &str, body: &str, url: Option<&str>) -> CommentPreview {
        CommentPreview {
            id: None,
            author: author.to_string(),
            body: body.to_string(),
            created_at: None,
            updated_at: None,
            url: url.map(str::to_string),
            parent_id: None,
            is_mine: false,
            review: None,
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
            parent_id: None,
            is_mine: true,
            review: None,
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

    fn notification_item(id: &str, unread: bool) -> WorkItem {
        WorkItem {
            id: id.to_string(),
            kind: ItemKind::PullRequest,
            repo: "rust-lang/rust".to_string(),
            number: Some(1),
            title: format!("Notification {id}"),
            body: None,
            author: None,
            state: None,
            url: "https://github.com/rust-lang/rust/pull/1".to_string(),
            updated_at: None,
            labels: Vec::new(),
            comments: None,
            unread: Some(unread),
            reason: Some("mention".to_string()),
            extra: Some("PullRequest".to_string()),
        }
    }
}
