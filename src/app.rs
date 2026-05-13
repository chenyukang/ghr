use std::collections::{HashMap, HashSet};
#[cfg(all(not(test), unix, not(target_os = "macos")))]
use std::env;
use std::io;
use std::io::IsTerminal;
#[cfg(not(test))]
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
#[cfg(not(test))]
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use chrono::{DateTime, Local, Utc};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, HighlightSpacing, Paragraph, Row, Table, TableState,
    Tabs, Widget, Wrap,
};
use ratatui::{Frame, Terminal};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, warn};

use crate::config::{
    Config, DEFAULT_COMMAND_PALETTE_KEY, RepoConfig, SavedSearchFilterConfig,
    github_repo_from_remote_url,
};
use crate::dirs::Paths;
use crate::github::{
    AssigneeAction, CommentFetchResult, ItemDetailsMetadata, MergeMethod,
    PullRequestReviewCommentTarget, PullRequestReviewEvent, RefreshScope,
    add_issue_comment_reaction, add_issue_label, add_issue_reaction,
    add_pull_request_review_comment_reaction, approve_pull_request, change_issue_milestone,
    close_issue, close_pull_request, convert_pull_request_to_draft, create_issue, create_milestone,
    create_pending_pull_request_review, create_pull_request, disable_pull_request_auto_merge,
    discard_pending_pull_request_review, edit_issue_comment, edit_item_metadata,
    edit_pull_request_review_comment, enable_pull_request_auto_merge, fetch_comments,
    fetch_open_milestones, fetch_pull_request_action_hints, fetch_pull_request_diff,
    fetch_repository_assignees, fetch_repository_labels, mark_all_notifications_read,
    mark_notification_thread_done, mark_notification_thread_read,
    mark_pull_request_ready_for_review, merge_pull_request, mute_notification_thread,
    post_issue_comment, post_pull_request_review_comment, post_pull_request_review_reply,
    refresh_dashboard, refresh_dashboard_with_progress, refresh_idle_search_sections,
    refresh_section_page, remove_issue_label, remove_pull_request_reviewers, reopen_issue,
    reopen_pull_request, request_pull_request_reviewers, rerun_failed_pull_request_checks,
    search_github_users, search_global, submit_pending_pull_request_review,
    submit_pull_request_review, subscribe_notification_thread, unsubscribe_notification_thread,
    update_issue_assignees, update_item_subscription, update_pull_request_branch,
    with_background_github_priority,
};
use crate::model::{
    ActionHints, CheckSummary, CommentPreview, EditorDraft, FailedCheckRunSummary, ItemKind,
    Milestone, PullRequestBranch, PullRequestReviewActor, ReactionSummary, SectionKind,
    SectionSnapshot, WorkItem, builtin_view_key, configured_sections, global_search_view_key,
    mark_all_notifications_read_in_section, mark_notification_done_in_section,
    mark_notification_read_in_section, merge_cached_sections, merge_refreshed_sections,
    repo_view_key, section_view_key,
};
use crate::snapshot::{RepoCandidateCache, SnapshotStore};
use crate::state::{
    GlobalSearchSavedState, GlobalSearchState, MAX_GLOBAL_SAVED_SEARCHES_PER_REPO,
    MAX_RECENT_COMMANDS, MAX_RECENT_ITEMS, RecentCommandState, RecentItemState,
    RepoUnseenItemsState, UiState, ViewSnapshot as SavedViewSnapshot,
};
use crate::theme::{ThemeFamily, ThemeName, ThemePreference, active_theme, set_active_theme};

mod command_palette;
mod details;
mod dialogs;
mod diff;
mod drafts;
mod editor;
mod global_search;
mod input;
mod keymap;
mod layout;
mod mentions;
mod participants;
mod pr_checkout;
mod render;
mod runtime;
mod search;
mod status;
mod switchers;
mod tasks;
mod text;

use command_palette::{
    CommandPalette, PaletteAction, PaletteCommand, command_palette_area, command_palette_commands,
    command_palette_filtered_indices, command_palette_input_line, command_palette_normalized_text,
    command_palette_result_line, command_palette_visible_start,
};
use details::*;
use dialogs::*;
use diff::{
    DiffFile, DiffLine, DiffLineKind, PullRequestDiff, parse_inline_diff_hunk,
    parse_pull_request_diff,
};
use drafts::*;
use editor::*;
use global_search::*;
use input::*;
use keymap::{command_palette_key_binding, normalized_command_palette_key};
use layout::{
    block_inner, body_areas_with_ratio, centered_rect, centered_rect_width,
    centered_rect_with_size, details_area_for, page_areas, rect_contains,
    split_percent_from_column, splitter_contains,
};
use participants::*;
use pr_checkout::{
    PrCheckoutPlan, PrCheckoutResult, checkout_directory_notice, configured_local_dir_for_repo,
    current_git_branch_for_directory, ensure_directory_tracks_repo, resolve_pr_checkout_directory,
    resolve_pull_request_head_ref, run_pr_checkout,
};
use render::*;
use runtime::*;
use search::{QuickFilter, filtered_indices, fuzzy_score, quick_filter_query};
use status::{
    comment_pending_dialog, compact_error_label, info_message_dialog, message_dialog,
    operation_error_body, persistent_success_message_dialog, pr_action_error_body,
    pr_action_error_status, pr_action_error_title, pr_action_success_body, pr_action_success_title,
    refresh_error_status, retryable_message_dialog, reviewer_action_error_status,
    reviewer_action_error_title, reviewer_action_success_body, reviewer_action_success_title,
    setup_dialog_from_error, success_message_dialog,
};
use switchers::*;
use tasks::*;
use text::{display_width, display_width_char, normalize_text, truncate_inline, truncate_text};

const NO_SELECTED_COMMENT_INDEX: usize = usize::MAX;

enum AppMsg {
    RefreshStarted {
        scope: RefreshScope,
    },
    RefreshFinished {
        sections: Vec<SectionSnapshot>,
        save_error: Option<String>,
    },
    RefreshSectionLoaded {
        section: SectionSnapshot,
        save_error: Option<String>,
    },
    IdleSweepStarted,
    IdleSweepFinished {
        sections: Vec<SectionSnapshot>,
        next_cursor: usize,
    },
    CommentsLoaded {
        item_id: String,
        comments: std::result::Result<CommentFetchResult, String>,
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
        result: std::result::Result<CommentPreview, String>,
    },
    CommentUpdated {
        item_id: String,
        comment_index: usize,
        result: std::result::Result<CommentFetchResult, String>,
    },
    ReviewCommentPosted {
        item_id: String,
        result: std::result::Result<CommentPreview, String>,
    },
    ReactionPosted {
        item_id: String,
        result: std::result::Result<CommentFetchResult, String>,
    },
    LabelUpdated {
        item_id: String,
        action: LabelAction,
        result: std::result::Result<(), String>,
    },
    LabelSuggestionsLoaded {
        repo: String,
        result: std::result::Result<Vec<String>, String>,
    },
    AssigneeSuggestionsLoaded {
        repo: String,
        result: std::result::Result<Vec<String>, String>,
    },
    ReviewerSuggestionsLoaded {
        repo: String,
        result: std::result::Result<Vec<String>, String>,
    },
    MentionUserSearchLoaded {
        query: String,
        result: std::result::Result<Vec<String>, String>,
    },
    IssueCreated {
        result: std::result::Result<WorkItem, String>,
    },
    PullRequestCreated {
        result: std::result::Result<WorkItem, String>,
    },
    ReviewDraftCreated {
        item_id: String,
        result: std::result::Result<PendingReviewState, String>,
    },
    ReviewSubmitted {
        item_id: String,
        event: PullRequestReviewEvent,
        result: std::result::Result<(), String>,
    },
    PendingReviewSubmitted {
        item_id: String,
        review_id: u64,
        event: PullRequestReviewEvent,
        result: std::result::Result<(), String>,
    },
    PendingReviewDiscarded {
        item_id: String,
        review_id: u64,
        result: std::result::Result<(), String>,
    },
    PrActionFinished {
        item_id: String,
        item_kind: ItemKind,
        action: PrAction,
        merge_method: Option<MergeMethod>,
        result: std::result::Result<(), String>,
    },
    PrCheckoutFinished {
        result: std::result::Result<PrCheckoutResult, String>,
    },
    MilestonesLoaded {
        item_id: String,
        result: std::result::Result<Vec<Milestone>, String>,
    },
    MilestoneChanged {
        item_id: String,
        milestone: Option<Milestone>,
        result: std::result::Result<(), String>,
    },
    ItemEdited {
        item_id: String,
        result: std::result::Result<ItemEditUpdate, String>,
    },
    AssigneesUpdated {
        item_id: String,
        action: AssigneeAction,
        result: std::result::Result<WorkItem, String>,
    },
    ReviewerActionFinished {
        item_id: String,
        action: ReviewerAction,
        reviewers: Vec<String>,
        result: std::result::Result<(), String>,
    },
    NotificationReadFinished {
        thread_id: String,
        result: std::result::Result<Option<String>, String>,
    },
    NotificationDoneFinished {
        thread_id: String,
        result: std::result::Result<Option<String>, String>,
    },
    InboxMarkAllReadFinished {
        result: std::result::Result<Option<String>, String>,
    },
    InboxThreadActionFinished {
        action: InboxThreadAction,
        result: std::result::Result<(), String>,
    },
    ItemSubscriptionUpdated {
        item_id: String,
        item_kind: ItemKind,
        action: ItemSubscriptionAction,
        result: std::result::Result<(), String>,
    },
    SectionPageLoaded {
        section_key: String,
        section: SectionSnapshot,
        save_error: Option<String>,
    },
    FilterSectionLoaded {
        section_key: String,
        section: SectionSnapshot,
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

#[derive(Debug, Clone, PartialEq, Eq)]
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

    fn from_state_str(value: &str) -> Self {
        match value {
            "diff" => Self::Diff,
            _ => Self::Conversation,
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
    body: EditorText,
    scroll: u16,
}

#[derive(Debug, Clone)]
struct PendingCommentSubmit {
    item: WorkItem,
    body: String,
    mode: PendingCommentMode,
    dialog: CommentDialog,
    draft_key: Option<String>,
}

#[derive(Debug, Clone)]
struct DraftClearTask {
    key: String,
    store: SnapshotStore,
}

#[derive(Debug, Clone)]
struct ReviewSubmitDialog {
    item: WorkItem,
    event: PullRequestReviewEvent,
    body: EditorText,
    scroll: u16,
    mode: ReviewSubmitMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewSubmitMode {
    New,
    Pending { review_id: u64 },
}

struct PendingReviewSubmit {
    item: WorkItem,
    event: PullRequestReviewEvent,
    body: String,
    mode: ReviewSubmitMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingReviewState {
    review_id: u64,
    body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReactionContent {
    PlusOne,
    MinusOne,
    Laugh,
    Hooray,
    Confused,
    Heart,
    Rocket,
    Eyes,
}

impl ReactionContent {
    const ALL: [Self; 8] = [
        Self::PlusOne,
        Self::MinusOne,
        Self::Laugh,
        Self::Hooray,
        Self::Confused,
        Self::Heart,
        Self::Rocket,
        Self::Eyes,
    ];

    fn api_value(self) -> &'static str {
        match self {
            Self::PlusOne => "+1",
            Self::MinusOne => "-1",
            Self::Laugh => "laugh",
            Self::Hooray => "hooray",
            Self::Confused => "confused",
            Self::Heart => "heart",
            Self::Rocket => "rocket",
            Self::Eyes => "eyes",
        }
    }

    fn emoji(self) -> &'static str {
        match self {
            Self::PlusOne => "👍",
            Self::MinusOne => "👎",
            Self::Laugh => "😄",
            Self::Hooray => "🎉",
            Self::Confused => "😕",
            Self::Heart => "❤️",
            Self::Rocket => "🚀",
            Self::Eyes => "👀",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::PlusOne => "+1",
            Self::MinusOne => "-1",
            Self::Laugh => "laugh",
            Self::Hooray => "hooray",
            Self::Confused => "confused",
            Self::Heart => "heart",
            Self::Rocket => "rocket",
            Self::Eyes => "eyes",
        }
    }

    fn from_digit(value: char) -> Option<Self> {
        let index = value.to_digit(10)?.checked_sub(1)? as usize;
        Self::ALL.get(index).copied()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReactionTarget {
    Item,
    IssueComment { index: usize, comment_id: u64 },
    ReviewComment { index: usize, comment_id: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReactionDialog {
    target: ReactionTarget,
    target_label: String,
    selected: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DetailsTextPosition {
    line: usize,
    column: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DetailsTextDrag {
    item_id: String,
    start: DetailsTextPosition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DetailsTextSelection {
    item_id: String,
    start: DetailsTextPosition,
    end: DetailsTextPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DialogTextTarget {
    Comment,
    ReviewSubmit,
    IssueBody,
    PrCreateBody,
    ItemEditBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DialogTextDrag {
    target: DialogTextTarget,
    start: DetailsTextPosition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DialogTextSelection {
    target: DialogTextTarget,
    start: DetailsTextPosition,
    end: DetailsTextPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MentionTarget {
    Comment,
    ReviewSubmit,
    IssueTitle,
    IssueBody,
    PrCreateTitle,
    PrCreateBody,
    ItemEditTitle,
    ItemEditBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MentionContext {
    target: MentionTarget,
    repo: String,
    query: String,
    trigger_start: usize,
    cursor: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MentionCandidateView {
    repo: String,
    query: String,
    candidates: Vec<String>,
    selected: usize,
    loading: bool,
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LabelDialogMode {
    Add { repo: String },
    Remove { label: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LabelDialog {
    mode: LabelDialogMode,
    input: String,
    existing_labels: Vec<String>,
    suggestions: Vec<String>,
    suggestions_loading: bool,
    suggestions_error: Option<String>,
    selected_suggestion: usize,
}

struct PendingLabelUpdate {
    item: WorkItem,
    action: LabelAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LabelAction {
    Add(String),
    Remove(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IssueDialogField {
    Repo,
    Title,
    Labels,
    Body,
}

impl IssueDialogField {
    fn next(self, delta: isize) -> Self {
        const FIELDS: [IssueDialogField; 4] = [
            IssueDialogField::Repo,
            IssueDialogField::Title,
            IssueDialogField::Labels,
            IssueDialogField::Body,
        ];
        let index = FIELDS.iter().position(|field| *field == self).unwrap_or(0);
        let next = move_wrapping(index, FIELDS.len(), delta);
        FIELDS[next]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IssueDialog {
    repo: EditorText,
    title: EditorText,
    labels: EditorText,
    body: EditorText,
    field: IssueDialogField,
    body_scroll: u16,
}

#[derive(Debug, Clone)]
struct PendingIssueCreate {
    repo: String,
    title: String,
    body: String,
    labels: Vec<String>,
    dialog: IssueDialog,
    draft_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrCreateField {
    Title,
    Body,
}

impl PrCreateField {
    fn next(self, delta: isize) -> Self {
        const FIELDS: [PrCreateField; 2] = [PrCreateField::Title, PrCreateField::Body];
        let index = FIELDS.iter().position(|field| *field == self).unwrap_or(0);
        let next = move_wrapping(index, FIELDS.len(), delta);
        FIELDS[next]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PrCreateDialog {
    repo: String,
    local_dir: PathBuf,
    branch: String,
    head_ref: String,
    title: EditorText,
    body: EditorText,
    field: PrCreateField,
    body_scroll: u16,
}

#[derive(Debug, Clone)]
struct PendingPrCreate {
    repo: String,
    local_dir: PathBuf,
    head_ref: String,
    title: String,
    body: String,
    dialog: PrCreateDialog,
    draft_key: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct NewIssueDraft {
    repo: String,
    title: String,
    labels: String,
    body: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct NewPrDraft {
    repo: String,
    title: String,
    body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemEditField {
    Title,
    Assignees,
    Labels,
    Body,
}

impl ItemEditField {
    fn next(self, delta: isize) -> Self {
        const FIELDS: [ItemEditField; 4] = [
            ItemEditField::Title,
            ItemEditField::Assignees,
            ItemEditField::Labels,
            ItemEditField::Body,
        ];
        let index = FIELDS.iter().position(|field| *field == self).unwrap_or(0);
        let next = move_wrapping(index, FIELDS.len(), delta);
        FIELDS[next]
    }

    fn label(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Assignees => "assign",
            Self::Labels => "labels",
            Self::Body => "body",
        }
    }
}

#[derive(Debug, Clone)]
struct ItemEditDialog {
    item: WorkItem,
    title: EditorText,
    body: EditorText,
    assignees: Vec<String>,
    labels: Vec<String>,
    field: ItemEditField,
    body_scroll: u16,
    assignee_input: String,
    assignee_suggestions: Vec<String>,
    assignee_suggestions_loading: bool,
    assignee_suggestions_error: Option<String>,
    selected_assignee_suggestion: usize,
    label_input: String,
    label_suggestions: Vec<String>,
    label_suggestions_loading: bool,
    label_suggestions_error: Option<String>,
    selected_label_suggestion: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ItemEditUpdate {
    title: String,
    body: Option<String>,
    labels: Vec<String>,
    assignees: Vec<String>,
    updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
struct PendingItemEdit {
    item: WorkItem,
    title: String,
    body: String,
    labels: Vec<String>,
    assignees: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrAction {
    Merge,
    Close,
    Reopen,
    #[allow(dead_code)]
    Approve,
    EnableAutoMerge,
    DisableAutoMerge,
    Checkout,
    RerunFailedChecks,
    UpdateBranch,
    ConvertToDraft,
    MarkReadyForReview,
}

type PrActionDialogSummary = Vec<(&'static str, String)>;
type PrActionDialogSummaryError = (&'static str, String, String);

#[derive(Debug, Clone)]
struct PrActionDialog {
    item: WorkItem,
    action: PrAction,
    checkout: Option<PrCheckoutPlan>,
    summary: PrActionDialogSummary,
    merge_method: MergeMethod,
}

#[derive(Debug, Clone)]
struct MilestoneDialog {
    item: WorkItem,
    state: MilestoneDialogState,
    input: String,
    selected: usize,
}

#[derive(Debug, Clone)]
enum MilestoneDialogState {
    Loading,
    Loaded(Vec<Milestone>),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MilestoneChoice {
    Clear,
    Set(Milestone),
    Create(String),
}

#[derive(Debug, Clone)]
struct AssigneeDialog {
    item: WorkItem,
    action: AssigneeAction,
    input: String,
    suggestions: Vec<String>,
    suggestions_loading: bool,
    suggestions_error: Option<String>,
    selected_suggestion: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ProjectSwitcher {
    query: String,
    selected: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TopMenuSwitcher {
    query: String,
    selected: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ThemeChoice {
    #[default]
    Auto,
    Name(ThemeName),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ThemeSwitcher {
    query: String,
    selected: usize,
    current: ThemeChoice,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ThemeCandidate {
    choice: ThemeChoice,
    label: String,
    detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RecentItemsDialog {
    query: String,
    selected: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GlobalSearchField {
    Title,
    Status,
    Label,
    Author,
    Assignee,
    Sort,
}

impl GlobalSearchField {
    const FIELDS: [GlobalSearchField; 6] = [
        GlobalSearchField::Title,
        GlobalSearchField::Status,
        GlobalSearchField::Label,
        GlobalSearchField::Author,
        GlobalSearchField::Assignee,
        GlobalSearchField::Sort,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Title => "Title / #",
            Self::Status => "Status",
            Self::Label => "Label",
            Self::Author => "Author",
            Self::Assignee => "Assignee",
            Self::Sort => "Sort",
        }
    }

    fn as_state_str(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Status => "status",
            Self::Label => "label",
            Self::Author => "author",
            Self::Assignee => "assignee",
            Self::Sort => "sort",
        }
    }

    fn from_state_str(value: &str) -> Self {
        match value {
            "status" => Self::Status,
            "label" => Self::Label,
            "author" => Self::Author,
            "assignee" => Self::Assignee,
            "sort" => Self::Sort,
            _ => Self::Title,
        }
    }

    fn next(self, delta: isize) -> Self {
        let fields = Self::FIELDS;
        let index = fields
            .iter()
            .position(|field| *field == self)
            .unwrap_or_default();
        let next = (index as isize + delta).rem_euclid(fields.len() as isize) as usize;
        fields[next]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GlobalSearchSuggestionFilter {
    field: GlobalSearchField,
    prefix: String,
}

#[derive(Debug, Clone)]
struct GlobalSearchDialog {
    repo: Option<String>,
    title: EditorText,
    status: EditorText,
    label: EditorText,
    author: EditorText,
    assignee: EditorText,
    sort: EditorText,
    field: GlobalSearchField,
    labels: Vec<String>,
    labels_loading: bool,
    labels_error: Option<String>,
    author_candidates: Vec<String>,
    assignee_candidates: Vec<String>,
    assignees_loading: bool,
    assignees_error: Option<String>,
    selected_suggestion: usize,
    suggestion_filter: Option<GlobalSearchSuggestionFilter>,
}

impl Default for GlobalSearchDialog {
    fn default() -> Self {
        Self {
            repo: None,
            title: EditorText::from_text(""),
            status: EditorText::from_text(""),
            label: EditorText::from_text(""),
            author: EditorText::from_text(""),
            assignee: EditorText::from_text(""),
            sort: EditorText::from_text("created_at"),
            field: GlobalSearchField::Title,
            labels: Vec::new(),
            labels_loading: false,
            labels_error: None,
            author_candidates: Vec::new(),
            assignee_candidates: Vec::new(),
            assignees_loading: false,
            assignees_error: None,
            selected_suggestion: 0,
            suggestion_filter: None,
        }
    }
}

impl GlobalSearchDialog {
    fn active_editor_mut(&mut self) -> &mut EditorText {
        match self.field {
            GlobalSearchField::Title => &mut self.title,
            GlobalSearchField::Status => &mut self.status,
            GlobalSearchField::Label => &mut self.label,
            GlobalSearchField::Author => &mut self.author,
            GlobalSearchField::Assignee => &mut self.assignee,
            GlobalSearchField::Sort => &mut self.sort,
        }
    }

    fn active_editor(&self) -> &EditorText {
        match self.field {
            GlobalSearchField::Title => &self.title,
            GlobalSearchField::Status => &self.status,
            GlobalSearchField::Label => &self.label,
            GlobalSearchField::Author => &self.author,
            GlobalSearchField::Assignee => &self.assignee,
            GlobalSearchField::Sort => &self.sort,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SaveSearchDialog {
    name: EditorText,
    repo: String,
    kind: SectionKind,
    search: GlobalSearchState,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SavedSearchDialog {
    query: String,
    selected: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SavedSearchCandidate {
    name: String,
    repo: String,
    kind: SectionKind,
    search: GlobalSearchState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecentItem {
    id: String,
    kind: ItemKind,
    repo: String,
    number: u64,
    title: String,
    url: String,
    visited_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecentCommand {
    id: String,
    selected_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DetailsVisitState {
    item: RecentItem,
    started_at: Instant,
    recorded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecentItemTarget {
    view: String,
    section_position: usize,
    selected_position: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ProjectRemoveDialog {
    query: String,
    selected: usize,
    candidates: Vec<ProjectRemoveCandidate>,
    confirm: Option<ProjectRemoveCandidate>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CacheClearDialog {
    selected: usize,
    confirm: Option<CacheClearTarget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CacheClearTarget {
    CurrentSection,
    CurrentView,
    ListSnapshots,
    CandidateSuggestions,
    LoadedDetails,
    All,
}

const CACHE_CLEAR_TARGETS: [CacheClearTarget; 6] = [
    CacheClearTarget::CurrentSection,
    CacheClearTarget::CurrentView,
    CacheClearTarget::ListSnapshots,
    CacheClearTarget::CandidateSuggestions,
    CacheClearTarget::LoadedDetails,
    CacheClearTarget::All,
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CacheClearSummary {
    snapshot_rows: usize,
    list_items: usize,
    candidate_rows: usize,
    candidate_entries: usize,
    loaded_entries: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectAddField {
    Title,
    RepoUrl,
    LocalDir,
}

impl ProjectAddField {
    fn next(self, delta: isize) -> Self {
        const FIELDS: [ProjectAddField; 3] = [
            ProjectAddField::Title,
            ProjectAddField::RepoUrl,
            ProjectAddField::LocalDir,
        ];
        let index = FIELDS.iter().position(|field| *field == self).unwrap_or(0);
        let next = move_wrapping(index, FIELDS.len(), delta);
        FIELDS[next]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProjectAddDialog {
    title: String,
    repo_url: String,
    local_dir: String,
    field: ProjectAddField,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProjectRemoveCandidate {
    index: usize,
    name: String,
    repo: String,
    local_dir: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InboxThreadAction {
    Mute,
    Subscribe,
    Unsubscribe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemSubscriptionAction {
    Subscribe,
    Unsubscribe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewerAction {
    Request,
    Remove,
}

#[derive(Debug, Clone)]
struct ReviewerDialog {
    item: WorkItem,
    action: ReviewerAction,
    input: String,
    suggestions: Vec<String>,
    suggestions_loading: bool,
    suggestions_error: Option<String>,
    selected_suggestion: usize,
}

fn inbox_thread_action_label(action: InboxThreadAction) -> &'static str {
    match action {
        InboxThreadAction::Mute => "mute thread",
        InboxThreadAction::Subscribe => "subscribe thread",
        InboxThreadAction::Unsubscribe => "unsubscribe thread",
    }
}

fn inbox_thread_action_running_status(action: InboxThreadAction) -> &'static str {
    match action {
        InboxThreadAction::Mute => "muting inbox thread",
        InboxThreadAction::Subscribe => "subscribing to inbox thread",
        InboxThreadAction::Unsubscribe => "unsubscribing from inbox thread",
    }
}

fn inbox_thread_action_success_status(action: InboxThreadAction) -> &'static str {
    match action {
        InboxThreadAction::Mute => "inbox thread muted",
        InboxThreadAction::Subscribe => "subscribed to inbox thread",
        InboxThreadAction::Unsubscribe => "unsubscribed from inbox thread",
    }
}

fn item_subscription_action_label(action: ItemSubscriptionAction) -> &'static str {
    match action {
        ItemSubscriptionAction::Subscribe => "subscribe item",
        ItemSubscriptionAction::Unsubscribe => "unsubscribe item",
    }
}

fn item_subscription_action_running_status(
    action: ItemSubscriptionAction,
    item_kind: ItemKind,
) -> String {
    let label = item_kind_label(item_kind);
    match action {
        ItemSubscriptionAction::Subscribe => format!("subscribing to {label} conversation"),
        ItemSubscriptionAction::Unsubscribe => {
            format!("unsubscribing from {label} conversation")
        }
    }
}

fn item_subscription_action_success_status(
    action: ItemSubscriptionAction,
    item_kind: ItemKind,
) -> String {
    let label = item_kind_label(item_kind);
    match action {
        ItemSubscriptionAction::Subscribe => format!("subscribed to {label} conversation"),
        ItemSubscriptionAction::Unsubscribe => {
            format!("unsubscribed from {label} conversation")
        }
    }
}

fn item_kind_label(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Issue => "issue",
        ItemKind::PullRequest => "pull request",
        ItemKind::Notification => "notification",
    }
}

fn item_kind_confirmation_label(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Issue => "issue",
        ItemKind::PullRequest => "PR",
        ItemKind::Notification => "item",
    }
}

fn pr_action_success_status(action: PrAction, item_kind: ItemKind) -> String {
    let label = item_kind_label(item_kind);
    match action {
        PrAction::Merge => "pull request merged; refreshing".to_string(),
        PrAction::Close => format!("{label} closed; refreshing"),
        PrAction::Reopen => format!("{label} reopened; refreshing"),
        PrAction::Approve => "pull request approved; refreshing".to_string(),
        PrAction::EnableAutoMerge => "pull request auto-merge enabled; refreshing".to_string(),
        PrAction::DisableAutoMerge => "pull request auto-merge disabled; refreshing".to_string(),
        PrAction::Checkout => "pull request checked out locally".to_string(),
        PrAction::RerunFailedChecks => "failed checks rerun; refreshing".to_string(),
        PrAction::UpdateBranch => "pull request branch update accepted; refreshing".to_string(),
        PrAction::ConvertToDraft => "pull request converted to draft; refreshing".to_string(),
        PrAction::MarkReadyForReview => "pull request marked ready; refreshing".to_string(),
    }
}

fn pr_action_confirm_status(action: PrAction, item_kind: ItemKind) -> String {
    let label = item_kind_label(item_kind);
    match action {
        PrAction::Merge => "confirm pull request merge".to_string(),
        PrAction::Close => format!("confirm {label} close"),
        PrAction::Reopen => format!("confirm {label} reopen"),
        PrAction::Approve => "confirm pull request approval".to_string(),
        PrAction::EnableAutoMerge => "confirm pull request auto-merge enable".to_string(),
        PrAction::DisableAutoMerge => "confirm pull request auto-merge disable".to_string(),
        PrAction::Checkout => "confirm local pull request checkout".to_string(),
        PrAction::RerunFailedChecks => "confirm failed check rerun".to_string(),
        PrAction::UpdateBranch => "confirm pull request branch update".to_string(),
        PrAction::ConvertToDraft => "confirm convert pull request to draft".to_string(),
        PrAction::MarkReadyForReview => "confirm mark pull request ready".to_string(),
    }
}

fn pr_action_running_status(action: PrAction, item_kind: ItemKind) -> String {
    let label = item_kind_label(item_kind);
    match action {
        PrAction::Merge => "merging pull request".to_string(),
        PrAction::Close => format!("closing {label}"),
        PrAction::Reopen => format!("reopening {label}"),
        PrAction::Approve => "approving pull request".to_string(),
        PrAction::EnableAutoMerge => "enabling pull request auto-merge".to_string(),
        PrAction::DisableAutoMerge => "disabling pull request auto-merge".to_string(),
        PrAction::Checkout => "checking out pull request locally".to_string(),
        PrAction::RerunFailedChecks => "rerunning failed checks".to_string(),
        PrAction::UpdateBranch => "updating pull request branch".to_string(),
        PrAction::ConvertToDraft => "converting pull request to draft".to_string(),
        PrAction::MarkReadyForReview => "marking pull request ready for review".to_string(),
    }
}

fn close_or_reopen_action_for_item(item: &WorkItem) -> std::result::Result<PrAction, &'static str> {
    if !matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest) {
        return Err("selected item is not an issue or pull request");
    }
    if item.number.is_none() {
        return Err("selected item has no issue or pull request number");
    }

    let state = item.state.as_deref().unwrap_or("open").to_ascii_lowercase();
    match state.as_str() {
        "closed" => Ok(PrAction::Reopen),
        "merged" if item.kind == ItemKind::PullRequest => {
            Err("merged pull requests cannot be reopened")
        }
        _ => Ok(PrAction::Close),
    }
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
    RetryableError,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DraftSaveTrigger {
    Manual,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DraftSaveOutcome {
    Saved,
    Cleared,
    Unchanged,
    MissingTarget,
}

const TABLE_HEADER_HEIGHT: u16 = 1;
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
const COMMENTS_AUTO_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const COMMENTS_POST_REFRESH_DELAY: Duration = Duration::from_secs(1);
const EDITOR_DRAFT_AUTO_SAVE_INTERVAL: Duration = Duration::from_secs(2);
const RECENT_ITEM_DWELL: Duration = Duration::from_secs(5);
const AUTO_THEME_CHECK_INTERVAL: Duration = Duration::from_secs(30);
const EVENT_POLL_TIMEOUT: Duration = Duration::from_millis(250);
#[cfg(not(test))]
const TERMINAL_DISCONNECT_CHECK_INTERVAL: Duration = Duration::from_secs(1);
const LABEL_SUGGESTION_LIMIT: usize = 6;
const ASSIGNEE_SUGGESTION_LIMIT: usize = 6;
const REVIEWER_SUGGESTION_LIMIT: usize = 6;
const GLOBAL_SEARCH_SUGGESTION_LIMIT: usize = 6;
const MENTION_SUGGESTION_LIMIT: usize = 6;
const IDLE_SWEEP_SECTION_LIMIT: usize = 2;
const INITIAL_IDLE_SWEEP_DELAY: Duration = Duration::from_secs(300);

struct AppState {
    theme_name: ThemeName,
    last_auto_theme_check: Instant,
    active_view: String,
    sections: Vec<SectionSnapshot>,
    section_index: HashMap<String, usize>,
    base_section_filters: HashMap<String, String>,
    quick_filters: HashMap<String, QuickFilter>,
    section_filter_overrides: HashMap<String, SectionFilterOverride>,
    selected_index: HashMap<String, usize>,
    list_scroll_offset: HashMap<String, usize>,
    view_snapshots: HashMap<String, ViewSnapshotState>,
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
    global_search_dialog: Option<GlobalSearchDialog>,
    save_search_dialog: Option<SaveSearchDialog>,
    saved_search_dialog: Option<SavedSearchDialog>,
    global_search_by_repo: HashMap<String, GlobalSearchState>,
    global_search_saved_by_repo: HashMap<String, Vec<GlobalSearchSavedState>>,
    global_search_running: bool,
    global_search_return_view: Option<String>,
    global_search_scope: Option<String>,
    global_search_preferred_kind: Option<SectionKind>,
    global_search_started_at: Option<Instant>,
    filter_input_active: bool,
    filter_input_query: String,
    command_palette: Option<CommandPalette>,
    project_switcher: Option<ProjectSwitcher>,
    top_menu_switcher: Option<TopMenuSwitcher>,
    theme_switcher: Option<ThemeSwitcher>,
    recent_items_dialog: Option<RecentItemsDialog>,
    recent_items: Vec<RecentItem>,
    recent_items_dirty: bool,
    recent_commands: Vec<RecentCommand>,
    repo_unseen_items: HashMap<String, RepoUnseenItems>,
    repo_views_seen_this_session: HashSet<String>,
    details_visit: Option<DetailsVisitState>,
    project_add_dialog: Option<ProjectAddDialog>,
    project_remove_dialog: Option<ProjectRemoveDialog>,
    cache_clear_dialog: Option<CacheClearDialog>,
    command_palette_key: String,
    status: String,
    refreshing: bool,
    current_refresh_scope: RefreshScope,
    section_page_loading: Option<SectionPageLoading>,
    last_refresh_request: Instant,
    idle_sweep_refreshing: bool,
    idle_sweep_cursor: usize,
    last_idle_sweep_request: Instant,
    details: HashMap<String, DetailState>,
    details_synced_at: HashMap<String, DateTime<Utc>>,
    details_refreshed_at: HashMap<String, DateTime<Utc>>,
    optimistic_comment_ids: HashMap<String, HashSet<u64>>,
    diffs: HashMap<String, DiffState>,
    selected_diff_file: HashMap<String, usize>,
    selected_diff_line: HashMap<String, usize>,
    diff_file_details_scroll: HashMap<String, u16>,
    ignored_items: HashSet<String>,
    diff_mark: HashMap<String, DiffMarkState>,
    last_diff_click: Option<DiffClickState>,
    diff_mode_state: HashMap<String, DiffModeState>,
    diff_inline_comments_visible: bool,
    revealed_diff_inline_comments: HashMap<String, HashSet<usize>>,
    conversation_details_state: HashMap<String, ConversationDetailsState>,
    viewed_item_at: HashMap<String, DateTime<Utc>>,
    action_hints: HashMap<String, ActionHintState>,
    action_hints_stale: HashSet<String>,
    action_hints_refreshing: HashSet<String>,
    label_suggestions_cache: HashMap<String, Vec<String>>,
    assignee_suggestions_cache: HashMap<String, Vec<String>>,
    reviewer_suggestions_cache: HashMap<String, Vec<String>>,
    mention_candidate_loading_repos: HashSet<String>,
    mention_candidate_errors: HashMap<String, String>,
    mention_user_search_cache: HashMap<String, Vec<String>>,
    mention_user_search_loading_queries: HashSet<String>,
    mention_user_search_errors: HashMap<String, String>,
    mention_selected: usize,
    details_stale: HashSet<String>,
    details_refreshing: HashSet<String>,
    pending_details_load: Option<PendingDetailsLoad>,
    comments_refresh_requested_at: HashMap<String, Instant>,
    comments_refresh_after: HashMap<String, Instant>,
    notification_read_pending: HashSet<String>,
    notification_done_pending: HashSet<String>,
    selected_comment_index: usize,
    expanded_comments: HashSet<String>,
    comment_dialog: Option<CommentDialog>,
    editor_drafts: HashMap<String, EditorDraft>,
    comment_draft_key: Option<String>,
    comment_draft_original_body: String,
    comment_draft_last_saved_body: String,
    comment_draft_last_auto_save_at: Instant,
    posting_comment: bool,
    reaction_dialog: Option<ReactionDialog>,
    posting_reaction: bool,
    pending_comment_submit: Option<PendingCommentSubmit>,
    label_dialog: Option<LabelDialog>,
    label_updating: bool,
    issue_dialog: Option<IssueDialog>,
    issue_draft_key: Option<String>,
    issue_draft_last_saved_body: String,
    issue_draft_last_auto_save_at: Instant,
    issue_creating: bool,
    pending_issue_create: Option<PendingIssueCreate>,
    pr_create_dialog: Option<PrCreateDialog>,
    pr_create_draft_key: Option<String>,
    pr_create_draft_last_saved_body: String,
    pr_create_draft_last_auto_save_at: Instant,
    pr_creating: bool,
    pending_pr_create: Option<PendingPrCreate>,
    review_submit_dialog: Option<ReviewSubmitDialog>,
    review_submit_running: bool,
    pending_reviews: HashMap<String, PendingReviewState>,
    item_edit_dialog: Option<ItemEditDialog>,
    item_edit_running: bool,
    pr_action_dialog: Option<PrActionDialog>,
    pr_action_running: bool,
    milestone_dialog: Option<MilestoneDialog>,
    milestone_action_running: bool,
    assignee_dialog: Option<AssigneeDialog>,
    assignee_action_running: bool,
    reviewer_dialog: Option<ReviewerDialog>,
    reviewer_action_running: bool,
    setup_dialog: Option<SetupDialog>,
    startup_dialog: Option<StartupDialog>,
    message_dialog: Option<MessageDialog>,
    mouse_capture_enabled: bool,
    details_text_drag: Option<DetailsTextDrag>,
    details_text_selection: Option<DetailsTextSelection>,
    dialog_text_drag: Option<DialogTextDrag>,
    dialog_text_selection: Option<DialogTextSelection>,
    help_dialog: bool,
    diff_return_state: Option<DiffReturnState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ViewTab {
    key: String,
    label: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RepoUnseenItems {
    issues: HashSet<String>,
    pull_requests: HashSet<String>,
}

impl RepoUnseenItems {
    fn is_empty(&self) -> bool {
        self.issues.is_empty() && self.pull_requests.is_empty()
    }

    fn counts(&self) -> (usize, usize) {
        (self.issues.len(), self.pull_requests.len())
    }

    fn insert(&mut self, kind: SectionKind, item_id: String) {
        match kind {
            SectionKind::Issues => {
                self.issues.insert(item_id);
            }
            SectionKind::PullRequests => {
                self.pull_requests.insert(item_id);
            }
            SectionKind::Notifications => {}
        }
    }

    fn from_saved(saved: &RepoUnseenItemsState) -> Self {
        Self {
            issues: saved.issues.iter().cloned().collect(),
            pull_requests: saved.pull_requests.iter().cloned().collect(),
        }
    }

    fn to_saved(&self) -> RepoUnseenItemsState {
        RepoUnseenItemsState {
            issues: sorted_strings(&self.issues),
            pull_requests: sorted_strings(&self.pull_requests),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RefreshAnchor {
    active_view: String,
    section_key: Option<String>,
    item_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ViewSnapshotState {
    focus: FocusTarget,
    section_key: Option<String>,
    item_id: Option<String>,
    selected_index: usize,
    list_scroll_offset: usize,
    details_mode: DetailsMode,
    details_scroll: u16,
    selected_comment_index: usize,
}

impl ViewSnapshotState {
    fn from_saved(snapshot: &SavedViewSnapshot) -> Self {
        Self {
            focus: FocusTarget::from_state_str(&snapshot.focus),
            section_key: snapshot.section_key.clone(),
            item_id: snapshot.item_id.clone(),
            selected_index: snapshot.selected_index,
            list_scroll_offset: snapshot.list_scroll_offset,
            details_mode: DetailsMode::from_state_str(&snapshot.details_mode),
            details_scroll: snapshot.details_scroll,
            selected_comment_index: snapshot.selected_comment_index,
        }
    }

    fn to_saved(&self) -> SavedViewSnapshot {
        SavedViewSnapshot {
            focus: self.focus.as_state_str().to_string(),
            section_key: self.section_key.clone(),
            item_id: self.item_id.clone(),
            selected_index: self.selected_index,
            list_scroll_offset: self.list_scroll_offset,
            details_mode: self.details_mode.as_state_str().to_string(),
            details_scroll: self.details_scroll,
            selected_comment_index: self.selected_comment_index,
        }
    }
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

#[derive(Debug, Clone)]
struct SectionPageLoading {
    section_key: String,
    title: String,
    page_label: String,
    started_at: Instant,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct SectionFilterOverride {
    display: String,
    filters: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SavedSearchSectionTarget {
    section_key: String,
    section_position: usize,
    view: String,
    kind: SectionKind,
    title: String,
    filters: String,
    page_size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefreshPriority {
    User,
    Background,
}

pub async fn run(mut config: Config, paths: Paths, store: SnapshotStore) -> Result<()> {
    let cached = store.load_all()?;
    let show_startup_dialog = should_show_startup_dialog(&cached);
    let sections = merge_cached_sections(configured_sections(&config), cached);
    let repo_candidate_cache = store.load_repo_candidate_cache()?;
    let editor_drafts = store.load_editor_drafts()?;
    let ui_state = UiState::load_or_default(&paths.state_path);
    let migrated_saved_search_filters =
        saved_search_filters_from_map(&ui_state.global_search_saved_by_repo);
    if config.saved_search_filters.is_empty() && !migrated_saved_search_filters.is_empty() {
        config.saved_search_filters = migrated_saved_search_filters;
        if let Err(error) = config.save(&paths.config_path) {
            warn!(
                error = %error,
                path = %paths.config_path.display(),
                "failed to migrate saved search filters to config"
            );
        }
    }
    let mut app = AppState::with_ui_state(config.defaults.view, sections, ui_state);
    app.load_saved_search_filters(&config);
    app.load_repo_candidate_cache(repo_candidate_cache);
    app.load_editor_drafts(editor_drafts);
    app.command_palette_key = normalized_command_palette_key(&config.defaults.command_palette_key);
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
            RefreshScope::View(app.active_view.clone()),
        );
    }
    app.apply_theme_preference(&config);

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
    start_terminal_disconnect_watchdog();

    let result = run_loop(
        &mut terminal,
        &mut app,
        &mut config,
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
    debug!(command = "gh --version", "gh request started");
    let result = Command::new("gh")
        .env("GH_PROMPT_DISABLED", "1")
        .arg("--version")
        .output();
    match &result {
        Ok(output) => {
            debug!(
                command = "gh --version",
                status = %output.status,
                success = output.status.success(),
                stdout_bytes = output.stdout.len(),
                stderr_bytes = output.stderr.len(),
                "gh request finished"
            );
            if !output.status.success() {
                error!(
                    command = "gh --version",
                    status = %output.status,
                    message = %gh_version_output_message(output),
                    stdout_bytes = output.stdout.len(),
                    stderr_bytes = output.stderr.len(),
                    "gh request returned failure"
                );
            }
        }
        Err(error) => {
            debug!(
                command = "gh --version",
                error = %error,
                "gh request failed to start"
            );
            error!(
                command = "gh --version",
                error = %error,
                "gh request failed to start"
            );
        }
    }
    startup_setup_dialog_from_gh_probe(result.map(|_| ()))
}

fn gh_version_output_message(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stderr.is_empty() { stdout } else { stderr }
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
    config: &mut Config,
    paths: &Paths,
    store: &SnapshotStore,
    tx: &UnboundedSender<AppMsg>,
    rx: &mut UnboundedReceiver<AppMsg>,
) -> Result<()> {
    let mut mouse_capture_enabled = true;
    let initial_size = terminal.size()?;
    let mut last_area = Rect::new(0, 0, initial_size.width, initial_size.height);
    let mut needs_draw = true;
    loop {
        if !terminal_streams_connected() {
            warn!("terminal disconnected; exiting ghr");
            break;
        }

        needs_draw |= drain_app_messages(app, rx);
        needs_draw |= app.ensure_current_details_loading(tx);
        needs_draw |= app.ensure_current_comments_auto_refresh(tx);
        needs_draw |= app.ensure_current_diff_loading(tx);
        app.sync_recent_details_visit(Instant::now());
        if app.take_recent_items_dirty() {
            save_ui_state(app, paths);
        }
        needs_draw |= app.auto_save_active_editor_drafts(store, Instant::now());
        needs_draw |= app.dismiss_expired_message_dialog(Instant::now());
        needs_draw |= app.refresh_auto_theme(config, Instant::now());

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
                RefreshScope::View(app.active_view.clone()),
            );
        } else if app.should_start_idle_sweep(config) {
            start_idle_sweep(
                config.clone(),
                store.clone(),
                tx.clone(),
                app.active_view.clone(),
                app.idle_sweep_cursor,
            );
        }

        needs_draw |= drain_app_messages(app, rx);
        let size = terminal.size()?;
        let area = Rect::new(0, 0, size.width, size.height);
        if area != last_area {
            last_area = area;
            needs_draw = true;
        }
        if needs_draw {
            terminal.draw(|frame| draw(frame, app, paths))?;
            needs_draw = false;
        }

        let mut should_quit = false;
        let Some(event_ready) = poll_terminal_event()? else {
            break;
        };
        if event_ready {
            let Some(first_event) = read_terminal_event()? else {
                break;
            };
            let events = read_event_batch(first_event)?;
            let size = terminal.size()?;
            let area = Rect::new(0, 0, size.width, size.height);
            should_quit = handle_event_batch_mut(app, events, area, config, paths, store, tx);
            needs_draw = true;
        }
        sync_mouse_capture(terminal, app, &mut mouse_capture_enabled)?;
        if should_quit {
            break;
        }
    }

    save_ui_state(app, paths);
    Ok(())
}

fn drain_app_messages(app: &mut AppState, rx: &mut UnboundedReceiver<AppMsg>) -> bool {
    let mut handled = false;
    while let Ok(message) = rx.try_recv() {
        app.handle_msg(message);
        handled = true;
    }
    handled
}

fn terminal_streams_connected() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

#[cfg(not(test))]
fn start_terminal_disconnect_watchdog() {
    let _ = std::thread::Builder::new()
        .name("terminal-disconnect-watchdog".to_string())
        .spawn(|| {
            loop {
                std::thread::sleep(TERMINAL_DISCONNECT_CHECK_INTERVAL);
                if !terminal_streams_connected() {
                    warn!("terminal disconnected; exiting ghr");
                    std::process::exit(0);
                }
            }
        });
}

#[cfg(test)]
fn start_terminal_disconnect_watchdog() {}

fn terminal_disconnect_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::NotConnected
            | io::ErrorKind::UnexpectedEof
    ) || matches!(error.raw_os_error(), Some(5 | 6 | 9 | 19 | 25))
}

fn poll_terminal_event() -> Result<Option<bool>> {
    poll_terminal_event_with_timeout(EVENT_POLL_TIMEOUT)
}

fn poll_terminal_event_now() -> Result<Option<bool>> {
    poll_terminal_event_with_timeout(Duration::from_millis(0))
}

fn poll_terminal_event_with_timeout(timeout: Duration) -> Result<Option<bool>> {
    match event::poll(timeout) {
        Ok(ready) => Ok(Some(ready)),
        Err(error) if terminal_disconnect_error(&error) || !terminal_streams_connected() => {
            warn!(error = %error, "terminal event poll failed after terminal disconnected");
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

fn read_terminal_event() -> Result<Option<Event>> {
    match event::read() {
        Ok(event) => Ok(Some(event)),
        Err(error) if terminal_disconnect_error(&error) || !terminal_streams_connected() => {
            warn!(error = %error, "terminal event read failed after terminal disconnected");
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

fn read_event_batch(first: Event) -> Result<Vec<Event>> {
    let mut events = vec![first];
    while events.len() < EVENT_BATCH_LIMIT && poll_terminal_event_now()?.unwrap_or(false) {
        let Some(event) = read_terminal_event()? else {
            break;
        };
        events.push(event);
    }
    Ok(events)
}

fn handle_event_batch_mut(
    app: &mut AppState,
    events: Vec<Event>,
    area: Rect,
    config: &mut Config,
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
                if handle_key_in_area_mut(app, key, config, paths, store, tx, Some(area)) {
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

fn pull_request_changes_url(item: &WorkItem) -> String {
    match item.number {
        Some(number) if !item.repo.trim().is_empty() => {
            format!("https://github.com/{}/pull/{number}/changes", item.repo)
        }
        _ => format!("{}/changes", item.url.trim_end_matches('/')),
    }
}

fn pull_request_commits_url(item: &WorkItem) -> String {
    match item.number {
        Some(number) if !item.repo.trim().is_empty() => {
            format!("https://github.com/{}/pull/{number}/commits", item.repo)
        }
        _ => format!("{}/commits", item.url.trim_end_matches('/')),
    }
}

fn same_view_key(left: &str, right: &str) -> bool {
    left == right
        || (left.starts_with("repo:")
            && right.starts_with("repo:")
            && left.eq_ignore_ascii_case(right))
}

fn view_supports_snapshot(view: &str) -> bool {
    view.starts_with("repo:")
}

fn section_repo_scope(section: &SectionSnapshot) -> Option<String> {
    section
        .filters
        .split_whitespace()
        .find_map(repo_token_value)
}

fn created_issue_matches_section(
    section: &SectionSnapshot,
    item: &WorkItem,
    active_view: &str,
) -> bool {
    if section.kind != SectionKind::Issues {
        return false;
    }

    match section_repo_scope(section) {
        Some(repo) => repo == item.repo,
        None => same_view_key(&section_view_key(section), active_view),
    }
}

fn created_pull_request_matches_section(
    section: &SectionSnapshot,
    item: &WorkItem,
    active_view: &str,
) -> bool {
    if section.kind != SectionKind::PullRequests {
        return false;
    }

    match section_repo_scope(section) {
        Some(repo) => repo.eq_ignore_ascii_case(&item.repo),
        None => same_view_key(&section_view_key(section), active_view),
    }
}

fn repo_token_value(token: &str) -> Option<String> {
    token
        .strip_prefix("repo:")
        .filter(|repo| !repo.trim().is_empty())
        .map(str::to_string)
}

fn item_supports_metadata_edit(item: &WorkItem) -> bool {
    matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest) && item.number.is_some()
}

#[cfg(any(test, all(not(test), unix, not(target_os = "macos"))))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ClipboardCommand {
    command: &'static str,
    args: &'static [&'static str],
    required_env: Option<&'static str>,
}

#[cfg(any(test, all(not(test), unix, not(target_os = "macos"))))]
const UNIX_CLIPBOARD_COMMANDS: &[ClipboardCommand] = &[
    ClipboardCommand {
        command: "wl-copy",
        args: &[],
        required_env: Some("WAYLAND_DISPLAY"),
    },
    ClipboardCommand {
        command: "xclip",
        args: &["-selection", "clipboard"],
        required_env: Some("DISPLAY"),
    },
    ClipboardCommand {
        command: "xsel",
        args: &["--clipboard", "--input"],
        required_env: Some("DISPLAY"),
    },
];

#[cfg(all(not(test), unix, not(target_os = "macos")))]
fn env_var_present(name: &str) -> bool {
    env::var_os(name).is_some_and(|value| !value.as_os_str().is_empty())
}

#[cfg(any(test, all(not(test), unix, not(target_os = "macos"))))]
fn clipboard_command_enabled(command: ClipboardCommand, has_env: impl Fn(&str) -> bool) -> bool {
    command.required_env.is_none_or(has_env)
}

#[cfg(any(test, all(not(test), unix, not(target_os = "macos"))))]
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);

        encoded.push(TABLE[(first >> 2) as usize] as char);
        encoded.push(TABLE[(((first & 0b0000_0011) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            encoded.push(TABLE[(((second & 0b0000_1111) << 2) | (third >> 6)) as usize] as char);
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(TABLE[(third & 0b0011_1111) as usize] as char);
        } else {
            encoded.push('=');
        }
    }

    encoded
}

fn copy_text_to_clipboard(text: &str) -> io::Result<()> {
    #[cfg(test)]
    {
        let _ = text;
        Ok(())
    }

    #[cfg(all(not(test), target_os = "macos"))]
    {
        copy_text_with_command("pbcopy", &[], text)
    }

    #[cfg(all(not(test), target_os = "windows"))]
    {
        copy_text_with_command("cmd", &["/C", "clip"], text)
    }

    #[cfg(all(not(test), unix, not(target_os = "macos")))]
    {
        let mut errors = Vec::new();

        if env_var_present("TMUX") {
            match copy_text_with_tmux(text) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    errors.push("tmux not found".to_string());
                }
                Err(error) => errors.push(error.to_string()),
            }
        }

        for command in UNIX_CLIPBOARD_COMMANDS {
            if !clipboard_command_enabled(*command, env_var_present) {
                if let Some(required_env) = command.required_env {
                    errors.push(format!(
                        "{} skipped; {required_env} is not set",
                        command.command
                    ));
                }
                continue;
            }

            match copy_text_with_command(command.command, command.args, text) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    errors.push(format!("{} not found", command.command));
                }
                Err(error) => errors.push(error.to_string()),
            }
        }

        match copy_text_with_osc52(text) {
            Ok(()) => Ok(()),
            Err(error) => {
                errors.push(error.to_string());
                Err(clipboard_copy_error(&errors))
            }
        }
    }

    #[cfg(not(any(test, unix, target_os = "windows")))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "clipboard copy is not supported on this platform",
        ))
    }
}

#[cfg(not(test))]
fn copy_text_with_command(command: &str, args: &[&str], text: &str) -> io::Result<()> {
    let mut child = Command::new(command)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let write_result = {
        let mut stdin = child.stdin.take().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                format!("failed to open stdin for {command}"),
            )
        })?;
        stdin.write_all(text.as_bytes())
    };

    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(io::Error::other(command_failure_message(
            command,
            &output.status.to_string(),
            &output.stderr,
        )));
    }

    write_result
}

#[cfg(all(not(test), unix, not(target_os = "macos")))]
fn copy_text_with_tmux(text: &str) -> io::Result<()> {
    let mut errors = Vec::new();

    match tmux_current_client() {
        Ok(client) => {
            let args = ["load-buffer", "-w", "-t", client.as_str(), "-"];
            match copy_text_with_command("tmux", &args, text) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(error),
                Err(error) => errors.push(error.to_string()),
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(error),
        Err(error) => errors.push(format!("failed to resolve tmux client: {error}")),
    }

    match copy_text_with_command("tmux", &["load-buffer", "-w", "-"], text) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(error),
        Err(error) => {
            errors.push(error.to_string());
            match copy_text_with_command("tmux", &["load-buffer", "-"], text) {
                Ok(()) => Ok(()),
                Err(buffer_error) if buffer_error.kind() == io::ErrorKind::NotFound => {
                    Err(buffer_error)
                }
                Err(buffer_error) => {
                    errors.push(buffer_error.to_string());
                    Err(tmux_clipboard_error(&errors))
                }
            }
        }
    }
}

#[cfg(all(not(test), unix, not(target_os = "macos")))]
fn tmux_current_client() -> io::Result<String> {
    let mut command = Command::new("tmux");
    command.arg("display-message").arg("-p");
    if let Some(pane) = env::var_os("TMUX_PANE").filter(|pane| !pane.is_empty()) {
        command.arg("-t").arg(pane);
    }
    let output = command
        .arg("#{client_name}")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    if !output.status.success() {
        return Err(io::Error::other(command_failure_message(
            "tmux",
            &output.status.to_string(),
            &output.stderr,
        )));
    }

    let client = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if client.is_empty() {
        Err(io::Error::other("tmux client name is empty"))
    } else {
        Ok(client)
    }
}

#[cfg(all(not(test), unix, not(target_os = "macos")))]
fn copy_text_with_osc52(text: &str) -> io::Result<()> {
    if !io::stdout().is_terminal() {
        return Err(io::Error::other("OSC 52 requires terminal stdout"));
    }

    let encoded = base64_encode(text.as_bytes());
    let mut stdout = io::stdout().lock();
    stdout.write_all(b"\x1b]52;c;")?;
    stdout.write_all(encoded.as_bytes())?;
    stdout.write_all(b"\x07")?;
    stdout.flush()
}

#[cfg(all(not(test), unix, not(target_os = "macos")))]
fn tmux_clipboard_error(errors: &[String]) -> io::Error {
    let detail = errors
        .iter()
        .filter(|error| !error.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
    if detail.is_empty() {
        io::Error::other("tmux clipboard copy failed")
    } else {
        io::Error::other(format!("tmux clipboard copy failed: {detail}"))
    }
}

fn command_failure_message(command: &str, status: &str, stderr: &[u8]) -> String {
    let detail = String::from_utf8_lossy(stderr)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if detail.is_empty() {
        format!("{command} failed ({status})")
    } else {
        format!("{command} failed ({status}): {detail}")
    }
}

#[cfg(any(test, all(not(test), unix, not(target_os = "macos"))))]
fn clipboard_copy_error(errors: &[String]) -> io::Error {
    let detail = errors
        .iter()
        .filter(|error| !error.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
    if detail.is_empty() {
        io::Error::other("no usable clipboard target; tried wl-copy, xclip, xsel, tmux, and OSC 52")
    } else {
        io::Error::other(format!("no usable clipboard target: {detail}"))
    }
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

fn is_ctrl_d_key(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char(value) if value.eq_ignore_ascii_case(&'d'))
}

fn should_open_command_palette(app: &AppState, key: KeyEvent) -> bool {
    let binding = command_palette_key_binding(&app.command_palette_key);
    if !binding.matches(key) {
        return false;
    }
    !(binding.is_plain_text_char() && text_input_active(app))
}

fn text_input_active(app: &AppState) -> bool {
    app.search_active
        || app.comment_search_active
        || app.global_search_active
        || app.filter_input_active
        || app.comment_dialog.is_some()
        || app.label_dialog.is_some()
        || app.issue_dialog.is_some()
        || app.pr_create_dialog.is_some()
        || app.review_submit_dialog.is_some()
        || app.item_edit_dialog.is_some()
        || app.milestone_dialog.is_some()
        || app.assignee_dialog.is_some()
        || app.reviewer_dialog.is_some()
        || app.project_add_dialog.is_some()
        || app.project_remove_dialog.is_some()
        || app.recent_items_dialog.is_some()
        || app.top_menu_switcher.is_some()
        || app.theme_switcher.is_some()
        || app.cache_clear_dialog.is_some()
}

fn is_assignee_assign_key(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('@'))
        || (matches!(key.code, KeyCode::Char('2')) && key.modifiers.contains(KeyModifiers::SHIFT))
}

fn is_diff_key(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char(value) if value.eq_ignore_ascii_case(&'v'))
}

fn is_reaction_key(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('+'))
        || (matches!(key.code, KeyCode::Char('=')) && key.modifiers.contains(KeyModifiers::SHIFT))
}

fn is_ignore_key(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char(value) if value.eq_ignore_ascii_case(&'i'))
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

fn sorted_strings(values: &HashSet<String>) -> Vec<String> {
    let mut values = values.iter().cloned().collect::<Vec<_>>();
    values.sort();
    values
}

fn item_supports_details_memory(item: &WorkItem) -> bool {
    matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest) && item.number.is_some()
}

fn recent_item_from_work_item(item: &WorkItem, visited_at: DateTime<Utc>) -> Option<RecentItem> {
    if !item_supports_details_memory(item) {
        return None;
    }
    Some(RecentItem {
        id: item.id.clone(),
        kind: item.kind,
        repo: item.repo.clone(),
        number: item.number?,
        title: item.title.clone(),
        url: item.url.clone(),
        visited_at,
    })
}

fn recent_items_from_saved(items: &[RecentItemState]) -> Vec<RecentItem> {
    items
        .iter()
        .filter_map(|item| {
            let kind = match item.kind.as_str() {
                "pull_request" => ItemKind::PullRequest,
                "issue" => ItemKind::Issue,
                _ => return None,
            };
            Some(RecentItem {
                id: item.id.clone(),
                kind,
                repo: item.repo.clone(),
                number: item.number?,
                title: item.title.clone(),
                url: item.url.clone(),
                visited_at: item.visited_at?,
            })
        })
        .collect()
}

fn recent_items_to_saved(items: &[RecentItem]) -> Vec<RecentItemState> {
    items
        .iter()
        .map(|item| RecentItemState {
            id: item.id.clone(),
            kind: recent_item_state_kind(item.kind).to_string(),
            repo: item.repo.clone(),
            number: Some(item.number),
            title: item.title.clone(),
            url: item.url.clone(),
            visited_at: Some(item.visited_at),
        })
        .collect()
}

fn recent_commands_from_saved(items: &[RecentCommandState]) -> Vec<RecentCommand> {
    items
        .iter()
        .filter_map(|item| {
            Some(RecentCommand {
                id: item.id.clone(),
                selected_at: item.selected_at?,
            })
        })
        .collect()
}

fn recent_commands_to_saved(items: &[RecentCommand]) -> Vec<RecentCommandState> {
    items
        .iter()
        .map(|item| RecentCommandState {
            id: item.id.clone(),
            selected_at: Some(item.selected_at),
        })
        .collect()
}

fn command_palette_command_id(command: &PaletteCommand) -> String {
    command.title.to_string()
}

fn recent_item_state_kind(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::PullRequest => "pull_request",
        ItemKind::Issue => "issue",
        ItemKind::Notification => "notification",
    }
}

fn recent_item_display_kind(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::PullRequest => "pr",
        ItemKind::Issue => "issue",
        ItemKind::Notification => "item",
    }
}

fn recent_item_key(item: &RecentItem) -> String {
    details_memory_key(item.kind, &item.repo, item.number)
}

fn details_memory_key(kind: ItemKind, repo: &str, number: u64) -> String {
    format!(
        "{}:{}:{}",
        recent_item_state_kind(kind),
        repo.to_ascii_lowercase(),
        number
    )
}

fn work_item_details_memory_key(item: &WorkItem) -> Option<String> {
    if !item_supports_details_memory(item) {
        return None;
    }
    Some(details_memory_key(item.kind, &item.repo, item.number?))
}

fn work_item_details_memory_keys(item: &WorkItem) -> Vec<String> {
    let Some(primary) = work_item_details_memory_key(item) else {
        return Vec::new();
    };
    if item.id.is_empty() || item.id == primary {
        vec![primary]
    } else {
        vec![primary, item.id.clone()]
    }
}

fn recent_item_label(item: &RecentItem) -> String {
    format!(
        "[{}] #{} {}.  {}",
        recent_item_display_kind(item.kind),
        item.number,
        item.title,
        item.repo
    )
}

fn recent_item_matches_work_item(recent: &RecentItem, item: &WorkItem) -> bool {
    if item.kind != recent.kind {
        return false;
    }
    if item.repo.eq_ignore_ascii_case(&recent.repo) && item.number == Some(recent.number) {
        return true;
    }
    !recent.id.is_empty() && item.id == recent.id
}

impl AppState {
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
        let base_section_filters = sections
            .iter()
            .map(|section| (section.key.clone(), section.filters.clone()))
            .collect::<HashMap<_, _>>();
        let view_snapshots = ui_state
            .view_snapshots
            .iter()
            .map(|(view, snapshot)| (view.clone(), ViewSnapshotState::from_saved(snapshot)))
            .collect::<HashMap<_, _>>();
        let mut state = Self {
            theme_name: ThemeName::Dark,
            last_auto_theme_check: Instant::now(),
            active_view,
            sections,
            section_index: ui_state.section_index.clone(),
            base_section_filters,
            quick_filters: HashMap::new(),
            section_filter_overrides: HashMap::new(),
            selected_index: ui_state.selected_index.clone(),
            list_scroll_offset: HashMap::new(),
            view_snapshots,
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
            global_search_dialog: None,
            save_search_dialog: None,
            saved_search_dialog: None,
            global_search_by_repo: ui_state.global_search_by_repo.clone(),
            global_search_saved_by_repo: HashMap::new(),
            global_search_running: false,
            global_search_return_view: None,
            global_search_scope: None,
            global_search_preferred_kind: None,
            global_search_started_at: None,
            filter_input_active: false,
            filter_input_query: String::new(),
            command_palette: None,
            project_switcher: None,
            top_menu_switcher: None,
            theme_switcher: None,
            recent_items_dialog: None,
            recent_items: recent_items_from_saved(&ui_state.recent_items),
            recent_items_dirty: false,
            recent_commands: recent_commands_from_saved(&ui_state.recent_commands),
            repo_unseen_items: ui_state
                .repo_unseen_items
                .iter()
                .map(|(view, unseen)| (view.clone(), RepoUnseenItems::from_saved(unseen)))
                .collect(),
            repo_views_seen_this_session: HashSet::new(),
            details_visit: None,
            project_add_dialog: None,
            project_remove_dialog: None,
            cache_clear_dialog: None,
            command_palette_key: DEFAULT_COMMAND_PALETTE_KEY.to_string(),
            status: "loading snapshot; background refresh started".to_string(),
            refreshing: false,
            current_refresh_scope: RefreshScope::Full,
            section_page_loading: None,
            last_refresh_request: Instant::now(),
            idle_sweep_refreshing: false,
            idle_sweep_cursor: 0,
            last_idle_sweep_request: Instant::now() - INITIAL_IDLE_SWEEP_DELAY,
            details: HashMap::new(),
            details_synced_at: HashMap::new(),
            details_refreshed_at: HashMap::new(),
            optimistic_comment_ids: HashMap::new(),
            diffs: HashMap::new(),
            selected_diff_file: ui_state.selected_diff_file.clone(),
            selected_diff_line: ui_state.selected_diff_line.clone(),
            diff_file_details_scroll: ui_state.diff_file_details_scroll.clone(),
            ignored_items: ui_state.ignored_items.iter().cloned().collect(),
            diff_mark: HashMap::new(),
            last_diff_click: None,
            diff_mode_state: HashMap::new(),
            diff_inline_comments_visible: true,
            revealed_diff_inline_comments: HashMap::new(),
            conversation_details_state,
            viewed_item_at: ui_state.viewed_item_at.clone(),
            action_hints: HashMap::new(),
            action_hints_stale: HashSet::new(),
            action_hints_refreshing: HashSet::new(),
            label_suggestions_cache: HashMap::new(),
            assignee_suggestions_cache: HashMap::new(),
            reviewer_suggestions_cache: HashMap::new(),
            mention_candidate_loading_repos: HashSet::new(),
            mention_candidate_errors: HashMap::new(),
            mention_user_search_cache: HashMap::new(),
            mention_user_search_loading_queries: HashSet::new(),
            mention_user_search_errors: HashMap::new(),
            mention_selected: 0,
            details_stale: HashSet::new(),
            details_refreshing: HashSet::new(),
            pending_details_load: None,
            comments_refresh_requested_at: HashMap::new(),
            comments_refresh_after: HashMap::new(),
            notification_read_pending: HashSet::new(),
            notification_done_pending: HashSet::new(),
            selected_comment_index: 0,
            expanded_comments: ui_state.expanded_comments.iter().cloned().collect(),
            comment_dialog: None,
            editor_drafts: HashMap::new(),
            comment_draft_key: None,
            comment_draft_original_body: String::new(),
            comment_draft_last_saved_body: String::new(),
            comment_draft_last_auto_save_at: Instant::now(),
            posting_comment: false,
            reaction_dialog: None,
            posting_reaction: false,
            pending_comment_submit: None,
            label_dialog: None,
            label_updating: false,
            issue_dialog: None,
            issue_draft_key: None,
            issue_draft_last_saved_body: String::new(),
            issue_draft_last_auto_save_at: Instant::now(),
            issue_creating: false,
            pending_issue_create: None,
            pr_create_dialog: None,
            pr_create_draft_key: None,
            pr_create_draft_last_saved_body: String::new(),
            pr_create_draft_last_auto_save_at: Instant::now(),
            pr_creating: false,
            pending_pr_create: None,
            review_submit_dialog: None,
            review_submit_running: false,
            pending_reviews: HashMap::new(),
            item_edit_dialog: None,
            item_edit_running: false,
            pr_action_dialog: None,
            pr_action_running: false,
            milestone_dialog: None,
            milestone_action_running: false,
            assignee_dialog: None,
            assignee_action_running: false,
            reviewer_dialog: None,
            reviewer_action_running: false,
            setup_dialog: None,
            startup_dialog: None,
            message_dialog: None,
            mouse_capture_enabled: true,
            details_text_drag: None,
            details_text_selection: None,
            dialog_text_drag: None,
            dialog_text_selection: None,
            help_dialog: false,
            diff_return_state: None,
        };
        state.clamp_positions();
        if view_supports_snapshot(&state.active_view)
            && let Some(snapshot) = state.view_snapshots.get(&state.active_view).cloned()
        {
            state.restore_view_snapshot(snapshot);
        } else {
            state.focus = if matches!(focus, FocusTarget::Details) && state.current_item().is_none()
            {
                FocusTarget::List
            } else {
                focus
            };
            state.details_scroll = details_scroll;
            state.selected_comment_index = selected_comment_index;
            if ui_state.details_mode == "diff" {
                state.restore_saved_details_mode();
            }
        }
        state
    }

    fn load_saved_search_filters(&mut self, config: &Config) {
        self.global_search_saved_by_repo = saved_search_map_from_config(config);
    }

    fn effective_theme(config: &Config) -> ThemeName {
        config
            .defaults
            .theme_name
            .unwrap_or_else(|| config.defaults.theme.effective())
    }

    fn configured_theme_choice(config: &Config) -> ThemeChoice {
        if let Some(theme_name) = config.defaults.theme_name {
            return ThemeChoice::Name(theme_name);
        }
        match config.defaults.theme {
            ThemePreference::Auto => ThemeChoice::Auto,
            ThemePreference::Dark => ThemeChoice::Name(ThemeName::Dark),
            ThemePreference::Light => ThemeChoice::Name(ThemeName::Light),
        }
    }

    fn set_theme(&mut self, theme_name: ThemeName) {
        self.theme_name = theme_name;
        set_active_theme(theme_name);
    }

    fn apply_theme_preference(&mut self, config: &Config) {
        self.set_theme(Self::effective_theme(config));
        self.last_auto_theme_check = Instant::now();
    }

    fn refresh_auto_theme(&mut self, config: &Config, now: Instant) -> bool {
        if !config.defaults.theme.is_auto() || config.defaults.theme_name.is_some() {
            return false;
        }
        if now.duration_since(self.last_auto_theme_check) < AUTO_THEME_CHECK_INTERVAL {
            return false;
        }
        self.last_auto_theme_check = now;
        let next = config.defaults.theme.effective();
        if next == self.theme_name {
            return false;
        }
        self.set_theme(next);
        self.status = format!("theme auto: {}", next.as_str());
        true
    }

    fn set_color_theme(&mut self, choice: ThemeChoice, config: &mut Config, paths: &Paths) {
        let previous_app_theme = self.theme_name;
        let previous_theme_name = config.defaults.theme_name;
        let previous_config_theme = config.defaults.theme;

        let next_effective = match choice {
            ThemeChoice::Auto => {
                config.defaults.theme = ThemePreference::Auto;
                config.defaults.theme_name = None;
                config.defaults.theme.effective()
            }
            ThemeChoice::Name(ThemeName::Dark) => {
                config.defaults.theme = ThemePreference::Dark;
                config.defaults.theme_name = None;
                ThemeName::Dark
            }
            ThemeChoice::Name(ThemeName::Light) => {
                config.defaults.theme = ThemePreference::Light;
                config.defaults.theme_name = None;
                ThemeName::Light
            }
            ThemeChoice::Name(theme_name) => {
                config.defaults.theme = ThemePreference::from_theme_name(theme_name);
                config.defaults.theme_name = Some(theme_name);
                theme_name
            }
        };

        self.set_theme(next_effective);
        self.last_auto_theme_check = Instant::now();

        if let Err(error) = config.save(&paths.config_path) {
            self.set_theme(previous_app_theme);
            config.defaults.theme_name = previous_theme_name;
            config.defaults.theme = previous_config_theme;
            self.status = format!("theme set failed: {error}");
            return;
        }

        self.status = match choice {
            ThemeChoice::Auto => format!("theme: auto ({})", next_effective.as_str()),
            ThemeChoice::Name(theme_name) => format!("theme: {}", theme_name.as_str()),
        };
    }

    fn load_repo_candidate_cache(&mut self, cache: RepoCandidateCache) {
        self.label_suggestions_cache = cache.labels;
        self.assignee_suggestions_cache = cache.assignees;
        self.reviewer_suggestions_cache = cache.reviewers;
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
        let mut diff_file_details_scroll = self.diff_file_details_scroll.clone();
        if let Some((key, details_scroll)) = self.current_diff_file_details_scroll_entry() {
            diff_file_details_scroll.insert(key, details_scroll);
        }
        let mut view_snapshots = self.view_snapshots.clone();
        if view_supports_snapshot(&self.active_view) {
            view_snapshots.insert(self.active_view.clone(), self.current_view_snapshot());
        }
        let mut global_search_by_repo = self.global_search_by_repo.clone();
        if self.global_search_active
            && let Some(dialog) = &self.global_search_dialog
        {
            let key = global_search_repo_state_key(dialog.repo.as_deref());
            if let Some(state) = global_search_dialog_state(dialog) {
                global_search_by_repo.insert(key, state);
            } else {
                global_search_by_repo.remove(&key);
            }
        }

        UiState {
            list_width_percent: self.list_width_percent,
            active_view: self.active_view.clone(),
            section_index: self.section_index.clone(),
            selected_index: self.selected_index.clone(),
            view_snapshots: view_snapshots
                .iter()
                .map(|(view, snapshot)| (view.clone(), snapshot.to_saved()))
                .collect(),
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
            viewed_item_at: self.viewed_item_at.clone(),
            selected_diff_file: self.selected_diff_file.clone(),
            selected_diff_line: self.selected_diff_line.clone(),
            diff_file_details_scroll,
            ignored_items: sorted_strings(&self.ignored_items),
            recent_items: recent_items_to_saved(&self.recent_items),
            recent_commands: recent_commands_to_saved(&self.recent_commands),
            repo_unseen_items: self
                .repo_unseen_items
                .iter()
                .map(|(view, unseen)| (view.clone(), unseen.to_saved()))
                .collect(),
            global_search_by_repo,
            global_search_saved_by_repo: HashMap::new(),
        }
    }

    fn current_view_snapshot(&self) -> ViewSnapshotState {
        let section_key = self.current_section().map(|section| section.key.clone());
        let list_scroll_offset = section_key
            .as_ref()
            .and_then(|key| self.list_scroll_offset.get(key).copied())
            .unwrap_or(0);
        ViewSnapshotState {
            focus: self.focus,
            section_key,
            item_id: self.current_item().map(|item| item.id.clone()),
            selected_index: self.current_selected_position(),
            list_scroll_offset,
            details_mode: self.details_mode,
            details_scroll: self.details_scroll,
            selected_comment_index: self.selected_comment_index,
        }
    }

    fn remember_current_view_snapshot(&mut self) {
        self.save_current_conversation_details_state();
        self.save_current_diff_mode_state();
        if !view_supports_snapshot(&self.active_view) {
            return;
        }
        self.view_snapshots
            .insert(self.active_view.clone(), self.current_view_snapshot());
    }

    fn restore_view_snapshot(&mut self, snapshot: ViewSnapshotState) {
        let active_view = self.active_view.clone();
        if let Some(section_key) = snapshot.section_key.as_deref()
            && let Some(position) = self.section_position_by_key(&active_view, section_key)
        {
            self.set_current_section_position(position);
        }

        if let Some(item_id) = snapshot.item_id.as_deref() {
            if !self.select_current_item_by_id(item_id)
                && !self.select_item_in_view(&active_view, item_id)
            {
                self.set_current_selected_position(snapshot.selected_index);
            }
        } else {
            self.set_current_selected_position(snapshot.selected_index);
        }

        if let Some(section_key) = snapshot.section_key {
            self.list_scroll_offset
                .insert(section_key, snapshot.list_scroll_offset);
        }

        self.details_mode = snapshot.details_mode;
        self.details_scroll = snapshot.details_scroll;
        self.selected_comment_index = snapshot.selected_comment_index;

        if self.details_mode == DetailsMode::Diff {
            let can_show_diff = self
                .current_item()
                .is_some_and(|item| matches!(item.kind, ItemKind::PullRequest));
            if can_show_diff {
                if let Some(item_id) = self.current_item().map(|item| item.id.clone()) {
                    self.selected_diff_file.entry(item_id.clone()).or_insert(0);
                    self.selected_diff_line.entry(item_id.clone()).or_insert(0);
                    self.restore_selected_diff_file_details_scroll(&item_id, self.details_scroll);
                }
                self.diff_return_state.get_or_insert(DiffReturnState {
                    focus: FocusTarget::Details,
                    details_scroll: 0,
                    selected_comment_index: 0,
                });
            } else {
                self.details_mode = DetailsMode::Conversation;
                self.diff_return_state = None;
            }
        }

        self.focus =
            if matches!(snapshot.focus, FocusTarget::Details) && self.current_item().is_none() {
                FocusTarget::List
            } else {
                snapshot.focus
            };
        let selected_comment_index = self.selected_comment_index;
        self.clamp_positions();
        self.selected_comment_index = selected_comment_index;
        if self.details_mode == DetailsMode::Conversation {
            self.restore_current_conversation_details_state();
        }
        if self.current_comments().is_some() {
            self.clamp_selected_comment();
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

    fn should_start_idle_sweep(&self, config: &Config) -> bool {
        !self.refreshing
            && !self.idle_sweep_refreshing
            && self.setup_dialog.is_none()
            && self.section_page_loading.is_none()
            && !self.global_search_running
            && config.defaults.refetch_interval_seconds > 0
            && self.last_idle_sweep_request.elapsed().as_secs()
                >= config.defaults.refetch_interval_seconds
    }

    fn record_unseen_repo_items_for_sections(&mut self, sections: &[SectionSnapshot]) {
        for section in sections {
            if section.error.is_some()
                || !matches!(
                    section.kind,
                    SectionKind::Issues | SectionKind::PullRequests
                )
            {
                continue;
            }

            let view_key = section_view_key(section);
            if !view_key.starts_with("repo:") {
                continue;
            }

            if same_view_key(&view_key, &self.active_view)
                && self.repo_view_seen_this_session(&view_key)
            {
                self.clear_repo_unseen_for_view(&view_key);
                continue;
            }

            let Some(current) = self
                .sections
                .iter()
                .find(|current| current.key == section.key)
            else {
                continue;
            };
            if current.refreshed_at.is_none() && current.items.is_empty() {
                continue;
            }

            let current_ids = current
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<HashSet<_>>();
            let new_item_ids = section
                .items
                .iter()
                .filter(|item| !self.ignored_items.contains(&item.id))
                .filter(|item| !current_ids.contains(item.id.as_str()))
                .filter(|item| matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest))
                .map(|item| item.id.clone())
                .collect::<Vec<_>>();

            if new_item_ids.is_empty() {
                continue;
            }

            let target_key = self
                .repo_unseen_items
                .keys()
                .find(|key| same_view_key(key, &view_key))
                .cloned()
                .unwrap_or_else(|| view_key.clone());
            let unseen = self.repo_unseen_items.entry(target_key).or_default();
            for item_id in new_item_ids {
                unseen.insert(section.kind, item_id);
            }
        }
        self.repo_unseen_items
            .retain(|_, unseen| !unseen.is_empty());
    }

    fn clear_repo_unseen_for_view(&mut self, view: &str) {
        self.repo_unseen_items
            .retain(|key, _| !same_view_key(key, view));
    }

    fn mark_repo_view_seen(&mut self, view: &str) {
        if !view.starts_with("repo:") {
            return;
        }
        self.repo_views_seen_this_session
            .retain(|key| !same_view_key(key, view));
        self.repo_views_seen_this_session.insert(view.to_string());
        self.clear_repo_unseen_for_view(view);
    }

    fn repo_view_seen_this_session(&self, view: &str) -> bool {
        self.repo_views_seen_this_session
            .iter()
            .any(|seen| same_view_key(seen, view))
    }

    fn repo_unseen_items_for_view(&self, view: &str) -> Option<&RepoUnseenItems> {
        self.repo_unseen_items
            .iter()
            .find(|(key, _)| same_view_key(key, view))
            .map(|(_, unseen)| unseen)
    }

    fn has_unseen_repo_items(&self) -> bool {
        self.repo_unseen_items
            .values()
            .any(|unseen| !unseen.is_empty())
    }

    fn apply_idle_refreshed_sections(&mut self, sections: Vec<SectionSnapshot>) {
        let active_view = self.active_view.clone();
        let sections = sections
            .into_iter()
            .filter(|section| !same_view_key(&section_view_key(section), &active_view))
            .filter(|section| !self.has_active_section_filter(&section.key))
            .collect::<Vec<_>>();

        if sections.is_empty() {
            return;
        }

        for section in &sections {
            self.remember_base_filters(section);
        }

        self.record_unseen_repo_items_for_sections(&sections);
        let current = std::mem::take(&mut self.sections);
        self.sections = merge_refreshed_sections(current, sections);
    }

    fn apply_refreshed_section(&mut self, section: SectionSnapshot, save_error: Option<String>) {
        let anchor = self.current_refresh_anchor();
        let previous_details_scroll = self.details_scroll;
        let previous_comment_index = self.selected_comment_index;
        let title = section.title.clone();
        let section_error = section.error.clone();
        let setup_dialog = section_error.as_deref().and_then(setup_dialog_from_error);
        self.remember_base_filters(&section);
        if self.has_active_section_filter(&section.key) {
            if self.setup_dialog.is_none() {
                self.setup_dialog = setup_dialog;
            }
            self.status = match (section_error.as_deref(), save_error) {
                (None, None) => format!("loaded {title}; filter still active"),
                (Some(error), None) => refresh_error_status(1, Some(error)),
                (_, Some(error)) => format!("snapshot save failed: {error}"),
            };
            return;
        }

        if self.should_preserve_user_section_page(&section) {
            if self.setup_dialog.is_none() {
                self.setup_dialog = setup_dialog;
            }
            if self.section_page_loading.is_none() {
                self.status = match (section_error.as_deref(), save_error) {
                    (None, None) => format!("kept current {title} page; still refreshing"),
                    (Some(error), None) => refresh_error_status(1, Some(error)),
                    (_, Some(error)) => format!("snapshot save failed: {error}"),
                };
            }
            return;
        }

        self.record_unseen_repo_items_for_sections(std::slice::from_ref(&section));
        let current = std::mem::take(&mut self.sections);
        self.sections = merge_refreshed_sections(current, vec![section]);

        let restored_item = self.restore_refresh_anchor(&anchor);
        if restored_item {
            self.details_scroll = previous_details_scroll;
            self.selected_comment_index = previous_comment_index;
            self.clamp_selected_comment();
        } else {
            self.reset_or_restore_current_conversation_details_state();
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
            AppMsg::RefreshStarted { scope } => {
                self.refreshing = true;
                self.current_refresh_scope = scope;
                self.last_refresh_request = Instant::now();
                if self.section_page_loading.is_none() {
                    self.status = refresh_started_status(&self.current_refresh_scope);
                }
            }
            AppMsg::RefreshSectionLoaded {
                section,
                save_error,
            } => {
                self.apply_refreshed_section(section, save_error);
            }
            AppMsg::IdleSweepStarted => {
                self.idle_sweep_refreshing = true;
                self.last_idle_sweep_request = Instant::now();
            }
            AppMsg::IdleSweepFinished {
                sections,
                next_cursor,
            } => {
                self.idle_sweep_refreshing = false;
                self.idle_sweep_cursor = next_cursor;
                self.last_idle_sweep_request = Instant::now();
                self.apply_idle_refreshed_sections(sections);
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
                self.invalidate_action_hints_for_sections(&sections);
                for section in &sections {
                    self.remember_base_filters(section);
                }
                let sections = sections
                    .into_iter()
                    .filter(|section| !self.has_active_section_filter(&section.key))
                    .filter(|section| !self.should_preserve_user_section_page(section))
                    .collect::<Vec<_>>();
                self.record_unseen_repo_items_for_sections(&sections);
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
                    self.reset_or_restore_current_conversation_details_state();
                }
                self.refreshing = false;
                self.last_refresh_request = Instant::now();
                if matches!(self.current_refresh_scope, RefreshScope::Full) {
                    self.last_idle_sweep_request = Instant::now();
                }
                if matches!(self.startup_dialog, Some(StartupDialog::Initializing)) {
                    self.startup_dialog = if setup_dialog.is_some() {
                        None
                    } else {
                        Some(StartupDialog::Ready)
                    };
                }
                self.setup_dialog = setup_dialog;
                if self.section_page_loading.is_none() {
                    self.status = match (errors, save_error) {
                        (0, None) => refresh_finished_status(&self.current_refresh_scope),
                        (count, None) => refresh_error_status(count, first_error.as_deref()),
                        (_, Some(error)) => format!("snapshot save failed: {error}"),
                    };
                }
            }
            AppMsg::CommentsLoaded { item_id, comments } => match comments {
                Ok(mut result) => {
                    self.details_stale.remove(&item_id);
                    self.details_refreshing.remove(&item_id);
                    self.remember_details_synced_at(&item_id, &result);
                    self.apply_comment_fetch_result_metadata(&item_id, &result);
                    self.merge_optimistic_comments(&item_id, &mut result.comments);
                    self.details
                        .insert(item_id.clone(), DetailState::Loaded(result.comments));
                    self.clamp_selected_comment();
                    self.mark_current_details_viewed_if_current(&item_id);
                }
                Err(error) => {
                    self.details_stale.remove(&item_id);
                    self.details_refreshing.remove(&item_id);
                    if matches!(self.details.get(&item_id), Some(DetailState::Loaded(_))) {
                        self.status = format!(
                            "comments refresh failed; keeping cached comments: {}",
                            compact_error_label(&error)
                        );
                        return;
                    }
                    if self.setup_dialog.is_none() {
                        self.setup_dialog = setup_dialog_from_error(&error);
                    }
                    self.details
                        .insert(item_id.clone(), DetailState::Error(error));
                }
            },
            AppMsg::ActionHintsLoaded { item_id, actions } => match actions {
                Ok(actions) => {
                    self.action_hints_refreshing.remove(&item_id);
                    if !matches!(
                        self.action_hints.get(&item_id),
                        Some(ActionHintState::Loaded(current)) if current == &actions
                    ) {
                        self.action_hints
                            .insert(item_id, ActionHintState::Loaded(actions));
                    }
                }
                Err(error) => {
                    self.action_hints_refreshing.remove(&item_id);
                    if !matches!(
                        self.action_hints.get(&item_id),
                        Some(ActionHintState::Loaded(_))
                    ) {
                        self.action_hints
                            .insert(item_id, ActionHintState::Error(error));
                    }
                }
            },
            AppMsg::DiffLoaded { item_id, diff } => match diff {
                Ok(diff) => {
                    let restore_current_scroll = self.details_mode == DetailsMode::Diff
                        && self
                            .current_item()
                            .is_some_and(|item| item.id.as_str() == item_id);
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
                    self.diffs.insert(item_id.clone(), DiffState::Loaded(diff));
                    if restore_current_scroll {
                        self.restore_selected_diff_file_details_scroll(
                            &item_id,
                            self.details_scroll,
                        );
                    }
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
                Ok(comment) => {
                    self.remember_optimistic_comment(&item_id, &comment);
                    let index = self.append_local_comment(&item_id, comment);
                    self.selected_comment_index = index;
                    self.details_stale.remove(&item_id);
                    self.details_refreshing.remove(&item_id);
                    self.schedule_comments_refresh_after_post(item_id.clone());
                    self.clamp_selected_comment();
                    self.mark_current_details_viewed_if_current(&item_id);
                    self.posting_comment = false;
                    self.clear_pending_comment_draft_local();
                    self.pending_comment_submit = None;
                    self.status = "comment posted".to_string();
                    self.message_dialog = Some(success_message_dialog(
                        "Comment Posted",
                        "GitHub accepted the comment; comments will refresh in the background.",
                    ));
                }
                Err(error) => {
                    let setup_dialog = setup_dialog_from_error(&error);
                    if self.setup_dialog.is_none() {
                        self.setup_dialog = setup_dialog;
                    }
                    if setup_dialog.is_none() {
                        self.restore_pending_comment_submit_dialog();
                        self.message_dialog = Some(retryable_message_dialog(
                            "Comment Failed",
                            retryable_operation_error_body(&error),
                        ));
                    } else {
                        self.message_dialog = None;
                        self.restore_pending_comment_submit_dialog();
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
                Ok(mut result) => {
                    self.selected_comment_index =
                        comment_index.min(result.comments.len().saturating_sub(1));
                    self.details_stale.remove(&item_id);
                    self.details_refreshing.remove(&item_id);
                    self.remember_details_synced_at(&item_id, &result);
                    self.apply_comment_fetch_result_metadata(&item_id, &result);
                    self.merge_optimistic_comments(&item_id, &mut result.comments);
                    self.details
                        .insert(item_id.clone(), DetailState::Loaded(result.comments));
                    self.clamp_selected_comment();
                    self.mark_current_details_viewed_if_current(&item_id);
                    self.posting_comment = false;
                    self.clear_pending_comment_draft_local();
                    self.pending_comment_submit = None;
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
                        self.restore_pending_comment_submit_dialog();
                        self.message_dialog = Some(retryable_message_dialog(
                            "Update Failed",
                            retryable_operation_error_body(&error),
                        ));
                    } else {
                        self.message_dialog = None;
                        self.restore_pending_comment_submit_dialog();
                    }
                    self.posting_comment = false;
                    self.status = "comment update failed".to_string();
                }
            },
            AppMsg::ReviewCommentPosted { item_id, result } => match result {
                Ok(comment) => {
                    self.remember_optimistic_comment(&item_id, &comment);
                    let index = self.append_local_comment(&item_id, comment);
                    self.selected_comment_index = index;
                    self.details_stale.remove(&item_id);
                    self.details_refreshing.remove(&item_id);
                    self.schedule_comments_refresh_after_post(item_id.clone());
                    self.clamp_selected_comment();
                    self.mark_current_details_viewed_if_current(&item_id);
                    self.posting_comment = false;
                    self.clear_pending_comment_draft_local();
                    self.pending_comment_submit = None;
                    self.status = "review comment posted".to_string();
                    self.message_dialog = Some(success_message_dialog(
                        "Review Comment Posted",
                        "GitHub accepted the review comment; comments will refresh in the background.",
                    ));
                }
                Err(error) => {
                    let setup_dialog = setup_dialog_from_error(&error);
                    if self.setup_dialog.is_none() {
                        self.setup_dialog = setup_dialog;
                    }
                    if setup_dialog.is_none() {
                        self.restore_pending_comment_submit_dialog();
                        self.message_dialog = Some(retryable_message_dialog(
                            "Review Comment Failed",
                            retryable_operation_error_body(&error),
                        ));
                    } else {
                        self.message_dialog = None;
                        self.restore_pending_comment_submit_dialog();
                    }
                    self.posting_comment = false;
                    self.status = "review comment failed".to_string();
                }
            },
            AppMsg::ReactionPosted { item_id, result } => match result {
                Ok(mut result) => {
                    if let Some(dialog) = &self.reaction_dialog {
                        match dialog.target {
                            ReactionTarget::IssueComment { index, .. }
                            | ReactionTarget::ReviewComment { index, .. } => {
                                self.selected_comment_index =
                                    index.min(result.comments.len().saturating_sub(1));
                            }
                            ReactionTarget::Item => {}
                        }
                    }
                    self.details_stale.remove(&item_id);
                    self.details_refreshing.remove(&item_id);
                    self.remember_details_synced_at(&item_id, &result);
                    self.apply_comment_fetch_result_metadata(&item_id, &result);
                    self.merge_optimistic_comments(&item_id, &mut result.comments);
                    self.details
                        .insert(item_id.clone(), DetailState::Loaded(result.comments));
                    self.clamp_selected_comment();
                    self.mark_current_details_viewed_if_current(&item_id);
                    self.posting_reaction = false;
                    self.reaction_dialog = None;
                    self.status = "reaction added".to_string();
                    self.message_dialog = Some(success_message_dialog(
                        "Reaction Added",
                        "GitHub accepted the reaction and comments were refreshed.",
                    ));
                }
                Err(error) => {
                    let setup_dialog = setup_dialog_from_error(&error);
                    if self.setup_dialog.is_none() {
                        self.setup_dialog = setup_dialog;
                    }
                    if setup_dialog.is_none() {
                        self.message_dialog = Some(message_dialog(
                            "Reaction Failed",
                            operation_error_body(&error),
                        ));
                    } else {
                        self.message_dialog = None;
                    }
                    self.posting_reaction = false;
                    self.reaction_dialog = None;
                    self.status = "reaction failed".to_string();
                }
            },
            AppMsg::LabelUpdated {
                item_id,
                action,
                result,
            } => {
                self.label_updating = false;
                self.label_dialog = None;
                match result {
                    Ok(()) => {
                        self.apply_label_update_local(&item_id, &action);
                        self.status = match action {
                            LabelAction::Add(label) => format!("label added: {label}"),
                            LabelAction::Remove(label) => format!("label removed: {label}"),
                        };
                        self.message_dialog = Some(success_message_dialog(
                            "Labels Updated",
                            "GitHub accepted the label update.",
                        ));
                    }
                    Err(error) => {
                        let setup_dialog = setup_dialog_from_error(&error);
                        if self.setup_dialog.is_none() {
                            self.setup_dialog = setup_dialog;
                        }
                        if setup_dialog.is_none() {
                            self.message_dialog = Some(message_dialog(
                                "Label Update Failed",
                                operation_error_body(&error),
                            ));
                        } else {
                            self.message_dialog = None;
                        }
                        self.status = "label update failed".to_string();
                    }
                }
            }
            AppMsg::LabelSuggestionsLoaded { repo, result } => match result {
                Ok(labels) => {
                    self.label_suggestions_cache
                        .insert(repo.clone(), labels.clone());
                    if let Some(dialog) = &mut self.label_dialog
                        && let LabelDialogMode::Add { repo: dialog_repo } = &dialog.mode
                        && dialog_repo == &repo
                    {
                        dialog.suggestions_loading = false;
                        if dialog.suggestions != labels {
                            dialog.suggestions = labels.clone();
                        }
                        dialog.suggestions_error = None;
                        clamp_label_dialog_selection(dialog);
                        self.status = "label suggestions loaded".to_string();
                    }
                    if let Some(dialog) = &mut self.item_edit_dialog
                        && dialog.item.repo == repo
                    {
                        dialog.label_suggestions_loading = false;
                        if dialog.label_suggestions != labels {
                            dialog.label_suggestions = labels.clone();
                        }
                        dialog.label_suggestions_error = None;
                        clamp_item_edit_label_selection(dialog);
                        self.status = "label candidates loaded".to_string();
                    }
                    if let Some(dialog) = &mut self.global_search_dialog
                        && dialog.repo.as_deref() == Some(repo.as_str())
                    {
                        dialog.labels_loading = false;
                        dialog.labels = labels;
                        dialog.labels_error = None;
                        clamp_global_search_dialog_selection(dialog);
                    }
                }
                Err(error) => {
                    let has_cached_suggestions = self.label_suggestions_cache.contains_key(&repo);
                    if let Some(dialog) = &mut self.label_dialog
                        && let LabelDialogMode::Add { repo: dialog_repo } = &dialog.mode
                        && dialog_repo == &repo
                    {
                        dialog.suggestions_loading = false;
                        if has_cached_suggestions {
                            dialog.suggestions_error = None;
                            self.status =
                                "label suggestions refresh failed; using cache".to_string();
                        } else {
                            dialog.suggestions.clear();
                            dialog.suggestions_error = Some(error.clone());
                            dialog.selected_suggestion = 0;
                            self.status = "label suggestions unavailable".to_string();
                        }
                    }
                    if let Some(dialog) = &mut self.item_edit_dialog
                        && dialog.item.repo == repo
                    {
                        dialog.label_suggestions_loading = false;
                        if has_cached_suggestions {
                            dialog.label_suggestions_error = None;
                            self.status =
                                "label candidates refresh failed; using cache".to_string();
                        } else {
                            dialog.label_suggestions.clear();
                            dialog.label_suggestions_error = Some(error.clone());
                            dialog.selected_label_suggestion = 0;
                            self.status = "label candidates unavailable".to_string();
                        }
                    }
                    if let Some(dialog) = &mut self.global_search_dialog
                        && dialog.repo.as_deref() == Some(repo.as_str())
                    {
                        dialog.labels_loading = false;
                        if has_cached_suggestions {
                            dialog.labels_error = None;
                        } else {
                            dialog.labels.clear();
                            dialog.labels_error = Some(error);
                            dialog.selected_suggestion = 0;
                        }
                    }
                }
            },
            AppMsg::AssigneeSuggestionsLoaded { repo, result } => match result {
                Ok(assignees) => {
                    self.assignee_suggestions_cache
                        .insert(repo.clone(), assignees.clone());
                    self.mention_candidate_loading_repos.remove(&repo);
                    self.mention_candidate_errors.remove(&repo);
                    if let Some(dialog) = &mut self.assignee_dialog
                        && dialog.action == AssigneeAction::Assign
                        && dialog.item.repo == repo
                    {
                        dialog.suggestions_loading = false;
                        if dialog.suggestions != assignees {
                            dialog.suggestions = assignees.clone();
                        }
                        dialog.suggestions_error = None;
                        clamp_assignee_dialog_selection(dialog);
                        self.status = "assignee candidates loaded".to_string();
                    }
                    if let Some(dialog) = &mut self.item_edit_dialog
                        && dialog.item.repo == repo
                    {
                        dialog.assignee_suggestions_loading = false;
                        if dialog.assignee_suggestions != assignees {
                            dialog.assignee_suggestions = assignees.clone();
                        }
                        dialog.assignee_suggestions_error = None;
                        clamp_item_edit_assignee_selection(dialog);
                        self.status = "assignee candidates loaded".to_string();
                    }
                    if let Some(dialog) = &mut self.global_search_dialog
                        && dialog.repo.as_deref() == Some(repo.as_str())
                    {
                        dialog.assignees_loading = false;
                        dialog.assignee_candidates = assignees.clone();
                        dialog.author_candidates =
                            merge_candidate_lists(dialog.author_candidates.clone(), assignees);
                        dialog.assignees_error = None;
                        clamp_global_search_dialog_selection(dialog);
                    }
                }
                Err(error) => {
                    let has_cached_suggestions =
                        self.assignee_suggestions_cache.contains_key(&repo);
                    self.mention_candidate_loading_repos.remove(&repo);
                    if has_cached_suggestions {
                        self.mention_candidate_errors.remove(&repo);
                    } else {
                        self.mention_candidate_errors
                            .insert(repo.clone(), error.clone());
                    }
                    if let Some(dialog) = &mut self.assignee_dialog
                        && dialog.action == AssigneeAction::Assign
                        && dialog.item.repo == repo
                    {
                        dialog.suggestions_loading = false;
                        if has_cached_suggestions {
                            dialog.suggestions_error = None;
                            self.status =
                                "assignee candidates refresh failed; using cache".to_string();
                        } else {
                            dialog.suggestions.clear();
                            dialog.suggestions_error = Some(error.clone());
                            dialog.selected_suggestion = 0;
                            self.status = "assignee candidates unavailable".to_string();
                        }
                    }
                    if let Some(dialog) = &mut self.item_edit_dialog
                        && dialog.item.repo == repo
                    {
                        dialog.assignee_suggestions_loading = false;
                        if has_cached_suggestions {
                            dialog.assignee_suggestions_error = None;
                            self.status =
                                "assignee candidates refresh failed; using cache".to_string();
                        } else {
                            dialog.assignee_suggestions.clear();
                            dialog.assignee_suggestions_error = Some(error.clone());
                            dialog.selected_assignee_suggestion = 0;
                            self.status = "assignee candidates unavailable".to_string();
                        }
                    }
                    if let Some(dialog) = &mut self.global_search_dialog
                        && dialog.repo.as_deref() == Some(repo.as_str())
                    {
                        dialog.assignees_loading = false;
                        if has_cached_suggestions {
                            dialog.assignees_error = None;
                        } else {
                            dialog.assignee_candidates.clear();
                            dialog.assignees_error = Some(error);
                            dialog.selected_suggestion = 0;
                        }
                    }
                }
            },
            AppMsg::ReviewerSuggestionsLoaded { repo, result } => match result {
                Ok(reviewers) => {
                    self.reviewer_suggestions_cache
                        .insert(repo.clone(), reviewers.clone());
                    if let Some(dialog) = &mut self.reviewer_dialog
                        && dialog.item.repo == repo
                    {
                        dialog.suggestions_loading = false;
                        if dialog.suggestions != reviewers {
                            dialog.suggestions = reviewers;
                        }
                        dialog.suggestions_error = None;
                        clamp_reviewer_dialog_selection(dialog);
                        self.status = "reviewer candidates loaded".to_string();
                    }
                }
                Err(error) => {
                    let has_cached_suggestions =
                        self.reviewer_suggestions_cache.contains_key(&repo)
                            || self.assignee_suggestions_cache.contains_key(&repo);
                    if let Some(dialog) = &mut self.reviewer_dialog
                        && dialog.item.repo == repo
                    {
                        dialog.suggestions_loading = false;
                        if has_cached_suggestions {
                            dialog.suggestions_error = None;
                            self.status =
                                "reviewer candidates refresh failed; using cache".to_string();
                        } else {
                            dialog.suggestions.clear();
                            dialog.suggestions_error = Some(error);
                            dialog.selected_suggestion = 0;
                            self.status = "reviewer candidates unavailable".to_string();
                        }
                    }
                }
            },
            AppMsg::MentionUserSearchLoaded { query, result } => {
                self.mention_user_search_loading_queries.remove(&query);
                match result {
                    Ok(users) => {
                        self.mention_user_search_cache.insert(query.clone(), users);
                        self.mention_user_search_errors.remove(&query);
                    }
                    Err(error) => {
                        if self.mention_user_search_cache.contains_key(&query) {
                            self.mention_user_search_errors.remove(&query);
                        } else {
                            self.mention_user_search_errors.insert(query, error);
                        }
                    }
                }
            }
            AppMsg::IssueCreated { result } => {
                self.issue_creating = false;
                self.issue_dialog = None;
                match result {
                    Ok(item) => {
                        let number = item
                            .number
                            .map(|number| format!("#{number}"))
                            .unwrap_or_else(|| item.id.clone());
                        let inserted = self.insert_created_issue(item.clone());
                        self.status = if inserted {
                            format!("issue created: {number}")
                        } else {
                            format!("issue created: {number}; refresh to show it in a list")
                        };
                        self.message_dialog = Some(success_message_dialog(
                            "Issue Created",
                            format!("Created {number}: {}", item.title),
                        ));
                        self.clear_pending_issue_draft_local();
                        self.pending_issue_create = None;
                    }
                    Err(error) => {
                        let setup_dialog = setup_dialog_from_error(&error);
                        if self.setup_dialog.is_none() {
                            self.setup_dialog = setup_dialog;
                        }
                        if setup_dialog.is_none() {
                            self.restore_pending_issue_create_dialog();
                            self.message_dialog = Some(retryable_message_dialog(
                                "Issue Create Failed",
                                retryable_operation_error_body(&error),
                            ));
                        } else {
                            self.message_dialog = None;
                            self.restore_pending_issue_create_dialog();
                        }
                        self.status = "issue create failed".to_string();
                    }
                }
            }
            AppMsg::PullRequestCreated { result } => {
                self.pr_creating = false;
                self.pr_create_dialog = None;
                match result {
                    Ok(item) => {
                        let number = item
                            .number
                            .map(|number| format!("#{number}"))
                            .unwrap_or_else(|| item.id.clone());
                        let inserted = self.insert_created_pull_request(item.clone());
                        self.status = if inserted {
                            format!("pull request created: {number}")
                        } else {
                            format!("pull request created: {number}; refresh to show it in a list")
                        };
                        self.message_dialog = Some(success_message_dialog(
                            "Pull Request Created",
                            format!("Created {number}: {}", item.title),
                        ));
                        self.clear_pending_pr_create_draft_local();
                        self.pending_pr_create = None;
                    }
                    Err(error) => {
                        let setup_dialog = setup_dialog_from_error(&error);
                        if self.setup_dialog.is_none() {
                            self.setup_dialog = setup_dialog;
                        }
                        if setup_dialog.is_none() {
                            self.restore_pending_pr_create_dialog();
                            self.message_dialog = Some(retryable_message_dialog(
                                "Pull Request Create Failed",
                                retryable_operation_error_body(&error),
                            ));
                        } else {
                            self.message_dialog = None;
                            self.restore_pending_pr_create_dialog();
                        }
                        self.status = "pull request create failed".to_string();
                    }
                }
            }
            AppMsg::ReviewDraftCreated { item_id, result } => {
                self.review_submit_running = false;
                match result {
                    Ok(pending) => {
                        self.pending_reviews
                            .insert(item_id.clone(), pending.clone());
                        self.mark_action_hints_stale(item_id.clone());
                        self.details_stale.insert(item_id);
                        self.status = "pending review created".to_string();
                        self.message_dialog = Some(success_message_dialog(
                            "Pending Review Created",
                            "GitHub created a pending review. Press s to submit it or D to discard it.",
                        ));
                    }
                    Err(error) => {
                        let setup_dialog = setup_dialog_from_error(&error);
                        if self.setup_dialog.is_none() {
                            self.setup_dialog = setup_dialog;
                        }
                        if setup_dialog.is_none() {
                            self.message_dialog = Some(message_dialog(
                                "Pending Review Failed",
                                operation_error_body(&error),
                            ));
                        } else {
                            self.message_dialog = None;
                        }
                        self.status = "pending review create failed".to_string();
                    }
                }
            }
            AppMsg::ReviewSubmitted {
                item_id,
                event,
                result,
            } => {
                self.review_submit_running = false;
                match result {
                    Ok(()) => {
                        self.mark_action_hints_stale(item_id.clone());
                        self.details_stale.insert(item_id.clone());
                        self.status = format!("{}; refreshing", event.success_label());
                        self.message_dialog = Some(success_message_dialog(
                            "Review Submitted",
                            format!("GitHub accepted the {} review.", event.label()),
                        ));
                    }
                    Err(error) => {
                        let setup_dialog = setup_dialog_from_error(&error);
                        if self.setup_dialog.is_none() {
                            self.setup_dialog = setup_dialog;
                        }
                        if setup_dialog.is_none() {
                            self.message_dialog = Some(message_dialog(
                                "Review Failed",
                                operation_error_body(&error),
                            ));
                        } else {
                            self.message_dialog = None;
                        }
                        self.status = "review submit failed".to_string();
                    }
                }
            }
            AppMsg::PendingReviewSubmitted {
                item_id,
                review_id,
                event,
                result,
            } => {
                self.review_submit_running = false;
                match result {
                    Ok(()) => {
                        self.pending_reviews.remove(&item_id);
                        self.mark_action_hints_stale(item_id.clone());
                        self.details_stale.insert(item_id);
                        self.status = format!("{}; refreshing", event.success_label());
                        self.message_dialog = Some(success_message_dialog(
                            "Pending Review Submitted",
                            format!("GitHub submitted pending review {review_id}."),
                        ));
                    }
                    Err(error) => {
                        let setup_dialog = setup_dialog_from_error(&error);
                        if self.setup_dialog.is_none() {
                            self.setup_dialog = setup_dialog;
                        }
                        if setup_dialog.is_none() {
                            self.message_dialog = Some(message_dialog(
                                "Pending Review Submit Failed",
                                operation_error_body(&error),
                            ));
                        } else {
                            self.message_dialog = None;
                        }
                        self.status = "pending review submit failed".to_string();
                    }
                }
            }
            AppMsg::PendingReviewDiscarded {
                item_id,
                review_id,
                result,
            } => {
                self.review_submit_running = false;
                match result {
                    Ok(()) => {
                        self.pending_reviews.remove(&item_id);
                        self.mark_action_hints_stale(item_id.clone());
                        self.details_stale.insert(item_id);
                        self.status = "pending review discarded; refreshing".to_string();
                        self.message_dialog = Some(success_message_dialog(
                            "Pending Review Discarded",
                            format!("GitHub discarded pending review {review_id}."),
                        ));
                    }
                    Err(error) => {
                        let setup_dialog = setup_dialog_from_error(&error);
                        if self.setup_dialog.is_none() {
                            self.setup_dialog = setup_dialog;
                        }
                        if setup_dialog.is_none() {
                            self.message_dialog = Some(message_dialog(
                                "Pending Review Discard Failed",
                                operation_error_body(&error),
                            ));
                        } else {
                            self.message_dialog = None;
                        }
                        self.status = "pending review discard failed".to_string();
                    }
                }
            }
            AppMsg::PrActionFinished {
                item_id,
                item_kind,
                action,
                merge_method,
                result,
            } => {
                self.pr_action_running = false;
                self.pr_action_dialog = None;
                match result {
                    Ok(()) => {
                        self.details_stale.insert(item_id.clone());
                        self.mark_action_hints_stale(item_id.clone());
                        self.diffs.remove(&item_id);
                        self.mark_item_after_pr_action(&item_id, action);
                        self.status = if action == PrAction::Merge {
                            format!(
                                "pull request merged using {}; refreshing",
                                merge_method.unwrap_or_default().label()
                            )
                        } else {
                            pr_action_success_status(action, item_kind)
                        };
                        self.message_dialog = Some(success_message_dialog(
                            pr_action_success_title(action, item_kind),
                            pr_action_success_body(action, item_kind),
                        ));
                    }
                    Err(error) => {
                        let setup_dialog = setup_dialog_from_error(&error);
                        if self.setup_dialog.is_none() {
                            self.setup_dialog = setup_dialog;
                        }
                        if setup_dialog.is_none() {
                            self.message_dialog = Some(message_dialog(
                                pr_action_error_title(action, item_kind),
                                pr_action_error_body(&error),
                            ));
                        } else {
                            self.message_dialog = None;
                        }
                        self.status = if action == PrAction::Merge {
                            format!(
                                "pull request {} merge failed",
                                merge_method.unwrap_or_default().label()
                            )
                        } else {
                            pr_action_error_status(action, item_kind)
                        };
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
                            pr_action_success_title(PrAction::Checkout, ItemKind::PullRequest),
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
                                pr_action_error_title(PrAction::Checkout, ItemKind::PullRequest),
                                pr_action_error_body(&error),
                            ));
                        } else {
                            self.message_dialog = None;
                        }
                        self.status =
                            pr_action_error_status(PrAction::Checkout, ItemKind::PullRequest);
                    }
                }
            }
            AppMsg::MilestonesLoaded { item_id, result } => {
                if let Some(dialog) = self.milestone_dialog.as_mut()
                    && dialog.item.id == item_id
                    && !self.milestone_action_running
                {
                    match result {
                        Ok(milestones) => {
                            dialog.state = MilestoneDialogState::Loaded(milestones);
                            reset_milestone_dialog_selection(dialog);
                            self.status = "choose milestone".to_string();
                        }
                        Err(error) => {
                            let setup_dialog = setup_dialog_from_error(&error);
                            if self.setup_dialog.is_none() {
                                self.setup_dialog = setup_dialog;
                            }
                            dialog.state = MilestoneDialogState::Error(error);
                            self.status = "milestone load failed".to_string();
                        }
                    }
                }
            }
            AppMsg::MilestoneChanged {
                item_id,
                milestone,
                result,
            } => {
                self.milestone_action_running = false;
                self.milestone_dialog = None;
                match result {
                    Ok(()) => {
                        self.details_stale.insert(item_id.clone());
                        self.mark_item_milestone(&item_id, milestone.as_ref());
                        self.status = match milestone {
                            Some(_) => "milestone changed; refreshing".to_string(),
                            None => "milestone cleared; refreshing".to_string(),
                        };
                        self.message_dialog = Some(success_message_dialog(
                            "Milestone Changed",
                            "GitHub accepted the milestone update. Refreshing details.",
                        ));
                    }
                    Err(error) => {
                        let setup_dialog = setup_dialog_from_error(&error);
                        if self.setup_dialog.is_none() {
                            self.setup_dialog = setup_dialog;
                        }
                        if setup_dialog.is_none() {
                            self.message_dialog = Some(message_dialog(
                                "Milestone Failed",
                                operation_error_body(&error),
                            ));
                        } else {
                            self.message_dialog = None;
                        }
                        self.status = "milestone change failed".to_string();
                    }
                }
            }
            AppMsg::ItemEdited { item_id, result } => {
                self.item_edit_running = false;
                match result {
                    Ok(update) => {
                        self.apply_item_edit_update(&item_id, update);
                        self.details_stale.insert(item_id);
                        self.item_edit_dialog = None;
                        self.status = "item updated".to_string();
                        self.message_dialog = Some(success_message_dialog(
                            "Item Updated",
                            "GitHub accepted the item update.",
                        ));
                    }
                    Err(error) => {
                        let setup_dialog = setup_dialog_from_error(&error);
                        if self.setup_dialog.is_none() {
                            self.setup_dialog = setup_dialog;
                        }
                        if setup_dialog.is_none() {
                            self.message_dialog = Some(message_dialog(
                                "Item Update Failed",
                                operation_error_body(&error),
                            ));
                        } else {
                            self.message_dialog = None;
                        }
                        self.status = "item update failed".to_string();
                    }
                }
            }
            AppMsg::AssigneesUpdated {
                item_id,
                action,
                result,
            } => {
                self.assignee_action_running = false;
                self.assignee_dialog = None;
                match result {
                    Ok(updated_item) => {
                        self.details_stale.insert(item_id.clone());
                        self.replace_item(&item_id, updated_item);
                        self.status = match action {
                            AssigneeAction::Assign => "assignee added".to_string(),
                            AssigneeAction::Unassign => "assignee removed".to_string(),
                        };
                        self.message_dialog = Some(success_message_dialog(
                            assignee_action_success_title(action),
                            assignee_action_success_body(action),
                        ));
                    }
                    Err(error) => {
                        let setup_dialog = setup_dialog_from_error(&error);
                        if self.setup_dialog.is_none() {
                            self.setup_dialog = setup_dialog;
                        }
                        if setup_dialog.is_none() {
                            self.message_dialog = Some(message_dialog(
                                assignee_action_error_title(action),
                                operation_error_body(&error),
                            ));
                        } else {
                            self.message_dialog = None;
                        }
                        self.status = match action {
                            AssigneeAction::Assign => "assign failed".to_string(),
                            AssigneeAction::Unassign => "unassign failed".to_string(),
                        };
                    }
                }
            }
            AppMsg::ReviewerActionFinished {
                item_id,
                action,
                reviewers,
                result,
            } => {
                self.reviewer_action_running = false;
                self.reviewer_dialog = None;
                match result {
                    Ok(()) => {
                        self.details_stale.insert(item_id.clone());
                        self.mark_action_hints_stale(item_id.clone());
                        self.status = match action {
                            ReviewerAction::Request => {
                                format!(
                                    "requested review from {}; refreshing",
                                    reviewers.join(", ")
                                )
                            }
                            ReviewerAction::Remove => {
                                format!(
                                    "removed review requests for {}; refreshing",
                                    reviewers.join(", ")
                                )
                            }
                        };
                        self.message_dialog = Some(success_message_dialog(
                            reviewer_action_success_title(action),
                            reviewer_action_success_body(action),
                        ));
                    }
                    Err(error) => {
                        let setup_dialog = setup_dialog_from_error(&error);
                        if self.setup_dialog.is_none() {
                            self.setup_dialog = setup_dialog;
                        }
                        if setup_dialog.is_none() {
                            self.message_dialog = Some(message_dialog(
                                reviewer_action_error_title(action),
                                operation_error_body(&error),
                            ));
                        } else {
                            self.message_dialog = None;
                        }
                        self.status = reviewer_action_error_status(action).to_string();
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
            AppMsg::NotificationDoneFinished { thread_id, result } => {
                self.notification_done_pending.remove(&thread_id);
                match result {
                    Ok(save_error) => {
                        let changed = self.apply_notification_done_local(&thread_id);
                        self.status = match (changed, save_error) {
                            (_, Some(error)) => {
                                format!("notification marked done; snapshot save failed: {error}")
                            }
                            (true, None) => "notification marked done".to_string(),
                            (false, None) => "notification done synced".to_string(),
                        };
                    }
                    Err(error) => {
                        let setup_dialog = setup_dialog_from_error(&error);
                        if self.setup_dialog.is_none() {
                            self.setup_dialog = setup_dialog;
                        }
                        self.status = format!(
                            "notification done sync failed: {}",
                            operation_error_body(&error)
                        );
                    }
                }
            }
            AppMsg::InboxMarkAllReadFinished { result } => match result {
                Ok(save_error) => {
                    let changed = self.apply_all_notifications_read_local();
                    self.status = match (changed, save_error) {
                        (_, Some(error)) => {
                            format!(
                                "all inbox notifications marked read; snapshot save failed: {error}"
                            )
                        }
                        (true, None) => "all inbox notifications marked read".to_string(),
                        (false, None) => "all inbox notifications already read".to_string(),
                    };
                }
                Err(error) => {
                    let setup_dialog = setup_dialog_from_error(&error);
                    if self.setup_dialog.is_none() {
                        self.setup_dialog = setup_dialog;
                    }
                    self.status = format!(
                        "mark all inbox read failed: {}",
                        operation_error_body(&error)
                    );
                }
            },
            AppMsg::InboxThreadActionFinished { action, result } => match result {
                Ok(()) => {
                    self.status = inbox_thread_action_success_status(action).to_string();
                }
                Err(error) => {
                    let setup_dialog = setup_dialog_from_error(&error);
                    if self.setup_dialog.is_none() {
                        self.setup_dialog = setup_dialog;
                    }
                    self.status = format!(
                        "{} failed: {}",
                        inbox_thread_action_label(action),
                        operation_error_body(&error)
                    );
                }
            },
            AppMsg::ItemSubscriptionUpdated {
                item_id,
                item_kind,
                action,
                result,
            } => match result {
                Ok(()) => {
                    let subscribed = matches!(action, ItemSubscriptionAction::Subscribe);
                    self.update_item_subscription_local(&item_id, subscribed);
                    self.details_stale.insert(item_id);
                    self.status = item_subscription_action_success_status(action, item_kind);
                }
                Err(error) => {
                    let setup_dialog = setup_dialog_from_error(&error);
                    if self.setup_dialog.is_none() {
                        self.setup_dialog = setup_dialog;
                    }
                    self.status = format!(
                        "{} failed: {}",
                        item_subscription_action_label(action),
                        operation_error_body(&error)
                    );
                }
            },
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
                self.invalidate_action_hints_for_sections(std::slice::from_ref(&section));
                self.replace_section_page(&section_key, section);
                self.section_page_loading = None;
                self.status = match (error.as_deref(), save_error) {
                    (None, None) => loaded_page_label
                        .map(|label| format!("loaded page {label}"))
                        .unwrap_or_else(|| "loaded result page".to_string()),
                    (Some(error), None) => refresh_error_status(1, Some(error)),
                    (_, Some(error)) => format!("snapshot save failed: {error}"),
                };
            }
            AppMsg::FilterSectionLoaded {
                section_key,
                section,
            } => {
                let error = section.error.clone();
                let filter_label = self.section_filter_label_for_key(&section_key);
                if self.setup_dialog.is_none() {
                    self.setup_dialog = error.as_deref().and_then(setup_dialog_from_error);
                }
                self.replace_section_page(&section_key, section);
                self.refreshing = false;
                self.section_page_loading = None;
                self.status = match (error.as_deref(), filter_label) {
                    (None, Some(filter)) if !filter.is_empty() => {
                        format!("filter applied: {filter}")
                    }
                    (None, _) => "filter cleared".to_string(),
                    (Some(error), _) => refresh_error_status(1, Some(error)),
                };
            }
            AppMsg::GlobalSearchFinished {
                query,
                mut sections,
            } => {
                let preferred_kind = self.global_search_preferred_kind;
                if let Some(kind) = preferred_kind {
                    sections.sort_by_key(|section| if section.kind == kind { 0 } else { 1 });
                }
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
                let first_result_section = preferred_kind
                    .and_then(|kind| {
                        sections
                            .iter()
                            .position(|section| section.kind == kind && !section.items.is_empty())
                    })
                    .or_else(|| {
                        sections
                            .iter()
                            .position(|section| !section.items.is_empty())
                    })
                    .unwrap_or(0);
                self.invalidate_action_hints_for_sections(&sections);
                self.replace_global_search_sections(sections);
                self.global_search_running = false;
                self.global_search_started_at = None;
                self.global_search_scope = None;
                self.global_search_preferred_kind = None;
                self.global_search_active = false;
                self.global_search_dialog = None;
                self.global_search_query = query.clone();
                self.filter_input_active = false;
                self.search_active = false;
                self.search_query.clear();
                self.active_view = global_search_view_key();
                self.set_current_section_position(first_result_section);
                self.set_current_selected_position(0);
                self.focus = FocusTarget::List;
                self.reset_or_restore_current_conversation_details_state();
                self.comment_dialog = None;
                self.label_dialog = None;
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

    fn dismiss_retryable_message_dialog(&mut self, cancel: bool) {
        self.message_dialog = None;
        if cancel {
            self.comment_dialog = None;
            self.issue_dialog = None;
            self.pr_create_dialog = None;
            self.pending_comment_submit = None;
            self.pending_issue_create = None;
            self.pending_pr_create = None;
            self.status = "retry cancelled".to_string();
        } else {
            self.status = "edit and retry".to_string();
        }
    }

    fn dismiss_expired_message_dialog(&mut self, now: Instant) -> bool {
        if self
            .message_dialog
            .as_ref()
            .and_then(|dialog| dialog.auto_close_at)
            .is_some_and(|deadline| now >= deadline)
        {
            self.message_dialog = None;
            true
        } else {
            false
        }
    }

    fn restore_pending_comment_submit_dialog(&mut self) {
        if let Some(pending) = self.pending_comment_submit.take() {
            self.comment_dialog = Some(pending.dialog);
        }
    }

    fn restore_pending_issue_create_dialog(&mut self) {
        if let Some(pending) = self.pending_issue_create.take() {
            self.issue_dialog = Some(pending.dialog);
        }
    }

    fn restore_pending_pr_create_dialog(&mut self) {
        if let Some(pending) = self.pending_pr_create.take() {
            self.pr_create_dialog = Some(pending.dialog);
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
        self.finish_details_visit(Instant::now());
        self.help_dialog = true;
        self.search_active = false;
        self.comment_search_active = false;
        self.global_search_active = false;
        self.command_palette = None;
        self.recent_items_dialog = None;
        self.top_menu_switcher = None;
        self.theme_switcher = None;
        self.cache_clear_dialog = None;
        self.reaction_dialog = None;
        self.filter_input_active = false;
        self.item_edit_dialog = None;
        self.status = "help".to_string();
    }

    fn dismiss_help_dialog(&mut self) {
        self.help_dialog = false;
        self.status = "help dismissed".to_string();
    }

    fn show_command_palette(&mut self) {
        self.finish_details_visit(Instant::now());
        self.command_palette = Some(CommandPalette::default());
        self.project_switcher = None;
        self.top_menu_switcher = None;
        self.theme_switcher = None;
        self.recent_items_dialog = None;
        self.project_add_dialog = None;
        self.project_remove_dialog = None;
        self.cache_clear_dialog = None;
        self.status = "command palette".to_string();
    }

    fn show_info_dialog(&mut self, config: &Config, paths: &Paths) {
        let body = runtime_info_body(self, config, paths);
        self.message_dialog = Some(info_message_dialog("Info", body));
        self.status = "info".to_string();
        debug!("info dialog opened");
    }

    fn dismiss_command_palette(&mut self) {
        self.command_palette = None;
        self.status = "command palette dismissed".to_string();
    }

    fn handle_command_palette_key(
        &mut self,
        key: KeyEvent,
        config: &mut Config,
        paths: &Paths,
        store: &SnapshotStore,
        tx: &UnboundedSender<AppMsg>,
        area: Option<Rect>,
    ) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.dismiss_command_palette();
                false
            }
            KeyCode::Enter => self.submit_command_palette_selection(config, paths, store, tx, area),
            KeyCode::Down | KeyCode::Tab => {
                self.move_command_palette_selection(1);
                false
            }
            KeyCode::Up | KeyCode::BackTab => {
                self.move_command_palette_selection(-1);
                false
            }
            KeyCode::PageDown => {
                self.move_command_palette_selection(8);
                false
            }
            KeyCode::PageUp => {
                self.move_command_palette_selection(-8);
                false
            }
            KeyCode::Backspace => {
                if let Some(palette) = &mut self.command_palette {
                    palette.query.pop();
                    palette.selected = 0;
                }
                false
            }
            KeyCode::Char(value)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                if let Some(palette) = &mut self.command_palette {
                    palette.query.push(value);
                    palette.selected = 0;
                }
                false
            }
            _ => false,
        }
    }

    fn move_command_palette_selection(&mut self, delta: isize) {
        let Some(query) = self
            .command_palette
            .as_ref()
            .map(|palette| palette.query.clone())
        else {
            return;
        };
        let commands = command_palette_commands(&self.command_palette_key);
        let len = self.command_palette_match_indices(&commands, &query).len();
        if let Some(palette) = &mut self.command_palette {
            palette.selected = move_wrapping(palette.selected, len, delta);
        }
    }

    fn submit_command_palette_selection(
        &mut self,
        config: &mut Config,
        paths: &Paths,
        store: &SnapshotStore,
        tx: &UnboundedSender<AppMsg>,
        area: Option<Rect>,
    ) -> bool {
        let Some(command) = self.selected_command_palette_command() else {
            self.status = "no matching command".to_string();
            return false;
        };

        self.remember_command_palette_selection(&command, Utc::now());
        self.command_palette = None;
        match command.action {
            PaletteAction::Key(key) => {
                handle_key_in_area_mut(self, key, config, paths, store, tx, area)
            }
            PaletteAction::Quit => true,
            PaletteAction::ShowInfo => {
                self.show_info_dialog(config, paths);
                false
            }
            PaletteAction::ShowHelp => {
                self.show_help_dialog();
                false
            }
            PaletteAction::ShowCommandPalette => {
                self.show_command_palette();
                false
            }
            PaletteAction::Refresh => {
                trigger_refresh(self, config, store, tx);
                false
            }
            PaletteAction::RecentItems => {
                self.show_recent_items_dialog();
                false
            }
            PaletteAction::SetColorTheme => {
                self.show_theme_switcher(config);
                false
            }
            PaletteAction::TopMenuSwitch => {
                self.show_top_menu_switcher();
                false
            }
            PaletteAction::SearchCurrentRepo => {
                self.start_global_search_input_with_store(Some(store), Some(tx));
                false
            }
            PaletteAction::SavedSearchFilter => {
                self.show_saved_search_dialog(config);
                false
            }
            PaletteAction::SwitchProject => {
                self.show_project_switcher();
                false
            }
            PaletteAction::ProjectAdd => {
                self.show_project_add_dialog();
                false
            }
            PaletteAction::ProjectRemove => {
                self.show_project_remove_dialog(config);
                false
            }
            PaletteAction::CopyGithubLink => {
                self.copy_github_link();
                false
            }
            PaletteAction::CopyContent => {
                self.copy_content();
                false
            }
            PaletteAction::ToggleMouseCapture => {
                self.toggle_mouse_capture();
                false
            }
            PaletteAction::OpenSelected => {
                self.open_selected();
                false
            }
            PaletteAction::ShowDiff => {
                self.show_diff();
                false
            }
            PaletteAction::ClearIgnoredItems => {
                self.clear_ignored_items();
                false
            }
            PaletteAction::ClearCache => {
                self.show_cache_clear_dialog();
                false
            }
            PaletteAction::InboxMarkDone => {
                self.mark_current_notification_done(store, tx);
                false
            }
            PaletteAction::InboxMarkAllRead => {
                self.mark_all_inbox_read(store, tx);
                false
            }
            PaletteAction::InboxThreadAction(action) => {
                self.start_inbox_thread_action(action, tx);
                false
            }
            PaletteAction::ItemSubscriptionAction(action) => {
                self.start_item_subscription_action(action, tx);
                false
            }
        }
    }

    fn selected_command_palette_command(&self) -> Option<PaletteCommand> {
        let palette = self.command_palette.as_ref()?;
        let commands = command_palette_commands(&self.command_palette_key);
        let matches = self.command_palette_match_indices(&commands, &palette.query);
        let selected = palette.selected.min(matches.len().saturating_sub(1));
        matches
            .get(selected)
            .and_then(|index| commands.get(*index))
            .cloned()
    }

    fn command_palette_match_indices(
        &self,
        commands: &[PaletteCommand],
        query: &str,
    ) -> Vec<usize> {
        let mut matches = command_palette_filtered_indices(commands, query);
        matches.sort_by(|left, right| {
            let left_selected_at = self.command_palette_selected_at(&commands[*left]);
            let right_selected_at = self.command_palette_selected_at(&commands[*right]);
            match (left_selected_at, right_selected_at) {
                (Some(left_selected_at), Some(right_selected_at)) => right_selected_at
                    .cmp(&left_selected_at)
                    .then_with(|| left.cmp(right)),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => left.cmp(right),
            }
        });
        matches
    }

    fn command_palette_selected_at(&self, command: &PaletteCommand) -> Option<DateTime<Utc>> {
        let id = command_palette_command_id(command);
        self.recent_commands
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.selected_at)
    }

    fn remember_command_palette_selection(
        &mut self,
        command: &PaletteCommand,
        selected_at: DateTime<Utc>,
    ) {
        let id = command_palette_command_id(command);
        self.recent_commands.retain(|item| item.id != id);
        self.recent_commands.push(RecentCommand { id, selected_at });
        self.recent_commands
            .sort_by_key(|item| std::cmp::Reverse(item.selected_at));
        self.recent_commands.truncate(MAX_RECENT_COMMANDS);
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
                    PrAction::Reopen => item.state = Some("open".to_string()),
                    PrAction::Approve => {}
                    PrAction::EnableAutoMerge => {}
                    PrAction::DisableAutoMerge => {}
                    PrAction::Checkout => {}
                    PrAction::RerunFailedChecks => {}
                    PrAction::UpdateBranch => {}
                    PrAction::ConvertToDraft => mark_item_draft(item),
                    PrAction::MarkReadyForReview => mark_item_ready_for_review(item),
                }
            }
        }
    }

    fn apply_label_update_local(&mut self, item_id: &str, action: &LabelAction) {
        for section in &mut self.sections {
            for item in &mut section.items {
                if item.id != item_id {
                    continue;
                }
                match action {
                    LabelAction::Add(label) => {
                        if !item.labels.iter().any(|existing| existing == label) {
                            item.labels.push(label.clone());
                        }
                    }
                    LabelAction::Remove(label) => {
                        item.labels.retain(|existing| existing != label);
                    }
                }
            }
        }
    }

    fn replace_item(&mut self, item_id: &str, updated_item: WorkItem) {
        for section in &mut self.sections {
            for item in &mut section.items {
                if item.id == item_id {
                    *item = updated_item.clone();
                }
            }
        }
    }

    fn apply_item_details_metadata(&mut self, item_id: &str, metadata: &ItemDetailsMetadata) {
        for section in &mut self.sections {
            for item in &mut section.items {
                if item.id != item_id {
                    continue;
                }
                if let Some(title) = &metadata.title {
                    item.title = title.clone();
                }
                item.body = metadata.body.clone();
                if let Some(author) = &metadata.author {
                    item.author = Some(author.clone());
                }
                if let Some(state) = &metadata.state {
                    item.state = Some(state.clone());
                }
                if let Some(url) = &metadata.url {
                    item.url = url.clone();
                }
                if metadata.created_at.is_some() {
                    item.created_at = metadata.created_at;
                }
                if metadata.updated_at.is_some() {
                    item.updated_at = metadata.updated_at;
                }
                if let Some(labels) = &metadata.labels {
                    item.labels = labels.clone();
                }
                if let Some(assignees) = &metadata.assignees {
                    item.assignees = assignees.clone();
                }
                if metadata.comments.is_some() {
                    item.comments = metadata.comments;
                }
                if let Some(subscription) = &metadata.viewer_subscription {
                    item.viewer_subscription = Some(subscription.clone());
                }
            }
        }
    }

    fn mark_item_milestone(&mut self, item_id: &str, milestone: Option<&Milestone>) {
        for section in &mut self.sections {
            for item in &mut section.items {
                if item.id == item_id {
                    item.milestone = milestone.cloned();
                }
            }
        }
    }

    fn invalidate_action_hints_for_sections(&mut self, sections: &[SectionSnapshot]) {
        let item_ids = sections
            .iter()
            .flat_map(|section| {
                section
                    .items
                    .iter()
                    .filter(|item| item.kind == ItemKind::PullRequest)
                    .map(|item| item.id.clone())
            })
            .collect::<Vec<_>>();
        for item_id in item_ids {
            self.mark_action_hints_stale(item_id);
        }
    }

    fn mark_action_hints_stale(&mut self, item_id: impl Into<String>) {
        let item_id = item_id.into();
        if self.action_hints.contains_key(&item_id) {
            self.action_hints_stale.insert(item_id);
        }
    }

    fn action_hints_state_is_loading(&self, item_id: &str) -> bool {
        matches!(
            self.action_hints.get(item_id),
            Some(ActionHintState::Loading)
        )
    }

    fn action_hints_state_exists(&self, item_id: &str) -> bool {
        self.action_hints.contains_key(item_id)
    }

    fn action_hints_can_refresh_in_background(&self, item_id: &str) -> bool {
        self.action_hints_state_exists(item_id) && !self.action_hints_state_is_loading(item_id)
    }

    fn action_hints_refresh_is_needed(&self, item_id: &str) -> bool {
        self.action_hints_stale.contains(item_id)
            && self.action_hints_can_refresh_in_background(item_id)
            && !self.action_hints_refreshing.contains(item_id)
    }

    fn action_hints_first_load_is_needed(&self, item_id: &str) -> bool {
        !self.action_hints_state_exists(item_id)
    }

    fn action_hints_load_should_start(&self, item_id: &str) -> bool {
        self.action_hints_first_load_is_needed(item_id)
            || self.action_hints_refresh_is_needed(item_id)
    }

    fn insert_created_issue(&mut self, item: WorkItem) -> bool {
        let active_view = self.active_view.clone();
        let item_id = item.id.clone();
        let mut target = None;
        let mut inserted = false;

        for section in &mut self.sections {
            if !created_issue_matches_section(section, &item, &active_view) {
                continue;
            }

            section.items.retain(|existing| existing.id != item_id);
            section.items.insert(0, item.clone());
            section.total_count = section.total_count.map(|count| count.saturating_add(1));
            let view = section_view_key(section);
            let key = section.key.clone();
            if target.is_none() || same_view_key(&view, &active_view) {
                target = Some((view, key));
            }
            inserted = true;
        }

        if let Some((view, section_key)) = target {
            self.active_view = view.clone();
            if let Some(section_position) = self.section_position_by_key(&view, &section_key) {
                self.set_current_section_position(section_position);
            }
            self.set_current_selected_position(0);
            self.focus = FocusTarget::Details;
            self.details
                .insert(item_id, DetailState::Loaded(Vec::new()));
            self.reset_or_restore_current_conversation_details_state();
        }

        self.clamp_positions();
        inserted
    }

    fn insert_created_pull_request(&mut self, item: WorkItem) -> bool {
        let active_view = self.active_view.clone();
        let item_id = item.id.clone();
        let mut target = None;
        let mut inserted = false;

        for section in &mut self.sections {
            if !created_pull_request_matches_section(section, &item, &active_view) {
                continue;
            }

            section.items.retain(|existing| existing.id != item_id);
            section.items.insert(0, item.clone());
            section.total_count = section.total_count.map(|count| count.saturating_add(1));
            let view = section_view_key(section);
            let key = section.key.clone();
            if target.is_none() || same_view_key(&view, &active_view) {
                target = Some((view, key));
            }
            inserted = true;
        }

        if let Some((view, section_key)) = target {
            self.active_view = view.clone();
            if let Some(section_position) = self.section_position_by_key(&view, &section_key) {
                self.set_current_section_position(section_position);
            }
            self.set_current_selected_position(0);
            self.focus = FocusTarget::Details;
            self.details
                .insert(item_id, DetailState::Loaded(Vec::new()));
            self.reset_or_restore_current_conversation_details_state();
        }

        self.clamp_positions();
        inserted
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

    fn mark_current_notification_done(
        &mut self,
        store: &SnapshotStore,
        tx: &UnboundedSender<AppMsg>,
    ) {
        let Some(thread_id) = self.current_inbox_thread_id() else {
            self.status = "select an inbox item to mark done".to_string();
            return;
        };

        self.mark_notification_done(thread_id, store, tx);
    }

    fn mark_notification_done(
        &mut self,
        thread_id: String,
        store: &SnapshotStore,
        tx: &UnboundedSender<AppMsg>,
    ) {
        if !self.notification_done_pending.insert(thread_id.clone()) {
            return;
        }

        self.status = "marking notification done".to_string();
        start_notification_done_sync(thread_id, store.clone(), tx.clone());
    }

    fn mark_all_inbox_read(&mut self, store: &SnapshotStore, tx: &UnboundedSender<AppMsg>) {
        self.status = "marking all inbox notifications read".to_string();
        start_inbox_mark_all_read_sync(store.clone(), tx.clone());
    }

    fn start_inbox_thread_action(
        &mut self,
        action: InboxThreadAction,
        tx: &UnboundedSender<AppMsg>,
    ) {
        let Some(thread_id) = self.current_inbox_thread_id() else {
            self.status = format!(
                "select an inbox item to {}",
                inbox_thread_action_label(action)
            );
            return;
        };

        self.status = inbox_thread_action_running_status(action).to_string();
        start_inbox_thread_action_sync(thread_id, action, tx.clone());
    }

    fn start_item_subscription_action(
        &mut self,
        action: ItemSubscriptionAction,
        tx: &UnboundedSender<AppMsg>,
    ) {
        let Some(item) = self.current_item().cloned() else {
            self.status = format!(
                "select an issue or pull request to {}",
                item_subscription_action_label(action)
            );
            return;
        };
        if !matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest) {
            self.status = "selected item is not an issue or pull request".to_string();
            return;
        }
        if item.number.is_none() {
            self.status = "selected item has no issue or pull request number".to_string();
            return;
        }

        self.status = item_subscription_action_running_status(action, item.kind);
        start_item_subscription_sync(item, action, tx.clone());
    }

    fn current_inbox_thread_id(&self) -> Option<String> {
        let section = self.current_section()?;
        if !matches!(section.kind, SectionKind::Notifications) {
            return None;
        }
        self.current_item().map(|item| item.id.clone())
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

    fn apply_notification_done_local(&mut self, thread_id: &str) -> bool {
        let mut changed = false;
        for section in &mut self.sections {
            changed |= mark_notification_done_in_section(section, thread_id);
        }
        if changed {
            self.clamp_positions();
        }
        changed
    }

    fn apply_all_notifications_read_local(&mut self) -> bool {
        let mut changed = false;
        let last_read_at = Utc::now();
        for section in &mut self.sections {
            changed |= mark_all_notifications_read_in_section(section, last_read_at);
        }
        if changed {
            self.clamp_positions();
        }
        changed
    }

    fn update_item_reactions(&mut self, item_id: &str, reactions: ReactionSummary) {
        for section in &mut self.sections {
            for item in &mut section.items {
                if item.id == item_id {
                    item.reactions = reactions.clone();
                }
            }
        }
    }

    fn update_item_subscription_local(&mut self, item_id: &str, subscribed: bool) {
        let state = if subscribed {
            "SUBSCRIBED"
        } else {
            "UNSUBSCRIBED"
        };
        for section in &mut self.sections {
            for item in &mut section.items {
                if item.id == item_id {
                    item.viewer_subscription = Some(state.to_string());
                }
            }
        }
    }

    fn apply_comment_fetch_result_metadata(&mut self, item_id: &str, result: &CommentFetchResult) {
        if let Some(metadata) = &result.item_metadata {
            self.apply_item_details_metadata(item_id, metadata);
        }
        if let Some(reactions) = &result.item_reactions {
            self.update_item_reactions(item_id, reactions.clone());
        }
        if let Some(milestone) = &result.item_milestone {
            self.mark_item_milestone(item_id, milestone.as_ref());
        }
    }

    fn remember_details_synced_at(&mut self, item_id: &str, result: &CommentFetchResult) {
        let metadata_updated_at = result
            .item_metadata
            .as_ref()
            .and_then(|metadata| metadata.updated_at);
        let synced_at = self
            .item_updated_at_by_id(item_id)
            .into_iter()
            .chain(metadata_updated_at)
            .max();
        if let Some(synced_at) = synced_at {
            self.details_synced_at
                .insert(item_id.to_string(), synced_at);
        }
        self.details_refreshed_at
            .insert(item_id.to_string(), Utc::now());
    }

    fn remember_optimistic_comment(&mut self, item_id: &str, comment: &CommentPreview) {
        let Some(comment_id) = comment.id else {
            return;
        };
        self.optimistic_comment_ids
            .entry(item_id.to_string())
            .or_default()
            .insert(comment_id);
    }

    fn merge_optimistic_comments(&mut self, item_id: &str, comments: &mut Vec<CommentPreview>) {
        let Some(tracked_ids) = self.optimistic_comment_ids.get(item_id).cloned() else {
            return;
        };

        let fetched_ids = comments
            .iter()
            .filter_map(|comment| comment.id)
            .collect::<HashSet<_>>();
        let tracking_empty = if let Some(ids) = self.optimistic_comment_ids.get_mut(item_id) {
            ids.retain(|id| !fetched_ids.contains(id));
            ids.is_empty()
        } else {
            false
        };
        if tracking_empty {
            self.optimistic_comment_ids.remove(item_id);
            return;
        }

        let missing_ids = tracked_ids
            .difference(&fetched_ids)
            .copied()
            .collect::<HashSet<_>>();
        if missing_ids.is_empty() {
            return;
        }

        let Some(DetailState::Loaded(current_comments)) = self.details.get(item_id) else {
            return;
        };
        comments.extend(current_comments.iter().filter_map(|comment| {
            comment
                .id
                .filter(|id| missing_ids.contains(id))
                .map(|_| comment.clone())
        }));
        comments.sort_by_key(|comment| comment.created_at);
    }

    fn item_updated_at_by_id(&self, item_id: &str) -> Option<DateTime<Utc>> {
        self.sections
            .iter()
            .flat_map(|section| section.items.iter())
            .find(|item| item.id == item_id)
            .and_then(|item| item.updated_at)
    }

    fn append_local_comment(&mut self, item_id: &str, comment: CommentPreview) -> usize {
        let comments = match self.details.entry(item_id.to_string()) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                if !matches!(entry.get(), DetailState::Loaded(_)) {
                    entry.insert(DetailState::Loaded(Vec::new()));
                }
                match entry.into_mut() {
                    DetailState::Loaded(comments) => comments,
                    _ => unreachable!("detail state was normalized to Loaded"),
                }
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                match entry.insert(DetailState::Loaded(Vec::new())) {
                    DetailState::Loaded(comments) => comments,
                    _ => unreachable!("inserted detail state is Loaded"),
                }
            }
        };

        if let Some(comment_id) = comment.id
            && let Some(index) = comments
                .iter()
                .position(|existing| existing.id == Some(comment_id))
        {
            comments[index] = comment;
            return index;
        }

        comments.push(comment);
        comments.len().saturating_sub(1)
    }

    fn replace_section_page(&mut self, section_key: &str, refreshed: SectionSnapshot) {
        let was_current = self
            .current_section()
            .is_some_and(|section| section.key == section_key);
        if was_current {
            self.save_current_conversation_details_state();
        }
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
                self.reset_or_restore_current_conversation_details_state();
                self.comment_dialog = None;
                self.label_dialog = None;
                self.issue_dialog = None;
                self.pr_action_dialog = None;
                self.item_edit_dialog = None;
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
            filters: self.effective_filters_for_section(section),
            page: next_page,
            page_size,
            total_pages,
            total_is_capped,
        })
    }

    fn base_filters_for_section(&self, section: &SectionSnapshot) -> String {
        self.base_section_filters
            .get(&section.key)
            .cloned()
            .unwrap_or_else(|| section.filters.clone())
    }

    fn remember_base_filters(&mut self, section: &SectionSnapshot) {
        if section.error.is_none() {
            self.base_section_filters
                .insert(section.key.clone(), section.filters.clone());
        }
    }

    fn should_preserve_user_section_page(&self, refreshed: &SectionSnapshot) -> bool {
        if refreshed.error.is_some() {
            return false;
        }

        if self
            .section_page_loading
            .as_ref()
            .is_some_and(|loading| loading.section_key == refreshed.key)
        {
            return true;
        }

        self.sections
            .iter()
            .find(|section| section.key == refreshed.key)
            .is_some_and(|section| section.page > 1 && refreshed.page <= 1)
    }

    fn effective_filters_for_section(&self, section: &SectionSnapshot) -> String {
        if let Some(override_filter) = self.section_filter_overrides.get(&section.key) {
            return override_filter.filters.clone();
        }

        let base_filters = self.base_filters_for_section(section);
        self.quick_filters
            .get(&section.key)
            .map(|filter| quick_filter_query(&base_filters, filter))
            .unwrap_or(base_filters)
    }

    fn has_active_section_filter(&self, section_key: &str) -> bool {
        self.quick_filters.contains_key(section_key)
            || self.section_filter_overrides.contains_key(section_key)
    }

    fn current_filter_label(&self) -> Option<String> {
        self.current_section()
            .and_then(|section| self.quick_filter_label_for_section(section))
    }

    fn quick_filter_label_for_section(&self, section: &SectionSnapshot) -> Option<String> {
        self.section_filter_label_for_key(&section.key)
    }

    fn section_filter_label_for_key(&self, section_key: &str) -> Option<String> {
        self.section_filter_overrides
            .get(section_key)
            .map(|filter| filter.display.clone())
            .or_else(|| {
                self.quick_filters
                    .get(section_key)
                    .map(QuickFilter::display)
            })
            .filter(|label| !label.is_empty())
    }

    fn replace_global_search_sections(&mut self, sections: Vec<SectionSnapshot>) {
        let search_view = global_search_view_key();
        self.sections
            .retain(|section| section_view_key(section) != search_view);
        self.base_section_filters
            .retain(|key, _| !key.starts_with("search:"));
        self.quick_filters
            .retain(|key, _| !key.starts_with("search:"));
        self.section_filter_overrides
            .retain(|key, _| !key.starts_with("search:"));
        for section in &sections {
            self.base_section_filters
                .insert(section.key.clone(), section.filters.clone());
        }
        self.sections.extend(sections);
    }

    fn is_global_search_results_view(&self) -> bool {
        same_view_key(&self.active_view, &global_search_view_key())
    }

    fn leave_global_search_results(&mut self) {
        if !self.is_global_search_results_view() {
            return;
        }
        self.save_current_conversation_details_state();

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
        self.global_search_dialog = None;
        self.global_search_query.clear();
        self.search_active = false;
        self.search_query.clear();
        self.comment_search_active = false;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.focus = FocusTarget::List;
        self.status = "search results cleared".to_string();
        self.clamp_positions();
        self.reset_or_restore_current_conversation_details_state();
    }

    fn section_view_exists(&self, view: &str) -> bool {
        self.sections
            .iter()
            .any(|section| same_view_key(&section_view_key(section), view))
    }

    fn ensure_current_details_loading(&mut self, tx: &UnboundedSender<AppMsg>) -> bool {
        let Some(item) = self.current_item().cloned() else {
            self.pending_details_load = None;
            return false;
        };
        if !item_supports_comments_refresh(&item) {
            self.pending_details_load = None;
            return false;
        }
        if !self.details_load_needed(&item) {
            self.pending_details_load = None;
            return false;
        }
        if !self.details_load_ready(&item.id) {
            return false;
        }

        self.pending_details_load = None;
        let mut started = false;
        if self.start_comments_load_if_needed(&item) {
            start_comments_load(item.clone(), tx.clone());
            started = true;
        }
        if self.start_action_hints_load_if_needed(&item) {
            start_action_hints_load(item, tx.clone());
            started = true;
        }
        started
    }

    fn ensure_current_comments_auto_refresh(&mut self, tx: &UnboundedSender<AppMsg>) -> bool {
        let Some(item) = self.current_item().cloned() else {
            return false;
        };
        if self.start_comments_auto_refresh_if_due(&item, Instant::now()) {
            start_comments_load(item, tx.clone());
        }
        false
    }

    fn details_load_needed(&self, item: &WorkItem) -> bool {
        self.comments_load_needed(item) || self.action_hints_load_needed(item)
    }

    fn comments_load_needed(&self, item: &WorkItem) -> bool {
        !self.details.contains_key(&item.id)
            || self.details_stale.contains(&item.id)
            || self.details_cache_outdated(item)
    }

    fn details_cache_outdated(&self, item: &WorkItem) -> bool {
        if !self.details.contains_key(&item.id) {
            return false;
        }
        let Some(updated_at) = item.updated_at else {
            return false;
        };
        self.details_synced_at
            .get(&item.id)
            .map(|synced_at| updated_at > *synced_at)
            .unwrap_or_else(|| self.item_has_unseen_details(item))
    }

    fn action_hints_load_needed(&self, item: &WorkItem) -> bool {
        matches!(item.kind, ItemKind::PullRequest) && self.action_hints_load_should_start(&item.id)
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

    fn ensure_current_diff_loading(&mut self, tx: &UnboundedSender<AppMsg>) -> bool {
        if self.details_mode != DetailsMode::Diff {
            return false;
        }
        let Some(item) = self.current_item().cloned() else {
            return false;
        };
        if !matches!(item.kind, ItemKind::PullRequest) || item.number.is_none() {
            return false;
        }
        if self.diffs.contains_key(&item.id) {
            return false;
        }

        self.diffs.insert(item.id.clone(), DiffState::Loading);
        start_diff_load(item, tx.clone());
        true
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

    fn current_diff_file_details_scroll_entry(&self) -> Option<(String, u16)> {
        self.current_diff_file_details_scroll_key()
            .map(|key| (key, self.details_scroll))
    }

    fn current_diff_file_details_scroll_key(&self) -> Option<String> {
        if self.details_mode != DetailsMode::Diff {
            return None;
        }
        let item = self.current_item()?;
        let diff = match self.diffs.get(&item.id)? {
            DiffState::Loaded(diff) => diff,
            _ => return None,
        };
        if diff.files.is_empty() {
            return None;
        }
        let selected_file = self.selected_diff_file_index_for(&item.id, diff);
        Some(diff_file_details_scroll_key(
            &item.id,
            &diff.files[selected_file],
        ))
    }

    fn save_current_diff_file_details_scroll(&mut self) {
        if let Some((key, details_scroll)) = self.current_diff_file_details_scroll_entry() {
            self.diff_file_details_scroll.insert(key, details_scroll);
        }
    }

    fn selected_diff_file_details_scroll(&self, item_id: &str) -> Option<u16> {
        let diff = match self.diffs.get(item_id)? {
            DiffState::Loaded(diff) => diff,
            _ => return None,
        };
        if diff.files.is_empty() {
            return None;
        }
        let selected_file = self.selected_diff_file_index_for(item_id, diff);
        let key = diff_file_details_scroll_key(item_id, &diff.files[selected_file]);
        self.diff_file_details_scroll.get(&key).copied()
    }

    fn restore_selected_diff_file_details_scroll(&mut self, item_id: &str, fallback: u16) {
        self.details_scroll = self
            .selected_diff_file_details_scroll(item_id)
            .unwrap_or(fallback);
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
        self.start_comments_load_if_needed_at(item, Instant::now())
    }

    fn start_comments_load_if_needed_at(&mut self, item: &WorkItem, now: Instant) -> bool {
        let should_refresh =
            self.details_stale.remove(&item.id) || self.details_cache_outdated(item);
        if self.details.contains_key(&item.id) && !should_refresh {
            return false;
        }
        if should_refresh {
            self.details_refreshing.insert(item.id.clone());
        }
        if !self.details.contains_key(&item.id) {
            self.details.insert(item.id.clone(), DetailState::Loading);
        }
        self.comments_refresh_requested_at
            .insert(item.id.clone(), now);
        true
    }

    fn start_comments_auto_refresh_if_due(&mut self, item: &WorkItem, now: Instant) -> bool {
        if !self.comments_auto_refresh_due(item, now) {
            return false;
        }
        self.details_refreshing.insert(item.id.clone());
        self.comments_refresh_after.remove(&item.id);
        self.comments_refresh_requested_at
            .insert(item.id.clone(), now);
        true
    }

    fn comments_auto_refresh_due(&self, item: &WorkItem, now: Instant) -> bool {
        if self.focus != FocusTarget::Details
            || self.details_mode != DetailsMode::Conversation
            || !item_supports_comments_refresh(item)
        {
            return false;
        }
        if self.details_refreshing.contains(&item.id)
            || matches!(
                self.details.get(&item.id),
                Some(DetailState::Loading) | None
            )
        {
            return false;
        }
        if let Some(ready_at) = self.comments_refresh_after.get(&item.id) {
            return now >= *ready_at;
        }
        self.comments_refresh_requested_at
            .get(&item.id)
            .is_some_and(|requested_at| {
                now.duration_since(*requested_at) >= COMMENTS_AUTO_REFRESH_INTERVAL
            })
    }

    fn schedule_comments_refresh_after_post(&mut self, item_id: String) {
        self.comments_refresh_after
            .insert(item_id, Instant::now() + COMMENTS_POST_REFRESH_DELAY);
    }

    fn start_action_hints_load_if_needed(&mut self, item: &WorkItem) -> bool {
        if !self.action_hints_load_needed(item) {
            return false;
        }
        if self.action_hints_state_exists(&item.id) {
            self.action_hints_stale.remove(&item.id);
            self.action_hints_refreshing.insert(item.id.clone());
        } else {
            self.action_hints
                .insert(item.id.clone(), ActionHintState::Loading);
        }
        true
    }

    fn switch_view(&mut self, view: impl Into<String>) -> bool {
        self.switch_view_with_fallback(view, self.focus)
    }

    fn switch_project_view(&mut self, view: impl Into<String>) -> bool {
        self.switch_view_with_fallback(view, FocusTarget::Ghr)
    }

    fn switch_top_menu_view(&mut self, view: impl Into<String>) -> bool {
        let restored = self.switch_view_with_fallback(view, FocusTarget::Ghr);
        self.focus = FocusTarget::Ghr;
        restored
    }

    fn switch_view_with_fallback(
        &mut self,
        view: impl Into<String>,
        fallback_focus: FocusTarget,
    ) -> bool {
        self.remember_current_view_snapshot();
        let requested_view = view.into();
        let previous_view = self.active_view.clone();
        let previous_focus = self.focus;
        let target_snapshot = self
            .canonical_view_key(&requested_view)
            .or_else(|| view_supports_snapshot(&requested_view).then_some(requested_view.clone()))
            .and_then(|view| {
                self.view_snapshots
                    .iter()
                    .find(|(saved_view, _)| same_view_key(saved_view, &view))
                    .map(|(_, snapshot)| snapshot.clone())
            });
        self.active_view = requested_view;
        let active_view = self.active_view.clone();
        self.mark_repo_view_seen(&active_view);
        self.details_scroll = 0;
        self.selected_comment_index = 0;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.global_search_active = false;
        self.comment_search_active = false;
        self.comment_search_query.clear();
        self.clamp_positions();
        let restored = if let Some(snapshot) = target_snapshot {
            self.restore_view_snapshot(snapshot);
            true
        } else {
            self.details_mode = DetailsMode::Conversation;
            self.diff_return_state = None;
            self.focus = if matches!(fallback_focus, FocusTarget::Details)
                && self.current_item().is_none()
            {
                FocusTarget::List
            } else {
                fallback_focus
            };
            self.reset_or_restore_current_conversation_details_state();
            false
        };
        debug!(
            from = %previous_view,
            to = %self.active_view,
            from_focus = ?previous_focus,
            to_focus = ?self.focus,
            restored,
            "ui view switched"
        );
        restored
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
            debug!(
                from_index = current,
                to_index = next,
                delta,
                from = %self.active_view,
                to = %view.key,
                "ui view tab moved"
            );
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
        let previous_focus = self.focus;
        self.save_current_conversation_details_state();
        self.focus = FocusTarget::List;
        if self.details_mode != DetailsMode::Diff {
            self.reset_or_restore_current_conversation_details_state();
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
        debug!(
            from = ?previous_focus,
            to = ?self.focus,
            mode = ?self.details_mode,
            "ui focus changed"
        );
    }

    fn focus_ghr(&mut self) {
        let previous_focus = self.focus;
        self.save_current_conversation_details_state();
        self.focus = FocusTarget::Ghr;
        self.search_active = false;
        self.comment_search_active = false;
        self.global_search_active = false;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.status = "GHR focused".to_string();
        debug!(
            from = ?previous_focus,
            to = ?self.focus,
            "ui focus changed"
        );
    }

    fn focus_sections(&mut self) {
        let previous_focus = self.focus;
        self.save_current_conversation_details_state();
        self.focus = FocusTarget::Sections;
        self.search_active = false;
        self.comment_search_active = false;
        self.global_search_active = false;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.status = "Sections focused".to_string();
        self.clamp_positions();
        debug!(
            from = ?previous_focus,
            to = ?self.focus,
            "ui focus changed"
        );
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
        self.reset_or_restore_current_conversation_details_state();
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.global_search_active = false;
        self.comment_search_active = false;
        self.comment_search_query.clear();
        debug!(
            from_index = current,
            to_index = next,
            delta,
            view = %self.active_view,
            "ui section moved"
        );
    }

    fn select_section(&mut self, index: usize) {
        let len = self.visible_sections().len();
        if len == 0 {
            return;
        }
        self.save_current_conversation_details_state();
        let previous = self.current_section_position().min(len - 1);
        let next = index.min(len - 1);
        self.set_current_section_position(next);
        self.set_current_selected_position(0);
        self.clear_current_list_scroll_offset();
        self.focus = FocusTarget::Sections;
        self.reset_or_restore_current_conversation_details_state();
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.search_active = false;
        self.comment_search_active = false;
        self.comment_search_query.clear();
        self.global_search_active = false;
        self.status = "Sections focused".to_string();
        debug!(
            from_index = previous,
            to_index = next,
            view = %self.active_view,
            "ui section selected"
        );
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
        let next = move_bounded(current, len, delta);
        self.set_current_selected_position(next);
        self.clear_current_list_scroll_offset();
        self.reset_or_restore_current_conversation_details_state();
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.global_search_active = false;
        self.comment_search_active = false;
        self.comment_search_query.clear();
        debug!(
            from_index = current,
            to_index = next,
            delta,
            view = %self.active_view,
            section = self.current_section().map(|section| section.title.as_str()).unwrap_or(""),
            "ui list selection moved"
        );
    }

    fn set_selection(&mut self, index: usize) {
        self.save_current_conversation_details_state();
        let previous = self.current_selected_position();
        self.set_current_selected_position(index);
        self.clear_current_list_scroll_offset();
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.global_search_active = false;
        self.comment_search_active = false;
        self.comment_search_query.clear();
        self.clamp_positions();
        self.reset_or_restore_current_conversation_details_state();
        debug!(
            from_index = previous,
            to_index = self.current_selected_position(),
            view = %self.active_view,
            section = self.current_section().map(|section| section.title.as_str()).unwrap_or(""),
            "ui list selection set"
        );
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
        self.save_current_diff_file_details_scroll();
        if count == 0 {
            self.selected_diff_file.insert(item_id.clone(), 0);
            self.selected_diff_line.insert(item_id.clone(), 0);
            self.diff_mark.remove(&item_id);
            self.details_scroll = 0;
            self.status = "no diff files".to_string();
            return;
        }

        let next = index.min(count - 1);
        let previous = self.selected_diff_file.get(&item_id).copied().unwrap_or(0);
        self.selected_diff_file.insert(item_id.clone(), next);
        self.selected_diff_line.insert(item_id.clone(), 0);
        self.diff_mark.remove(&item_id);
        self.restore_selected_diff_file_details_scroll(&item_id, 0);
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
        debug!(
            item_id = %item_id,
            from_index = previous,
            to_index = next,
            position,
            count,
            "ui diff file selected"
        );
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

        if had_pending_mark {
            self.end_diff_mark();
            self.last_diff_click = None;
        } else if is_double_click {
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
                    self.status = format!(
                        "marking {}; click end line or press e",
                        target.location_label()
                    );
                } else {
                    self.status = format!(
                        "mark started at {}; click end line or press e",
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
        if direction == 0 {
            return;
        }

        let Some(area) = area else {
            if self.diff_line_page_would_cross_file(direction) {
                self.move_diff_file_from_page_boundary(direction, None);
                return;
            }
            self.move_diff_line(diff_line_page_delta(self, None, direction), None);
            return;
        };

        let details_area = details_area_for(self, area);
        let inner = block_inner(details_area);
        let page_height = usize::from(inner.height.max(1));
        let max_scroll = usize::from(max_details_scroll(self, details_area));
        let current_scroll = usize::from(self.details_scroll).min(max_scroll);
        if direction > 0 && current_scroll >= max_scroll {
            self.move_diff_file_from_page_boundary(direction, Some(area));
            return;
        }
        if direction < 0 && current_scroll == 0 {
            self.move_diff_file_from_page_boundary(direction, Some(area));
            return;
        }
        let next_scroll = if direction < 0 {
            current_scroll.saturating_sub(page_height)
        } else {
            current_scroll.saturating_add(page_height).min(max_scroll)
        };
        self.details_scroll = next_scroll.min(usize::from(u16::MAX)) as u16;
        self.select_first_visible_diff_line(area);
    }

    fn diff_line_page_would_cross_file(&self, direction: isize) -> bool {
        let Some(item_id) = self.current_item().map(|item| item.id.as_str()) else {
            return false;
        };
        let Some(targets) = self.current_diff_review_targets() else {
            return false;
        };
        if targets.is_empty() {
            return false;
        }
        let current = self
            .selected_diff_line
            .get(item_id)
            .copied()
            .unwrap_or(0)
            .min(targets.len() - 1);
        (direction > 0 && current == targets.len() - 1) || (direction < 0 && current == 0)
    }

    fn move_diff_file_from_page_boundary(&mut self, direction: isize, area: Option<Rect>) {
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

        let current = self
            .selected_diff_file
            .get(&item_id)
            .copied()
            .unwrap_or_else(|| order[0]);
        let current_position = order
            .iter()
            .position(|file_index| *file_index == current)
            .unwrap_or(0);
        let next_position = if direction > 0 {
            current_position
                .checked_add(1)
                .filter(|position| *position < order.len())
        } else {
            current_position.checked_sub(1)
        };
        let Some(next_position) = next_position else {
            self.set_diff_page_boundary_status(direction);
            return;
        };

        self.select_diff_file(order[next_position]);
        if direction > 0 {
            self.details_scroll = 0;
            if let Some(area) = area {
                self.select_first_visible_diff_line(area);
            } else {
                self.select_diff_line(0, None);
            }
        } else {
            self.scroll_diff_details_to_bottom(area);
        }
    }

    fn set_diff_page_boundary_status(&mut self, direction: isize) {
        if direction > 0 {
            self.status = "already at bottom of diff".to_string();
        } else {
            self.status = "already at top of diff".to_string();
        }
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

    fn scroll_conversation_details_to_top(&mut self) {
        self.select_details_body();
    }

    fn scroll_conversation_details_to_bottom(&mut self, area: Option<Rect>) {
        self.scroll_details_to_bottom(area);
        self.select_last_comment();
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
            self.reset_or_restore_current_conversation_details_state();
            self.comment_dialog = None;
            self.pr_action_dialog = None;
            self.global_search_active = false;
            self.comment_search_active = false;
            self.comment_search_query.clear();
        }
    }

    fn focus_details(&mut self) {
        let previous_focus = self.focus;
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
                self.mark_current_details_stale_if_unseen();
                self.mark_current_details_viewed();
            }
            self.sync_recent_details_visit(Instant::now());
            self.status = "details focused".to_string();
            debug!(
                from = ?previous_focus,
                to = ?self.focus,
                mode = ?self.details_mode,
                item_id = self.current_item().map(|item| item.id.as_str()).unwrap_or(""),
                "ui focus changed"
            );
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

    fn toggle_diff_inline_comments(&mut self) {
        self.diff_inline_comments_visible = !self.diff_inline_comments_visible;
        if self.diff_inline_comments_visible {
            self.revealed_diff_inline_comments.clear();
            self.status = "diff comments shown".to_string();
        } else {
            self.status = "diff comments hidden; click markers to reveal threads".to_string();
        }
    }

    fn toggle_revealed_diff_inline_comments(&mut self, comment_indices: &[usize]) {
        let Some(item_id) = self.current_item().map(|item| item.id.clone()) else {
            return;
        };
        if comment_indices.is_empty() {
            return;
        }
        if self.diff_inline_comments_visible {
            self.status = "diff comments already shown".to_string();
            return;
        }

        let revealed = self
            .revealed_diff_inline_comments
            .entry(item_id.clone())
            .or_default();
        let all_revealed = comment_indices
            .iter()
            .all(|comment_index| revealed.contains(comment_index));
        if all_revealed {
            for comment_index in comment_indices {
                revealed.remove(comment_index);
            }
            if revealed.is_empty() {
                self.revealed_diff_inline_comments.remove(&item_id);
            }
            self.status = "diff comment thread hidden".to_string();
        } else {
            revealed.extend(comment_indices.iter().copied());
            self.status = "diff comment thread shown".to_string();
        }
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
        let previous_mode = self.details_mode;
        let previous_focus = self.focus;
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
        self.restore_selected_diff_file_details_scroll(&item_id, self.details_scroll);
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
        debug!(
            item_id = %item_id,
            from_mode = ?previous_mode,
            to_mode = ?self.details_mode,
            from_focus = ?previous_focus,
            to_focus = ?self.focus,
            loading,
            "ui diff mode opened"
        );
    }

    fn leave_diff(&mut self) {
        let previous_focus = self.focus;
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
            self.restore_current_conversation_details_state();
            self.clamp_selected_comment();
            if self.focus == FocusTarget::Details && self.current_item().is_none() {
                self.focus = FocusTarget::List;
            }
        } else {
            self.focus = FocusTarget::Details;
            self.reset_or_restore_current_conversation_details_state();
        }
        self.status = "returned from diff".to_string();
        debug!(
            from_focus = ?previous_focus,
            to_focus = ?self.focus,
            "ui diff mode closed"
        );
    }

    fn save_current_diff_mode_state(&mut self) {
        if self.details_mode != DetailsMode::Diff {
            return;
        }
        let Some(item_id) = self.current_item().map(|item| item.id.clone()) else {
            return;
        };
        self.save_current_diff_file_details_scroll();
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

    fn focus_list(&mut self) {
        let previous_focus = self.focus;
        self.save_current_conversation_details_state();
        self.focus = FocusTarget::List;
        self.status = if self.details_mode == DetailsMode::Diff {
            "files focused".to_string()
        } else {
            "list focused".to_string()
        };
        debug!(
            from = ?previous_focus,
            to = ?self.focus,
            mode = ?self.details_mode,
            "ui focus changed"
        );
    }

    fn scroll_details(&mut self, delta: i16) {
        if delta < 0 {
            self.details_scroll = self.details_scroll.saturating_sub(delta.unsigned_abs());
        } else {
            self.details_scroll = self.details_scroll.saturating_add(delta as u16);
        }
        self.remember_current_conversation_details_position();
    }

    fn select_comment(&mut self, index: usize) {
        let previous = self.selected_comment_index;
        self.selected_comment_index = index;
        self.clamp_selected_comment();
        self.status = if self.comment_selection_cleared() {
            "no comment focused".to_string()
        } else {
            format!("comment {} focused", self.selected_comment_index + 1)
        };
        debug!(
            from_index = previous,
            to_index = self.selected_comment_index,
            "ui comment selected"
        );
        self.remember_current_conversation_details_position();
    }

    fn clear_selected_comment(&mut self) {
        self.selected_comment_index = NO_SELECTED_COMMENT_INDEX;
    }

    fn comment_selection_cleared(&self) -> bool {
        self.selected_comment_index == NO_SELECTED_COMMENT_INDEX
    }

    fn select_details_body(&mut self) {
        self.details_scroll = 0;
        self.select_details_body_without_scroll();
    }

    fn select_details_body_without_scroll(&mut self) {
        self.clear_selected_comment();
        self.status = self.details_body_focus_status();
        self.remember_current_conversation_details_position();
    }

    fn details_body_focus_status(&self) -> String {
        self.current_item()
            .map(|item| format!("{} details focused", item_kind_label(item.kind)))
            .unwrap_or_else(|| "details focused".to_string())
    }

    fn select_last_comment(&mut self) {
        let Some((index, len)) = self.current_comments().and_then(|comments| {
            let entries = comment_display_entries(comments);
            entries.last().map(|entry| (entry.index, entries.len()))
        }) else {
            self.selected_comment_index = 0;
            self.status = "details bottom; no comments".to_string();
            return;
        };

        self.selected_comment_index = index;
        self.status = format!("details bottom; comment {len}/{len} focused");
        self.remember_current_conversation_details_position();
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
        if self.comment_selection_cleared() {
            if delta > 0 {
                self.selected_comment_index = order[0];
                self.status = format!("comment 1/{} focused", order.len());
            } else {
                self.select_details_body();
            }
            return;
        }

        let Some(current_position) = order
            .iter()
            .position(|index| *index == self.selected_comment_index)
        else {
            let next_position = if delta < 0 { order.len() - 1 } else { 0 };
            self.selected_comment_index = order[next_position];
            self.status = format!("comment {}/{} focused", next_position + 1, order.len());
            return;
        };

        if current_position == 0 && delta < 0 {
            self.select_details_body();
            return;
        }

        let next_position = move_bounded(current_position, order.len(), delta);
        self.selected_comment_index = order[next_position];
        self.status = format!("comment {}/{} focused", next_position + 1, order.len());
    }

    fn move_comment_in_view(&mut self, delta: isize, area: Option<Rect>) {
        if self.details_mode == DetailsMode::Conversation
            && self.move_comment_search_match(delta, area)
        {
            return;
        }
        if self.move_hidden_diff_inline_comment(delta, area) {
            return;
        }
        if self.move_rendered_comment(delta, area) {
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

    fn move_hidden_diff_inline_comment(&mut self, delta: isize, area: Option<Rect>) -> bool {
        if self.details_mode != DetailsMode::Diff || self.diff_inline_comments_visible {
            return false;
        }
        let Some(area) = area else {
            return false;
        };
        let Some(item_id) = self.current_item().map(|item| item.id.clone()) else {
            self.status = "no comments".to_string();
            return true;
        };
        let details_area = details_area_for(self, area);
        let inner = block_inner(details_area);
        if inner.height == 0 {
            return true;
        }
        let document = build_details_document(self, inner.width);
        let marker_order = document
            .inline_comment_markers
            .iter()
            .filter(|marker| !marker.comment_indices.is_empty())
            .map(|marker| marker.comment_indices.clone())
            .collect::<Vec<_>>();
        if marker_order.is_empty() {
            self.status = "no comments".to_string();
            return true;
        }

        let revealed = self.revealed_diff_inline_comments.get(&item_id);
        let current_position = revealed.and_then(|revealed| {
            marker_order.iter().position(|comment_indices| {
                comment_indices.contains(&self.selected_comment_index)
                    && comment_indices
                        .iter()
                        .any(|comment_index| revealed.contains(comment_index))
            })
        });
        let next_position = match current_position {
            Some(0) if delta < 0 => {
                self.select_details_body();
                return true;
            }
            Some(position) => move_bounded(position, marker_order.len(), delta),
            None if delta < 0 => marker_order.len() - 1,
            None => 0,
        };

        let comment_indices = marker_order[next_position].clone();
        self.selected_comment_index = comment_indices[0];
        self.revealed_diff_inline_comments.insert(
            item_id,
            comment_indices.iter().copied().collect::<HashSet<_>>(),
        );
        self.status = format!(
            "comment {}/{} focused",
            next_position + 1,
            marker_order.len()
        );
        self.scroll_selected_comment_into_view(Some(area));
        true
    }

    fn move_rendered_comment(&mut self, delta: isize, area: Option<Rect>) -> bool {
        let Some(area) = area else {
            return false;
        };
        let details_area = details_area_for(self, area);
        let inner = block_inner(details_area);
        if inner.height == 0 {
            return false;
        }
        let document = build_details_document(self, inner.width);
        let order = document
            .comments
            .iter()
            .map(|comment| comment.index)
            .collect::<Vec<_>>();
        if order.is_empty() {
            self.status = "no comments".to_string();
            return true;
        }
        if self.comment_selection_cleared() {
            if delta > 0 {
                self.selected_comment_index = order[0];
                self.status = format!("comment 1/{} focused", order.len());
                self.scroll_selected_comment_into_view(Some(area));
            } else {
                self.select_details_body();
            }
            return true;
        }

        let Some(current_position) = order
            .iter()
            .position(|index| *index == self.selected_comment_index)
        else {
            let next_position = if delta < 0 { order.len() - 1 } else { 0 };
            self.selected_comment_index = order[next_position];
            self.status = format!("comment {}/{} focused", next_position + 1, order.len());
            self.scroll_selected_comment_into_view(Some(area));
            return true;
        };

        if current_position == 0 && delta < 0 {
            self.select_details_body();
            return true;
        }

        let next_position = move_bounded(current_position, order.len(), delta);
        self.selected_comment_index = order[next_position];
        self.status = format!("comment {}/{} focused", next_position + 1, order.len());
        self.scroll_selected_comment_into_view(Some(area));
        true
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
        self.remember_current_conversation_details_position();
    }

    fn scroll_selected_comment_into_view(&mut self, area: Option<Rect>) {
        let Some(area) = area else {
            self.remember_current_conversation_details_position();
            return;
        };
        let details_area = details_area_for(self, area);
        let inner = block_inner(details_area);
        if inner.height == 0 {
            self.remember_current_conversation_details_position();
            return;
        }
        let document = build_details_document(self, inner.width);
        let Some(region) = document.comment_region(self.selected_comment_index) else {
            self.remember_current_conversation_details_position();
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
        self.remember_current_conversation_details_position();
    }

    fn start_keyboard_reaction_dialog(&mut self, area: Option<Rect>) {
        if self.selected_comment_is_visible(area) {
            self.start_selected_comment_reaction_dialog();
        } else {
            self.start_item_reaction_dialog();
        }
    }

    fn selected_comment_is_visible(&self, area: Option<Rect>) -> bool {
        if self.details_mode != DetailsMode::Conversation {
            return false;
        }
        let Some(area) = area else {
            return self.current_selected_comment().is_some();
        };
        let details_area = details_area_for(self, area);
        let inner = block_inner(details_area);
        if inner.height == 0 {
            return false;
        }
        let document = build_details_document(self, inner.width);
        let Some(region) = document.comment_region(self.selected_comment_index) else {
            return false;
        };
        let viewport_start = usize::from(self.details_scroll);
        let viewport_end = viewport_start.saturating_add(usize::from(inner.height));
        let focus_line = region.focus_line();
        focus_line >= viewport_start && focus_line < viewport_end
    }

    fn handle_detail_action(
        &mut self,
        action: DetailAction,
        store: Option<&SnapshotStore>,
        tx: Option<&UnboundedSender<AppMsg>>,
    ) {
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
            DetailAction::ReactItem => self.start_item_reaction_dialog(),
            DetailAction::ReactComment(index) => {
                self.select_comment(index);
                self.start_comment_reaction_dialog(index);
            }
            DetailAction::CopyBlock(text) => self.copy_block_to_clipboard(&text),
            DetailAction::SubscribeItem => {
                if let Some(tx) = tx {
                    self.start_item_subscription_action(ItemSubscriptionAction::Subscribe, tx);
                } else {
                    self.status = "subscription action unavailable".to_string();
                }
            }
            DetailAction::UnsubscribeItem => {
                if let Some(tx) = tx {
                    self.start_item_subscription_action(ItemSubscriptionAction::Unsubscribe, tx);
                } else {
                    self.status = "subscription action unavailable".to_string();
                }
            }
            DetailAction::AddLabel => self.start_add_label_dialog_with_store(store, tx),
            DetailAction::RemoveLabel(label) => self.start_remove_label_dialog(label),
            DetailAction::AssignAssignee => {
                self.start_assignee_dialog_with_store(AssigneeAction::Assign, store, tx)
            }
            DetailAction::UnassignAssignee => {
                self.start_assignee_dialog_with_store(AssigneeAction::Unassign, store, tx)
            }
        }
    }

    fn start_item_reaction_dialog(&mut self) {
        let Some(item) = self.current_item().cloned() else {
            self.status = "nothing selected".to_string();
            return;
        };
        if !item.supports_reactions() {
            self.status = "reactions are available for issues and pull requests".to_string();
            return;
        }
        self.finish_details_visit(Instant::now());
        self.reaction_dialog = Some(ReactionDialog {
            target: ReactionTarget::Item,
            target_label: format!("{} #{} {}", item.repo, item.number.unwrap_or(0), item.title),
            selected: 0,
        });
        self.posting_reaction = false;
        self.comment_dialog = None;
        self.label_dialog = None;
        self.issue_dialog = None;
        self.review_submit_dialog = None;
        self.item_edit_dialog = None;
        self.milestone_dialog = None;
        self.pr_action_dialog = None;
        self.assignee_dialog = None;
        self.search_active = false;
        self.comment_search_active = false;
        self.global_search_active = false;
        self.focus = FocusTarget::Details;
        self.status = "reaction mode".to_string();
    }

    fn start_selected_comment_reaction_dialog(&mut self) {
        self.start_comment_reaction_dialog(self.selected_comment_index);
    }

    fn start_comment_reaction_dialog(&mut self, index: usize) {
        let Some(comment) = self
            .current_comments()
            .and_then(|comments| comments.get(index))
            .cloned()
        else {
            self.status = "no comment selected".to_string();
            return;
        };
        if comment.kind.is_activity() {
            self.status = "activity cannot be reacted to".to_string();
            return;
        }
        let Some(comment_id) = comment.id else {
            self.status = "comment has no GitHub id".to_string();
            return;
        };
        let target = if comment.review.is_some() {
            ReactionTarget::ReviewComment { index, comment_id }
        } else {
            ReactionTarget::IssueComment { index, comment_id }
        };
        self.finish_details_visit(Instant::now());
        self.reaction_dialog = Some(ReactionDialog {
            target,
            target_label: format!("comment by {}", comment.author),
            selected: 0,
        });
        self.posting_reaction = false;
        self.comment_dialog = None;
        self.label_dialog = None;
        self.issue_dialog = None;
        self.review_submit_dialog = None;
        self.item_edit_dialog = None;
        self.milestone_dialog = None;
        self.pr_action_dialog = None;
        self.assignee_dialog = None;
        self.search_active = false;
        self.comment_search_active = false;
        self.global_search_active = false;
        self.focus = FocusTarget::Details;
        self.status = "reaction mode".to_string();
    }

    fn handle_reaction_dialog_key(&mut self, key: KeyEvent, tx: &UnboundedSender<AppMsg>) {
        if self.posting_reaction {
            return;
        }
        match key.code {
            KeyCode::Esc => {
                self.reaction_dialog = None;
                self.status = "reaction cancelled".to_string();
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => self.move_reaction_selection(1),
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => self.move_reaction_selection(-1),
            KeyCode::Enter => self.submit_selected_reaction(tx),
            KeyCode::Char(value) => {
                if let Some(reaction) = ReactionContent::from_digit(value) {
                    self.submit_reaction(reaction, tx);
                }
            }
            _ => {}
        }
    }

    fn move_reaction_selection(&mut self, delta: isize) {
        if let Some(dialog) = &mut self.reaction_dialog {
            dialog.selected = move_wrapping(dialog.selected, ReactionContent::ALL.len(), delta);
            let reaction = ReactionContent::ALL[dialog.selected];
            self.status = format!(
                "selected reaction {} {}",
                reaction.emoji(),
                reaction.label()
            );
        }
    }

    fn submit_selected_reaction(&mut self, tx: &UnboundedSender<AppMsg>) {
        let Some(dialog) = &self.reaction_dialog else {
            return;
        };
        let reaction = ReactionContent::ALL[dialog.selected.min(ReactionContent::ALL.len() - 1)];
        self.submit_reaction(reaction, tx);
    }

    fn submit_reaction(&mut self, reaction: ReactionContent, tx: &UnboundedSender<AppMsg>) {
        let Some(item) = self.current_item().cloned() else {
            self.status = "nothing selected".to_string();
            return;
        };
        let Some(target) = self
            .reaction_dialog
            .as_ref()
            .map(|dialog| dialog.target.clone())
        else {
            return;
        };
        self.posting_reaction = true;
        self.status = format!("adding reaction {} {}", reaction.emoji(), reaction.label());
        start_reaction_submit(item, target, reaction, tx.clone());
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

    fn start_close_or_reopen_dialog(&mut self) {
        let Some(item) = self.current_item().cloned() else {
            self.status = "nothing selected".to_string();
            return;
        };
        let action = match close_or_reopen_action_for_item(&item) {
            Ok(action) => action,
            Err(message) => {
                self.status = message.to_string();
                return;
            }
        };
        self.start_item_action_dialog(item, action);
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
        self.start_item_action_dialog(item, action);
    }

    fn start_item_action_dialog(&mut self, item: WorkItem, action: PrAction) {
        if pr_action_requires_open_pull_request(action) && !item_is_open_pull_request(&item) {
            self.status = "selected pull request is not open".to_string();
            return;
        }
        let summary = match self.pr_action_dialog_summary(&item, action) {
            Ok(summary) => summary,
            Err((title, body, status)) => {
                self.status = status;
                self.message_dialog = Some(message_dialog(title, body));
                return;
            }
        };
        self.finish_details_visit(Instant::now());
        let item_kind = item.kind;
        self.search_active = false;
        self.global_search_active = false;
        self.comment_search_active = false;
        self.filter_input_active = false;
        self.comment_dialog = None;
        self.label_dialog = None;
        self.issue_dialog = None;
        self.reaction_dialog = None;
        self.review_submit_dialog = None;
        self.reviewer_dialog = None;
        self.item_edit_dialog = None;
        self.milestone_dialog = None;
        self.pr_action_dialog = Some(PrActionDialog {
            item,
            action,
            checkout: None,
            summary,
            merge_method: MergeMethod::default(),
        });
        self.pr_action_running = false;
        self.status = if action == PrAction::Merge {
            "confirm pull request merge (method: merge)".to_string()
        } else {
            pr_action_confirm_status(action, item_kind)
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

        self.finish_details_visit(Instant::now());
        self.search_active = false;
        self.global_search_active = false;
        self.comment_search_active = false;
        self.filter_input_active = false;
        self.comment_dialog = None;
        self.label_dialog = None;
        self.issue_dialog = None;
        self.reaction_dialog = None;
        self.review_submit_dialog = None;
        self.reviewer_dialog = None;
        self.item_edit_dialog = None;
        self.milestone_dialog = None;
        self.pr_action_dialog = Some(PrActionDialog {
            item,
            action: PrAction::Checkout,
            checkout: Some(PrCheckoutPlan { directory, branch }),
            summary: Vec::new(),
            merge_method: MergeMethod::default(),
        });
        self.pr_action_running = false;
        self.status = "confirm local pull request checkout".to_string();
    }

    fn start_pr_draft_ready_dialog(&mut self) {
        let Some(item) = self.current_item().cloned() else {
            self.status = "nothing selected".to_string();
            return;
        };
        if item.kind != ItemKind::PullRequest || item.number.is_none() {
            self.status = "selected item is not a pull request".to_string();
            return;
        }
        if !item_is_open_pull_request(&item) {
            self.status = "selected pull request is not open".to_string();
            return;
        }

        let action = if item_is_draft_pull_request(&item) {
            PrAction::MarkReadyForReview
        } else {
            PrAction::ConvertToDraft
        };
        self.start_pr_action_dialog(action);
    }

    fn pr_action_dialog_summary(
        &self,
        item: &WorkItem,
        action: PrAction,
    ) -> Result<PrActionDialogSummary, PrActionDialogSummaryError> {
        if action != PrAction::RerunFailedChecks {
            return Ok(Vec::new());
        }

        match self.action_hints.get(&item.id) {
            Some(ActionHintState::Loaded(hints)) => match &hints.checks {
                Some(checks) if checks.failed == 0 => Err((
                    "No Failed Checks",
                    "This pull request has no failed checks in the latest loaded action hints. Refresh if checks changed.".to_string(),
                    "no failed checks to rerun".to_string(),
                )),
                Some(checks) => {
                    let mut summary =
                        vec![("failed checks", format!("{} of {}", checks.failed, checks.total))];
                    if hints.failed_check_runs.is_empty() {
                        summary.push((
                            "workflow runs",
                            "not mapped in loaded hints; ghr will query latest PR checks before rerunning".to_string(),
                        ));
                    } else {
                        summary.push((
                            "workflow runs",
                            failed_check_runs_summary(&hints.failed_check_runs),
                        ));
                    }
                    Ok(summary)
                }
                None => Ok(vec![(
                    "checks",
                    "not loaded; ghr will query latest PR checks before rerunning".to_string(),
                )]),
            },
            Some(ActionHintState::Loading) | None => Ok(vec![(
                "checks",
                "loading; ghr will query latest PR checks before rerunning".to_string(),
            )]),
            Some(ActionHintState::Error(error)) => Ok(vec![(
                "checks",
                format!("hints unavailable ({error}); ghr will query latest PR checks"),
            )]),
        }
    }

    #[cfg(test)]
    fn start_item_edit_dialog(&mut self) {
        self.start_item_edit_dialog_with_store(None, None);
    }

    fn start_item_edit_dialog_with_store(
        &mut self,
        store: Option<&SnapshotStore>,
        tx: Option<&UnboundedSender<AppMsg>>,
    ) {
        let Some(item) = self.current_item().cloned() else {
            self.status = "nothing selected".to_string();
            return;
        };
        if !item_supports_metadata_edit(&item) {
            self.status = "selected item is not an issue or pull request".to_string();
            return;
        }
        self.finish_details_visit(Instant::now());
        self.search_active = false;
        self.comment_search_active = false;
        self.global_search_active = false;
        self.filter_input_active = false;
        self.comment_dialog = None;
        self.label_dialog = None;
        self.issue_dialog = None;
        self.reaction_dialog = None;
        self.review_submit_dialog = None;
        self.pr_action_dialog = None;
        self.milestone_dialog = None;
        self.assignee_dialog = None;
        self.reviewer_dialog = None;
        let repo = item.repo.clone();
        let cached_labels = self.label_suggestions_cache.get(&repo).cloned();
        let cached_assignees = self.assignee_suggestions_cache.get(&repo).cloned();
        let has_cached_labels = cached_labels.is_some();
        let has_cached_assignees = cached_assignees.is_some();
        self.item_edit_dialog = Some(ItemEditDialog {
            title: EditorText::from_text(&item.title),
            body: EditorText::from_text(item.body.clone().unwrap_or_default()),
            assignees: item.assignees.clone(),
            labels: item.labels.clone(),
            field: ItemEditField::Title,
            body_scroll: 0,
            assignee_input: String::new(),
            assignee_suggestions: cached_assignees.unwrap_or_default(),
            assignee_suggestions_loading: false,
            assignee_suggestions_error: None,
            selected_assignee_suggestion: 0,
            label_input: String::new(),
            label_suggestions: cached_labels.unwrap_or_default(),
            label_suggestions_loading: false,
            label_suggestions_error: None,
            selected_label_suggestion: 0,
            item,
        });
        self.item_edit_running = false;
        let labels_refreshing = tx
            .map(|tx| start_label_suggestions_load(repo.clone(), store.cloned(), tx.clone()))
            .unwrap_or(false);
        let assignees_refreshing = tx
            .map(|tx| start_assignee_suggestions_load(repo, store.cloned(), tx.clone()))
            .unwrap_or(false);
        if let Some(dialog) = &mut self.item_edit_dialog {
            dialog.label_suggestions_loading = labels_refreshing && !has_cached_labels;
            dialog.assignee_suggestions_loading = assignees_refreshing && !has_cached_assignees;
        }
        self.status = if labels_refreshing || assignees_refreshing {
            "editing item; loading candidates".to_string()
        } else {
            "editing item".to_string()
        };
    }

    fn handle_item_edit_dialog_key(
        &mut self,
        key: KeyEvent,
        tx: &UnboundedSender<AppMsg>,
        area: Option<Rect>,
    ) {
        let tx_for_submit = tx.clone();
        let tx_for_mentions = tx.clone();
        self.handle_item_edit_dialog_key_with_submit(key, area, move |pending| {
            start_item_edit(pending, tx_for_submit.clone());
        });
        self.ensure_mention_candidates_for_active_editor(None, &tx_for_mentions);
    }

    fn handle_item_edit_dialog_key_with_submit<F>(
        &mut self,
        key: KeyEvent,
        area: Option<Rect>,
        mut submit: F,
    ) where
        F: FnMut(PendingItemEdit),
    {
        if self.item_edit_running {
            self.status = "item edit already running".to_string();
            return;
        }
        if self.handle_active_mention_key(key) {
            return;
        }
        match key.code {
            KeyCode::Esc => {
                self.item_edit_dialog = None;
                self.item_edit_running = false;
                self.status = "edit cancelled".to_string();
            }
            KeyCode::Tab => self.move_item_edit_field(1),
            KeyCode::BackTab => self.move_item_edit_field(-1),
            KeyCode::PageDown => self.scroll_item_edit_body(6, area),
            KeyCode::PageUp => self.scroll_item_edit_body(-6, area),
            _ if is_comment_submit_key(key) => {
                if let Some(pending) = self.prepare_item_edit_submit() {
                    submit(pending);
                }
            }
            _ => self.handle_item_edit_field_key(key, area),
        }
    }

    fn move_item_edit_field(&mut self, delta: isize) {
        let Some(dialog) = &mut self.item_edit_dialog else {
            return;
        };
        dialog.field = dialog.field.next(delta);
        self.status = format!("editing {}", dialog.field.label());
    }

    fn handle_item_edit_field_key(&mut self, key: KeyEvent, area: Option<Rect>) {
        let Some(field) = self.item_edit_dialog.as_ref().map(|dialog| dialog.field) else {
            return;
        };
        match field {
            ItemEditField::Title => {
                if let Some(dialog) = &mut self.item_edit_dialog {
                    dialog.title.input_key(key, false);
                }
            }
            ItemEditField::Body => {
                if let Some(dialog) = &mut self.item_edit_dialog
                    && dialog.body.input_key(key, true)
                {
                    self.scroll_item_edit_body_to_cursor_in_area(area);
                }
            }
            ItemEditField::Assignees => self.handle_item_edit_assignee_key(key),
            ItemEditField::Labels => self.handle_item_edit_label_key(key),
        }
    }

    fn handle_item_edit_assignee_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.toggle_item_edit_assignee_from_input(),
            KeyCode::Up => self.move_item_edit_assignee_suggestion(-1),
            KeyCode::Down => self.move_item_edit_assignee_suggestion(1),
            KeyCode::Backspace => {
                if let Some(dialog) = &mut self.item_edit_dialog {
                    if dialog.assignee_input.is_empty() {
                        if let Some(login) = dialog.assignees.pop() {
                            self.status = format!("removed assignee {login}");
                        }
                    } else {
                        dialog.assignee_input.pop();
                        dialog.selected_assignee_suggestion = 0;
                        clamp_item_edit_assignee_selection(dialog);
                    }
                }
            }
            KeyCode::Delete
                if self
                    .item_edit_dialog
                    .as_ref()
                    .is_some_and(|dialog| dialog.assignee_input.is_empty()) =>
            {
                if let Some(dialog) = &mut self.item_edit_dialog
                    && let Some(login) = dialog.assignees.pop()
                {
                    self.status = format!("removed assignee {login}");
                }
            }
            KeyCode::Char(value) if !value.is_control() => {
                if let Some(dialog) = &mut self.item_edit_dialog {
                    dialog.assignee_input.push(value);
                    dialog.selected_assignee_suggestion = 0;
                    clamp_item_edit_assignee_selection(dialog);
                }
            }
            _ => {}
        }
    }

    fn handle_item_edit_label_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.toggle_item_edit_label_from_input(),
            KeyCode::Up => self.move_item_edit_label_suggestion(-1),
            KeyCode::Down => self.move_item_edit_label_suggestion(1),
            KeyCode::Backspace => {
                if let Some(dialog) = &mut self.item_edit_dialog {
                    if dialog.label_input.is_empty() {
                        if let Some(label) = dialog.labels.pop() {
                            self.status = format!("removed label {label}");
                        }
                    } else {
                        dialog.label_input.pop();
                        dialog.selected_label_suggestion = 0;
                        clamp_item_edit_label_selection(dialog);
                    }
                }
            }
            KeyCode::Delete
                if self
                    .item_edit_dialog
                    .as_ref()
                    .is_some_and(|dialog| dialog.label_input.is_empty()) =>
            {
                if let Some(dialog) = &mut self.item_edit_dialog
                    && let Some(label) = dialog.labels.pop()
                {
                    self.status = format!("removed label {label}");
                }
            }
            KeyCode::Char(value) if !value.is_control() => {
                if let Some(dialog) = &mut self.item_edit_dialog {
                    dialog.label_input.push(value);
                    dialog.selected_label_suggestion = 0;
                    clamp_item_edit_label_selection(dialog);
                }
            }
            _ => {}
        }
    }

    fn toggle_item_edit_assignee_from_input(&mut self) {
        let Some(dialog) = &mut self.item_edit_dialog else {
            return;
        };
        let mut assignees = parse_assignee_input(&dialog.assignee_input);
        if !assignees.is_empty()
            && !assignee_input_prefix(&dialog.assignee_input).is_empty()
            && let Some(selected) = selected_item_edit_assignee_suggestion(dialog)
            && let Some(last) = assignees.last_mut()
        {
            *last = selected;
        } else if assignees.is_empty()
            && let Some(selected) = selected_item_edit_assignee_suggestion(dialog)
        {
            assignees = vec![selected];
        }
        if assignees.is_empty() {
            self.status = "assignee login is empty".to_string();
            return;
        }
        let mut changed = false;
        for login in assignees {
            if let Some(index) = dialog
                .assignees
                .iter()
                .position(|existing| existing.eq_ignore_ascii_case(&login))
            {
                let removed = dialog.assignees.remove(index);
                self.status = format!("removed assignee {removed}");
                changed = true;
            } else {
                dialog.assignees.push(login.clone());
                self.status = format!("added assignee {login}");
                changed = true;
            }
        }
        if changed {
            dialog.assignees = dedupe_assignee_logins(dialog.assignees.clone());
            dialog.assignee_input.clear();
            dialog.selected_assignee_suggestion = 0;
            clamp_item_edit_assignee_selection(dialog);
        }
    }

    fn toggle_item_edit_label_from_input(&mut self) {
        let Some(dialog) = &mut self.item_edit_dialog else {
            return;
        };
        let label_prefix = label_completion_prefix(&dialog.label_input);
        let mut labels = if !label_prefix.is_empty()
            && let Some(selected) = selected_item_edit_label_suggestion(dialog)
        {
            parse_issue_labels(&replace_last_comma_component(
                &dialog.label_input,
                &selected,
            ))
        } else {
            parse_issue_labels(&dialog.label_input)
        };
        if labels.is_empty()
            && let Some(selected) = selected_item_edit_label_suggestion(dialog)
        {
            labels = vec![selected];
        }
        if labels.is_empty() {
            self.status = "label is empty".to_string();
            return;
        }
        let mut changed = false;
        for label in labels {
            if let Some(index) = dialog
                .labels
                .iter()
                .position(|existing| existing.eq_ignore_ascii_case(&label))
            {
                let removed = dialog.labels.remove(index);
                self.status = format!("removed label {removed}");
                changed = true;
            } else {
                dialog.labels.push(label.clone());
                self.status = format!("added label {label}");
                changed = true;
            }
        }
        if changed {
            dialog.labels = dedupe_label_names(dialog.labels.clone());
            dialog.label_input.clear();
            dialog.selected_label_suggestion = 0;
            clamp_item_edit_label_selection(dialog);
        }
    }

    fn move_item_edit_assignee_suggestion(&mut self, delta: isize) {
        let Some(dialog) = &mut self.item_edit_dialog else {
            return;
        };
        let count = item_edit_assignee_suggestion_matches(dialog).len();
        if count == 0 {
            self.status = "no assignee candidates match".to_string();
            return;
        }
        dialog.selected_assignee_suggestion =
            move_wrapping(dialog.selected_assignee_suggestion, count, delta);
        let login = selected_item_edit_assignee_suggestion(dialog).unwrap_or_default();
        self.status = format!("selected assignee candidate: {login}");
    }

    fn move_item_edit_label_suggestion(&mut self, delta: isize) {
        let Some(dialog) = &mut self.item_edit_dialog else {
            return;
        };
        let count = item_edit_label_suggestion_matches(dialog).len();
        if count == 0 {
            self.status = "no label candidates match".to_string();
            return;
        }
        dialog.selected_label_suggestion =
            move_wrapping(dialog.selected_label_suggestion, count, delta);
        let label = selected_item_edit_label_suggestion(dialog).unwrap_or_default();
        self.status = format!("selected label candidate: {label}");
    }

    fn scroll_item_edit_body(&mut self, delta: i16, area: Option<Rect>) {
        let Some(dialog) = &mut self.item_edit_dialog else {
            return;
        };
        let (width, height) = item_edit_body_editor_size(area, dialog.field);
        let max_scroll = max_comment_dialog_scroll(dialog.body.text(), width, height);
        if delta < 0 {
            dialog.body_scroll = dialog.body_scroll.saturating_sub(delta.unsigned_abs());
        } else {
            dialog.body_scroll = dialog.body_scroll.saturating_add(delta as u16);
        }
        dialog.body_scroll = dialog.body_scroll.min(max_scroll);
    }

    fn scroll_item_edit_body_to_cursor_in_area(&mut self, area: Option<Rect>) {
        if let Some(dialog) = &mut self.item_edit_dialog {
            let (width, height) = item_edit_body_editor_size(area, dialog.field);
            dialog.body_scroll = scroll_for_comment_dialog_cursor(
                dialog.body.text(),
                dialog.body.cursor_byte(),
                width,
                height,
                dialog.body_scroll,
            );
        }
    }

    fn prepare_item_edit_submit(&mut self) -> Option<PendingItemEdit> {
        let Some(dialog) = &self.item_edit_dialog else {
            return None;
        };
        let title = dialog.title.text().trim().to_string();
        if title.is_empty() {
            self.status = "title is empty".to_string();
            return None;
        }
        let body = dialog.body.text().trim().to_string();
        let labels = dedupe_label_names(dialog.labels.clone());
        let assignees = dedupe_assignee_logins(dialog.assignees.clone());
        let original_body = dialog.item.body.clone().unwrap_or_default();
        if title == dialog.item.title
            && body == original_body
            && same_names_ignore_case(&labels, &dialog.item.labels)
            && same_names_ignore_case(&assignees, &dialog.item.assignees)
        {
            self.status = "no item changes to save".to_string();
            return None;
        }
        self.item_edit_running = true;
        self.status = "updating item".to_string();
        Some(PendingItemEdit {
            item: dialog.item.clone(),
            title,
            body,
            labels,
            assignees,
        })
    }

    fn apply_item_edit_update(&mut self, item_id: &str, update: ItemEditUpdate) -> bool {
        let mut changed = false;
        for section in &mut self.sections {
            for item in &mut section.items {
                if item.id != item_id {
                    continue;
                }
                item.title = update.title.clone();
                item.body = update.body.clone();
                item.labels = update.labels.clone();
                item.assignees = update.assignees.clone();
                if update.updated_at.is_some() {
                    item.updated_at = update.updated_at;
                }
                changed = true;
            }
        }
        changed
    }

    fn handle_pr_action_dialog_key(
        &mut self,
        key: KeyEvent,
        config: &Config,
        store: &SnapshotStore,
        tx: &UnboundedSender<AppMsg>,
    ) {
        self.handle_pr_action_dialog_key_with_submit(
            key,
            |item, action, checkout, merge_method| {
                start_pr_action(
                    item,
                    action,
                    checkout,
                    merge_method,
                    config.clone(),
                    store.clone(),
                    tx.clone(),
                );
            },
        );
    }

    fn handle_pr_action_dialog_key_with_submit<F>(&mut self, key: KeyEvent, mut submit: F)
    where
        F: FnMut(WorkItem, PrAction, Option<PrCheckoutPlan>, Option<MergeMethod>),
    {
        if self.pr_action_running {
            self.status = "item action already running".to_string();
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.pr_action_dialog = None;
                self.status = "item action cancelled".to_string();
            }
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                if let Some(action) = self.pr_action_dialog.as_ref().map(|dialog| dialog.action) {
                    self.submit_pr_action(action, &mut submit);
                }
            }
            KeyCode::Tab => self.cycle_merge_method(),
            KeyCode::Char('m') | KeyCode::Char('M') => self.select_merge_method(MergeMethod::Merge),
            KeyCode::Char('s') | KeyCode::Char('S') => {
                self.select_merge_method(MergeMethod::Squash)
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.select_merge_method(MergeMethod::Rebase)
            }
            _ => {}
        }
    }

    fn submit_pr_action<F>(&mut self, action: PrAction, submit: &mut F)
    where
        F: FnMut(WorkItem, PrAction, Option<PrCheckoutPlan>, Option<MergeMethod>),
    {
        let Some(dialog) = &self.pr_action_dialog else {
            return;
        };
        let item = dialog.item.clone();
        let checkout = dialog.checkout.clone();
        let merge_method = (action == PrAction::Merge).then_some(dialog.merge_method);
        self.pr_action_running = true;
        self.status = if action == PrAction::Merge {
            format!(
                "merging pull request with {}",
                merge_method.unwrap_or_default().label()
            )
        } else {
            pr_action_running_status(action, item.kind)
        };
        submit(item, action, checkout, merge_method);
    }

    fn select_merge_method(&mut self, method: MergeMethod) {
        let Some(dialog) = &mut self.pr_action_dialog else {
            return;
        };
        if dialog.action != PrAction::Merge {
            return;
        }
        dialog.merge_method = method;
        self.status = format!("merge method: {}", method.label());
    }

    fn cycle_merge_method(&mut self) {
        let Some(method) = self
            .pr_action_dialog
            .as_ref()
            .filter(|dialog| dialog.action == PrAction::Merge)
            .map(|dialog| dialog.merge_method.next())
        else {
            return;
        };
        self.select_merge_method(method);
    }

    fn start_review_submit_dialog(&mut self, event: PullRequestReviewEvent) {
        let Some(item) = self.current_item().cloned() else {
            self.status = "nothing selected".to_string();
            return;
        };
        if item.kind != ItemKind::PullRequest || item.number.is_none() {
            self.status = "selected item is not a pull request".to_string();
            return;
        }

        let (mode, body) = self
            .pending_reviews
            .get(&item.id)
            .map(|pending| {
                (
                    ReviewSubmitMode::Pending {
                        review_id: pending.review_id,
                    },
                    pending.body.clone(),
                )
            })
            .unwrap_or((ReviewSubmitMode::New, String::new()));
        self.finish_details_visit(Instant::now());
        self.focus = FocusTarget::Details;
        self.search_active = false;
        self.global_search_active = false;
        self.comment_search_active = false;
        self.pr_action_dialog = None;
        self.comment_dialog = None;
        self.review_submit_dialog = Some(ReviewSubmitDialog {
            item,
            event,
            body: EditorText::from_text(body),
            scroll: 0,
            mode,
        });
        self.scroll_review_submit_dialog_to_cursor();
        self.status = match mode {
            ReviewSubmitMode::New => format!("review summary: {}", event.label()),
            ReviewSubmitMode::Pending { .. } => {
                format!("pending review summary: {}", event.label())
            }
        };
    }

    fn handle_review_submit_dialog_key(
        &mut self,
        key: KeyEvent,
        tx: &UnboundedSender<AppMsg>,
        area: Option<Rect>,
    ) {
        let tx_for_submit = tx.clone();
        let tx_for_pending = tx.clone();
        let tx_for_mentions = tx.clone();
        self.handle_review_submit_dialog_key_with_submit(
            key,
            area,
            {
                let tx = tx_for_submit;
                move |pending| match pending.mode {
                    ReviewSubmitMode::New => {
                        start_review_submit(pending.item, pending.event, pending.body, tx.clone());
                    }
                    ReviewSubmitMode::Pending { review_id } => {
                        start_pending_review_submit(
                            pending.item,
                            review_id,
                            pending.event,
                            pending.body,
                            tx.clone(),
                        );
                    }
                }
            },
            move |item, body| {
                start_review_draft_create(item, body, tx_for_pending.clone());
            },
        );
        self.ensure_mention_candidates_for_active_editor(None, &tx_for_mentions);
    }

    fn handle_review_submit_dialog_key_with_submit<F, G>(
        &mut self,
        key: KeyEvent,
        area: Option<Rect>,
        mut submit: F,
        mut create_pending: G,
    ) where
        F: FnMut(PendingReviewSubmit),
        G: FnMut(WorkItem, String),
    {
        if self.review_submit_running {
            self.status = "review action already running".to_string();
            return;
        }
        if self.handle_active_mention_key(key) {
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.review_submit_dialog = None;
                self.status = "review cancelled".to_string();
            }
            KeyCode::Tab => self.cycle_review_submit_event(),
            KeyCode::Char('1') => self.set_review_submit_event(PullRequestReviewEvent::Comment),
            KeyCode::Char('2') => {
                self.set_review_submit_event(PullRequestReviewEvent::RequestChanges)
            }
            KeyCode::Char('3') => self.set_review_submit_event(PullRequestReviewEvent::Approve),
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.create_pending_review_from_dialog(area, &mut create_pending)
            }
            KeyCode::PageDown => self.scroll_review_submit_dialog(6, area),
            KeyCode::PageUp => self.scroll_review_submit_dialog(-6, area),
            _ if is_comment_submit_key(key) => {
                if let Some(pending) = self.prepare_review_submit() {
                    submit(pending);
                }
            }
            _ => {
                if let Some(dialog) = &mut self.review_submit_dialog
                    && dialog.body.input_key(key, true)
                {
                    self.scroll_review_submit_dialog_to_cursor_in_area(area);
                }
            }
        }
    }

    fn cycle_review_submit_event(&mut self) {
        let Some(dialog) = &mut self.review_submit_dialog else {
            return;
        };
        dialog.event = dialog.event.next();
        self.status = format!("review event: {}", dialog.event.label());
    }

    fn set_review_submit_event(&mut self, event: PullRequestReviewEvent) {
        let Some(dialog) = &mut self.review_submit_dialog else {
            return;
        };
        dialog.event = event;
        self.status = format!("review event: {}", event.label());
    }

    fn create_pending_review_from_dialog<G>(&mut self, area: Option<Rect>, create_pending: &mut G)
    where
        G: FnMut(WorkItem, String),
    {
        let Some(dialog) = self.review_submit_dialog.take() else {
            return;
        };
        let body = dialog.body.text().trim().to_string();
        let item = dialog.item.clone();
        if item.kind != ItemKind::PullRequest || item.number.is_none() {
            self.review_submit_dialog = Some(dialog);
            self.status = "selected item is not a pull request".to_string();
            return;
        }
        self.review_submit_running = true;
        self.message_dialog = Some(message_dialog(
            "Creating Pending Review",
            "Waiting for GitHub to create the pending review...",
        ));
        self.status = "creating pending review".to_string();
        create_pending(item, body);
        self.scroll_review_submit_dialog_to_cursor_in_area(area);
    }

    fn prepare_review_submit(&mut self) -> Option<PendingReviewSubmit> {
        let dialog = self.review_submit_dialog.take()?;
        let body = dialog.body.text().trim().to_string();
        if dialog.event.requires_body() && body.is_empty() {
            let event = dialog.event;
            self.review_submit_dialog = Some(dialog);
            self.status = format!("{} review needs a summary", event.label());
            return None;
        }
        if dialog.item.kind != ItemKind::PullRequest || dialog.item.number.is_none() {
            self.review_submit_dialog = Some(dialog);
            self.status = "selected item is not a pull request".to_string();
            return None;
        }
        let event = dialog.event;
        let mode = dialog.mode;
        let item = dialog.item;
        self.review_submit_running = true;
        self.message_dialog = Some(message_dialog(
            "Submitting Review",
            format!("Waiting for GitHub to {}...", event.label()),
        ));
        self.status = match mode {
            ReviewSubmitMode::New => format!("submitting review: {}", event.label()),
            ReviewSubmitMode::Pending { .. } => {
                format!("submitting pending review: {}", event.label())
            }
        };
        Some(PendingReviewSubmit {
            item,
            event,
            body,
            mode,
        })
    }

    fn discard_pending_review(&mut self, tx: &UnboundedSender<AppMsg>) {
        self.discard_pending_review_with_submit(|item, review_id| {
            start_pending_review_discard(item, review_id, tx.clone());
        });
    }

    fn discard_pending_review_with_submit<F>(&mut self, mut submit: F)
    where
        F: FnMut(WorkItem, u64),
    {
        let Some(item) = self.current_item().cloned() else {
            self.status = "nothing selected".to_string();
            return;
        };
        if item.kind != ItemKind::PullRequest || item.number.is_none() {
            self.status = "selected item is not a pull request".to_string();
            return;
        }
        let Some(pending) = self.pending_reviews.get(&item.id).cloned() else {
            self.status = "no pending review to discard".to_string();
            return;
        };
        if self.review_submit_running {
            self.status = "review action already running".to_string();
            return;
        }
        self.review_submit_dialog = None;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.review_submit_running = true;
        self.message_dialog = Some(message_dialog(
            "Discarding Pending Review",
            "Waiting for GitHub to discard the pending review...",
        ));
        self.status = "discarding pending review".to_string();
        submit(item, pending.review_id);
    }

    fn scroll_review_submit_dialog(&mut self, delta: i16, area: Option<Rect>) {
        let Some(dialog) = &mut self.review_submit_dialog else {
            return;
        };
        let (width, height) = review_submit_editor_size(dialog, area);
        let body = dialog.body.text();
        let max_scroll = max_comment_dialog_scroll(body, width, height);
        if delta < 0 {
            dialog.scroll = dialog.scroll.saturating_sub(delta.unsigned_abs());
        } else {
            dialog.scroll = dialog.scroll.saturating_add(delta as u16);
        }
        dialog.scroll = dialog.scroll.min(max_scroll);
    }

    fn scroll_review_submit_dialog_to_cursor(&mut self) {
        self.scroll_review_submit_dialog_to_cursor_in_area(None);
    }

    fn scroll_review_submit_dialog_to_cursor_in_area(&mut self, area: Option<Rect>) {
        if let Some(dialog) = &mut self.review_submit_dialog {
            let (width, height) = review_submit_editor_size(dialog, area);
            let body = dialog.body.text();
            dialog.scroll = scroll_for_comment_dialog_cursor(
                body,
                dialog.body.cursor_byte(),
                width,
                height,
                dialog.scroll,
            );
        }
    }

    fn start_milestone_dialog(&mut self, tx: &UnboundedSender<AppMsg>) {
        let Some(item) = self.current_item().cloned() else {
            self.status = "nothing selected".to_string();
            return;
        };
        if !matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest) || item.number.is_none() {
            self.status = "selected item is not an issue or pull request".to_string();
            return;
        }
        self.finish_details_visit(Instant::now());
        self.search_active = false;
        self.global_search_active = false;
        self.comment_search_active = false;
        self.pr_action_dialog = None;
        self.milestone_dialog = Some(MilestoneDialog {
            item: item.clone(),
            state: MilestoneDialogState::Loading,
            input: String::new(),
            selected: 0,
        });
        self.milestone_action_running = false;
        self.status = "loading milestones".to_string();
        if tokio::runtime::Handle::try_current().is_ok() {
            start_milestones_load(item, tx.clone());
        }
    }

    fn handle_milestone_dialog_key(
        &mut self,
        key: KeyEvent,
        config: &Config,
        store: &SnapshotStore,
        tx: &UnboundedSender<AppMsg>,
    ) {
        self.handle_milestone_dialog_key_with_submit(key, |item, choice| {
            start_milestone_change(item, choice, config.clone(), store.clone(), tx.clone());
        });
    }

    fn handle_milestone_dialog_key_with_submit<F>(&mut self, key: KeyEvent, mut submit: F)
    where
        F: FnMut(WorkItem, MilestoneChoice),
    {
        if self.milestone_action_running {
            self.status = "milestone change already running".to_string();
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.milestone_dialog = None;
                self.status = "milestone change cancelled".to_string();
            }
            KeyCode::Backspace => {
                if let Some(dialog) = self.milestone_dialog.as_mut() {
                    dialog.input.pop();
                    reset_milestone_dialog_selection(dialog);
                }
            }
            KeyCode::Down => self.move_milestone_choice(1),
            KeyCode::Up => self.move_milestone_choice(-1),
            KeyCode::Enter => self.submit_milestone_choice(&mut submit),
            KeyCode::Char(value) if !value.is_control() => {
                if let Some(dialog) = self.milestone_dialog.as_mut() {
                    dialog.input.push(value);
                    reset_milestone_dialog_selection(dialog);
                }
            }
            _ => {}
        }
    }

    fn move_milestone_choice(&mut self, delta: isize) {
        let Some(dialog) = self.milestone_dialog.as_mut() else {
            return;
        };
        let choices = milestone_choices(dialog);
        if choices.is_empty() {
            dialog.selected = 0;
            return;
        }
        dialog.selected = move_bounded(dialog.selected, choices.len(), delta);
    }

    fn submit_milestone_choice<F>(&mut self, submit: &mut F)
    where
        F: FnMut(WorkItem, MilestoneChoice),
    {
        let Some(dialog) = self.milestone_dialog.as_ref() else {
            return;
        };
        match &dialog.state {
            MilestoneDialogState::Loading => {
                self.status = "milestones still loading".to_string();
            }
            MilestoneDialogState::Error(_) => {
                self.status = "milestones failed to load".to_string();
            }
            MilestoneDialogState::Loaded(_) => {
                let choices = milestone_choices(dialog);
                let Some(choice) = choices.get(dialog.selected).cloned() else {
                    self.status = "no milestone matches prefix".to_string();
                    return;
                };
                let item = dialog.item.clone();
                self.milestone_action_running = true;
                self.status = match &choice {
                    MilestoneChoice::Clear => "clearing milestone".to_string(),
                    MilestoneChoice::Set(_) => "changing milestone".to_string(),
                    MilestoneChoice::Create(_) => "creating milestone".to_string(),
                };
                submit(item, choice);
            }
        }
    }

    fn start_assignee_dialog_with_store(
        &mut self,
        action: AssigneeAction,
        store: Option<&SnapshotStore>,
        tx: Option<&UnboundedSender<AppMsg>>,
    ) {
        let Some(item) = self.current_item().cloned() else {
            self.status = "nothing selected".to_string();
            return;
        };
        if !matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest) || item.number.is_none() {
            self.status = "selected item cannot have assignees".to_string();
            return;
        }
        if action == AssigneeAction::Unassign && item.assignees.is_empty() {
            self.status = "selected item has no assignees".to_string();
            return;
        }
        self.finish_details_visit(Instant::now());
        self.search_active = false;
        self.global_search_active = false;
        self.comment_search_active = false;
        self.comment_dialog = None;
        self.label_dialog = None;
        self.issue_dialog = None;
        self.reaction_dialog = None;
        self.review_submit_dialog = None;
        self.item_edit_dialog = None;
        self.milestone_dialog = None;
        self.pr_action_dialog = None;
        let repo = item.repo.clone();
        let cached_suggestions = (action == AssigneeAction::Assign)
            .then(|| self.assignee_suggestions_cache.get(&repo).cloned())
            .flatten();
        let has_cached_suggestions = cached_suggestions.is_some();
        self.assignee_dialog = Some(AssigneeDialog {
            item,
            action,
            input: String::new(),
            suggestions: cached_suggestions.unwrap_or_default(),
            suggestions_loading: false,
            suggestions_error: None,
            selected_suggestion: 0,
        });
        self.assignee_action_running = false;
        let suggestions_refreshing = action == AssigneeAction::Assign
            && tx
                .map(|tx| start_assignee_suggestions_load(repo, store.cloned(), tx.clone()))
                .unwrap_or(false);
        if let Some(dialog) = &mut self.assignee_dialog {
            dialog.suggestions_loading = suggestions_refreshing && !has_cached_suggestions;
        }
        self.status = match action {
            AssigneeAction::Assign if has_cached_suggestions && suggestions_refreshing => {
                "assignee candidates cached; refreshing".to_string()
            }
            AssigneeAction::Assign if suggestions_refreshing => {
                "loading assignee candidates".to_string()
            }
            AssigneeAction::Assign => "enter assignee to add".to_string(),
            AssigneeAction::Unassign => "enter assignee to remove".to_string(),
        };
    }

    fn handle_assignee_dialog_key(&mut self, key: KeyEvent, tx: &UnboundedSender<AppMsg>) {
        let tx = tx.clone();
        self.handle_assignee_dialog_key_with_submit(key, move |item, action, assignees| {
            start_assignee_update(item, action, assignees, tx.clone());
        });
    }

    fn handle_assignee_dialog_key_with_submit<F>(&mut self, key: KeyEvent, mut submit: F)
    where
        F: FnMut(WorkItem, AssigneeAction, Vec<String>),
    {
        if self.assignee_action_running {
            self.status = "assignee action already running".to_string();
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.assignee_dialog = None;
                self.status = "assignee action cancelled".to_string();
            }
            KeyCode::Enter => {
                self.submit_assignee_action(&mut submit);
            }
            KeyCode::Down | KeyCode::Tab => self.move_assignee_suggestion(1),
            KeyCode::Up | KeyCode::BackTab => self.move_assignee_suggestion(-1),
            KeyCode::Backspace => {
                if let Some(dialog) = &mut self.assignee_dialog {
                    dialog.input.pop();
                    dialog.selected_suggestion = 0;
                    clamp_assignee_dialog_selection(dialog);
                }
            }
            KeyCode::Char(value) => {
                if let Some(dialog) = &mut self.assignee_dialog {
                    dialog.input.push(value);
                    dialog.selected_suggestion = 0;
                    clamp_assignee_dialog_selection(dialog);
                }
            }
            _ => {}
        }
    }

    fn submit_assignee_action<F>(&mut self, submit: &mut F)
    where
        F: FnMut(WorkItem, AssigneeAction, Vec<String>),
    {
        let Some(dialog) = &self.assignee_dialog else {
            return;
        };
        let assignees = assignee_dialog_submit_logins(dialog);
        if assignees.is_empty() {
            self.status = "assignee login is empty".to_string();
            return;
        }
        let item = dialog.item.clone();
        let action = dialog.action;
        self.assignee_action_running = true;
        self.status = match action {
            AssigneeAction::Assign => "assigning assignee".to_string(),
            AssigneeAction::Unassign => "removing assignee".to_string(),
        };
        submit(item, action, assignees);
    }

    fn move_assignee_suggestion(&mut self, delta: isize) {
        let Some(dialog) = &mut self.assignee_dialog else {
            return;
        };
        let count = assignee_dialog_suggestion_matches(dialog).len();
        if count == 0 {
            self.status = "no assignee candidates match".to_string();
            return;
        }
        dialog.selected_suggestion = move_wrapping(dialog.selected_suggestion, count, delta);
        let login = assignee_dialog_suggestion_matches(dialog)
            .get(dialog.selected_suggestion)
            .cloned()
            .unwrap_or_else(|| dialog.input.trim().to_string());
        self.status = format!("selected assignee candidate: {login}");
    }

    fn start_reviewer_dialog_with_store(
        &mut self,
        action: ReviewerAction,
        store: Option<&SnapshotStore>,
        tx: Option<&UnboundedSender<AppMsg>>,
    ) {
        let Some(item) = self.current_item().cloned() else {
            self.status = "nothing selected".to_string();
            return;
        };
        if item.kind != ItemKind::PullRequest || item.number.is_none() {
            self.status = "selected item is not a pull request".to_string();
            return;
        }
        self.finish_details_visit(Instant::now());
        self.search_active = false;
        self.global_search_active = false;
        self.comment_search_active = false;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        let repo = item.repo.clone();
        let cached_suggestions = self
            .reviewer_suggestions_cache
            .get(&repo)
            .cloned()
            .or_else(|| self.assignee_suggestions_cache.get(&repo).cloned());
        let has_cached_suggestions = cached_suggestions.is_some();
        self.reviewer_dialog = Some(ReviewerDialog {
            item,
            action,
            input: String::new(),
            suggestions: cached_suggestions.unwrap_or_default(),
            suggestions_loading: false,
            suggestions_error: None,
            selected_suggestion: 0,
        });
        self.reviewer_action_running = false;
        let suggestions_refreshing = tx
            .map(|tx| start_reviewer_suggestions_load(repo, store.cloned(), tx.clone()))
            .unwrap_or(false);
        if let Some(dialog) = &mut self.reviewer_dialog {
            dialog.suggestions_loading = suggestions_refreshing && !has_cached_suggestions;
        }
        self.status = match (action, has_cached_suggestions, suggestions_refreshing) {
            (ReviewerAction::Request, true, true) => {
                "reviewer candidates cached; refreshing".to_string()
            }
            (ReviewerAction::Remove, true, true) => {
                "reviewer candidates cached; refreshing".to_string()
            }
            (ReviewerAction::Request, _, true) => "loading reviewer candidates".to_string(),
            (ReviewerAction::Remove, _, true) => "loading reviewer candidates".to_string(),
            (ReviewerAction::Request, _, _) => "enter reviewer logins to request".to_string(),
            (ReviewerAction::Remove, _, _) => "enter reviewer logins to remove".to_string(),
        };
    }

    fn handle_reviewer_dialog_key(
        &mut self,
        key: KeyEvent,
        config: &Config,
        store: &SnapshotStore,
        tx: &UnboundedSender<AppMsg>,
    ) {
        self.handle_reviewer_dialog_key_with_submit(key, |item, action, reviewers| {
            start_reviewer_action(
                item,
                action,
                reviewers,
                config.clone(),
                store.clone(),
                tx.clone(),
            );
        });
    }

    fn handle_reviewer_dialog_key_with_submit<F>(&mut self, key: KeyEvent, mut submit: F)
    where
        F: FnMut(WorkItem, ReviewerAction, Vec<String>),
    {
        if self.reviewer_action_running {
            self.status = "reviewer action already running".to_string();
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.reviewer_dialog = None;
                self.status = "reviewer action cancelled".to_string();
            }
            KeyCode::Enter => {
                if let Some((item, action, reviewers)) = self.prepare_reviewer_submit() {
                    submit(item, action, reviewers);
                }
            }
            KeyCode::Up => self.move_reviewer_suggestion(-1),
            KeyCode::Down => self.move_reviewer_suggestion(1),
            KeyCode::Backspace => {
                if let Some(dialog) = &mut self.reviewer_dialog {
                    dialog.input.pop();
                    clamp_reviewer_dialog_selection(dialog);
                }
            }
            KeyCode::Char(value) => {
                if let Some(dialog) = &mut self.reviewer_dialog {
                    dialog.input.push(value);
                    dialog.selected_suggestion = 0;
                    clamp_reviewer_dialog_selection(dialog);
                }
            }
            _ => {}
        }
    }

    fn move_reviewer_suggestion(&mut self, delta: isize) {
        let Some(dialog) = &mut self.reviewer_dialog else {
            return;
        };
        let count = reviewer_dialog_suggestion_matches(dialog).len();
        if count == 0 {
            self.status = "no reviewer candidates match".to_string();
            return;
        }
        dialog.selected_suggestion = move_wrapping(dialog.selected_suggestion, count, delta);
        let login = reviewer_dialog_suggestion_matches(dialog)
            .get(dialog.selected_suggestion)
            .cloned()
            .unwrap_or_else(|| dialog.input.trim().to_string());
        self.status = format!("selected reviewer candidate: {login}");
    }

    fn prepare_reviewer_submit(&mut self) -> Option<(WorkItem, ReviewerAction, Vec<String>)> {
        let Some(dialog) = &self.reviewer_dialog else {
            return None;
        };
        let reviewers = reviewer_dialog_submit_logins(dialog);
        if reviewers.is_empty() {
            self.status = "enter at least one reviewer login".to_string();
            return None;
        }
        let item = dialog.item.clone();
        let action = dialog.action;
        self.reviewer_action_running = true;
        self.status = match action {
            ReviewerAction::Request => format!("requesting review from {}", reviewers.join(", ")),
            ReviewerAction::Remove => {
                format!("removing review requests for {}", reviewers.join(", "))
            }
        };
        Some((item, action, reviewers))
    }

    fn start_new_comment_dialog(&mut self) {
        let Some(item) = self.current_item().cloned() else {
            self.status = "selected item cannot be commented on".to_string();
            return;
        };
        if !item_supports_metadata_edit(&item) {
            self.status = "selected item cannot be commented on".to_string();
            return;
        }
        self.finish_details_visit(Instant::now());
        self.focus = FocusTarget::Details;
        self.search_active = false;
        self.comment_search_active = false;
        self.global_search_active = false;
        self.filter_input_active = false;
        self.pr_action_dialog = None;
        self.label_dialog = None;
        self.issue_dialog = None;
        self.reaction_dialog = None;
        self.review_submit_dialog = None;
        self.item_edit_dialog = None;
        self.assignee_dialog = None;
        self.reviewer_dialog = None;
        let loaded = self.open_comment_dialog_with_draft(
            CommentDialogMode::New,
            String::new(),
            new_comment_draft_key(&item),
        );
        self.scroll_comment_dialog_to_cursor();
        self.status = if loaded {
            "loaded comment draft".to_string()
        } else {
            "new comment".to_string()
        };
    }

    fn start_reply_to_selected_comment(&mut self) {
        if !self.current_item_supports_comments() {
            self.status = "selected item cannot be commented on".to_string();
            return;
        }
        let Some(item) = self.current_item().cloned() else {
            self.status = "nothing selected".to_string();
            return;
        };
        let Some(comment) = self.current_selected_comment().cloned() else {
            self.status = "no comment selected".to_string();
            return;
        };
        if comment.kind.is_activity() {
            self.status = "activity cannot be replied to".to_string();
            return;
        }
        let author = comment.author.clone();
        let review_comment_id = comment.review.as_ref().and(comment.id);
        self.finish_details_visit(Instant::now());
        self.focus = FocusTarget::Details;
        self.search_active = false;
        self.comment_search_active = false;
        self.global_search_active = false;
        self.filter_input_active = false;
        self.pr_action_dialog = None;
        self.label_dialog = None;
        self.issue_dialog = None;
        self.reaction_dialog = None;
        self.review_submit_dialog = None;
        self.item_edit_dialog = None;
        self.assignee_dialog = None;
        let body = quote_comment_for_reply(&comment);
        let loaded = self.open_comment_dialog_with_draft(
            CommentDialogMode::Reply {
                comment_index: self.selected_comment_index,
                author: author.clone(),
                review_comment_id,
            },
            body,
            reply_comment_draft_key(&item, &comment, self.selected_comment_index),
        );
        self.scroll_comment_dialog_to_cursor();
        self.status = if loaded {
            format!("loaded reply draft for @{author}")
        } else {
            format!("replying to @{author}")
        };
    }

    fn start_edit_selected_comment_dialog(&mut self) {
        if !self.current_item_supports_comments() {
            self.status = "selected item cannot be commented on".to_string();
            return;
        }
        let Some(item) = self.current_item().cloned() else {
            self.status = "nothing selected".to_string();
            return;
        };
        let Some(comment) = self.current_selected_comment().cloned() else {
            self.status = "no comment selected".to_string();
            return;
        };
        let Some(comment_id) = comment.id else {
            self.status = "comment id unavailable; cannot edit".to_string();
            return;
        };
        if !comment.can_edit() {
            self.status = "selected comment cannot be edited".to_string();
            return;
        }

        self.finish_details_visit(Instant::now());
        self.focus = FocusTarget::Details;
        self.search_active = false;
        self.comment_search_active = false;
        self.global_search_active = false;
        self.filter_input_active = false;
        self.pr_action_dialog = None;
        self.label_dialog = None;
        self.issue_dialog = None;
        self.reaction_dialog = None;
        self.review_submit_dialog = None;
        self.item_edit_dialog = None;
        self.assignee_dialog = None;
        let body = comment.body.clone();
        let loaded = self.open_comment_dialog_with_draft(
            CommentDialogMode::Edit {
                comment_index: self.selected_comment_index,
                comment_id,
                is_review: comment.review.is_some(),
            },
            body,
            edit_comment_draft_key(&item, comment_id, comment.review.is_some()),
        );
        self.scroll_comment_dialog_to_cursor();
        self.status = if loaded {
            "loaded comment edit draft".to_string()
        } else {
            "editing comment".to_string()
        };
    }

    fn start_review_comment_dialog(&mut self) {
        if self.details_mode != DetailsMode::Diff {
            self.status = "review comments are available in diff mode".to_string();
            return;
        }
        let Some(item) = self.current_item().cloned() else {
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

        self.finish_details_visit(Instant::now());
        self.focus = FocusTarget::Details;
        self.search_active = false;
        self.comment_search_active = false;
        self.global_search_active = false;
        self.filter_input_active = false;
        self.pr_action_dialog = None;
        self.label_dialog = None;
        self.issue_dialog = None;
        self.reaction_dialog = None;
        self.review_submit_dialog = None;
        self.item_edit_dialog = None;
        self.assignee_dialog = None;
        let loaded = self.open_comment_dialog_with_draft(
            CommentDialogMode::Review {
                target: target.clone(),
            },
            String::new(),
            review_comment_draft_key(&item, &target),
        );
        self.scroll_comment_dialog_to_cursor();
        self.status = if loaded {
            format!("loaded review draft for {}", target.location_label())
        } else {
            format!("reviewing {}", target.location_label())
        };
    }

    fn handle_comment_dialog_key_with_store(
        &mut self,
        key: KeyEvent,
        store: Option<&SnapshotStore>,
        tx: &UnboundedSender<AppMsg>,
        area: Option<Rect>,
    ) {
        let store = store.cloned();
        let store_for_submit = store.clone();
        let tx_for_submit = tx.clone();
        let tx_for_mentions = tx.clone();
        self.handle_comment_dialog_key_with_submit(key, area, move |submit| match submit.mode {
            PendingCommentMode::Post => {
                start_comment_submit(
                    submit.item,
                    submit.body,
                    draft_clear_task(submit.draft_key, store_for_submit.clone()),
                    tx_for_submit.clone(),
                );
            }
            PendingCommentMode::ReviewReply { comment_id } => {
                start_review_reply_submit(
                    submit.item,
                    comment_id,
                    submit.body,
                    draft_clear_task(submit.draft_key, store_for_submit.clone()),
                    tx_for_submit.clone(),
                );
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
                    draft_clear_task(submit.draft_key, store_for_submit.clone()),
                    tx_for_submit.clone(),
                );
            }
            PendingCommentMode::Review { target } => {
                start_review_comment_submit(
                    submit.item,
                    target,
                    submit.body,
                    draft_clear_task(submit.draft_key, store_for_submit.clone()),
                    tx_for_submit.clone(),
                );
            }
        });
        self.ensure_mention_candidates_for_active_editor(store.as_ref(), &tx_for_mentions);
    }

    fn start_add_label_dialog_with_store(
        &mut self,
        store: Option<&SnapshotStore>,
        tx: Option<&UnboundedSender<AppMsg>>,
    ) {
        let Some(item) = self.current_item().cloned() else {
            self.status = "nothing selected".to_string();
            return;
        };
        if !matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest) || item.number.is_none() {
            self.status = "labels are available for issues and pull requests".to_string();
            return;
        };
        let repo = item.repo.clone();
        let cached_suggestions = self.label_suggestions_cache.get(&repo).cloned();
        let has_cached_suggestions = cached_suggestions.is_some();
        self.finish_details_visit(Instant::now());
        self.label_dialog = Some(LabelDialog {
            mode: LabelDialogMode::Add { repo: repo.clone() },
            input: String::new(),
            existing_labels: item.labels,
            suggestions: cached_suggestions.unwrap_or_default(),
            suggestions_loading: false,
            suggestions_error: None,
            selected_suggestion: 0,
        });
        self.label_updating = false;
        self.comment_dialog = None;
        self.issue_dialog = None;
        self.reaction_dialog = None;
        self.pr_action_dialog = None;
        self.search_active = false;
        self.comment_search_active = false;
        self.global_search_active = false;
        self.focus = FocusTarget::Details;
        let suggestions_refreshing = tx
            .map(|tx| start_label_suggestions_load(repo, store.cloned(), tx.clone()))
            .unwrap_or(false);
        if let Some(dialog) = &mut self.label_dialog {
            dialog.suggestions_loading = suggestions_refreshing && !has_cached_suggestions;
        }
        self.status = if has_cached_suggestions && suggestions_refreshing {
            "label suggestions cached; refreshing".to_string()
        } else if suggestions_refreshing {
            "loading label suggestions".to_string()
        } else {
            "label input mode".to_string()
        };
    }

    fn start_remove_label_dialog(&mut self, label: String) {
        if !self.current_item_supports_labels() {
            self.status = "labels are available for issues and pull requests".to_string();
            return;
        }
        self.finish_details_visit(Instant::now());
        self.label_dialog = Some(LabelDialog {
            mode: LabelDialogMode::Remove { label },
            input: String::new(),
            existing_labels: Vec::new(),
            suggestions: Vec::new(),
            suggestions_loading: false,
            suggestions_error: None,
            selected_suggestion: 0,
        });
        self.label_updating = false;
        self.comment_dialog = None;
        self.issue_dialog = None;
        self.reaction_dialog = None;
        self.pr_action_dialog = None;
        self.search_active = false;
        self.comment_search_active = false;
        self.global_search_active = false;
        self.focus = FocusTarget::Details;
        self.status = "confirm label removal".to_string();
    }

    fn handle_label_dialog_key(&mut self, key: KeyEvent, tx: &UnboundedSender<AppMsg>) {
        self.handle_label_dialog_key_with_submit(key, |pending| {
            start_label_update(pending.item, pending.action, tx.clone());
        });
    }

    fn handle_label_dialog_key_with_submit<F>(&mut self, key: KeyEvent, mut submit: F)
    where
        F: FnMut(PendingLabelUpdate),
    {
        if self.label_updating {
            return;
        }

        let Some(mode) = self.label_dialog.as_ref().map(|dialog| dialog.mode.clone()) else {
            return;
        };

        match mode {
            LabelDialogMode::Add { .. } => match key.code {
                KeyCode::Esc => {
                    self.label_dialog = None;
                    self.status = "label update cancelled".to_string();
                }
                KeyCode::Enter => {
                    if let Some(pending) = self.prepare_label_add() {
                        submit(pending);
                    }
                }
                KeyCode::Down | KeyCode::Tab => self.move_label_suggestion(1),
                KeyCode::Up | KeyCode::BackTab => self.move_label_suggestion(-1),
                KeyCode::Backspace => {
                    if let Some(dialog) = &mut self.label_dialog {
                        dialog.input.pop();
                        dialog.selected_suggestion = 0;
                        clamp_label_dialog_selection(dialog);
                    }
                }
                KeyCode::Char(value) => {
                    if let Some(dialog) = &mut self.label_dialog {
                        dialog.input.push(value);
                        dialog.selected_suggestion = 0;
                        clamp_label_dialog_selection(dialog);
                    }
                }
                _ => {}
            },
            LabelDialogMode::Remove { label } => match key.code {
                KeyCode::Esc => {
                    self.label_dialog = None;
                    self.status = "label update cancelled".to_string();
                }
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Some(pending) = self.prepare_label_remove(label) {
                        submit(pending);
                    }
                }
                _ => {}
            },
        }
    }

    fn prepare_label_add(&mut self) -> Option<PendingLabelUpdate> {
        let dialog = self.label_dialog.take()?;
        let label = selected_label_for_add(&dialog);
        if label.is_empty() {
            self.label_dialog = Some(dialog);
            self.status = "label is empty".to_string();
            return None;
        }
        self.prepare_label_update(LabelAction::Add(label), Some(dialog))
    }

    fn move_label_suggestion(&mut self, delta: isize) {
        let Some(dialog) = &mut self.label_dialog else {
            return;
        };
        let count = label_dialog_suggestion_matches(dialog).len();
        if count == 0 {
            self.status = "no label suggestions match".to_string();
            return;
        }
        dialog.selected_suggestion = move_wrapping(dialog.selected_suggestion, count, delta);
        let label = label_dialog_suggestion_matches(dialog)
            .get(dialog.selected_suggestion)
            .cloned()
            .unwrap_or_else(|| dialog.input.trim().to_string());
        self.status = format!("selected label suggestion: {label}");
    }

    fn prepare_label_remove(&mut self, label: String) -> Option<PendingLabelUpdate> {
        self.label_dialog = None;
        self.prepare_label_update(LabelAction::Remove(label), None)
    }

    fn prepare_label_update(
        &mut self,
        action: LabelAction,
        restore_dialog: Option<LabelDialog>,
    ) -> Option<PendingLabelUpdate> {
        let Some(item) = self.current_item().cloned() else {
            self.label_dialog = restore_dialog;
            self.status = "nothing selected".to_string();
            return None;
        };
        if !matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest) {
            self.label_dialog = restore_dialog;
            self.status = "labels are available for issues and pull requests".to_string();
            return None;
        }
        self.label_updating = true;
        self.status = match &action {
            LabelAction::Add(label) => format!("adding label {label}"),
            LabelAction::Remove(label) => format!("removing label {label}"),
        };
        self.message_dialog = Some(message_dialog("Updating Labels", "Waiting for GitHub..."));
        Some(PendingLabelUpdate { item, action })
    }

    fn start_new_issue_dialog(&mut self) {
        let Some(repo) = self.current_repo_scope() else {
            self.status = "select an item or repo before creating an issue".to_string();
            return;
        };
        self.finish_details_visit(Instant::now());
        self.focus = FocusTarget::Details;
        self.search_active = false;
        self.comment_search_active = false;
        self.global_search_active = false;
        self.comment_dialog = None;
        self.label_dialog = None;
        self.issue_dialog = None;
        self.pr_create_dialog = None;
        self.reaction_dialog = None;
        self.pr_action_dialog = None;
        let loaded = self.open_issue_dialog_with_draft(repo);
        self.issue_creating = false;
        self.status = if loaded {
            "loaded issue draft".to_string()
        } else {
            "new issue".to_string()
        };
    }

    fn start_new_issue_or_pull_request_dialog(&mut self, config: &Config) {
        if self.new_dialog_target_is_pull_request() {
            self.start_new_pull_request_dialog(config);
        } else {
            self.start_new_issue_dialog();
        }
    }

    fn new_dialog_target_is_pull_request(&self) -> bool {
        if self.focus == FocusTarget::Details {
            match self.current_item().map(|item| item.kind) {
                Some(ItemKind::PullRequest) => return true,
                Some(ItemKind::Issue) => return false,
                _ => {}
            }
        }

        self.current_section()
            .is_some_and(|section| section.kind == SectionKind::PullRequests)
    }

    fn start_new_pull_request_dialog(&mut self, config: &Config) {
        let Some(repo) = self.current_repo_scope() else {
            self.status = "select a repo before creating a pull request".to_string();
            return;
        };
        let Some(local_dir) = configured_local_dir_for_repo(config, &repo) else {
            self.show_new_pull_request_unavailable(format!(
                "repo {repo} has no local_dir.\n\nSet [[repos]].local_dir to create a pull request from this repository."
            ));
            return;
        };
        if let Err(error) = ensure_directory_tracks_repo(&local_dir, &repo) {
            self.show_new_pull_request_unavailable(format!(
                "Configured local_dir for {repo} cannot be used.\n\n{error}\n\nSet [[repos]].local_dir to a checkout whose git remote points at {repo}."
            ));
            return;
        }
        let branch = match current_git_branch_for_directory(&local_dir) {
            Ok(branch) => branch,
            Err(error) => {
                self.show_new_pull_request_unavailable(format!(
                    "Cannot create a pull request from {repo}.\n\n{error}\n\nCheck out a branch in {} and retry.",
                    local_dir.display()
                ));
                return;
            }
        };

        self.finish_details_visit(Instant::now());
        self.focus = FocusTarget::Details;
        self.search_active = false;
        self.comment_search_active = false;
        self.global_search_active = false;
        self.filter_input_active = false;
        self.comment_dialog = None;
        self.label_dialog = None;
        self.issue_dialog = None;
        self.pr_create_dialog = None;
        self.reaction_dialog = None;
        self.pr_action_dialog = None;
        self.review_submit_dialog = None;
        self.item_edit_dialog = None;
        self.milestone_dialog = None;
        self.assignee_dialog = None;
        self.reviewer_dialog = None;
        let loaded = self.open_pr_create_dialog_with_draft(repo, local_dir, branch);
        self.pr_creating = false;
        self.status = if loaded {
            "loaded pull request draft".to_string()
        } else {
            "new pull request".to_string()
        };
    }

    fn show_new_pull_request_unavailable(&mut self, body: String) {
        self.message_dialog = Some(message_dialog(
            "New Pull Request Unavailable",
            truncate_text(&body, 900),
        ));
        self.status = "pull request creation unavailable".to_string();
    }

    fn handle_issue_dialog_key_with_store(
        &mut self,
        key: KeyEvent,
        store: Option<&SnapshotStore>,
        tx: &UnboundedSender<AppMsg>,
        area: Option<Rect>,
    ) {
        let store = store.cloned();
        let store_for_submit = store.clone();
        let tx_for_submit = tx.clone();
        let tx_for_mentions = tx.clone();
        self.handle_issue_dialog_key_with_submit(key, area, move |pending| {
            let draft_clear = draft_clear_task(pending.draft_key.clone(), store_for_submit.clone());
            start_issue_create(pending, draft_clear, tx_for_submit.clone());
        });
        self.ensure_mention_candidates_for_active_editor(store.as_ref(), &tx_for_mentions);
    }

    fn handle_issue_dialog_key_with_submit<F>(
        &mut self,
        key: KeyEvent,
        area: Option<Rect>,
        mut submit: F,
    ) where
        F: FnMut(PendingIssueCreate),
    {
        if self.issue_creating {
            return;
        }
        if self.handle_active_mention_key(key) {
            return;
        }

        if is_comment_submit_key(key) {
            if let Some(pending) = self.prepare_issue_create() {
                submit(pending);
            }
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.issue_dialog = None;
                self.status = "issue creation cancelled".to_string();
            }
            KeyCode::Tab => self.move_issue_dialog_field(1),
            KeyCode::BackTab => self.move_issue_dialog_field(-1),
            KeyCode::PageDown => self.scroll_issue_dialog_body(6, area),
            KeyCode::PageUp => self.scroll_issue_dialog_body(-6, area),
            KeyCode::Enter
                if self
                    .issue_dialog
                    .as_ref()
                    .is_some_and(|dialog| dialog.field != IssueDialogField::Body) =>
            {
                self.move_issue_dialog_field(1);
            }
            _ => {
                self.handle_issue_dialog_editor_key(key, area);
            }
        }
    }

    fn move_issue_dialog_field(&mut self, delta: isize) {
        if let Some(dialog) = &mut self.issue_dialog {
            dialog.field = dialog.field.next(delta);
            self.status = match dialog.field {
                IssueDialogField::Repo => "editing issue repo".to_string(),
                IssueDialogField::Title => "editing issue title".to_string(),
                IssueDialogField::Labels => "editing issue labels".to_string(),
                IssueDialogField::Body => "editing issue body".to_string(),
            };
        }
    }

    fn handle_issue_dialog_editor_key(&mut self, key: KeyEvent, area: Option<Rect>) {
        let should_scroll = {
            let Some(dialog) = &mut self.issue_dialog else {
                return;
            };
            let changed = match dialog.field {
                IssueDialogField::Repo => dialog.repo.input_key(key, false),
                IssueDialogField::Title => dialog.title.input_key(key, false),
                IssueDialogField::Labels => dialog.labels.input_key(key, false),
                IssueDialogField::Body => dialog.body.input_key(key, true),
            };
            changed && dialog.field == IssueDialogField::Body
        };
        if should_scroll {
            self.scroll_issue_dialog_to_cursor_in_area(area);
        }
    }

    fn scroll_issue_dialog_body(&mut self, delta: i16, area: Option<Rect>) {
        let Some(dialog) = &mut self.issue_dialog else {
            return;
        };
        let (width, height) = issue_dialog_body_editor_size(area);
        let body = dialog.body.text();
        let max_scroll = max_comment_dialog_scroll(body, width, height);
        if delta < 0 {
            dialog.body_scroll = dialog.body_scroll.saturating_sub(delta.unsigned_abs());
        } else {
            dialog.body_scroll = dialog.body_scroll.saturating_add(delta as u16);
        }
        dialog.body_scroll = dialog.body_scroll.min(max_scroll);
    }

    fn scroll_issue_dialog_to_cursor_in_area(&mut self, area: Option<Rect>) {
        if let Some(dialog) = &mut self.issue_dialog {
            if dialog.field != IssueDialogField::Body {
                return;
            }
            let (width, height) = issue_dialog_body_editor_size(area);
            let body = dialog.body.text();
            dialog.body_scroll = scroll_for_comment_dialog_cursor(
                body,
                dialog.body.cursor_byte(),
                width,
                height,
                dialog.body_scroll,
            );
        }
    }

    fn prepare_issue_create(&mut self) -> Option<PendingIssueCreate> {
        let dialog = self.issue_dialog.take()?;
        let repo = dialog.repo.text().trim().to_string();
        let title = dialog.title.text().trim().to_string();
        let body = dialog.body.text().trim().to_string();
        let labels = parse_issue_labels(dialog.labels.text());

        if !repo.contains('/') {
            self.issue_dialog = Some(dialog);
            self.status = "repo must be owner/name".to_string();
            return None;
        }
        if title.is_empty() {
            self.issue_dialog = Some(dialog);
            self.status = "issue title is empty".to_string();
            return None;
        }

        self.issue_creating = true;
        self.status = format!("creating issue in {repo}");
        self.message_dialog = Some(message_dialog("Creating Issue", "Waiting for GitHub..."));
        let pending = PendingIssueCreate {
            repo,
            title,
            body,
            labels,
            dialog,
            draft_key: self.issue_draft_key.clone(),
        };
        self.pending_issue_create = Some(pending.clone());
        Some(pending)
    }

    fn handle_pr_create_dialog_key_with_store(
        &mut self,
        key: KeyEvent,
        store: Option<&SnapshotStore>,
        tx: &UnboundedSender<AppMsg>,
        area: Option<Rect>,
    ) {
        let store = store.cloned();
        let store_for_submit = store.clone();
        let tx_for_submit = tx.clone();
        let tx_for_mentions = tx.clone();
        self.handle_pr_create_dialog_key_with_submit(key, area, move |pending| {
            let draft_clear = draft_clear_task(pending.draft_key.clone(), store_for_submit.clone());
            start_pr_create(pending, draft_clear, tx_for_submit.clone());
        });
        self.ensure_mention_candidates_for_active_editor(store.as_ref(), &tx_for_mentions);
    }

    fn handle_pr_create_dialog_key_with_submit<F>(
        &mut self,
        key: KeyEvent,
        area: Option<Rect>,
        mut submit: F,
    ) where
        F: FnMut(PendingPrCreate),
    {
        if self.pr_creating {
            return;
        }
        if self.handle_active_mention_key(key) {
            return;
        }

        if is_comment_submit_key(key) {
            if let Some(pending) = self.prepare_pr_create() {
                submit(pending);
            }
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.pr_create_dialog = None;
                self.status = "pull request creation cancelled".to_string();
            }
            KeyCode::Tab => self.move_pr_create_dialog_field(1),
            KeyCode::BackTab => self.move_pr_create_dialog_field(-1),
            KeyCode::PageDown => self.scroll_pr_create_dialog_body(6, area),
            KeyCode::PageUp => self.scroll_pr_create_dialog_body(-6, area),
            KeyCode::Enter
                if self
                    .pr_create_dialog
                    .as_ref()
                    .is_some_and(|dialog| dialog.field != PrCreateField::Body) =>
            {
                self.move_pr_create_dialog_field(1);
            }
            _ => {
                self.handle_pr_create_dialog_editor_key(key, area);
            }
        }
    }

    fn move_pr_create_dialog_field(&mut self, delta: isize) {
        if let Some(dialog) = &mut self.pr_create_dialog {
            dialog.field = dialog.field.next(delta);
            self.status = match dialog.field {
                PrCreateField::Title => "editing pull request title".to_string(),
                PrCreateField::Body => "editing pull request body".to_string(),
            };
        }
    }

    fn handle_pr_create_dialog_editor_key(&mut self, key: KeyEvent, area: Option<Rect>) {
        let should_scroll = {
            let Some(dialog) = &mut self.pr_create_dialog else {
                return;
            };
            let changed = match dialog.field {
                PrCreateField::Title => dialog.title.input_key(key, false),
                PrCreateField::Body => dialog.body.input_key(key, true),
            };
            changed && dialog.field == PrCreateField::Body
        };
        if should_scroll {
            self.scroll_pr_create_dialog_to_cursor_in_area(area);
        }
    }

    fn scroll_pr_create_dialog_body(&mut self, delta: i16, area: Option<Rect>) {
        let Some(dialog) = &mut self.pr_create_dialog else {
            return;
        };
        let (width, height) = pr_create_dialog_body_editor_size(area);
        let body = dialog.body.text();
        let max_scroll = max_comment_dialog_scroll(body, width, height);
        if delta < 0 {
            dialog.body_scroll = dialog.body_scroll.saturating_sub(delta.unsigned_abs());
        } else {
            dialog.body_scroll = dialog.body_scroll.saturating_add(delta as u16);
        }
        dialog.body_scroll = dialog.body_scroll.min(max_scroll);
    }

    fn scroll_pr_create_dialog_to_cursor_in_area(&mut self, area: Option<Rect>) {
        if let Some(dialog) = &mut self.pr_create_dialog {
            if dialog.field != PrCreateField::Body {
                return;
            }
            let (width, height) = pr_create_dialog_body_editor_size(area);
            let body = dialog.body.text();
            dialog.body_scroll = scroll_for_comment_dialog_cursor(
                body,
                dialog.body.cursor_byte(),
                width,
                height,
                dialog.body_scroll,
            );
        }
    }

    fn prepare_pr_create(&mut self) -> Option<PendingPrCreate> {
        let dialog = self.pr_create_dialog.take()?;
        let title = dialog.title.text().trim().to_string();
        let body = dialog.body.text().trim().to_string();

        if title.is_empty() {
            self.pr_create_dialog = Some(dialog);
            self.status = "pull request title is empty".to_string();
            return None;
        }

        self.pr_creating = true;
        self.status = format!("creating pull request from {}", dialog.head_ref);
        self.message_dialog = Some(message_dialog(
            "Creating Pull Request",
            "Waiting for GitHub...",
        ));
        let pending = PendingPrCreate {
            repo: dialog.repo.clone(),
            local_dir: dialog.local_dir.clone(),
            head_ref: dialog.head_ref.clone(),
            title,
            body,
            dialog,
            draft_key: self.pr_create_draft_key.clone(),
        };
        self.pending_pr_create = Some(pending.clone());
        Some(pending)
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
        if self.handle_active_mention_key(key) {
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
            _ => {
                if let Some(dialog) = &mut self.comment_dialog
                    && dialog.body.input_key(key, true)
                {
                    self.scroll_comment_dialog_to_cursor_in_area(area);
                }
            }
        }
    }

    fn scroll_comment_dialog(&mut self, delta: i16, area: Option<Rect>) {
        let Some(dialog) = &mut self.comment_dialog else {
            return;
        };
        let (width, height) = comment_dialog_editor_size(dialog, area);
        let body = dialog.body.text();
        let max_scroll = max_comment_dialog_scroll(body, width, height);
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
            let body = dialog.body.text();
            dialog.scroll = scroll_for_comment_dialog_cursor(
                body,
                dialog.body.cursor_byte(),
                width,
                height,
                dialog.scroll,
            );
        }
    }

    fn prepare_comment_submit(&mut self) -> Option<PendingCommentSubmit> {
        let dialog = self.comment_dialog.take()?;
        let body = dialog.body.text().trim().to_string();
        if body.is_empty() {
            self.comment_dialog = Some(dialog);
            self.status = "comment is empty".to_string();
            return None;
        }
        let Some(item) = self.current_item().cloned() else {
            self.comment_dialog = Some(dialog);
            self.status = "nothing selected".to_string();
            return None;
        };
        let mode = match dialog.mode.clone() {
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
        let pending = PendingCommentSubmit {
            item,
            body,
            mode,
            dialog,
            draft_key: self.comment_draft_key.clone(),
        };
        self.pending_comment_submit = Some(pending.clone());
        Some(pending)
    }

    fn start_global_search_input_with_store(
        &mut self,
        store: Option<&SnapshotStore>,
        tx: Option<&UnboundedSender<AppMsg>>,
    ) {
        self.save_current_conversation_details_state();
        if !self.is_global_search_results_view() {
            self.global_search_return_view = Some(self.active_view.clone());
        }
        self.global_search_preferred_kind = self.current_section().and_then(|section| {
            matches!(
                section.kind,
                SectionKind::PullRequests | SectionKind::Issues
            )
            .then_some(section.kind)
        });
        let repo_scope = self.current_repo_scope();
        let mut dialog = self.global_search_dialog_for_repo(repo_scope.clone());
        let labels_refreshing = repo_scope
            .as_ref()
            .and_then(|repo| {
                tx.map(|tx| start_label_suggestions_load(repo.clone(), store.cloned(), tx.clone()))
            })
            .unwrap_or(false);
        let assignees_refreshing = repo_scope
            .as_ref()
            .and_then(|repo| {
                tx.map(|tx| {
                    start_assignee_suggestions_load(repo.clone(), store.cloned(), tx.clone())
                })
            })
            .unwrap_or(false);
        dialog.labels_loading = labels_refreshing && dialog.labels.is_empty();
        dialog.assignees_loading = assignees_refreshing && dialog.assignee_candidates.is_empty();
        self.focus = FocusTarget::List;
        self.global_search_active = true;
        self.global_search_scope = repo_scope.clone();
        self.global_search_query.clear();
        self.global_search_dialog = Some(dialog);
        self.search_active = false;
        self.comment_search_active = false;
        self.filter_input_active = false;
        self.comment_dialog = None;
        self.label_dialog = None;
        self.issue_dialog = None;
        self.reaction_dialog = None;
        self.pr_action_dialog = None;
        self.review_submit_dialog = None;
        self.item_edit_dialog = None;
        self.reviewer_dialog = None;
        self.status = match self.current_repo_scope() {
            Some(repo) => format!("repo search mode in {repo}"),
            None => "search mode".to_string(),
        };
    }

    fn global_search_dialog_for_repo(&self, repo: Option<String>) -> GlobalSearchDialog {
        let mut dialog = GlobalSearchDialog {
            repo: repo.clone(),
            ..GlobalSearchDialog::default()
        };
        if let Some(repo) = repo.as_deref() {
            dialog.labels = self
                .label_suggestions_cache
                .get(repo)
                .cloned()
                .unwrap_or_default();
            dialog.assignee_candidates = self
                .assignee_suggestions_cache
                .get(repo)
                .cloned()
                .unwrap_or_default();
            dialog.author_candidates = merge_candidate_lists(
                global_search_author_candidates_from_sections(&self.sections, Some(repo)),
                dialog.assignee_candidates.clone(),
            );
        } else {
            dialog.author_candidates =
                global_search_author_candidates_from_sections(&self.sections, None);
        }
        let state_key = global_search_repo_state_key(repo.as_deref());
        if let Some(state) = self.global_search_by_repo.get(&state_key) {
            apply_global_search_state(&mut dialog, state);
        }
        dialog
    }

    fn start_filter_input(&mut self) {
        let Some(section) = self.current_section() else {
            self.status = "no section selected".to_string();
            return;
        };
        let section_key = section.key.clone();
        let section_kind = section.kind;
        if !matches!(
            section_kind,
            SectionKind::PullRequests | SectionKind::Issues
        ) {
            self.status = "quick filters are available for PR and issue sections".to_string();
            return;
        }

        self.save_current_conversation_details_state();
        self.focus = FocusTarget::List;
        self.filter_input_active = true;
        self.filter_input_query = self
            .quick_filters
            .get(&section_key)
            .map(QuickFilter::display)
            .unwrap_or_default();
        self.search_active = false;
        self.global_search_active = false;
        self.comment_search_active = false;
        self.comment_dialog = None;
        self.pr_action_dialog = None;
        self.status = "filter mode: state:closed label:bug author:alice".to_string();
    }

    fn handle_filter_input_key(
        &mut self,
        key: KeyEvent,
        config: &Config,
        tx: &UnboundedSender<AppMsg>,
    ) {
        match key.code {
            KeyCode::Esc => {
                self.filter_input_active = false;
                self.status = "filter cancelled".to_string();
            }
            KeyCode::Enter => match QuickFilter::parse(&self.filter_input_query) {
                Ok(filter) => {
                    self.filter_input_active = false;
                    self.filter_input_query.clear();
                    start_filtered_section_load(self, config, tx, filter);
                }
                Err(message) => {
                    self.status = message;
                }
            },
            KeyCode::Backspace => {
                self.filter_input_query.pop();
            }
            KeyCode::Char(value) => {
                self.filter_input_query.push(value);
            }
            _ => {}
        }
    }

    fn handle_global_search_key(
        &mut self,
        key: KeyEvent,
        config: &Config,
        tx: &UnboundedSender<AppMsg>,
    ) {
        let repo_scope = self.current_repo_scope();
        self.handle_global_search_key_with_submit(key, |query, display_query| {
            start_global_search(
                query,
                display_query,
                repo_scope.clone(),
                config.clone(),
                tx.clone(),
            );
        });
    }

    fn handle_global_search_key_with_submit<F>(&mut self, key: KeyEvent, mut submit: F)
    where
        F: FnMut(String, String),
    {
        match key.code {
            KeyCode::Esc => {
                self.remember_current_global_search_dialog();
                self.global_search_active = false;
                self.global_search_dialog = None;
                self.global_search_preferred_kind = None;
                self.status = "search cancelled".to_string();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.clear_current_global_search_dialog_conditions();
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.start_save_search_dialog();
            }
            KeyCode::Enter => {
                if self.accept_global_search_suggestion() {
                    return;
                }
                let Some(dialog) = &self.global_search_dialog else {
                    self.status = "search query is empty".to_string();
                    return;
                };
                let query = match global_search_dialog_query(dialog) {
                    Ok(query) => query,
                    Err(message) => {
                        self.status = message;
                        return;
                    }
                };
                if self.global_search_running {
                    self.status = "search already running".to_string();
                    return;
                }
                self.remember_current_global_search_dialog();
                let repo_scope = self.current_repo_scope();
                self.global_search_active = false;
                self.global_search_dialog = None;
                self.global_search_running = true;
                self.global_search_started_at = Some(Instant::now());
                self.global_search_scope = repo_scope.clone();
                let request_query =
                    scoped_global_search_query(&query, self.global_search_preferred_kind);
                self.global_search_query = query.clone();
                self.search_active = false;
                self.search_query.clear();
                self.comment_search_active = false;
                self.filter_input_active = false;
                self.status = match repo_scope {
                    Some(repo) => format!("searching {repo} for '{query}'"),
                    None => format!("searching GitHub for '{query}'"),
                };
                submit(request_query, query);
            }
            KeyCode::Tab => {
                if let Some(dialog) = &mut self.global_search_dialog {
                    dialog.field = dialog.field.next(1);
                    reset_global_search_dialog_suggestions(dialog);
                }
                self.remember_current_global_search_dialog();
            }
            KeyCode::BackTab => {
                if let Some(dialog) = &mut self.global_search_dialog {
                    dialog.field = dialog.field.next(-1);
                    reset_global_search_dialog_suggestions(dialog);
                }
                self.remember_current_global_search_dialog();
            }
            KeyCode::Down => self.move_global_search_suggestion(1),
            KeyCode::Up => self.move_global_search_suggestion(-1),
            _ => {
                if let Some(dialog) = &mut self.global_search_dialog
                    && dialog.active_editor_mut().input_key(key, false)
                {
                    reset_global_search_dialog_suggestions(dialog);
                    self.remember_current_global_search_dialog();
                }
            }
        }
    }

    fn remember_current_global_search_dialog(&mut self) {
        let Some(dialog) = &self.global_search_dialog else {
            return;
        };
        let key = global_search_repo_state_key(dialog.repo.as_deref());
        if let Some(state) = global_search_dialog_state(dialog) {
            self.global_search_by_repo.insert(key, state);
        } else {
            self.global_search_by_repo.remove(&key);
        }
    }

    fn clear_current_global_search_dialog_conditions(&mut self) {
        let Some(dialog) = &mut self.global_search_dialog else {
            return;
        };
        let key = global_search_repo_state_key(dialog.repo.as_deref());
        clear_global_search_dialog_conditions(dialog);
        self.global_search_by_repo.remove(&key);
        self.status = "search conditions cleared".to_string();
    }

    fn start_save_search_dialog(&mut self) {
        let Some(dialog) = &self.global_search_dialog else {
            return;
        };
        let Some(state) = global_search_dialog_state(dialog) else {
            self.status = "nothing to save".to_string();
            return;
        };
        let Some(repo) = dialog.repo.clone() else {
            self.status = "saved search filters need a repo scope".to_string();
            return;
        };
        let Some(kind) = self.global_search_preferred_kind else {
            self.status = "saved search filters work from PR or issue lists".to_string();
            return;
        };
        if saved_search_kind_value(kind).is_none() {
            self.status = "saved search filters work from PR or issue lists".to_string();
            return;
        }
        let name = saved_search_default_name(&repo, kind, &state);
        self.remember_current_global_search_dialog();
        self.save_search_dialog = Some(SaveSearchDialog {
            name: EditorText::from_text(name),
            repo,
            kind,
            search: state,
        });
        self.status = "name saved search filter".to_string();
    }

    fn accept_global_search_suggestion(&mut self) -> bool {
        let Some(dialog) = &mut self.global_search_dialog else {
            return false;
        };
        let in_suggestion_mode = dialog
            .suggestion_filter
            .as_ref()
            .is_some_and(|filter| filter.field == dialog.field);
        let matches = global_search_dialog_suggestion_matches(dialog);
        let Some(selected) = matches.get(dialog.selected_suggestion).cloned() else {
            return false;
        };
        if !in_suggestion_mode {
            let current = global_search_dialog_current_suggestion_value(dialog);
            if current.is_empty() || selected.eq_ignore_ascii_case(&current) {
                return false;
            }
        }
        let selected_field = dialog.field;
        apply_global_search_dialog_suggestion(dialog, &selected);
        reset_global_search_dialog_suggestions(dialog);
        self.status = format!("selected {}: {selected}", selected_field.label());
        self.remember_current_global_search_dialog();
        true
    }

    fn move_global_search_suggestion(&mut self, delta: isize) {
        let Some(dialog) = &mut self.global_search_dialog else {
            return;
        };
        let entering_suggestion_mode = dialog
            .suggestion_filter
            .as_ref()
            .is_none_or(|filter| filter.field != dialog.field);
        if entering_suggestion_mode {
            let prefix = global_search_dialog_current_suggestion_prefix(dialog);
            dialog.suggestion_filter = Some(GlobalSearchSuggestionFilter {
                field: dialog.field,
                prefix,
            });
        }
        let matches = global_search_dialog_suggestion_matches(dialog);
        if matches.is_empty() {
            self.status = global_search_dialog_suggestion_state(dialog)
                .unwrap_or("no search candidates match")
                .to_string();
            return;
        }
        let current_index = global_search_dialog_current_suggestion_index(dialog, &matches);
        if entering_suggestion_mode {
            dialog.selected_suggestion = current_index.unwrap_or(0);
        }
        let should_select_first = entering_suggestion_mode && delta > 0 && current_index.is_none();
        dialog.selected_suggestion = if should_select_first {
            0
        } else {
            move_wrapping(dialog.selected_suggestion, matches.len(), delta)
        };
        let selected = matches
            .get(dialog.selected_suggestion)
            .cloned()
            .unwrap_or_default();
        clamp_global_search_dialog_selection(dialog);
        self.status = format!("candidate {}: {selected}", dialog.field.label());
    }

    fn handle_save_search_key(&mut self, key: KeyEvent, config: &mut Config, paths: &Paths) {
        match key.code {
            KeyCode::Esc => {
                self.save_search_dialog = None;
                self.status = "saved search cancelled".to_string();
            }
            KeyCode::Enter => self.finish_save_search_dialog(config, paths),
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.finish_save_search_dialog(config, paths)
            }
            _ => {
                if let Some(dialog) = &mut self.save_search_dialog
                    && dialog.name.input_key(key, false)
                {
                    self.status = "name saved search filter".to_string();
                }
            }
        }
    }

    fn finish_save_search_dialog(&mut self, config: &mut Config, paths: &Paths) {
        let Some(dialog) = self.save_search_dialog.take() else {
            return;
        };
        let name = dialog.name.text().trim().to_string();
        if name.is_empty() {
            self.save_search_dialog = Some(dialog);
            self.status = "saved search name is empty".to_string();
            return;
        }
        let previous_saved_searches = self.global_search_saved_by_repo.clone();
        let previous_config_filters = config.saved_search_filters.clone();
        upsert_named_saved_search(
            &mut self.global_search_saved_by_repo,
            name.clone(),
            dialog.repo,
            dialog.kind,
            dialog.search,
        );
        config.saved_search_filters =
            saved_search_filters_from_map(&self.global_search_saved_by_repo);
        if let Err(error) = config.save(&paths.config_path) {
            self.global_search_saved_by_repo = previous_saved_searches;
            config.saved_search_filters = previous_config_filters;
            self.status = format!("saved search filter save failed: {error}");
            return;
        }

        self.status = format!(
            "saved search filter '{name}' saved to {}",
            paths.config_path.display()
        );
    }

    fn show_saved_search_dialog(&mut self, config: &Config) {
        self.load_saved_search_filters(config);
        if self.saved_search_candidates().is_empty() {
            self.status = "no saved search filters".to_string();
            return;
        }
        self.saved_search_dialog = Some(SavedSearchDialog::default());
        self.search_active = false;
        self.global_search_active = false;
        self.filter_input_active = false;
        self.comment_search_active = false;
        self.comment_dialog = None;
        self.status = "saved search filters".to_string();
    }

    fn handle_saved_search_key(
        &mut self,
        key: KeyEvent,
        config: &Config,
        tx: &UnboundedSender<AppMsg>,
    ) {
        match key.code {
            KeyCode::Esc => {
                self.saved_search_dialog = None;
                self.status = "saved search cancelled".to_string();
            }
            KeyCode::Enter => {
                self.submit_saved_search_selection(config, tx);
            }
            KeyCode::Down | KeyCode::Tab => self.move_saved_search_selection(1),
            KeyCode::Up | KeyCode::BackTab => self.move_saved_search_selection(-1),
            KeyCode::PageDown => self.move_saved_search_selection(8),
            KeyCode::PageUp => self.move_saved_search_selection(-8),
            KeyCode::Backspace => {
                if let Some(dialog) = &mut self.saved_search_dialog {
                    dialog.query.pop();
                    dialog.selected = 0;
                }
            }
            KeyCode::Char(value) => {
                if let Some(dialog) = &mut self.saved_search_dialog {
                    dialog.query.push(value);
                    dialog.selected = 0;
                }
            }
            _ => {}
        }
    }

    fn move_saved_search_selection(&mut self, delta: isize) {
        let Some(query) = self
            .saved_search_dialog
            .as_ref()
            .map(|dialog| dialog.query.clone())
        else {
            return;
        };
        let len = self.saved_search_candidates_for_query(&query).len();
        if let Some(dialog) = &mut self.saved_search_dialog {
            dialog.selected = move_wrapping(dialog.selected, len, delta);
        }
    }

    fn submit_saved_search_selection(&mut self, config: &Config, tx: &UnboundedSender<AppMsg>) {
        let Some(dialog) = &self.saved_search_dialog else {
            return;
        };
        let candidates = self.saved_search_candidates_for_query(&dialog.query);
        let Some(candidate) = candidates.get(dialog.selected).cloned() else {
            self.status = "no saved search filter selected".to_string();
            return;
        };
        self.saved_search_dialog = None;
        self.run_saved_search_filter(candidate, config, tx);
    }

    fn saved_search_candidates(&self) -> Vec<SavedSearchCandidate> {
        let mut candidates = self
            .global_search_saved_by_repo
            .values()
            .flat_map(|searches| {
                searches.iter().filter_map(move |saved| {
                    let saved = saved.clone().normalized()?;
                    let kind = section_kind_from_saved_search_kind(&saved.kind)?;
                    Some(SavedSearchCandidate {
                        name: saved.name,
                        repo: saved.repo,
                        kind,
                        search: saved.search,
                    })
                })
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
                .then_with(|| left.repo.cmp(&right.repo))
                .then_with(|| {
                    saved_search_kind_label(left.kind).cmp(saved_search_kind_label(right.kind))
                })
        });
        candidates
    }

    fn saved_search_candidates_for_query(&self, query: &str) -> Vec<SavedSearchCandidate> {
        let query = command_palette_normalized_text(query);
        self.saved_search_candidates()
            .into_iter()
            .filter(|candidate| {
                if query.is_empty() {
                    return true;
                }
                let haystack = command_palette_normalized_text(&format!(
                    "{} {} {} {}",
                    candidate.name,
                    candidate.repo,
                    saved_search_kind_label(candidate.kind),
                    global_search_state_display(&candidate.search)
                ));
                haystack.contains(&query) || fuzzy_score(&query, &haystack).is_some()
            })
            .collect()
    }

    fn run_saved_search_filter(
        &mut self,
        candidate: SavedSearchCandidate,
        config: &Config,
        tx: &UnboundedSender<AppMsg>,
    ) {
        let Some(target) = self.saved_search_section_target(&candidate, config) else {
            self.status = format!(
                "repo section not found for {} {}",
                candidate.repo,
                saved_search_kind_label(candidate.kind)
            );
            return;
        };
        self.save_current_conversation_details_state();
        self.switch_view(target.view.clone());
        self.set_current_section_position(target.section_position);
        self.set_current_selected_position(0);
        self.clear_current_list_scroll_offset();
        self.focus = FocusTarget::List;
        self.reset_or_restore_current_conversation_details_state();
        let filter_label = saved_search_active_filter_label(&candidate);
        self.quick_filters.remove(&target.section_key);
        self.section_filter_overrides.insert(
            target.section_key.clone(),
            SectionFilterOverride {
                display: filter_label.clone(),
                filters: target.filters.clone(),
            },
        );
        self.refreshing = true;
        self.section_page_loading = Some(SectionPageLoading {
            section_key: target.section_key.clone(),
            title: format!(
                "{} {}",
                candidate.repo,
                saved_search_kind_label(candidate.kind)
            ),
            page_label: "1".to_string(),
            started_at: Instant::now(),
        });
        self.last_refresh_request = Instant::now();
        self.status = format!("applying {filter_label}; Esc or f Enter clears it");

        let config = config.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            let section = refresh_section_page(
                target.view,
                target.kind,
                target.title,
                target.filters,
                1,
                target.page_size,
                &config,
            )
            .await;
            let _ = tx.send(AppMsg::FilterSectionLoaded {
                section_key: target.section_key,
                section,
            });
        });
    }

    fn saved_search_section_target(
        &self,
        candidate: &SavedSearchCandidate,
        config: &Config,
    ) -> Option<SavedSearchSectionTarget> {
        let repo_key = candidate.repo.to_ascii_lowercase();
        self.sections
            .iter()
            .filter(|section| section.kind == candidate.kind)
            .find(|section| {
                section_repo_scope(section).is_some_and(|repo| repo.eq_ignore_ascii_case(&repo_key))
            })
            .and_then(|section| {
                let view = section_view_key(section);
                let section_position = self
                    .sections
                    .iter()
                    .filter(|candidate_section| {
                        same_view_key(&section_view_key(candidate_section), &view)
                    })
                    .position(|candidate_section| candidate_section.key == section.key)?;
                let filters = saved_search_filter_query(
                    &self.base_filters_for_section(section),
                    &candidate.search,
                )
                .ok()?;
                Some(SavedSearchSectionTarget {
                    section_key: section.key.clone(),
                    section_position,
                    view,
                    kind: section.kind,
                    title: section.title.clone(),
                    filters,
                    page_size: section_page_size(section, config),
                })
            })
    }

    fn start_search(&mut self) {
        self.save_current_conversation_details_state();
        self.focus = FocusTarget::List;
        self.search_active = true;
        self.global_search_active = false;
        self.comment_search_active = false;
        self.filter_input_active = false;
        self.comment_dialog = None;
        self.label_dialog = None;
        self.issue_dialog = None;
        self.pr_action_dialog = None;
        self.status = "search mode".to_string();
        self.clamp_positions();
    }

    fn clear_search(&mut self) {
        self.save_current_conversation_details_state();
        self.search_active = false;
        self.global_search_active = false;
        self.filter_input_active = false;
        self.search_query.clear();
        self.focus = FocusTarget::List;
        self.status = "search cleared".to_string();
        self.clamp_positions();
        self.reset_or_restore_current_conversation_details_state();
    }

    fn push_search_char(&mut self, value: char) {
        self.save_current_conversation_details_state();
        self.search_query.push(value);
        self.set_current_selected_position(0);
        self.clamp_positions();
        self.reset_or_restore_current_conversation_details_state();
    }

    fn pop_search_char(&mut self) {
        self.save_current_conversation_details_state();
        self.search_query.pop();
        self.set_current_selected_position(0);
        self.clamp_positions();
        self.reset_or_restore_current_conversation_details_state();
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
        self.filter_input_active = false;
        self.comment_dialog = None;
        self.label_dialog = None;
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

    fn copy_github_link(&mut self) {
        let Some((url, label)) = self.selected_github_link() else {
            self.status = "no GitHub link selected".to_string();
            return;
        };

        match copy_text_to_clipboard(&url) {
            Ok(()) => {
                self.status = format!("copied {label} link");
            }
            Err(error) => {
                self.status = format!("copy failed: {error}");
            }
        }
    }

    fn copy_content(&mut self) {
        let Some((content, label)) = self.selected_copy_content() else {
            self.status = "no content selected".to_string();
            return;
        };

        match copy_text_to_clipboard(&content) {
            Ok(()) => {
                self.status = format!("copied {label}");
            }
            Err(error) => {
                self.status = format!("copy failed: {error}");
            }
        }
    }

    fn copy_block_to_clipboard(&mut self, text: &str) {
        match copy_text_to_clipboard(text) {
            Ok(()) => {
                self.status = "block copied".to_string();
            }
            Err(error) => {
                self.status = format!("copy failed: {error}");
            }
        }
    }

    fn selected_github_link(&self) -> Option<(String, &'static str)> {
        if self.focus == FocusTarget::Details
            && self.details_mode == DetailsMode::Conversation
            && let Some(url) = self
                .current_selected_comment()
                .and_then(|comment| comment.url.as_deref())
                .filter(|url| !url.trim().is_empty())
        {
            return Some((url.to_string(), "comment"));
        }

        let item = self.current_item()?;
        let url = item.url.trim();
        if url.is_empty() {
            return None;
        }
        let label = match item.kind {
            ItemKind::PullRequest => "pull request",
            ItemKind::Issue => "issue",
            ItemKind::Notification => "GitHub",
        };
        Some((url.to_string(), label))
    }

    fn selected_copy_content(&self) -> Option<(String, &'static str)> {
        if self.focus == FocusTarget::Details
            && self.details_mode == DetailsMode::Conversation
            && let Some(comment) = self.current_selected_comment()
            && !comment.body.trim().is_empty()
        {
            return Some((comment.body.clone(), "comment content"));
        }

        let item = self.current_item()?;
        let body = item
            .body
            .as_deref()
            .filter(|body| !body.trim().is_empty())?;
        let label = match item.kind {
            ItemKind::PullRequest => "pull request description",
            ItemKind::Issue => "issue description",
            ItemKind::Notification => "GitHub content",
        };
        Some((body.to_string(), label))
    }

    fn selected_open_url(&self) -> Option<String> {
        let item = self.current_item()?;
        if self.details_mode == DetailsMode::Diff && item.kind == ItemKind::PullRequest {
            return Some(pull_request_changes_url(item));
        }
        Some(item.url.clone())
    }

    fn open_url(&mut self, url: &str) {
        #[cfg(not(test))]
        let result: std::io::Result<()> = open::that(url);
        #[cfg(test)]
        let result: std::io::Result<()> = Ok(());

        match result {
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
            .map(item_supports_metadata_edit)
            .unwrap_or(false)
    }

    fn current_item_supports_labels(&self) -> bool {
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

    fn item_has_unseen_details(&self, item: &WorkItem) -> bool {
        let Some(key) = work_item_details_memory_key(item) else {
            return false;
        };
        let Some(updated_at) = item.updated_at else {
            return false;
        };
        self.viewed_item_at
            .get(&key)
            .is_some_and(|viewed_at| updated_at > *viewed_at)
    }

    fn mark_current_details_stale_if_unseen(&mut self) {
        let Some(item) = self.current_item().cloned() else {
            return;
        };
        if self.details_cache_outdated(&item) {
            self.details_stale.insert(item.id);
        }
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
        if let Some(key) = work_item_details_memory_key(&item) {
            self.viewed_item_at.insert(key, Utc::now());
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

    fn sync_recent_details_visit(&mut self, now: Instant) {
        let active_item =
            if self.focus == FocusTarget::Details && !self.recent_details_visit_suspended() {
                self.current_item()
                    .and_then(|item| recent_item_from_work_item(item, Utc::now()))
            } else {
                None
            };

        match active_item {
            Some(item) => {
                let same_item = self
                    .details_visit
                    .as_ref()
                    .is_some_and(|visit| recent_item_key(&visit.item) == recent_item_key(&item));
                if same_item {
                    if let Some(visit) = &mut self.details_visit {
                        visit.item = item;
                    }
                    self.promote_elapsed_details_visit(now);
                } else {
                    self.finish_details_visit(now);
                    self.details_visit = Some(DetailsVisitState {
                        item,
                        started_at: now,
                        recorded: false,
                    });
                }
            }
            None => self.finish_details_visit(now),
        }
    }

    fn recent_details_visit_suspended(&self) -> bool {
        self.setup_dialog.is_some()
            || self.startup_dialog.is_some()
            || self.message_dialog.is_some()
            || self.help_dialog
            || self.command_palette.is_some()
            || self.project_switcher.is_some()
            || self.top_menu_switcher.is_some()
            || self.theme_switcher.is_some()
            || self.recent_items_dialog.is_some()
            || self.project_add_dialog.is_some()
            || self.project_remove_dialog.is_some()
            || self.cache_clear_dialog.is_some()
            || self.search_active
            || self.comment_search_active
            || self.global_search_active
            || self.filter_input_active
            || self.comment_dialog.is_some()
            || self.reaction_dialog.is_some()
            || self.label_dialog.is_some()
            || self.issue_dialog.is_some()
            || self.pr_create_dialog.is_some()
            || self.review_submit_dialog.is_some()
            || self.item_edit_dialog.is_some()
            || self.pr_action_dialog.is_some()
            || self.milestone_dialog.is_some()
            || self.assignee_dialog.is_some()
            || self.reviewer_dialog.is_some()
    }

    fn promote_elapsed_details_visit(&mut self, now: Instant) {
        let item = match &mut self.details_visit {
            Some(visit)
                if !visit.recorded && now.duration_since(visit.started_at) >= RECENT_ITEM_DWELL =>
            {
                visit.recorded = true;
                let mut item = visit.item.clone();
                item.visited_at = Utc::now();
                Some(item)
            }
            _ => None,
        };
        if let Some(item) = item {
            self.upsert_recent_item(item);
        }
    }

    fn finish_details_visit(&mut self, now: Instant) {
        let Some(visit) = self.details_visit.take() else {
            return;
        };
        if now.duration_since(visit.started_at) < RECENT_ITEM_DWELL {
            return;
        }
        let mut item = visit.item;
        item.visited_at = Utc::now();
        self.upsert_recent_item(item);
    }

    fn upsert_recent_item(&mut self, item: RecentItem) {
        if !matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest) {
            return;
        }
        let key = recent_item_key(&item);
        self.recent_items
            .retain(|existing| recent_item_key(existing) != key);
        self.recent_items.push(item);
        self.recent_items
            .sort_by_key(|item| std::cmp::Reverse(item.visited_at));
        self.recent_items.truncate(MAX_RECENT_ITEMS);
        self.recent_items_dirty = true;
    }

    fn take_recent_items_dirty(&mut self) -> bool {
        let dirty = self.recent_items_dirty;
        self.recent_items_dirty = false;
        dirty
    }

    fn jump_to_recent_item(&mut self, item: &RecentItem) -> bool {
        let Some(target) = self.find_recent_item_target(item) else {
            self.status = format!("recent item not loaded: {}", recent_item_label(item));
            return false;
        };

        self.sync_recent_details_visit(Instant::now());
        self.remember_current_view_snapshot();
        self.active_view = target.view;
        self.set_current_section_position(target.section_position);
        self.set_current_selected_position(target.selected_position);
        self.clear_current_list_scroll_offset();
        self.search_active = false;
        self.search_query.clear();
        self.comment_search_active = false;
        self.comment_search_query.clear();
        self.global_search_active = false;
        self.filter_input_active = false;
        self.comment_dialog = None;
        self.project_switcher = None;
        self.theme_switcher = None;
        self.recent_items_dialog = None;
        self.details_mode = DetailsMode::Conversation;
        self.diff_return_state = None;
        self.reset_or_restore_current_conversation_details_state();
        self.focus_details();
        self.status = format!("recent item opened: {}", recent_item_label(item));
        true
    }

    fn find_recent_item_target(&self, recent: &RecentItem) -> Option<RecentItemTarget> {
        let mut views = vec![self.active_view.clone()];
        for view in self.view_tabs() {
            if !views
                .iter()
                .any(|candidate| same_view_key(candidate, &view.key))
            {
                views.push(view.key);
            }
        }

        views
            .iter()
            .find_map(|view| self.find_recent_item_target_in_view(view, recent))
    }

    fn find_recent_item_target_in_view(
        &self,
        view: &str,
        recent: &RecentItem,
    ) -> Option<RecentItemTarget> {
        self.sections
            .iter()
            .filter(|section| same_view_key(&section_view_key(section), view))
            .enumerate()
            .find_map(|(section_position, section)| {
                let mut selected_position = 0;
                for item in &section.items {
                    if self.ignored_items.contains(&item.id) {
                        continue;
                    }
                    if recent_item_matches_work_item(recent, item) {
                        return Some(RecentItemTarget {
                            view: section_view_key(section),
                            section_position,
                            selected_position,
                        });
                    }
                    selected_position += 1;
                }
                None
            })
    }

    fn save_current_conversation_details_state(&mut self) {
        if self.focus == FocusTarget::Details {
            self.finish_details_visit(Instant::now());
        }
        if self.details_mode == DetailsMode::Diff {
            self.save_current_diff_mode_state();
            return;
        }
        self.remember_current_conversation_details_position();
    }

    fn remember_current_conversation_details_position(&mut self) {
        if self.details_mode != DetailsMode::Conversation {
            return;
        }
        let Some(item) = self.current_item() else {
            return;
        };
        if !item_supports_details_memory(item) {
            return;
        }
        let keys = work_item_details_memory_keys(item);
        let state = ConversationDetailsState {
            details_scroll: self.details_scroll,
            selected_comment_index: self.selected_comment_index,
        };
        for key in keys {
            self.conversation_details_state.insert(key, state);
        }
    }

    fn restore_current_conversation_details_state(&mut self) {
        if self.details_mode != DetailsMode::Conversation {
            return;
        }
        let Some(item) = self.current_item() else {
            return;
        };
        let state = work_item_details_memory_keys(item)
            .into_iter()
            .find_map(|key| self.conversation_details_state.get(&key).copied());
        if let Some(state) = state {
            self.details_scroll = state.details_scroll;
            self.selected_comment_index = state.selected_comment_index;
            if self.current_comments().is_some() {
                self.clamp_selected_comment();
            }
        }
    }

    fn reset_or_restore_current_conversation_details_state(&mut self) {
        self.details_scroll = 0;
        self.selected_comment_index = 0;
        self.restore_current_conversation_details_state();
    }

    fn current_selected_comment(&self) -> Option<&CommentPreview> {
        if self.comment_selection_cleared() {
            return None;
        }
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
        if self.comment_selection_cleared() {
            return;
        }
        let len = self.current_comments().map(Vec::len).unwrap_or(0);
        if len == 0 {
            self.selected_comment_index = 0;
        } else {
            self.selected_comment_index = self.selected_comment_index.min(len - 1);
        }
    }

    fn filtered_indices(&self, section: &SectionSnapshot) -> Vec<usize> {
        filtered_indices(section, &self.search_query)
            .into_iter()
            .filter(|index| {
                section
                    .items
                    .get(*index)
                    .is_some_and(|item| !self.ignored_items.contains(&item.id))
            })
            .collect()
    }

    fn ignored_count_for_section(&self, section: &SectionSnapshot) -> usize {
        section
            .items
            .iter()
            .filter(|item| self.ignored_items.contains(&item.id))
            .count()
    }

    fn ignore_current_item(&mut self) {
        let Some(item) = self.current_item().cloned() else {
            self.status = "nothing to ignore".to_string();
            return;
        };
        if !matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest) {
            self.status = "only issues and pull requests can be ignored".to_string();
            return;
        }

        let label = item_kind_label(item.kind);
        let number = item
            .number
            .map(|number| format!(" #{number}"))
            .unwrap_or_default();
        self.ignored_items.insert(item.id.clone());
        self.focus = FocusTarget::List;
        self.details_mode = DetailsMode::Conversation;
        self.details_scroll = 0;
        self.selected_comment_index = 0;
        self.comment_dialog = None;
        self.label_dialog = None;
        self.issue_dialog = None;
        self.pr_action_dialog = None;
        self.item_edit_dialog = None;
        self.clamp_positions();
        self.reset_or_restore_current_conversation_details_state();
        self.status = format!("ignored {label}{number}; use Info to inspect ignored state");
    }

    fn clear_ignored_items(&mut self) {
        let count = self.ignored_items.len();
        self.ignored_items.clear();
        self.clamp_positions();
        self.status = if count == 0 {
            "ignored list already empty".to_string()
        } else {
            format!("cleared {count} ignored item(s)")
        };
    }

    fn view_tabs(&self) -> Vec<ViewTab> {
        let mut tabs = Vec::new();
        for kind in [
            SectionKind::Notifications,
            SectionKind::PullRequests,
            SectionKind::Issues,
        ] {
            let key = builtin_view_key(kind);
            if self
                .sections
                .iter()
                .any(|section| section_view_key(section) == key)
            {
                let label = if matches!(kind, SectionKind::Notifications) {
                    let unread = self.unread_notification_count();
                    if unread == 0 {
                        kind.label().to_string()
                    } else {
                        format!("{} ({unread})", kind.label())
                    }
                } else {
                    kind.label().to_string()
                };
                tabs.push(ViewTab { key, label });
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
            let label = self.repo_view_tab_label(&key, label);
            tabs.push(ViewTab { key, label });
        }

        tabs
    }

    fn repo_view_tab_label(&self, key: &str, label: String) -> String {
        let Some(unseen) = self.repo_unseen_items_for_view(key) else {
            return label;
        };
        let (issues, pull_requests) = unseen.counts();
        if issues == 0 && pull_requests == 0 {
            label
        } else {
            format!("{label}({issues}|{pull_requests})")
        }
    }

    fn unread_notification_count(&self) -> usize {
        let mut seen = HashSet::new();
        self.sections
            .iter()
            .filter(|section| matches!(section.kind, SectionKind::Notifications))
            .flat_map(|section| section.items.iter())
            .filter(|item| !self.ignored_items.contains(&item.id))
            .filter(|item| item.unread.unwrap_or(false))
            .filter(|item| seen.insert(item.id.clone()))
            .count()
    }

    fn has_unread_notifications(&self) -> bool {
        self.unread_notification_count() > 0
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
        if review.is_resolved {
            parts.push("resolved".to_string());
        }
        if review.is_outdated {
            parts.push("outdated".to_string());
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

fn local_datetime(value: Option<DateTime<Utc>>) -> String {
    value
        .map(|value| {
            value
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M %:z")
                .to_string()
        })
        .unwrap_or_else(|| "-".to_string())
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

fn dedupe_label_names(labels: Vec<String>) -> Vec<String> {
    let mut deduped = Vec::new();
    for label in labels
        .into_iter()
        .map(|label| label.trim().to_string())
        .filter(|label| !label.is_empty())
    {
        if !deduped
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&label))
        {
            deduped.push(label);
        }
    }
    deduped
}

fn same_names_ignore_case(left: &[String], right: &[String]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter().all(|name| {
        right
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(name))
    })
}

fn item_is_open_pull_request(item: &WorkItem) -> bool {
    item.state
        .as_deref()
        .is_none_or(|state| state.eq_ignore_ascii_case("open"))
}

fn item_is_draft_pull_request(item: &WorkItem) -> bool {
    item.extra
        .as_deref()
        .is_some_and(|extra| extra.split_whitespace().any(|part| part == "draft"))
}

fn pr_action_requires_open_pull_request(action: PrAction) -> bool {
    matches!(
        action,
        PrAction::UpdateBranch | PrAction::ConvertToDraft | PrAction::MarkReadyForReview
    )
}

fn mark_item_draft(item: &mut WorkItem) {
    if !item_is_draft_pull_request(item) {
        item.extra = Some(match item.extra.take() {
            Some(extra) if !extra.trim().is_empty() => format!("{extra} draft"),
            _ => "draft".to_string(),
        });
    }
}

fn mark_item_ready_for_review(item: &mut WorkItem) {
    let Some(extra) = item.extra.take() else {
        return;
    };
    let remaining = extra
        .split_whitespace()
        .filter(|part| *part != "draft")
        .collect::<Vec<_>>()
        .join(" ");
    item.extra = (!remaining.is_empty()).then_some(remaining);
}

#[cfg(test)]
mod tests;
