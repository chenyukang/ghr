use super::layout::{body_area, body_areas};
use super::pr_checkout::{
    command_output_text, pr_checkout_command_args, pr_checkout_command_display,
};
use super::*;
use crate::model::CommentPreviewKind;
use std::sync::atomic::{AtomicUsize, Ordering};

static CHECKOUT_TEST_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn handle_event_batch(
    app: &mut AppState,
    events: Vec<Event>,
    area: Rect,
    config: &Config,
    paths: &Paths,
    store: &SnapshotStore,
    tx: &UnboundedSender<AppMsg>,
) -> bool {
    let mut config = config.clone();
    handle_event_batch_mut(app, events, area, &mut config, paths, store, tx)
}

impl AppState {
    fn new(active_view: SectionKind, sections: Vec<SectionSnapshot>) -> Self {
        Self::with_ui_state(active_view, sections, UiState::default())
    }

    fn current_diff_review_target(&self) -> Option<DiffReviewTarget> {
        self.current_diff_review_target_result().ok().flatten()
    }

    fn show_conversation(&mut self) {
        self.details_mode = DetailsMode::Conversation;
        self.diff_return_state = None;
        self.focus = FocusTarget::Details;
        self.reset_or_restore_current_conversation_details_state();
        self.status = "conversation focused".to_string();
    }

    fn start_assignee_dialog(
        &mut self,
        action: AssigneeAction,
        tx: Option<&UnboundedSender<AppMsg>>,
    ) {
        self.start_assignee_dialog_with_store(action, None, tx);
    }

    fn start_reviewer_dialog(&mut self, action: ReviewerAction) {
        self.start_reviewer_dialog_with_store(action, None, None);
    }

    fn handle_comment_dialog_key(
        &mut self,
        key: KeyEvent,
        tx: &UnboundedSender<AppMsg>,
        area: Option<Rect>,
    ) {
        self.handle_comment_dialog_key_with_store(key, None, tx, area);
    }

    fn start_add_label_dialog(&mut self, tx: Option<&UnboundedSender<AppMsg>>) {
        self.start_add_label_dialog_with_store(None, tx);
    }

    fn start_global_search_input(&mut self) {
        self.start_global_search_input_with_store(None, None);
    }
}

fn rendered_line_without_block_copy(document: &DetailsDocument, line_index: usize) -> String {
    let rendered = document.lines[line_index].to_string();
    let Some(copy_exclusion) = document.copy_exclusions.iter().find(|exclusion| {
        exclusion.line == line_index
            && document.actions.iter().any(|action| {
                action.line == line_index
                    && matches!(&action.action, DetailAction::CopyBlock(_))
                    && exclusion.start <= action.start
                    && exclusion.end >= action.end
            })
    }) else {
        return rendered;
    };

    take_display_width(&rendered, usize::from(copy_exclusion.start))
}

fn rendered_lines_without_block_copy(document: &DetailsDocument) -> Vec<String> {
    document
        .lines
        .iter()
        .enumerate()
        .map(|(line_index, _)| rendered_line_without_block_copy(document, line_index))
        .collect()
}

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
    let error = "gh search prs --json number,title,body,repository,author,createdAt,updatedAt,url,state,isDraft,labels,commentsCount --limit 500 -- repo:rust-lang/rust is:open failed: HTTP 403: API rate limit exceeded for user ID 230646";

    assert_eq!(compact_error_label(error), "GitHub search rate limited");
    assert!(!compact_error_label(error).contains("--json"));
}

#[test]
fn error_chain_message_keeps_gh_failure_detail() {
    let error = anyhow::anyhow!(
        "gh pr create --repo owner/repo --head feature failed: a pull request already exists"
    )
    .context("failed to create pull request in owner/repo");
    let message = error_chain_message(error);

    assert!(message.contains("failed to create pull request in owner/repo"));
    assert!(message.contains("a pull request already exists"));
    assert_eq!(
        operation_error_body(&message),
        "a pull request already exists"
    );
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
    let rendered_lines = document
        .lines
        .iter()
        .map(Line::to_string)
        .collect::<Vec<_>>();
    let rendered = rendered_lines.join("\n");

    assert!(rendered.contains("Conversation | Diff"));
    assert!(rendered.contains("▾ src/lib.rs"));
    assert!(rendered.contains("@@ -1,2 +1,2 @@"));
    assert!(rendered.contains("   1      │ - old"));
    assert!(rendered.contains("        1 │ + new"));
    assert!(!rendered.contains("selected:"));
    assert!(!rendered.contains("selected range:"));
    let file_line = rendered_lines
        .iter()
        .position(|line| line.contains("▾ src/lib.rs"))
        .expect("diff file line");
    let removed_line = rendered_lines
        .iter()
        .position(|line| line.contains("   1      │ - old"))
        .expect("removed diff line");
    let added_line = rendered_lines
        .iter()
        .position(|line| line.contains("        1 │ + new"))
        .expect("added diff line");
    assert_eq!(document.diff_files, vec![file_line]);
    assert_document_link_for_text(
        &document,
        "src/lib.rs",
        "https://github.com/rust-lang/rust/blob/HEAD/src/lib.rs#L1",
    );
    assert_eq!(document.diff_line_at(removed_line), Some(0));
    assert_eq!(document.diff_line_at(added_line), Some(1));
}

#[test]
fn diff_file_header_links_to_head_branch_near_selected_line() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.show_diff();
    app.action_hints.insert(
        "1".to_string(),
        ActionHintState::Loaded(ActionHints {
            head: Some(PullRequestBranch {
                repository: "nervosnetwork/fiber".to_string(),
                branch: "feature/diff-links".to_string(),
            }),
            ..ActionHints::default()
        }),
    );
    app.diffs.insert(
        "1".to_string(),
        DiffState::Loaded(
            parse_pull_request_diff(
                r#"diff --git a/fiber-js/README.md b/fiber-js/README.md
--- a/fiber-js/README.md
+++ b/fiber-js/README.md
@@ -315,3 +315,9 @@
 source = "registry+https://github.com/rust-lang/crates.io-index"
 checksum = "4c7f02d4ea65f2c1853089ffd8d2787bdbc63de2f0d29dedbcf8ccdfa0ccd4cf"
 context
+[[package]]
+name = "base32"
+version = "0.4.0"
+source = "registry+https://github.com/rust-lang/crates.io-index"
+checksum = "23ce669cd6c8588f79e15cf450314f9638f967fc5770ff1c7c1deb0925ea7cfa"
"#,
            )
            .expect("parse diff"),
        ),
    );

    let document = build_details_document(&app, 120);

    assert_document_link_for_text(
        &document,
        "fiber-js/README.md",
        "https://github.com/nervosnetwork/fiber/blob/feature/diff-links/fiber-js/README.md#L318",
    );
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
        is_resolved: false,
        is_outdated: false,
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
        rendered.iter().any(|line| line.contains("│ 💬 + new")),
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
    assert_document_link_for_text(&document, "alice", "https://github.com/alice");
    assert_document_link_for_text(&document, "bob", "https://github.com/bob");
    let right_border_column = comment_right_border_column(120);
    let selected_right_border_columns = rendered
        .iter()
        .filter(|line| line.ends_with('┃'))
        .map(|line| display_width(line).saturating_sub(1))
        .collect::<Vec<_>>();
    let selected_top_border = rendered
        .iter()
        .find(|line| line.trim_start().starts_with('┏'))
        .expect("selected inline comment top border");
    let selected_bottom_border = rendered
        .iter()
        .find(|line| line.trim_start().starts_with('┗'))
        .expect("selected inline comment bottom border");
    assert!(
        selected_top_border.ends_with('┓'),
        "selected inline top border should close with a corner: {rendered:?}"
    );
    assert!(
        selected_bottom_border.ends_with('┛'),
        "selected inline bottom border should close with a corner: {rendered:?}"
    );
    assert!(
        !selected_right_border_columns.is_empty(),
        "selected inline comment should render a right border: {rendered:?}"
    );
    assert!(
        selected_right_border_columns
            .iter()
            .all(|column| *column == right_border_column),
        "selected inline comment right border should align: {rendered:?}"
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
        display_width(&rendered[header_line][..rendered[header_line].find("open").unwrap()]) as u16;
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
fn text_selection_mode_omits_inline_thread_markers() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.show_diff();
    app.focus_details();
    app.mouse_capture_enabled = false;
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
        is_resolved: false,
        is_outdated: false,
    };
    let mut parent = comment("alice", "Please keep this inline.", None);
    parent.id = Some(1);
    parent.review = Some(review.clone());
    let mut child = comment("bob", "Thread reply", None);
    child.id = Some(2);
    child.parent_id = Some(1);
    child.review = Some(review);
    app.details
        .insert("1".to_string(), DetailState::Loaded(vec![parent, child]));

    let rendered = build_details_document(&app, 120)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert!(
        rendered.iter().any(|line| line.contains("bob")),
        "reply should still be visible: {rendered:?}"
    );
    assert!(
        !rendered.iter().any(|line| line.contains('↳')),
        "text selection mode should not render thread marker prefixes: {rendered:?}"
    );
}

#[test]
fn diff_mode_can_hide_inline_review_comment_bodies_but_keeps_markers() {
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
@@ -1 +1 @@
-old
+new
"#,
            )
            .expect("parse diff"),
        ),
    );
    let mut inline = comment("alice", "Hidden until marker is opened.", None);
    inline.id = Some(1);
    inline.review = Some(crate::model::ReviewCommentPreview {
        path: "src/lib.rs".to_string(),
        line: Some(1),
        original_line: None,
        start_line: None,
        original_start_line: None,
        side: Some("RIGHT".to_string()),
        start_side: None,
        diff_hunk: None,
        is_resolved: false,
        is_outdated: false,
    });
    app.details
        .insert("1".to_string(), DetailState::Loaded(vec![inline]));

    app.toggle_diff_inline_comments();
    let document = build_details_document(&app, 120);
    let rendered = document
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert!(
        rendered.iter().any(|line| line.contains("│ 💬 + new")),
        "diff line should keep a clickable inline-comment marker when comments are hidden: {rendered:?}"
    );
    assert!(
        document
            .inline_comment_marker_at(
                rendered
                    .iter()
                    .position(|line| line.contains("│ 💬 + new"))
                    .expect("marker line")
            )
            .is_some(),
        "marker line should be discoverable for mouse clicks"
    );
    assert!(
        !rendered
            .iter()
            .any(|line| line.contains("alice") || line.contains("Hidden until marker is opened.")),
        "inline comment body/header should be hidden until marker click: {rendered:?}"
    );
}

#[test]
fn clicking_hidden_diff_inline_comment_marker_reveals_that_thread() {
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
@@ -1 +1 @@
-old
+new
"#,
            )
            .expect("parse diff"),
        ),
    );
    let mut inline = comment("alice", "Revealed by marker click.", None);
    inline.id = Some(1);
    inline.review = Some(crate::model::ReviewCommentPreview {
        path: "src/lib.rs".to_string(),
        line: Some(1),
        original_line: None,
        start_line: None,
        original_start_line: None,
        side: Some("RIGHT".to_string()),
        start_side: None,
        diff_hunk: None,
        is_resolved: false,
        is_outdated: false,
    });
    app.details
        .insert("1".to_string(), DetailState::Loaded(vec![inline]));

    app.toggle_diff_inline_comments();
    let marker_indices = build_details_document(&app, 120)
        .inline_comment_marker_at(
            build_details_document(&app, 120)
                .lines
                .iter()
                .position(|line| line.to_string().contains("│ 💬 + new"))
                .expect("marker line"),
        )
        .expect("marker indices")
        .to_vec();
    app.toggle_revealed_diff_inline_comments(&marker_indices);
    let rendered = build_details_document(&app, 120)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert!(
        rendered.iter().any(|line| line.contains("alice")),
        "marker click should reveal the inline comment header: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Revealed by marker click.")),
        "marker click should reveal the inline comment body: {rendered:?}"
    );
}

#[test]
fn mouse_clicking_hidden_diff_inline_comment_marker_toggles_that_thread() {
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
@@ -1 +1 @@
-old
+new
"#,
            )
            .expect("parse diff"),
        ),
    );
    let mut inline = comment("alice", "Toggle me from the marker.", None);
    inline.id = Some(1);
    inline.review = Some(crate::model::ReviewCommentPreview {
        path: "src/lib.rs".to_string(),
        line: Some(1),
        original_line: None,
        start_line: None,
        original_start_line: None,
        side: Some("RIGHT".to_string()),
        start_side: None,
        diff_hunk: None,
        is_resolved: false,
        is_outdated: false,
    });
    app.details
        .insert("1".to_string(), DetailState::Loaded(vec![inline]));

    app.toggle_diff_inline_comments();
    let area = Rect::new(0, 0, 120, 32);
    let details_area = details_area_for(&app, area);
    let inner = block_inner(details_area);
    let document = build_details_document(&app, inner.width);
    let marker_line = document
        .lines
        .iter()
        .position(|line| line.to_string().contains("│ 💬 + new"))
        .expect("marker line");

    let click_marker = |app: &mut AppState| {
        handle_mouse(
            app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: inner.x + 2,
                row: inner.y + marker_line as u16,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
            area,
        );
    };

    click_marker(&mut app);
    let rendered = build_details_document(&app, inner.width)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert_eq!(app.status, "diff comment thread shown");
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Toggle me from the marker.")),
        "first marker click should reveal the inline comment body: {rendered:?}"
    );

    click_marker(&mut app);
    let rendered = build_details_document(&app, inner.width)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert_eq!(app.status, "diff comment thread hidden");
    assert!(
        !rendered
            .iter()
            .any(|line| line.contains("alice") || line.contains("Toggle me from the marker.")),
        "second marker click should hide the inline comment thread: {rendered:?}"
    );
    assert!(
        !app.revealed_diff_inline_comments.contains_key("1"),
        "hidden thread should not leave an empty reveal set"
    );
}

#[test]
fn mouse_clicking_inline_comment_author_opens_profile() {
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
@@ -1 +1 @@
-old
+new
"#,
            )
            .expect("parse diff"),
        ),
    );

    let mut comment = comment(
        "cuviper",
        "I just meant stable vs. this nightly.",
        Some("https://github.com/rust-lang/rust/pull/1#discussion_r1"),
    );
    comment.id = Some(1);
    comment.review = Some(crate::model::ReviewCommentPreview {
        path: "src/lib.rs".to_string(),
        line: Some(1),
        original_line: None,
        start_line: None,
        original_start_line: None,
        side: Some("RIGHT".to_string()),
        start_side: None,
        diff_hunk: None,
        is_resolved: false,
        is_outdated: false,
    });
    app.details
        .insert("1".to_string(), DetailState::Loaded(vec![comment]));

    let area = Rect::new(0, 0, 140, 30);
    let details = details_area_for(&app, area);
    let inner = block_inner(details);
    let document = build_details_document(&app, inner.width);
    let author_line = document
        .lines
        .iter()
        .position(|line| {
            let rendered = line.to_string();
            rendered.contains("cuviper") && rendered.contains("reply")
        })
        .expect("inline comment author line");
    let rendered = document.lines[author_line].to_string();
    let author_column =
        display_width(&rendered[..rendered.find("cuviper").expect("author column")]) as u16;
    let author_row = inner.y + author_line as u16 - app.details_scroll;

    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: inner.x + author_column,
            row: author_row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
        area,
    );

    assert_eq!(app.status, "opened https://github.com/cuviper");
}

#[test]
fn diff_details_render_resolved_and_outdated_review_comment_states() {
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

    let mut resolved = comment("alice", "Fixed now.", None);
    resolved.id = Some(1);
    resolved.review = Some(crate::model::ReviewCommentPreview {
        path: "src/lib.rs".to_string(),
        line: Some(1),
        original_line: None,
        start_line: None,
        original_start_line: None,
        side: Some("RIGHT".to_string()),
        start_side: None,
        diff_hunk: None,
        is_resolved: true,
        is_outdated: false,
    });
    let mut outdated = comment("bob", "This pointed at an old line.", None);
    outdated.id = Some(2);
    outdated.review = Some(crate::model::ReviewCommentPreview {
        path: "src/lib.rs".to_string(),
        line: Some(99),
        original_line: Some(99),
        start_line: None,
        original_start_line: None,
        side: Some("RIGHT".to_string()),
        start_side: None,
        diff_hunk: None,
        is_resolved: false,
        is_outdated: true,
    });
    app.details.insert(
        "1".to_string(),
        DetailState::Loaded(vec![resolved, outdated]),
    );

    let rendered = build_details_document(&app, 120)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("💬 + new"));
    assert!(!rendered.contains("✓ + new"));
    assert!(!rendered.contains("◌ + new"));
    assert!(rendered.contains("resolved"));
    assert!(rendered.contains("outdated"));
    assert!(rendered.contains("not attached to a current diff line"));
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
    assert!(
        !app.diff_mark.contains_key("1"),
        "a plain mouse click should select one line without starting a mark"
    );
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
fn h_and_l_page_diff_details_instead_of_jumping_comments() {
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
        key(KeyCode::Char('h')),
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
        key(KeyCode::Char('l')),
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
fn h_and_l_page_diff_details_while_diff_files_are_focused() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    let mut diff_lines = String::new();
    for line in 1..=40 {
        diff_lines.push_str(&format!(" line {line}\n"));
    }
    app.show_diff();
    app.focus_list();
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
        key(KeyCode::Char('h')),
        &config,
        &store,
        &tx,
        Some(area)
    ));
    assert_eq!(app.focus, FocusTarget::List);
    assert_eq!(app.selected_diff_file.get("1").copied().unwrap_or(0), 0);
    assert!(app.details_scroll > 0);
    let after_next = app.details_scroll;
    let (expected_index, expected_line) = first_visible_diff_line(&app, area);
    assert_eq!(app.selected_diff_line.get("1"), Some(&expected_index));
    assert_eq!(selected_diff_document_line(&app, area), Some(expected_line));

    assert!(!handle_key_in_area(
        &mut app,
        key(KeyCode::Char('l')),
        &config,
        &store,
        &tx,
        Some(area)
    ));
    assert_eq!(app.focus, FocusTarget::List);
    assert_eq!(app.selected_diff_file.get("1").copied().unwrap_or(0), 0);
    assert!(app.details_scroll < after_next);
    let (expected_index, expected_line) = first_visible_diff_line(&app, area);
    assert_eq!(app.selected_diff_line.get("1"), Some(&expected_index));
    assert_eq!(selected_diff_document_line(&app, area), Some(expected_line));
}

#[test]
fn h_and_l_cross_diff_files_without_wrapping_at_global_edges() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    let mut first_file_lines = String::new();
    let mut second_file_lines = String::new();
    for line in 1..=50 {
        first_file_lines.push_str(&format!(" first file line {line}\n"));
        second_file_lines.push_str(&format!(" second file line {line}\n"));
    }
    app.show_diff();
    app.focus_details();
    app.diffs.insert(
            "1".to_string(),
            DiffState::Loaded(
                parse_pull_request_diff(&format!(
                    "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,50 +1,50 @@\n{first_file_lines}diff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -1,50 +1,50 @@\n{second_file_lines}"
                ))
                .expect("parse diff"),
            ),
        );
    let area = Rect::new(0, 0, 120, 24);
    app.scroll_diff_details_to_bottom(Some(area));
    let first_bottom_scroll = app.details_scroll;
    assert_eq!(app.selected_diff_file.get("1").copied().unwrap_or(0), 0);

    assert!(!handle_key_in_area(
        &mut app,
        key(KeyCode::Char('h')),
        &config,
        &store,
        &tx,
        Some(area)
    ));
    assert_eq!(app.selected_diff_file.get("1"), Some(&1));
    assert_eq!(app.details_scroll, 0);
    assert_eq!(
        app.current_diff_review_target().map(|target| target.path),
        Some("b.rs".to_string())
    );

    app.scroll_diff_details_to_bottom(Some(area));
    let second_bottom_scroll = app.details_scroll;
    assert!(!handle_key_in_area(
        &mut app,
        key(KeyCode::Char('h')),
        &config,
        &store,
        &tx,
        Some(area)
    ));
    assert_eq!(app.selected_diff_file.get("1"), Some(&1));
    assert_eq!(app.details_scroll, second_bottom_scroll);
    assert_eq!(app.status, "already at bottom of diff");

    app.scroll_diff_details_to_top(Some(area));
    assert!(!handle_key_in_area(
        &mut app,
        key(KeyCode::Char('l')),
        &config,
        &store,
        &tx,
        Some(area)
    ));
    assert_eq!(app.selected_diff_file.get("1"), Some(&0));
    assert_eq!(app.details_scroll, first_bottom_scroll);
    assert_eq!(
        app.current_diff_review_target().map(|target| target.path),
        Some("a.rs".to_string())
    );
    let (expected_index, expected_line) = first_visible_diff_line(&app, area);
    assert_eq!(app.selected_diff_line.get("1"), Some(&expected_index));
    assert_eq!(selected_diff_document_line(&app, area), Some(expected_line));

    app.scroll_diff_details_to_top(Some(area));
    assert!(!handle_key_in_area(
        &mut app,
        key(KeyCode::Char('l')),
        &config,
        &store,
        &tx,
        Some(area)
    ));
    assert_eq!(app.selected_diff_file.get("1"), Some(&0));
    assert_eq!(app.details_scroll, 0);
    assert_eq!(app.status, "already at top of diff");
}

#[test]
fn n_and_p_focus_comments_in_diff_details() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    let mut first = comment("alice", "first inline", None);
    first.review = Some(crate::model::ReviewCommentPreview {
        path: "src/lib.rs".to_string(),
        line: Some(1),
        original_line: None,
        start_line: None,
        original_start_line: None,
        side: Some("RIGHT".to_string()),
        start_side: None,
        diff_hunk: None,
        is_resolved: false,
        is_outdated: false,
    });
    let mut second = comment("bob", "second inline", None);
    second.review = Some(crate::model::ReviewCommentPreview {
        path: "src/lib.rs".to_string(),
        line: Some(2),
        original_line: None,
        start_line: None,
        original_start_line: None,
        side: Some("RIGHT".to_string()),
        start_side: None,
        diff_hunk: None,
        is_resolved: false,
        is_outdated: false,
    });
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
-old1
+new1
-old2
+new2
"#,
            )
            .expect("parse diff"),
        ),
    );
    app.details
        .insert("1".to_string(), DetailState::Loaded(vec![first, second]));
    let area = Rect::new(0, 0, 120, 36);

    assert!(!handle_key_in_area(
        &mut app,
        key(KeyCode::Char('n')),
        &config,
        &store,
        &tx,
        Some(area)
    ));
    assert_eq!(app.selected_comment_index, 1);
    assert_eq!(app.status, "comment 2/2 focused");

    assert!(!handle_key_in_area(
        &mut app,
        key(KeyCode::Char('p')),
        &config,
        &store,
        &tx,
        Some(area)
    ));
    assert_eq!(app.selected_comment_index, 0);
    assert_eq!(app.status, "comment 1/2 focused");
}

#[test]
fn n_and_p_reveal_hidden_diff_inline_comment_threads_one_at_a_time() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    let mut first = comment("alice", "first hidden inline", None);
    first.review = Some(crate::model::ReviewCommentPreview {
        path: "src/lib.rs".to_string(),
        line: Some(1),
        original_line: None,
        start_line: None,
        original_start_line: None,
        side: Some("RIGHT".to_string()),
        start_side: None,
        diff_hunk: None,
        is_resolved: false,
        is_outdated: false,
    });
    let mut second = comment("bob", "second hidden inline", None);
    second.review = Some(crate::model::ReviewCommentPreview {
        path: "src/lib.rs".to_string(),
        line: Some(2),
        original_line: None,
        start_line: None,
        original_start_line: None,
        side: Some("RIGHT".to_string()),
        start_side: None,
        diff_hunk: None,
        is_resolved: false,
        is_outdated: false,
    });
    app.show_diff();
    app.focus_details();
    app.clear_selected_comment();
    app.diffs.insert(
        "1".to_string(),
        DiffState::Loaded(
            parse_pull_request_diff(
                r#"diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,2 @@
-old1
+new1
-old2
+new2
"#,
            )
            .expect("parse diff"),
        ),
    );
    app.details
        .insert("1".to_string(), DetailState::Loaded(vec![first, second]));
    app.toggle_diff_inline_comments();
    let area = Rect::new(0, 0, 120, 36);

    assert!(!handle_key_in_area(
        &mut app,
        key(KeyCode::Char('n')),
        &config,
        &store,
        &tx,
        Some(area)
    ));
    assert_eq!(app.selected_comment_index, 0);
    assert_eq!(app.status, "comment 1/2 focused");
    let rendered = build_details_document(&app, 120)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("first hidden inline")),
        "n should reveal the first inline thread: {rendered:?}"
    );
    assert!(
        !rendered
            .iter()
            .any(|line| line.contains("second hidden inline")),
        "n should keep other hidden inline threads collapsed: {rendered:?}"
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
    assert_eq!(app.status, "comment 2/2 focused");
    let rendered = build_details_document(&app, 120)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert!(
        !rendered
            .iter()
            .any(|line| line.contains("first hidden inline")),
        "moving to the next hidden thread should collapse the previous one: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("second hidden inline")),
        "n should reveal the second inline thread: {rendered:?}"
    );

    assert!(!handle_key_in_area(
        &mut app,
        key(KeyCode::Char('p')),
        &config,
        &store,
        &tx,
        Some(area)
    ));
    assert_eq!(app.selected_comment_index, 0);
    assert_eq!(app.status, "comment 1/2 focused");
    let rendered = build_details_document(&app, 120)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("first hidden inline")),
        "p should reveal the previous inline thread: {rendered:?}"
    );
    assert!(
        !rendered
            .iter()
            .any(|line| line.contains("second hidden inline")),
        "p should keep non-selected inline threads collapsed: {rendered:?}"
    );
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
fn g_and_upper_g_jump_conversation_details_and_update_comment_focus() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    app.focus_details();
    app.details.insert(
        "1".to_string(),
        DetailState::Loaded(vec![
            comment("alice", "first comment", None),
            comment(
                "bob",
                &(0..30)
                    .map(|index| format!("middle line {index}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                None,
            ),
            comment("carol", "last comment", None),
        ]),
    );
    let area = Rect::new(0, 0, 100, 18);
    let max_scroll = max_details_scroll(&app, details_area_for(&app, area));
    assert!(max_scroll > 0);

    assert!(!handle_key_in_area(
        &mut app,
        key(KeyCode::Char('G')),
        &config,
        &store,
        &tx,
        Some(area)
    ));
    assert_eq!(app.details_scroll, max_scroll);
    assert_eq!(app.selected_comment_index, 2);
    assert_eq!(
        app.current_selected_comment()
            .map(|comment| comment.body.as_str()),
        Some("last comment")
    );

    assert!(!handle_key_in_area(
        &mut app,
        key(KeyCode::Char('g')),
        &config,
        &store,
        &tx,
        Some(area)
    ));
    assert_eq!(app.details_scroll, 0);
    assert_eq!(app.selected_comment_index, NO_SELECTED_COMMENT_INDEX);
    assert!(app.current_selected_comment().is_none());
}

#[test]
fn p_from_first_comment_focuses_pr_or_issue_details_body() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    let area = Rect::new(0, 0, 100, 20);

    let mut pr_app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    pr_app.focus_details();
    pr_app.details.insert(
        "1".to_string(),
        DetailState::Loaded(vec![
            comment("alice", "first PR comment", None),
            comment("bob", "second PR comment", None),
        ]),
    );
    pr_app.selected_comment_index = 1;

    assert!(!handle_key_in_area(
        &mut pr_app,
        key(KeyCode::Char('p')),
        &config,
        &store,
        &tx,
        Some(area)
    ));
    assert_eq!(pr_app.selected_comment_index, 0);

    assert!(!handle_key_in_area(
        &mut pr_app,
        key(KeyCode::Char('p')),
        &config,
        &store,
        &tx,
        Some(area)
    ));
    assert_eq!(pr_app.details_scroll, 0);
    assert_eq!(pr_app.selected_comment_index, NO_SELECTED_COMMENT_INDEX);
    assert!(pr_app.current_selected_comment().is_none());
    assert_eq!(pr_app.status, "pull request details focused");

    assert!(!handle_key_in_area(
        &mut pr_app,
        key(KeyCode::Char('n')),
        &config,
        &store,
        &tx,
        Some(area)
    ));
    assert_eq!(pr_app.selected_comment_index, 0);
    assert_eq!(
        pr_app
            .current_selected_comment()
            .map(|comment| comment.body.as_str()),
        Some("first PR comment")
    );

    let mut issue_section = test_section();
    issue_section.kind = SectionKind::Issues;
    issue_section.key = "issues:test".to_string();
    issue_section.items[0].kind = ItemKind::Issue;
    issue_section.items[0].url = "https://github.com/rust-lang/rust/issues/1".to_string();
    let mut issue_app = AppState::new(SectionKind::Issues, vec![issue_section]);
    issue_app.focus_details();
    issue_app.details.insert(
        "1".to_string(),
        DetailState::Loaded(vec![comment("alice", "first issue comment", None)]),
    );

    assert!(!handle_key_in_area(
        &mut issue_app,
        key(KeyCode::Char('p')),
        &config,
        &store,
        &tx,
        Some(area)
    ));
    assert_eq!(issue_app.selected_comment_index, NO_SELECTED_COMMENT_INDEX);
    assert!(issue_app.current_selected_comment().is_none());
    assert_eq!(issue_app.status, "issue details focused");
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
fn double_click_begins_and_single_click_ends_diff_range() {
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

    for review_index in [0, 0] {
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
    assert_eq!(target.line, 1);
    assert!(
        app.diff_mark.get("1").is_some_and(|mark| mark.is_pending()),
        "double-clicking should begin a pending review range"
    );

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
fn esc_in_diff_mode_returns_to_state_before_diff() {
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
    assert_eq!(app.details_mode, DetailsMode::Conversation);
    assert_eq!(app.focus, FocusTarget::Sections);
    assert_eq!(app.status, "returned from diff");
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

    app.details_scroll = 6;
    app.move_diff_file(1);
    assert_eq!(app.selected_diff_file.get("1"), Some(&1));
    assert_eq!(app.details_scroll, 0);

    app.details_scroll = 13;
    app.move_diff_file(1);
    assert_eq!(app.selected_diff_file.get("1"), Some(&0));
    assert_eq!(app.details_scroll, 6);

    app.move_diff_file(1);
    assert_eq!(app.selected_diff_file.get("1"), Some(&1));
    assert_eq!(app.details_scroll, 13);
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
                entry
                    .stats
                    .as_ref()
                    .map(DiffFileStats::label)
                    .unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();

    assert!(labels.contains(&(0, None, "src", String::new())));
    assert!(labels.contains(&(1, Some(0), "app.rs", "M +1 -1".to_string())));
    assert!(labels.contains(&(1, Some(1), "github.rs", "M +1 -1".to_string())));
    assert!(labels.contains(&(0, Some(2), "README.md", "M +1 -1".to_string())));
}

#[test]
fn diff_tree_file_stats_include_review_comment_counts() {
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
"#,
    )
    .expect("parse diff");
    let mut app_comment = comment("alice", "inline", None);
    app_comment.id = Some(1);
    app_comment.review = Some(crate::model::ReviewCommentPreview {
        path: "src/app.rs".to_string(),
        line: Some(1),
        original_line: None,
        start_line: None,
        original_start_line: None,
        side: Some("RIGHT".to_string()),
        start_side: None,
        diff_hunk: None,
        is_resolved: false,
        is_outdated: false,
    });
    let mut app_reply = comment("bob", "reply", None);
    app_reply.id = Some(2);
    app_reply.parent_id = Some(1);
    let mut github_comment = comment("carol", "inline", None);
    github_comment.id = Some(3);
    github_comment.review = Some(crate::model::ReviewCommentPreview {
        path: "src/github.rs".to_string(),
        line: Some(1),
        original_line: None,
        start_line: None,
        original_start_line: None,
        side: Some("RIGHT".to_string()),
        start_side: None,
        diff_hunk: None,
        is_resolved: false,
        is_outdated: false,
    });
    let counts = diff_file_comment_counts(&diff, &[app_comment, app_reply, github_comment]);
    let entries = diff_tree_entries_with_comment_counts(&diff, &counts);
    let labels = entries
        .iter()
        .map(|entry| {
            (
                entry.label.as_str(),
                entry
                    .stats
                    .as_ref()
                    .map(DiffFileStats::label)
                    .unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();

    assert!(labels.contains(&("app.rs", "M +1 -1 2c".to_string())));
    assert!(labels.contains(&("github.rs", "M +1 -1 1c".to_string())));
}

#[test]
fn diff_file_list_aligns_stat_columns() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.show_diff();
    app.focus_list();
    app.diffs.insert(
        "1".to_string(),
        DiffState::Loaded(PullRequestDiff {
            files: vec![
                test_diff_file("src/small.rs", 3, 0),
                test_diff_file("src/large.rs", 268, 5),
            ],
            additions: 271,
            deletions: 5,
        }),
    );
    app.details.insert(
        "1".to_string(),
        DetailState::Loaded(vec![
            test_review_comment(1, "src/small.rs"),
            test_review_comment(2, "src/large.rs"),
            test_review_comment(3, "src/large.rs"),
            test_review_comment(4, "src/large.rs"),
            test_review_comment(5, "src/large.rs"),
            test_review_comment(6, "src/large.rs"),
            test_review_comment(7, "src/large.rs"),
            test_review_comment(8, "src/large.rs"),
            test_review_comment(9, "src/large.rs"),
            test_review_comment(10, "src/large.rs"),
            test_review_comment(11, "src/large.rs"),
            test_review_comment(12, "src/large.rs"),
        ]),
    );
    let backend = ratatui::backend::TestBackend::new(90, 14);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    terminal
        .draw(|frame| draw_diff_files(frame, &app, frame.area()))
        .expect("draw");

    let lines = buffer_lines(terminal.backend().buffer());
    let small = lines
        .iter()
        .find(|line| line.contains("small.rs"))
        .expect("small row");
    let large = lines
        .iter()
        .find(|line| line.contains("large.rs"))
        .expect("large row");

    assert_eq!(
        token_end_column(small, "+3"),
        token_end_column(large, "+268")
    );
    assert_eq!(token_end_column(small, "-0"), token_end_column(large, "-5"));
    assert_eq!(
        token_end_column(small, "1c"),
        token_end_column(large, "11c")
    );
    assert!(
        token_end_column(large, "11c") <= 82,
        "stats should keep a visible right gutter: {large:?}"
    );
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
fn terminal_disconnect_error_classifies_closed_terminal_io() {
    for kind in [
        io::ErrorKind::BrokenPipe,
        io::ErrorKind::ConnectionAborted,
        io::ErrorKind::ConnectionReset,
        io::ErrorKind::NotConnected,
        io::ErrorKind::UnexpectedEof,
    ] {
        assert!(terminal_disconnect_error(&io::Error::new(
            kind,
            "terminal closed"
        )));
    }

    for raw_error in [5, 6, 9, 19, 25] {
        assert!(terminal_disconnect_error(&io::Error::from_raw_os_error(
            raw_error
        )));
    }

    assert!(!terminal_disconnect_error(&io::Error::new(
        io::ErrorKind::PermissionDenied,
        "permission denied"
    )));
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
fn refresh_finished_resets_background_refresh_interval() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.refreshing = true;
    app.last_refresh_request = Instant::now() - Duration::from_secs(120);

    app.handle_msg(AppMsg::RefreshFinished {
        sections: vec![test_section()],
        save_error: None,
    });

    assert!(!app.refreshing);
    assert!(app.last_refresh_request.elapsed() < Duration::from_secs(1));
}

#[test]
fn idle_sweep_merges_non_active_sections_without_changing_status() {
    let pull_requests = test_section();
    let mut issue_item = work_item("issue-1", "rust-lang/rust", 1, "old issue", None);
    issue_item.kind = ItemKind::Issue;
    let issues = SectionSnapshot {
        key: "issues:test".to_string(),
        kind: SectionKind::Issues,
        title: "Test".to_string(),
        filters: String::new(),
        items: vec![issue_item],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(
        SectionKind::PullRequests,
        vec![pull_requests, issues.clone()],
    );
    app.status = "steady".to_string();

    app.handle_msg(AppMsg::IdleSweepStarted);
    let mut refreshed_issues = issues.clone();
    refreshed_issues.items[0].title = "new issue".to_string();
    app.handle_msg(AppMsg::IdleSweepFinished {
        sections: vec![refreshed_issues],
        next_cursor: 7,
    });

    assert!(!app.idle_sweep_refreshing);
    assert_eq!(app.idle_sweep_cursor, 7);
    assert_eq!(app.status, "steady");
    let section = app
        .sections
        .iter()
        .find(|section| section.key == "issues:test")
        .expect("issue section should remain");
    assert_eq!(section.items[0].title, "new issue");
}

#[test]
fn idle_sweep_does_not_merge_current_active_view() {
    let mut pull_requests = test_section();
    pull_requests.items[0].title = "old pr".to_string();
    let mut app = AppState::new(SectionKind::PullRequests, vec![pull_requests.clone()]);
    app.status = "steady".to_string();

    let mut refreshed_pull_requests = pull_requests;
    refreshed_pull_requests.items[0].title = "new pr".to_string();
    app.handle_msg(AppMsg::IdleSweepFinished {
        sections: vec![refreshed_pull_requests],
        next_cursor: 1,
    });

    assert_eq!(app.status, "steady");
    assert_eq!(app.current_section().unwrap().items[0].title, "old pr");
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
    assert_eq!(app.status, "latest");

    let (_title, lines, show_ok) = startup_dialog_content(StartupDialog::Ready, &app, &paths, 0);
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
    assert_eq!(app.status, "latest");
}

#[test]
fn refresh_finished_marks_pr_action_hints_stale_without_hiding_loaded_fields() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.action_hints.insert(
        "1".to_string(),
        ActionHintState::Loaded(ActionHints {
            labels: Vec::new(),
            checks: None,
            commits: None,
            failed_check_runs: Vec::new(),
            note: Some("Merge blocked: GitHub is still computing mergeability".to_string()),
            head: None,
            ..ActionHints::default()
        }),
    );

    app.handle_msg(AppMsg::RefreshFinished {
        sections: vec![test_section()],
        save_error: None,
    });

    assert!(matches!(
        app.action_hints.get("1"),
        Some(ActionHintState::Loaded(_))
    ));
    assert!(app.action_hints_stale.contains("1"));
    let item = app.current_item().expect("current PR item").clone();
    assert!(app.action_hints_load_needed(&item));
    assert!(app.start_action_hints_load_if_needed(&item));
    assert!(matches!(
        app.action_hints.get("1"),
        Some(ActionHintState::Loaded(_))
    ));
    assert!(app.action_hints_refreshing.contains("1"));
    assert!(!app.action_hints_stale.contains("1"));
}

#[test]
fn refresh_finished_does_not_overwrite_active_quick_filter_rows() {
    let mut filtered = test_section();
    filtered.items.truncate(1);
    filtered.filters = "repo:owner/repo archived:false is:closed sort:updated-desc".to_string();
    let mut refreshed = test_section();
    refreshed.filters = "repo:owner/repo is:open archived:false sort:updated-desc".to_string();
    let mut app = AppState::new(SectionKind::PullRequests, vec![filtered]);
    app.quick_filters.insert(
        "pull_requests:test".to_string(),
        QuickFilter::parse("state:closed").unwrap().expect("filter"),
    );

    app.handle_msg(AppMsg::RefreshFinished {
        sections: vec![refreshed],
        save_error: None,
    });

    assert_eq!(app.sections[0].items.len(), 1);
    assert_eq!(
        app.base_section_filters.get("pull_requests:test"),
        Some(&"repo:owner/repo is:open archived:false sort:updated-desc".to_string())
    );
    assert_eq!(app.status, "latest");
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
fn progressive_refresh_does_not_force_current_details_reload() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.refreshing = true;
    app.focus_details();
    app.details_scroll = 9;
    app.selected_comment_index = 0;
    app.details.insert(
        "1".to_string(),
        DetailState::Loaded(vec![comment("alice", "cached", None)]),
    );
    app.action_hints.insert(
        "1".to_string(),
        ActionHintState::Loaded(ActionHints {
            labels: Vec::new(),
            checks: None,
            commits: None,
            failed_check_runs: Vec::new(),
            note: None,
            head: None,
            ..ActionHints::default()
        }),
    );

    app.handle_msg(AppMsg::RefreshSectionLoaded {
        section: test_section(),
        save_error: None,
    });

    assert_eq!(app.current_item().map(|item| item.id.as_str()), Some("1"));
    assert_eq!(app.details_scroll, 9);
    assert!(!app.details_stale.contains("1"));
    assert!(!app.action_hints_stale.contains("1"));
    let item = app.current_item().expect("current item").clone();
    assert!(!app.start_comments_load_if_needed(&item));
    assert!(matches!(app.details.get("1"), Some(DetailState::Loaded(_))));
}

#[test]
fn progressive_refresh_does_not_overwrite_active_user_page_load() {
    let mut current = many_items_section(1);
    current.total_count = Some(120);
    current.page_size = 50;
    current.items[0].title = "Cached page one".to_string();
    let section_key = current.key.clone();
    let title = current.title.clone();
    let mut background_page_one = current.clone();
    background_page_one.items[0].title = "Background page one".to_string();
    let mut app = AppState::new(SectionKind::PullRequests, vec![current]);
    app.refreshing = true;
    app.section_page_loading = Some(SectionPageLoading {
        section_key,
        title,
        page_label: "2/3".to_string(),
        started_at: Instant::now(),
    });

    app.handle_msg(AppMsg::RefreshSectionLoaded {
        section: background_page_one,
        save_error: None,
    });

    assert!(app.section_page_loading.is_some());
    assert_eq!(
        app.current_item().map(|item| item.title.as_str()),
        Some("Cached page one")
    );
}

#[test]
fn section_page_loaded_keeps_background_refresh_running() {
    let mut current = many_items_section(1);
    current.total_count = Some(120);
    current.page_size = 50;
    let section_key = current.key.clone();
    let title = current.title.clone();
    let mut page_two = current.clone();
    page_two.items[0].title = "User page two".to_string();
    page_two.page = 2;
    let mut app = AppState::new(SectionKind::PullRequests, vec![current]);
    app.refreshing = true;
    app.section_page_loading = Some(SectionPageLoading {
        section_key: section_key.clone(),
        title,
        page_label: "2/3".to_string(),
        started_at: Instant::now(),
    });

    app.handle_msg(AppMsg::SectionPageLoaded {
        section_key,
        section: page_two,
        save_error: None,
    });

    assert!(app.refreshing);
    assert!(app.section_page_loading.is_none());
    assert_eq!(app.current_section().map(|section| section.page), Some(2));
    assert_eq!(
        app.current_item().map(|item| item.title.as_str()),
        Some("User page two")
    );
}

#[test]
fn refresh_finished_preserves_loaded_user_result_page() {
    let mut page_two = many_items_section(1);
    page_two.total_count = Some(120);
    page_two.page = 2;
    page_two.page_size = 50;
    page_two.items[0].title = "User page two".to_string();
    let mut background_page_one = page_two.clone();
    background_page_one.page = 1;
    background_page_one.items[0].title = "Background page one".to_string();
    let mut app = AppState::new(SectionKind::PullRequests, vec![page_two]);
    app.refreshing = true;

    app.handle_msg(AppMsg::RefreshFinished {
        sections: vec![background_page_one],
        save_error: None,
    });

    assert!(!app.refreshing);
    assert_eq!(app.current_section().map(|section| section.page), Some(2));
    assert_eq!(
        app.current_item().map(|item| item.title.as_str()),
        Some("User page two")
    );
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
    assert_eq!(app.status, "latest");
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
    assert_eq!(app.status, "latest");
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
        view_snapshots: HashMap::new(),
        focus: "details".to_string(),
        details_mode: "conversation".to_string(),
        details_scroll: 7,
        selected_comment_index: 2,
        expanded_comments: Vec::new(),
        details_scroll_by_item: HashMap::new(),
        selected_comment_index_by_item: HashMap::new(),
        viewed_item_at: HashMap::new(),
        selected_diff_file: HashMap::new(),
        selected_diff_line: HashMap::new(),
        diff_file_details_scroll: HashMap::new(),
        ignored_items: Vec::new(),
        recent_items: Vec::new(),
        recent_commands: Vec::new(),
        repo_unseen_items: HashMap::new(),
        global_search_by_repo: HashMap::new(),
        global_search_saved_by_repo: HashMap::new(),
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
fn ui_state_restores_diff_file_details_scroll_after_diff_load() {
    let state = UiState {
        active_view: builtin_view_key(SectionKind::PullRequests),
        selected_index: HashMap::from([(builtin_view_key(SectionKind::PullRequests), 0)]),
        focus: "details".to_string(),
        details_mode: "diff".to_string(),
        details_scroll: 3,
        selected_diff_file: HashMap::from([("1".to_string(), 1)]),
        diff_file_details_scroll: HashMap::from([("1::src/main.rs".to_string(), 22)]),
        ..UiState::default()
    };
    let mut app = AppState::with_ui_state(SectionKind::PullRequests, vec![test_section()], state);

    app.handle_msg(AppMsg::DiffLoaded {
        item_id: "1".to_string(),
        diff: Ok(parse_pull_request_diff(
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
        .expect("parse diff")),
    });

    assert_eq!(app.details_mode, DetailsMode::Diff);
    assert_eq!(app.selected_diff_file.get("1"), Some(&1));
    assert_eq!(app.details_scroll, 22);
}

#[test]
fn ui_state_saves_current_diff_file_details_scroll() {
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
    app.selected_diff_file.insert("1".to_string(), 1);
    app.details_scroll = 31;

    let saved = app.ui_state();

    assert_eq!(
        saved.diff_file_details_scroll.get("1::src/main.rs"),
        Some(&31)
    );
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

    assert_eq!(app.current_item().map(|item| item.id.as_str()), Some("1"));
    assert_eq!(app.details_scroll, 9);
    assert_eq!(app.selected_comment_index, 1);

    app.focus_details();
    assert_eq!(app.details_scroll, 9);
    assert_eq!(app.selected_comment_index, 1);
}

#[test]
fn selecting_current_list_item_keeps_conversation_details_position() {
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

    app.set_selection(0);

    assert_eq!(app.current_item().map(|item| item.id.as_str()), Some("1"));
    assert_eq!(app.details_scroll, 9);
    assert_eq!(app.selected_comment_index, 1);
}

#[test]
fn section_switch_restores_conversation_details_position_for_current_item() {
    let mut other_notification = notification_item("thread-2", true);
    other_notification.number = Some(2);
    other_notification.url = "https://github.com/rust-lang/rust/pull/2".to_string();
    let sections = vec![
        SectionSnapshot {
            key: "notifications:all".to_string(),
            kind: SectionKind::Notifications,
            title: "All".to_string(),
            filters: String::new(),
            items: vec![notification_item("thread-1", true)],
            total_count: None,
            page: 1,
            page_size: 0,
            refreshed_at: None,
            error: None,
        },
        SectionSnapshot {
            key: "notifications:others".to_string(),
            kind: SectionKind::Notifications,
            title: "Others".to_string(),
            filters: String::new(),
            items: vec![other_notification],
            total_count: None,
            page: 1,
            page_size: 0,
            refreshed_at: None,
            error: None,
        },
    ];
    let mut app = AppState::new(SectionKind::Notifications, sections);
    app.details.insert(
        "thread-1".to_string(),
        DetailState::Loaded(vec![
            comment("alice", "first", None),
            comment("bob", "second", None),
        ]),
    );
    app.focus_details();
    app.details_scroll = 9;
    app.selected_comment_index = 1;

    app.select_section(1);
    assert_eq!(
        app.current_item().map(|item| item.id.as_str()),
        Some("thread-2")
    );
    assert_eq!(app.details_scroll, 0);
    assert_eq!(app.selected_comment_index, 0);

    app.select_section(0);
    assert_eq!(
        app.current_item().map(|item| item.id.as_str()),
        Some("thread-1")
    );
    assert_eq!(app.details_scroll, 9);
    assert_eq!(app.selected_comment_index, 1);
}

#[test]
fn viewed_item_at_marks_items_updated_after_last_view_unseen() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let viewed_at = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let updated_before_view = DateTime::from_timestamp(1_699_999_999, 0).unwrap();
    let updated_after_view = DateTime::from_timestamp(1_700_000_001, 0).unwrap();
    app.sections[0].items[0].updated_at = Some(updated_before_view);
    let key = work_item_details_memory_key(app.current_item().expect("item"))
        .expect("details memory key");

    assert!(!app.item_has_unseen_details(app.current_item().expect("item")));

    app.viewed_item_at.insert(key, viewed_at);
    assert!(!app.item_has_unseen_details(app.current_item().expect("item")));

    app.sections[0].items[0].updated_at = Some(updated_after_view);
    assert!(app.item_has_unseen_details(app.current_item().expect("item")));
}

#[test]
fn focusing_details_marks_current_item_viewed_by_time() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let updated_at = DateTime::from_timestamp(1_700_000_001, 0).unwrap();
    let viewed_at = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    app.sections[0].items[0].updated_at = Some(updated_at);
    let key = work_item_details_memory_key(app.current_item().expect("item"))
        .expect("details memory key");
    app.viewed_item_at.insert(key, viewed_at);
    assert!(app.item_has_unseen_details(app.current_item().expect("item")));

    app.focus_details();
    assert!(!app.item_has_unseen_details(app.current_item().expect("item")));
}

#[test]
fn focusing_details_refreshes_cached_comments_when_item_updated_after_view() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let viewed_at = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let updated_after_view = DateTime::from_timestamp(1_700_000_001, 0).unwrap();
    app.sections[0].items[0].updated_at = Some(updated_after_view);
    app.details.insert(
        "1".to_string(),
        DetailState::Loaded(vec![comment("alice", "cached before update", None)]),
    );
    let key = work_item_details_memory_key(app.current_item().expect("item"))
        .expect("details memory key");
    app.viewed_item_at.insert(key, viewed_at);

    app.focus_details();

    assert!(app.details_stale.contains("1"));
    assert!(app.item_has_unseen_details(app.current_item().expect("item")));
    assert!(app.comments_load_needed(app.current_item().expect("item")));
}

#[test]
fn comments_loaded_records_item_updated_at_for_cache_freshness() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let viewed_at = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let updated_after_view = DateTime::from_timestamp(1_700_000_001, 0).unwrap();
    app.sections[0].items[0].updated_at = Some(updated_after_view);
    let key = work_item_details_memory_key(app.current_item().expect("item"))
        .expect("details memory key");
    app.viewed_item_at.insert(key, viewed_at);

    app.handle_msg(AppMsg::CommentsLoaded {
        item_id: "1".to_string(),
        comments: Ok(CommentFetchResult {
            item_metadata: Some(ItemDetailsMetadata {
                title: None,
                body: None,
                author: None,
                state: None,
                url: None,
                created_at: None,
                updated_at: Some(updated_after_view),
                labels: None,
                assignees: None,
                comments: Some(1),
                viewer_subscription: None,
            }),
            item_reactions: Some(ReactionSummary::default()),
            item_milestone: Some(None),
            comments: vec![comment("alice", "fresh comment", None)],
        }),
    });

    assert!(app.item_has_unseen_details(app.current_item().expect("item")));
    assert!(!app.details_cache_outdated(app.current_item().expect("item")));
    assert!(!app.comments_load_needed(app.current_item().expect("item")));
    assert!(app.details_refreshed_at.contains_key("1"));
}

#[test]
fn comments_loaded_uses_notification_updated_at_when_it_is_newer_than_linked_item() {
    let mut item = notification_item("thread-1", true);
    item.number = Some(156354);
    item.repo = "rust-lang/rust".to_string();
    item.updated_at = DateTime::from_timestamp(1_700_000_023, 0);
    let section = SectionSnapshot {
        key: "notifications:all".to_string(),
        kind: SectionKind::Notifications,
        title: "All".to_string(),
        filters: String::new(),
        items: vec![item],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(SectionKind::Notifications, vec![section]);
    app.details.insert(
        "thread-1".to_string(),
        DetailState::Loaded(vec![comment("rustbot", "cached", None)]),
    );

    app.handle_msg(AppMsg::CommentsLoaded {
        item_id: "thread-1".to_string(),
        comments: Ok(CommentFetchResult {
            item_metadata: Some(ItemDetailsMetadata {
                title: None,
                body: None,
                author: None,
                state: None,
                url: None,
                created_at: None,
                updated_at: DateTime::from_timestamp(1_700_000_005, 0),
                labels: None,
                assignees: None,
                comments: Some(3),
                viewer_subscription: None,
            }),
            item_reactions: Some(ReactionSummary::default()),
            item_milestone: Some(None),
            comments: vec![comment("rust-bors[bot]", "fresh", None)],
        }),
    });

    assert_eq!(
        app.details_synced_at.get("thread-1").copied(),
        DateTime::from_timestamp(1_700_000_023, 0)
    );
    assert!(app.details_refreshed_at.contains_key("thread-1"));
    assert!(!app.details_cache_outdated(app.current_item().expect("item")));
    assert!(!app.comments_load_needed(app.current_item().expect("item")));
}

#[test]
fn comment_metadata_updated_at_marks_unseen_when_not_focused() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let updated_before_view = DateTime::from_timestamp(1_699_999_999, 0).unwrap();
    let viewed_at = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let updated_after_view = DateTime::from_timestamp(1_700_000_001, 0).unwrap();
    app.sections[0].items[0].updated_at = Some(updated_before_view);
    let key = work_item_details_memory_key(app.current_item().expect("item"))
        .expect("details memory key");
    app.viewed_item_at.insert(key, viewed_at);
    assert!(!app.item_has_unseen_details(app.current_item().expect("item")));

    app.handle_msg(AppMsg::CommentsLoaded {
        item_id: "1".to_string(),
        comments: Ok(CommentFetchResult {
            item_metadata: Some(ItemDetailsMetadata {
                title: None,
                body: None,
                author: None,
                state: None,
                url: None,
                created_at: None,
                updated_at: Some(updated_after_view),
                labels: None,
                assignees: None,
                comments: Some(3),
                viewer_subscription: None,
            }),
            item_reactions: Some(ReactionSummary::default()),
            item_milestone: Some(None),
            comments: vec![comment("alice", "old", None), comment("bob", "new", None)],
        }),
    });

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
fn project_switch_restores_saved_view_snapshot() {
    let fiber_key = "repo:Fiber:pull_requests:Pull Requests";
    let ghr_key = "repo:ghr:pull_requests:Pull Requests";
    let sections = vec![
        SectionSnapshot {
            key: fiber_key.to_string(),
            kind: SectionKind::PullRequests,
            title: "Pull Requests".to_string(),
            filters: String::new(),
            items: vec![
                work_item("fiber-1", "nervosnetwork/fiber", 1, "First", None),
                work_item("fiber-2", "nervosnetwork/fiber", 2, "Second", None),
            ],
            total_count: None,
            page: 1,
            page_size: 0,
            refreshed_at: None,
            error: None,
        },
        SectionSnapshot {
            key: ghr_key.to_string(),
            kind: SectionKind::PullRequests,
            title: "Pull Requests".to_string(),
            filters: String::new(),
            items: vec![work_item("ghr-1", "chenyukang/ghr", 1, "Ghr", None)],
            total_count: None,
            page: 1,
            page_size: 0,
            refreshed_at: None,
            error: None,
        },
    ];
    let mut app = AppState::new(SectionKind::PullRequests, sections);

    app.switch_project_view(repo_view_key("Fiber"));
    app.set_selection(1);
    app.details.insert(
        "fiber-2".to_string(),
        DetailState::Loaded(vec![
            comment("alice", "first", None),
            comment("bob", "second", None),
            comment("carol", "third", None),
        ]),
    );
    app.focus_details();
    app.details_scroll = 12;
    app.selected_comment_index = 2;
    app.set_current_list_scroll_offset(5);

    app.switch_project_view(repo_view_key("ghr"));
    app.sections[0].items.swap(0, 1);
    app.switch_project_view(repo_view_key("Fiber"));

    assert_eq!(app.active_view, repo_view_key("Fiber"));
    assert_eq!(app.focus, FocusTarget::Details);
    assert_eq!(
        app.current_item().map(|item| item.id.as_str()),
        Some("fiber-2")
    );
    assert_eq!(app.current_selected_position(), 0);
    assert_eq!(app.details_scroll, 12);
    assert_eq!(app.selected_comment_index, 2);
    assert_eq!(app.list_scroll_offset.get(fiber_key), Some(&5));
}

#[test]
fn ui_state_saves_and_restores_project_view_snapshot() {
    let fiber_key = "repo:Fiber:pull_requests:Pull Requests";
    let sections = vec![SectionSnapshot {
        key: fiber_key.to_string(),
        kind: SectionKind::PullRequests,
        title: "Pull Requests".to_string(),
        filters: String::new(),
        items: vec![
            work_item("fiber-1", "nervosnetwork/fiber", 1, "First", None),
            work_item("fiber-2", "nervosnetwork/fiber", 2, "Second", None),
        ],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    }];
    let mut app = AppState::new(SectionKind::PullRequests, sections.clone());

    app.switch_project_view(repo_view_key("Fiber"));
    app.set_selection(1);
    app.focus_details();
    app.details_scroll = 8;
    app.selected_comment_index = 1;
    app.set_current_list_scroll_offset(3);

    let saved = app.ui_state();
    let snapshot = saved
        .view_snapshots
        .get(&repo_view_key("Fiber"))
        .expect("repo view snapshot");
    assert_eq!(snapshot.focus, "details");
    assert_eq!(snapshot.section_key.as_deref(), Some(fiber_key));
    assert_eq!(snapshot.item_id.as_deref(), Some("fiber-2"));
    assert_eq!(snapshot.selected_index, 1);
    assert_eq!(snapshot.list_scroll_offset, 3);
    assert_eq!(snapshot.details_scroll, 8);

    let restored = AppState::with_ui_state(SectionKind::PullRequests, sections, saved);

    assert_eq!(restored.active_view, repo_view_key("Fiber"));
    assert_eq!(restored.focus, FocusTarget::Details);
    assert_eq!(
        restored.current_item().map(|item| item.id.as_str()),
        Some("fiber-2")
    );
    assert_eq!(restored.details_scroll, 8);
    assert_eq!(restored.selected_comment_index, 1);
    assert_eq!(restored.list_scroll_offset.get(fiber_key), Some(&3));
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
fn stale_details_refresh_failure_preserves_loaded_comments() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.details.insert(
        "1".to_string(),
        DetailState::Loaded(vec![comment("alice", "old cached comment", None)]),
    );
    app.details_stale.insert("1".to_string());
    let item = app.current_item().cloned().expect("selected item");

    assert!(app.start_comments_load_if_needed(&item));
    app.handle_msg(AppMsg::CommentsLoaded {
        item_id: "1".to_string(),
        comments: Err("gh failed: rate limit".to_string()),
    });

    assert!(
        matches!(
            app.details.get("1"),
            Some(DetailState::Loaded(comments)) if comments[0].body == "old cached comment"
        ),
        "background refresh failure should keep old comments visible"
    );
    assert!(!app.details_stale.contains("1"));
    assert!(!app.details_refreshing.contains("1"));
    assert!(app.setup_dialog.is_none());
    assert!(app.status.contains("keeping cached comments"));

    let rendered = build_details_document(&app, 120)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("old cached comment"));
    assert!(!rendered.contains("Failed to load comments"));
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
fn stale_pr_action_hints_refresh_keeps_loaded_fields_visible() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.action_hints.insert(
        "1".to_string(),
        ActionHintState::Loaded(ActionHints {
            labels: vec!["Mergeable".to_string()],
            checks: Some(CheckSummary {
                passed: 4,
                failed: 0,
                pending: 0,
                skipped: 0,
                total: 4,
                incomplete: false,
            }),
            commits: Some(2),
            failed_check_runs: Vec::new(),
            note: Some("Mergeable".to_string()),
            head: Some(PullRequestBranch {
                repository: "rust-lang/rust".to_string(),
                branch: "feature/no-flicker".to_string(),
            }),
            ..ActionHints::default()
        }),
    );
    app.action_hints_stale.insert("1".to_string());
    let item = app.current_item().cloned().expect("item");

    assert!(app.start_action_hints_load_if_needed(&item));

    let rendered = build_details_document(&app, 120)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("commits: 2"));
    assert!(rendered.contains("branch: rust-lang/rust:feature/no-flicker"));
    assert!(rendered.contains("action: Mergeable"));
    assert!(rendered.contains("checks: 4 pass"));
    assert!(!rendered.contains("action: loading..."));
    assert!(!rendered.contains("branch: loading..."));
    assert!(!rendered.contains("checks: loading..."));
}

#[test]
fn background_action_hints_error_keeps_last_loaded_fields() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let hints = ActionHints {
        labels: vec!["Mergeable".to_string()],
        checks: None,
        commits: None,
        failed_check_runs: Vec::new(),
        note: None,
        head: None,
        ..ActionHints::default()
    };
    app.action_hints
        .insert("1".to_string(), ActionHintState::Loaded(hints.clone()));
    app.action_hints_refreshing.insert("1".to_string());

    app.handle_msg(AppMsg::ActionHintsLoaded {
        item_id: "1".to_string(),
        actions: Err("temporary gh api failure".to_string()),
    });

    assert!(!app.action_hints_refreshing.contains("1"));
    assert_eq!(
        app.action_hints.get("1"),
        Some(&ActionHintState::Loaded(hints))
    );
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
    let text = help_dialog_content(DEFAULT_COMMAND_PALETTE_KEY)
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Tab / Shift+Tab"));
    assert!(text.contains(":"));
    assert!(text.contains("R"));
    assert!(text.contains("edit selected issue or PR fields"));
    assert!(text.contains("drag split border"));
    assert!(text.contains("open PR merge confirmation"));
    assert!(text.contains("open close or reopen confirmation"));
    assert!(text.contains("open PR enable auto-merge confirmation"));
    assert!(text.contains("open PR disable auto-merge confirmation"));
    assert!(text.contains("open PR update-branch confirmation"));
    assert!(text.contains("toggle PR draft / ready for review"));
    assert!(text.contains("change issue or PR milestone"));
    assert!(text.contains("search PRs and issues in the current repo"));
    assert!(text.contains("terminal text selection"));
    assert!(text.contains("@ / -"));
    assert!(text.contains("add a reaction"));
    assert!(!text.contains("Reaction Dialog"));
    assert!(!text.contains("Ctrl+Enter"));
}

#[test]
fn wide_help_dialog_uses_two_columns() {
    let single_column = help_dialog_content(DEFAULT_COMMAND_PALETTE_KEY);
    let two_columns = help_dialog_content_for_width(140, DEFAULT_COMMAND_PALETTE_KEY);
    let text = two_columns
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(two_columns.len() < single_column.len());
    assert!(text.contains("General"));
    assert!(text.contains("Mouse"));
    assert!(
        two_columns
            .iter()
            .all(|line| display_width(&line.to_string()) <= 140)
    );
}

#[test]
fn help_dialog_two_columns_wrap_instead_of_clipping() {
    let width = 110_u16;
    let two_columns = help_dialog_content_for_width(width, DEFAULT_COMMAND_PALETTE_KEY);
    let text = two_columns
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");

    assert!(two_columns.len() > help_dialog_content(DEFAULT_COMMAND_PALETTE_KEY).len() / 2);
    assert!(collapsed.contains("open PR disable auto-merge confirmation"));
    assert!(collapsed.contains("filter with state:closed"));
    assert!(collapsed.contains("label:bug author:alice"));
    assert!(
        two_columns
            .iter()
            .all(|line| display_width(&line.to_string()) <= usize::from(width))
    );
}

#[test]
fn help_dialog_width_can_use_wide_terminals() {
    assert_eq!(
        help_dialog_width(Rect::new(0, 0, 200, 40)),
        HELP_DIALOG_MAX_WIDTH
    );
    assert_eq!(help_dialog_width(Rect::new(0, 0, 80, 40)), 78);
}

#[test]
fn colon_opens_command_palette() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char(':')),
        &config,
        &store,
        &tx
    ));

    assert_eq!(app.command_palette, Some(CommandPalette::default()));
    assert_eq!(app.status, "command palette");
}

#[test]
fn printable_command_palette_key_is_text_inside_comment_editor() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    app.start_new_comment_dialog();
    {
        let dialog = app.comment_dialog.as_mut().unwrap();
        dialog.body.set_text("draft");
    }

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char(':')),
        &config,
        &store,
        &tx
    ));

    assert!(app.command_palette.is_none());
    assert_eq!(
        app.comment_dialog
            .as_ref()
            .map(|dialog| dialog.body.as_str()),
        Some("draft:")
    );
}

#[test]
fn printable_command_palette_key_is_text_inside_filter_and_repo_search() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

    app.start_filter_input();
    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char(':')),
        &config,
        &store,
        &tx
    ));
    assert!(app.command_palette.is_none());
    assert_eq!(app.filter_input_query, ":");

    app.filter_input_active = false;
    app.start_global_search_input();
    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char(':')),
        &config,
        &store,
        &tx
    ));
    assert!(app.command_palette.is_none());
    assert_eq!(
        app.global_search_dialog
            .as_ref()
            .map(|dialog| dialog.title.text()),
        Some(":")
    );
}

#[test]
fn modified_command_palette_key_can_open_over_text_input() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut config = Config::default();
    config.defaults.command_palette_key = "Ctrl+L".to_string();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    app.start_new_comment_dialog();
    {
        let dialog = app.comment_dialog.as_mut().unwrap();
        dialog.body.set_text("draft");
    }

    assert!(!handle_key(
        &mut app,
        ctrl_key(KeyCode::Char('l')),
        &config,
        &store,
        &tx
    ));

    assert!(app.command_palette.is_some());
    assert_eq!(
        app.comment_dialog
            .as_ref()
            .map(|dialog| dialog.body.as_str()),
        Some("draft")
    );
    assert_eq!(app.command_palette_key, "Ctrl+L");
}

#[test]
fn command_palette_orders_recently_selected_commands_first() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let commands = command_palette_commands(DEFAULT_COMMAND_PALETTE_KEY);
    app.recent_commands = vec![
        RecentCommand {
            id: "Refresh".to_string(),
            selected_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        },
        RecentCommand {
            id: "Info".to_string(),
            selected_at: DateTime::from_timestamp(1_700_000_100, 0).unwrap(),
        },
    ];

    let matches = app.command_palette_match_indices(&commands, "");

    assert_eq!(commands[matches[0]].title, "Info");
    assert_eq!(commands[matches[1]].title, "Refresh");
}

#[test]
fn command_palette_recent_ties_fall_back_to_default_order() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let commands = command_palette_commands(DEFAULT_COMMAND_PALETTE_KEY);
    let selected_at = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    app.recent_commands = vec![
        RecentCommand {
            id: "Refresh".to_string(),
            selected_at,
        },
        RecentCommand {
            id: "Show Help".to_string(),
            selected_at,
        },
    ];

    let matches = app.command_palette_match_indices(&commands, "");

    assert_eq!(commands[matches[0]].title, "Show Help");
    assert_eq!(commands[matches[1]].title, "Refresh");
}

#[test]
fn command_palette_submission_records_recent_command() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut config = Config::default();
    let paths = unique_test_paths("command-palette-recent");
    let store = SnapshotStore::new(paths.db_path.clone());
    app.command_palette = Some(CommandPalette {
        query: "info".to_string(),
        selected: 0,
    });

    assert!(!handle_key_in_area_mut(
        &mut app,
        key(KeyCode::Enter),
        &mut config,
        &paths,
        &store,
        &tx,
        None,
    ));

    assert_eq!(
        app.recent_commands
            .first()
            .map(|command| command.id.as_str()),
        Some("Info")
    );
    assert_eq!(
        app.ui_state()
            .recent_commands
            .first()
            .map(|command| command.id.as_str()),
        Some("Info")
    );
}

#[test]
fn command_palette_info_opens_runtime_info_dialog() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut config = Config::default();
    config.defaults.log_level = "debug".to_string();
    let paths = unique_test_paths("runtime-info");
    let store = SnapshotStore::new(paths.db_path.clone());
    app.command_palette = Some(CommandPalette {
        query: "info".to_string(),
        selected: 0,
    });

    assert!(!handle_key_in_area_mut(
        &mut app,
        key(KeyCode::Enter),
        &mut config,
        &paths,
        &store,
        &tx,
        None,
    ));

    let dialog = app.message_dialog.as_ref().expect("info dialog");
    assert_eq!(dialog.kind, MessageDialogKind::Info);
    assert_eq!(dialog.title, "Info");
    assert!(dialog.body.contains("version:"));
    assert!(dialog.body.contains("ghr memory:"));
    assert!(dialog.body.contains("ignored items:"));
    assert!(
        dialog
            .body
            .contains(&format!("config: {}", paths.config_path.display()))
    );
    assert!(
        dialog
            .body
            .contains(&format!("db: {}", paths.db_path.display()))
    );
    assert!(dialog.body.contains("log_level: debug"));
    assert!(app.command_palette.is_none());
    assert_eq!(app.status, "info");
}

#[test]
fn command_palette_clear_cache_opens_cache_picker() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut config = Config::default();
    let paths = unique_test_paths("clear-cache-open");
    let store = SnapshotStore::new(paths.db_path.clone());
    app.command_palette = Some(CommandPalette {
        query: "clear cache".to_string(),
        selected: 0,
    });

    assert!(!handle_key_in_area_mut(
        &mut app,
        key(KeyCode::Enter),
        &mut config,
        &paths,
        &store,
        &tx,
        None,
    ));

    assert!(app.command_palette.is_none());
    assert_eq!(app.cache_clear_dialog, Some(CacheClearDialog::default()));
    assert_eq!(app.status, "clear cache");
}

#[test]
fn command_palette_recent_items_opens_recent_picker() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut config = Config::default();
    let paths = unique_test_paths("recent-items-open");
    let store = SnapshotStore::new(paths.db_path.clone());
    app.command_palette = Some(CommandPalette {
        query: "recent items".to_string(),
        selected: 0,
    });

    assert!(!handle_key_in_area_mut(
        &mut app,
        key(KeyCode::Enter),
        &mut config,
        &paths,
        &store,
        &tx,
        None,
    ));

    assert!(app.command_palette.is_none());
    assert_eq!(app.recent_items_dialog, Some(RecentItemsDialog::default()));
    assert_eq!(app.status, "recent items");
}

#[test]
fn command_palette_set_color_theme_opens_theme_switcher() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut config = Config::default();
    let paths = unique_test_paths("set-color-theme-open");
    let store = SnapshotStore::new(paths.db_path.clone());
    app.command_palette = Some(CommandPalette {
        query: "set color theme".to_string(),
        selected: 0,
    });

    assert!(!handle_key_in_area_mut(
        &mut app,
        key(KeyCode::Enter),
        &mut config,
        &paths,
        &store,
        &tx,
        None,
    ));

    assert!(app.command_palette.is_none());
    assert_eq!(
        app.theme_switcher,
        Some(ThemeSwitcher {
            query: String::new(),
            selected: 0,
            current: ThemeChoice::Auto,
        })
    );
    assert_eq!(app.status, "set color theme");
}

#[test]
fn theme_switcher_selects_fixed_theme_and_saves_config() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let mut config = Config::default();
    let paths = unique_test_paths("set-color-theme-fixed");
    config.save(&paths.config_path).expect("save config");

    app.show_theme_switcher(&config);
    app.handle_theme_switcher_key(
        KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE),
        &mut config,
        &paths,
    );
    app.handle_theme_switcher_key(
        KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE),
        &mut config,
        &paths,
    );
    app.handle_theme_switcher_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut config,
        &paths,
    );

    assert!(app.theme_switcher.is_none());
    assert_eq!(app.theme_name, ThemeName::CatppuccinMocha);
    assert_eq!(config.defaults.theme, ThemePreference::Dark);
    assert_eq!(config.defaults.theme_name, Some(ThemeName::CatppuccinMocha));
    assert_eq!(app.status, "theme: catppuccin_mocha");

    let saved = Config::load_or_create(&paths.config_path).expect("load saved config");
    assert_eq!(saved.defaults.theme, ThemePreference::Dark);
    assert_eq!(saved.defaults.theme_name, Some(ThemeName::CatppuccinMocha));
}

#[test]
fn theme_switcher_auto_clears_fixed_theme_and_saves_config() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.set_theme(ThemeName::Monokai);
    let mut config = Config::default();
    config.defaults.theme = ThemePreference::Dark;
    config.defaults.theme_name = Some(ThemeName::Monokai);
    let paths = unique_test_paths("set-color-theme-auto");
    config.save(&paths.config_path).expect("save config");

    app.show_theme_switcher(&config);
    app.theme_switcher.as_mut().expect("theme switcher").query = "auto".to_string();
    app.handle_theme_switcher_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut config,
        &paths,
    );

    assert!(app.theme_switcher.is_none());
    assert_eq!(config.defaults.theme, ThemePreference::Auto);
    assert_eq!(config.defaults.theme_name, None);
    assert!(app.status.starts_with("theme: auto ("));

    let saved = Config::load_or_create(&paths.config_path).expect("load saved config");
    assert_eq!(saved.defaults.theme, ThemePreference::Auto);
    assert_eq!(saved.defaults.theme_name, None);
}

#[test]
fn explicit_color_theme_disables_auto_theme_refresh() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.set_theme(ThemeName::CatppuccinMocha);
    app.last_auto_theme_check = Instant::now() - AUTO_THEME_CHECK_INTERVAL;
    let mut config = Config::default();
    config.defaults.theme = ThemePreference::Dark;
    config.defaults.theme_name = Some(ThemeName::CatppuccinMocha);

    let changed = app.refresh_auto_theme(&config, Instant::now());

    assert!(!changed);
    assert_eq!(app.theme_name, ThemeName::CatppuccinMocha);
}

#[test]
fn recent_items_only_records_details_visits_after_dwell_time() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);

    app.focus_details();
    app.details_visit
        .as_mut()
        .expect("details visit")
        .started_at = Instant::now() - RECENT_ITEM_DWELL + Duration::from_millis(100);
    app.focus_list();

    assert!(app.recent_items.is_empty());
    assert!(app.details_visit.is_none());

    app.focus_details();
    app.details_visit
        .as_mut()
        .expect("details visit")
        .started_at = Instant::now() - RECENT_ITEM_DWELL - Duration::from_millis(100);
    app.sync_recent_details_visit(Instant::now());

    assert_eq!(app.recent_items.len(), 1);
    assert_eq!(
        recent_item_label(&app.recent_items[0]),
        "[pr] #1 Compiler diagnostics.  rust-lang/rust"
    );
}

#[test]
fn recent_items_record_linked_inbox_notifications_as_pr_or_issue() {
    let section = SectionSnapshot {
        key: "notifications:all".to_string(),
        kind: SectionKind::Notifications,
        title: "All".to_string(),
        filters: "is:all".to_string(),
        items: vec![notification_item("thread-1", true)],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(SectionKind::Notifications, vec![section]);

    app.focus_details();
    app.details_visit
        .as_mut()
        .expect("details visit")
        .started_at = Instant::now() - RECENT_ITEM_DWELL - Duration::from_millis(100);
    app.sync_recent_details_visit(Instant::now());

    assert_eq!(app.recent_items.len(), 1);
    assert_eq!(app.recent_items[0].id, "thread-1");
    assert_eq!(app.recent_items[0].kind, ItemKind::PullRequest);
    assert_eq!(
        recent_item_label(&app.recent_items[0]),
        "[pr] #1 Notification thread-1.  rust-lang/rust"
    );
}

#[test]
fn recent_items_filter_keeps_recent_order() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.recent_items = vec![
        recent_item(
            "pr-2",
            ItemKind::PullRequest,
            "nervosnetwork/fiber",
            2,
            "Fix funding state",
            300,
        ),
        recent_item(
            "issue-1",
            ItemKind::Issue,
            "chenyukang/ghr",
            1,
            "Fix fuzzy search",
            200,
        ),
        recent_item(
            "pr-1",
            ItemKind::PullRequest,
            "rust-lang/rust",
            1,
            "Compiler diagnostics",
            100,
        ),
    ];

    let matches = app
        .recent_item_candidates_for_query("fix")
        .into_iter()
        .map(|item| recent_item_label(&item))
        .collect::<Vec<_>>();

    assert_eq!(
        matches,
        vec![
            "[pr] #2 Fix funding state.  nervosnetwork/fiber",
            "[issue] #1 Fix fuzzy search.  chenyukang/ghr",
        ]
    );
}

#[test]
fn recent_items_candidates_omit_current_item() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.focus_details();
    app.recent_items = vec![
        recent_item(
            "current-pr",
            ItemKind::PullRequest,
            "rust-lang/rust",
            1,
            "Compiler diagnostics",
            300,
        ),
        recent_item(
            "pr-2",
            ItemKind::PullRequest,
            "nervosnetwork/fiber",
            2,
            "Fix funding state",
            200,
        ),
    ];

    let matches = app
        .recent_item_candidates_for_query("")
        .into_iter()
        .map(|item| recent_item_label(&item))
        .collect::<Vec<_>>();

    assert_eq!(
        matches,
        vec!["[pr] #2 Fix funding state.  nervosnetwork/fiber"]
    );

    let matches = app
        .recent_item_candidates_for_query("compiler")
        .into_iter()
        .map(|item| recent_item_label(&item))
        .collect::<Vec<_>>();

    assert_eq!(
        matches,
        vec!["[pr] #1 Compiler diagnostics.  rust-lang/rust"]
    );
}

#[test]
fn recent_items_recording_marks_ui_state_dirty_once() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.focus_details();
    app.details_visit
        .as_mut()
        .expect("details visit")
        .started_at = Instant::now() - RECENT_ITEM_DWELL - Duration::from_millis(100);

    app.sync_recent_details_visit(Instant::now());

    assert_eq!(app.recent_items.len(), 1);
    assert!(app.take_recent_items_dirty());
    assert!(!app.take_recent_items_dirty());
}

#[test]
fn recent_item_jump_restores_position_across_inbox_and_pr_list_ids() {
    let notification_section = SectionSnapshot {
        key: "notifications:all".to_string(),
        kind: SectionKind::Notifications,
        title: "All".to_string(),
        filters: String::new(),
        items: vec![notification_item("thread-1", true)],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let pr = work_item(
        "regular-pr-1",
        "rust-lang/rust",
        1,
        "Compiler diagnostics",
        None,
    );
    let other = work_item(
        "regular-pr-2",
        "nervosnetwork/fiber",
        2,
        "Funding state",
        None,
    );
    let pull_request_section = SectionSnapshot {
        key: "pull_requests:test".to_string(),
        kind: SectionKind::PullRequests,
        title: "Test".to_string(),
        filters: String::new(),
        items: vec![other, pr.clone()],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(
        SectionKind::Notifications,
        vec![notification_section, pull_request_section],
    );
    app.details.insert(
        "thread-1".to_string(),
        DetailState::Loaded(vec![
            comment("alice", "first", None),
            comment("bob", "second", None),
        ]),
    );
    app.focus_details();
    app.details_scroll = 9;
    app.selected_comment_index = 1;
    app.save_current_conversation_details_state();

    app.switch_view(builtin_view_key(SectionKind::PullRequests));
    app.set_selection(0);
    app.recent_items = vec![recent_item_from_work_item(&pr, Utc::now()).unwrap()];

    app.show_recent_items_dialog();
    app.handle_recent_items_key(key(KeyCode::Enter));

    assert_eq!(
        app.current_item().map(|item| item.id.as_str()),
        Some("regular-pr-1")
    );
    assert_eq!(app.details_scroll, 9);
    assert_eq!(app.selected_comment_index, 1);
}

#[test]
fn recent_item_jump_preserves_current_position_from_builtin_pr_view() {
    let first = work_item(
        "regular-pr-1",
        "rust-lang/rust",
        156194,
        "Avoid deriving bounds from FnPtr",
        None,
    );
    let second = work_item(
        "regular-pr-2",
        "nervosnetwork/fiber",
        1197,
        "feat: add backup and restore",
        None,
    );
    let pull_request_section = SectionSnapshot {
        key: "pull_requests:needs-attention".to_string(),
        kind: SectionKind::PullRequests,
        title: "Needs Attention".to_string(),
        filters: String::new(),
        items: vec![first.clone(), second.clone()],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(SectionKind::PullRequests, vec![pull_request_section]);
    app.details.insert(
        "regular-pr-1".to_string(),
        DetailState::Loaded(vec![
            comment("alice", "first", None),
            comment("bob", "second", None),
        ]),
    );
    app.details.insert(
        "regular-pr-2".to_string(),
        DetailState::Loaded(vec![comment("carol", "other", None)]),
    );
    app.focus_details();
    app.details_scroll = 14;
    app.selected_comment_index = 1;
    app.recent_items = vec![
        recent_item_from_work_item(&first, Utc::now()).unwrap(),
        recent_item_from_work_item(&second, Utc::now()).unwrap(),
    ];

    app.show_recent_items_dialog();
    app.handle_recent_items_key(key(KeyCode::Enter));

    assert_eq!(
        app.current_item().map(|item| item.id.as_str()),
        Some("regular-pr-2")
    );

    app.show_recent_items_dialog();
    app.handle_recent_items_key(key(KeyCode::Enter));

    assert_eq!(
        app.current_item().map(|item| item.id.as_str()),
        Some("regular-pr-1")
    );
    assert_eq!(app.details_scroll, 14);
    assert_eq!(app.selected_comment_index, 1);
}

#[test]
fn issue_details_position_updates_memory_while_focused() {
    let mut issue = work_item(
        "issue-1",
        "nervosnetwork/fiber",
        941,
        "Atomic MPP design",
        None,
    );
    issue.kind = ItemKind::Issue;
    issue.url = "https://github.com/nervosnetwork/fiber/issues/941".to_string();
    let issue_section = SectionSnapshot {
        key: "issues:involved".to_string(),
        kind: SectionKind::Issues,
        title: "Involved".to_string(),
        filters: String::new(),
        items: vec![issue.clone()],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(SectionKind::Issues, vec![issue_section]);
    app.details.insert(
        "issue-1".to_string(),
        DetailState::Loaded(vec![
            comment("alice", "first", None),
            comment("bob", "second", None),
        ]),
    );

    app.focus_details();
    app.scroll_details(8);
    app.select_comment(1);

    let key = work_item_details_memory_key(&issue).expect("issue memory key");
    assert_eq!(
        app.conversation_details_state.get(&key),
        Some(&ConversationDetailsState {
            details_scroll: 8,
            selected_comment_index: 1,
        })
    );

    app.details_scroll = 0;
    app.selected_comment_index = 0;
    app.restore_current_conversation_details_state();

    assert_eq!(app.details_scroll, 8);
    assert_eq!(app.selected_comment_index, 1);
}

#[test]
fn recent_item_jump_preserves_current_position_from_builtin_issues_view() {
    let mut first = work_item(
        "issue-1",
        "nervosnetwork/fiber",
        941,
        "Atomic MPP design",
        None,
    );
    first.kind = ItemKind::Issue;
    first.url = "https://github.com/nervosnetwork/fiber/issues/941".to_string();
    let mut second = work_item(
        "issue-2",
        "rust-lang/rust",
        155758,
        "Rustc version fails",
        None,
    );
    second.kind = ItemKind::Issue;
    second.url = "https://github.com/rust-lang/rust/issues/155758".to_string();
    let issue_section = SectionSnapshot {
        key: "issues:involved".to_string(),
        kind: SectionKind::Issues,
        title: "Involved".to_string(),
        filters: String::new(),
        items: vec![first.clone(), second.clone()],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(SectionKind::Issues, vec![issue_section]);
    app.details.insert(
        "issue-1".to_string(),
        DetailState::Loaded(vec![
            comment("alice", "first", None),
            comment("bob", "second", None),
        ]),
    );
    app.details.insert(
        "issue-2".to_string(),
        DetailState::Loaded(vec![comment("carol", "other", None)]),
    );
    app.focus_details();
    app.details_scroll = 11;
    app.select_comment(1);
    app.recent_items = vec![
        recent_item_from_work_item(&first, Utc::now()).unwrap(),
        recent_item_from_work_item(&second, Utc::now()).unwrap(),
    ];

    app.show_recent_items_dialog();
    app.handle_recent_items_key(key(KeyCode::Enter));

    assert_eq!(
        app.current_item().map(|item| item.id.as_str()),
        Some("issue-2")
    );

    app.show_recent_items_dialog();
    app.handle_recent_items_key(key(KeyCode::Enter));

    assert_eq!(
        app.current_item().map(|item| item.id.as_str()),
        Some("issue-1")
    );
    assert_eq!(app.details_scroll, 11);
    assert_eq!(app.selected_comment_index, 1);
}

#[test]
fn view_switch_restores_shared_position_for_same_pr_across_inbox_and_repo_view() {
    let mut inbox_item = notification_item("thread-ghr-34", true);
    inbox_item.repo = "chenyukang/ghr".to_string();
    inbox_item.number = Some(34);
    inbox_item.url = "https://github.com/chenyukang/ghr/pull/34".to_string();
    let inbox_section = SectionSnapshot {
        key: "notifications:all".to_string(),
        kind: SectionKind::Notifications,
        title: "All".to_string(),
        filters: String::new(),
        items: vec![inbox_item],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let repo_pr = work_item(
        "repo-ghr-pr-34",
        "chenyukang/ghr",
        34,
        "Restore details state",
        None,
    );
    let repo_section = SectionSnapshot {
        key: "repo:ghr:pull_requests:Pull Requests".to_string(),
        kind: SectionKind::PullRequests,
        title: "Pull Requests".to_string(),
        filters: String::new(),
        items: vec![repo_pr],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(
        SectionKind::Notifications,
        vec![inbox_section, repo_section],
    );
    app.details.insert(
        "thread-ghr-34".to_string(),
        DetailState::Loaded(vec![
            comment("alice", "first", None),
            comment("bob", "second", None),
        ]),
    );
    app.focus_details();
    app.details_scroll = 0;
    app.selected_comment_index = 0;
    app.remember_current_view_snapshot();

    app.switch_view(repo_view_key("ghr"));
    app.focus_details();
    app.details_scroll = 9;
    app.selected_comment_index = 1;
    app.save_current_conversation_details_state();

    app.switch_view(builtin_view_key(SectionKind::Notifications));

    assert_eq!(
        app.current_item().map(|item| item.id.as_str()),
        Some("thread-ghr-34")
    );
    assert_eq!(app.details_scroll, 9);
    assert_eq!(app.selected_comment_index, 1);
}

#[test]
fn recent_items_enter_jumps_to_loaded_issue() {
    let mut issue = work_item(
        "issue-10",
        "chenyukang/ghr",
        10,
        "Recent bug",
        Some("alice"),
    );
    issue.kind = ItemKind::Issue;
    issue.url = "https://github.com/chenyukang/ghr/issues/10".to_string();
    let issue_section = SectionSnapshot {
        key: "issues:triage".to_string(),
        kind: SectionKind::Issues,
        title: "Triage".to_string(),
        filters: String::new(),
        items: vec![issue.clone()],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(
        SectionKind::PullRequests,
        vec![test_section(), issue_section],
    );
    app.recent_items = vec![recent_item_from_work_item(&issue, Utc::now()).unwrap()];

    app.show_recent_items_dialog();
    app.handle_recent_items_key(key(KeyCode::Enter));

    assert_eq!(app.active_view, "issues");
    assert_eq!(app.focus, FocusTarget::Details);
    assert_eq!(
        app.current_item().map(|item| item.id.as_str()),
        Some("issue-10")
    );
    assert!(app.recent_items_dialog.is_none());
    assert!(
        app.status
            .contains("[issue] #10 Recent bug.  chenyukang/ghr")
    );
}

#[test]
fn clear_cache_current_section_removes_persisted_and_loaded_list_items() {
    let paths = unique_test_paths("clear-cache-current-section");
    let store = SnapshotStore::new(paths.db_path.clone());
    store.init().expect("init snapshot store");
    let mut section = test_section();
    section.refreshed_at = Some(Utc::now());
    let mut other_item = work_item("issue-1", "chenyukang/ghr", 1, "Bug", Some("alice"));
    other_item.kind = ItemKind::Issue;
    other_item.url = "https://github.com/chenyukang/ghr/issues/1".to_string();
    let other = SectionSnapshot {
        key: "issues:test".to_string(),
        kind: SectionKind::Issues,
        title: "Issues".to_string(),
        filters: "is:open".to_string(),
        items: vec![other_item],
        total_count: Some(1),
        page: 1,
        page_size: 20,
        refreshed_at: Some(Utc::now()),
        error: None,
    };
    store.save_section(&section).expect("save current section");
    store.save_section(&other).expect("save other section");
    let mut app = AppState::new(SectionKind::PullRequests, vec![section.clone(), other]);

    app.show_cache_clear_dialog();
    app.handle_cache_clear_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &store);
    assert_eq!(
        app.cache_clear_dialog
            .as_ref()
            .and_then(|dialog| dialog.confirm),
        Some(CacheClearTarget::CurrentSection)
    );
    app.handle_cache_clear_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &store);

    assert!(app.cache_clear_dialog.is_none());
    assert!(app.sections[0].items.is_empty());
    assert_eq!(app.sections[0].refreshed_at, None);
    assert_eq!(app.sections[1].items.len(), 1);
    let snapshots = store.load_all().expect("load snapshots");
    assert!(!snapshots.contains_key(&section.key));
    assert!(snapshots.contains_key("issues:test"));
    assert!(app.status.contains("current section snapshot"));
}

#[test]
fn clear_cache_all_clears_snapshots_candidates_and_loaded_details() {
    let paths = unique_test_paths("clear-cache-all");
    let store = SnapshotStore::new(paths.db_path.clone());
    store.init().expect("init snapshot store");
    let mut section = test_section();
    section.refreshed_at = Some(Utc::now());
    store.save_section(&section).expect("save section");
    store
        .save_label_candidates("chenyukang/ghr", &["ui".to_string()])
        .expect("save labels");
    let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
    app.label_suggestions_cache
        .insert("chenyukang/ghr".to_string(), vec!["ui".to_string()]);
    app.details
        .insert("1".to_string(), DetailState::Loaded(Vec::new()));

    let summary = app
        .clear_cache(CacheClearTarget::All, &store)
        .expect("clear all cache");

    assert_eq!(summary.snapshot_rows, 1);
    assert_eq!(summary.candidate_rows, 1);
    assert_eq!(summary.loaded_entries, 1);
    assert!(app.sections[0].items.is_empty());
    assert!(app.label_suggestions_cache.is_empty());
    assert!(app.details.is_empty());
    assert!(store.load_all().expect("load snapshots").is_empty());
    assert!(
        store
            .load_repo_candidate_cache()
            .expect("load candidates")
            .labels
            .is_empty()
    );
}

#[test]
fn project_switcher_lists_repo_tabs_and_filters_by_prefix() {
    let mut config = Config::default();
    config.repos.push(crate::config::RepoConfig {
        name: "ghr".to_string(),
        repo: "chenyukang/ghr".to_string(),
        remote: None,
        local_dir: None,
        show_prs: true,
        show_issues: true,
        labels: Vec::new(),
        pr_labels: Vec::new(),
        issue_labels: Vec::new(),
    });
    config.repos.push(crate::config::RepoConfig {
        name: "Fiber".to_string(),
        repo: "nervosnetwork/fiber".to_string(),
        remote: None,
        local_dir: None,
        show_prs: true,
        show_issues: true,
        labels: Vec::new(),
        pr_labels: Vec::new(),
        issue_labels: Vec::new(),
    });
    config.repos.push(crate::config::RepoConfig {
        name: "CKB".to_string(),
        repo: "nervosnetwork/ckb".to_string(),
        remote: None,
        local_dir: None,
        show_prs: true,
        show_issues: true,
        labels: Vec::new(),
        pr_labels: Vec::new(),
        issue_labels: Vec::new(),
    });
    let mut app = AppState::new(SectionKind::PullRequests, configured_sections(&config));

    app.show_project_switcher();

    assert_eq!(
        app.project_switcher_candidates_for_query("")
            .into_iter()
            .map(|candidate| candidate.label)
            .collect::<Vec<_>>(),
        vec!["ghr", "Fiber", "CKB"]
    );
    assert_eq!(
        app.project_switcher_candidates_for_query("fi")
            .into_iter()
            .map(|candidate| candidate.label)
            .collect::<Vec<_>>(),
        vec!["Fiber"]
    );
}

#[test]
fn command_palette_switch_project_opens_project_switcher() {
    let mut config = Config::default();
    config.repos.push(crate::config::RepoConfig {
        name: "Fiber".to_string(),
        repo: "nervosnetwork/fiber".to_string(),
        remote: None,
        local_dir: None,
        show_prs: true,
        show_issues: true,
        labels: Vec::new(),
        pr_labels: Vec::new(),
        issue_labels: Vec::new(),
    });
    let mut app = AppState::new(SectionKind::PullRequests, configured_sections(&config));
    let (tx, _rx) = mpsc::unbounded_channel();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    app.command_palette = Some(CommandPalette {
        query: "project switch".to_string(),
        selected: 0,
    });

    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &config,
        &store,
        &tx,
    );

    assert!(app.command_palette.is_none());
    assert_eq!(app.project_switcher, Some(ProjectSwitcher::default()));
    assert_eq!(app.status, "project switch");
}

#[test]
fn top_menu_switcher_lists_all_top_tabs_and_filters() {
    let sections = vec![
        SectionSnapshot::empty(
            SectionKind::Notifications,
            "All",
            "is:unread reason:subscribed",
        ),
        test_section(),
        SectionSnapshot::empty(SectionKind::Issues, "Issues", "is:open"),
        SectionSnapshot::empty_for_view(
            "repo:Fiber",
            SectionKind::PullRequests,
            "Pull Requests",
            "repo:nervosnetwork/fiber is:open",
        ),
    ];
    let mut app = AppState::new(SectionKind::PullRequests, sections);

    app.show_top_menu_switcher();

    assert_eq!(
        app.top_menu_switcher_candidates_for_query("")
            .into_iter()
            .map(|candidate| candidate.label)
            .collect::<Vec<_>>(),
        vec!["Inbox", "Pull Requests", "Issues", "Fiber"]
    );
    assert_eq!(
        app.top_menu_switcher_candidates_for_query("fi")
            .into_iter()
            .map(|candidate| candidate.label)
            .collect::<Vec<_>>(),
        vec!["Fiber"]
    );
}

#[test]
fn command_palette_top_menu_switch_opens_top_menu_switcher() {
    let config = Config::default();
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    app.command_palette = Some(CommandPalette {
        query: "top menu switch".to_string(),
        selected: 0,
    });

    handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &config,
        &store,
        &tx,
    );

    assert!(app.command_palette.is_none());
    assert_eq!(app.top_menu_switcher, Some(TopMenuSwitcher::default()));
    assert_eq!(app.status, "top menu switch");
}

#[test]
fn top_menu_switcher_enter_switches_view_and_focuses_top_menu() {
    let sections = vec![
        test_section(),
        SectionSnapshot::empty(SectionKind::Issues, "Issues", "is:open"),
    ];
    let mut app = AppState::new(SectionKind::PullRequests, sections);

    app.focus_details();
    app.show_top_menu_switcher();
    app.handle_top_menu_switcher_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_top_menu_switcher_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app.active_view, builtin_view_key(SectionKind::Issues));
    assert_eq!(app.focus, FocusTarget::Ghr);
    assert!(app.top_menu_switcher.is_none());
    assert_eq!(app.status, "top menu switched: Issues");
}

#[test]
fn command_palette_project_add_opens_project_add_dialog() {
    let mut config = Config::default();
    let mut app = AppState::new(SectionKind::PullRequests, configured_sections(&config));
    let (tx, _rx) = mpsc::unbounded_channel();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    let paths = unique_test_paths("project-add-open");
    app.command_palette = Some(CommandPalette {
        query: "project add".to_string(),
        selected: 0,
    });

    assert!(!handle_key_in_area_mut(
        &mut app,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut config,
        &paths,
        &store,
        &tx,
        None,
    ));

    assert!(app.command_palette.is_none());
    assert_eq!(
        app.project_add_dialog.as_ref().map(|dialog| dialog.field),
        Some(ProjectAddField::RepoUrl)
    );
    assert_eq!(app.status, "project add");
}

#[test]
fn project_switcher_enter_switches_to_selected_project() {
    let mut config = Config::default();
    config.repos.push(crate::config::RepoConfig {
        name: "ghr".to_string(),
        repo: "chenyukang/ghr".to_string(),
        remote: None,
        local_dir: None,
        show_prs: true,
        show_issues: true,
        labels: Vec::new(),
        pr_labels: Vec::new(),
        issue_labels: Vec::new(),
    });
    config.repos.push(crate::config::RepoConfig {
        name: "Fiber".to_string(),
        repo: "nervosnetwork/fiber".to_string(),
        remote: None,
        local_dir: None,
        show_prs: true,
        show_issues: true,
        labels: Vec::new(),
        pr_labels: Vec::new(),
        issue_labels: Vec::new(),
    });
    let mut app = AppState::new(SectionKind::PullRequests, configured_sections(&config));

    app.show_project_switcher();
    app.handle_project_switcher_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
    app.handle_project_switcher_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app.active_view, "repo:Fiber");
    assert_eq!(app.focus, FocusTarget::Ghr);
    assert!(app.project_switcher.is_none());
    assert_eq!(app.status, "project switched: Fiber");
}

#[test]
fn project_add_saves_repo_to_config_and_adds_menu_tab() {
    let paths = unique_test_paths("project-add-confirm");
    let mut config = Config::default();
    config.save(&paths.config_path).expect("save config");
    let mut app = AppState::new(SectionKind::PullRequests, configured_sections(&config));
    let (tx, _rx) = mpsc::unbounded_channel();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

    app.show_project_add_dialog();
    {
        let dialog = app.project_add_dialog.as_mut().expect("project add dialog");
        dialog.repo_url = "https://github.com/chenyukang/ghr".to_string();
        dialog.local_dir = String::new();
    }
    app.handle_project_add_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
        &mut config,
        &paths,
        &store,
        &tx,
    );

    assert_eq!(config.repos.len(), 1);
    assert_eq!(config.repos[0].name, "ghr");
    assert_eq!(config.repos[0].repo, "chenyukang/ghr");
    assert_eq!(config.repos[0].local_dir.as_deref(), Some(""));
    assert!(app.project_add_dialog.is_none());
    assert_eq!(app.active_view, "repo:ghr");
    assert!(
        app.view_tabs()
            .iter()
            .any(|view| view.key == "repo:ghr" && view.label == "ghr")
    );
    assert_eq!(
        app.visible_sections()
            .iter()
            .map(|section| section.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Issues", "Pull Requests"]
    );
    assert_eq!(app.status, "project added: ghr");

    let saved = Config::load_or_create(&paths.config_path).expect("load saved config");
    assert_eq!(saved.repos.len(), 1);
    assert_eq!(saved.repos[0].name, "ghr");
    assert_eq!(saved.repos[0].repo, "chenyukang/ghr");
    assert_eq!(saved.repos[0].local_dir.as_deref(), Some(""));
}

#[test]
fn current_repo_remote_dialog_saves_selected_remote_and_adds_project_tab() {
    let paths = unique_test_paths("current-repo-remote");
    let store = SnapshotStore::new(paths.db_path.clone());
    let (tx, _rx) = mpsc::unbounded_channel();
    let directory =
        std::env::temp_dir().join(format!("ghr-current-repo-remote-{}", std::process::id()));
    let mut config = Config::default();
    config.save(&paths.config_path).expect("save config");
    let mut app = AppState::new(SectionKind::PullRequests, configured_sections(&config));
    app.show_current_repo_remote_dialog(CurrentRepoRemotePrompt {
        directory: directory.clone(),
        candidates: vec![
            GitHubRemoteCandidate {
                remote: "origin".to_string(),
                repo: "Officeyutong/tentacle".to_string(),
            },
            GitHubRemoteCandidate {
                remote: "upstream".to_string(),
                repo: "nervosnetwork/tentacle".to_string(),
            },
        ],
    });

    app.handle_current_repo_remote_key(key(KeyCode::Down), &mut config, &paths, &store, &tx);
    app.handle_current_repo_remote_key(key(KeyCode::Enter), &mut config, &paths, &store, &tx);

    assert!(app.current_repo_remote_dialog.is_none());
    assert_eq!(config.repos[0].name, "tentacle");
    assert_eq!(config.repos[0].repo, "nervosnetwork/tentacle");
    assert_eq!(config.repos[0].remote.as_deref(), Some("upstream"));
    let directory_string = directory.display().to_string();
    assert_eq!(
        config.repos[0].local_dir.as_deref(),
        Some(directory_string.as_str())
    );
    assert_eq!(app.active_view, "repo:tentacle");
    assert!(
        app.view_tabs()
            .iter()
            .any(|view| view.key == "repo:tentacle")
    );

    let saved = Config::load_or_create(&paths.config_path).expect("load saved config");
    assert_eq!(saved.repos[0].remote.as_deref(), Some("upstream"));
}

#[test]
fn project_add_uses_custom_title_and_local_dir() {
    let paths = unique_test_paths("project-add-custom");
    let mut config = Config::default();
    config.save(&paths.config_path).expect("save config");
    let mut app = AppState::new(SectionKind::PullRequests, configured_sections(&config));
    let (tx, _rx) = mpsc::unbounded_channel();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

    app.show_project_add_dialog();
    {
        let dialog = app.project_add_dialog.as_mut().expect("project add dialog");
        dialog.title = "GHR Local".to_string();
        dialog.repo_url = "git@github.com:chenyukang/ghr.git".to_string();
        dialog.local_dir = "/Users/yukang/code/playground/ghr".to_string();
    }
    app.handle_project_add_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
        &mut config,
        &paths,
        &store,
        &tx,
    );

    assert_eq!(config.repos[0].name, "GHR Local");
    assert_eq!(config.repos[0].repo, "chenyukang/ghr");
    assert_eq!(
        config.repos[0].local_dir.as_deref(),
        Some("/Users/yukang/code/playground/ghr")
    );
    assert_eq!(app.active_view, "repo:GHR Local");
}

#[test]
fn project_add_repo_input_accepts_owner_repo_and_github_urls() {
    assert_eq!(
        project_add_repo_from_input("chenyukang/ghr"),
        Some("chenyukang/ghr".to_string())
    );
    assert_eq!(
        project_add_repo_from_input("https://github.com/chenyukang/ghr"),
        Some("chenyukang/ghr".to_string())
    );
    assert_eq!(
        project_add_repo_from_input("https://github.com/chenyukang/ghr/pulls"),
        Some("chenyukang/ghr".to_string())
    );
    assert_eq!(
        project_add_repo_from_input("git@github.com:chenyukang/ghr.git"),
        Some("chenyukang/ghr".to_string())
    );
}

#[test]
fn command_palette_project_remove_opens_configured_project_picker() {
    let mut config = Config::default();
    config.repos.push(crate::config::RepoConfig {
        name: "Fiber".to_string(),
        repo: "nervosnetwork/fiber".to_string(),
        remote: None,
        local_dir: Some("/tmp/fiber".to_string()),
        show_prs: true,
        show_issues: true,
        labels: Vec::new(),
        pr_labels: Vec::new(),
        issue_labels: Vec::new(),
    });
    let mut app = AppState::new(SectionKind::PullRequests, configured_sections(&config));
    let (tx, _rx) = mpsc::unbounded_channel();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    let paths = unique_test_paths("project-remove-open");
    app.command_palette = Some(CommandPalette {
        query: "project remove".to_string(),
        selected: 0,
    });

    assert!(!handle_key_in_area_mut(
        &mut app,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut config,
        &paths,
        &store,
        &tx,
        None,
    ));

    let dialog = app
        .project_remove_dialog
        .as_ref()
        .expect("remove dialog should open");
    assert!(app.command_palette.is_none());
    assert_eq!(dialog.candidates.len(), 1);
    assert_eq!(dialog.candidates[0].name, "Fiber");
    assert_eq!(app.status, "project remove");
}

#[test]
fn project_remove_confirmation_removes_repo_from_config_and_ui() {
    let paths = unique_test_paths("project-remove-confirm");
    let mut config = Config::default();
    config.repos.push(crate::config::RepoConfig {
        name: "ghr".to_string(),
        repo: "chenyukang/ghr".to_string(),
        remote: None,
        local_dir: Some("/tmp/ghr".to_string()),
        show_prs: true,
        show_issues: true,
        labels: Vec::new(),
        pr_labels: Vec::new(),
        issue_labels: Vec::new(),
    });
    config.repos.push(crate::config::RepoConfig {
        name: "Fiber".to_string(),
        repo: "nervosnetwork/fiber".to_string(),
        remote: None,
        local_dir: Some("/tmp/fiber".to_string()),
        show_prs: true,
        show_issues: true,
        labels: Vec::new(),
        pr_labels: Vec::new(),
        issue_labels: Vec::new(),
    });
    config.save(&paths.config_path).expect("save config");
    let mut app = AppState::new(SectionKind::PullRequests, configured_sections(&config));
    app.switch_view("repo:Fiber");

    app.show_project_remove_dialog(&config);
    app.handle_project_remove_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut config,
        &paths,
    );

    assert_eq!(
        app.project_remove_dialog
            .as_ref()
            .and_then(|dialog| dialog.confirm.as_ref())
            .map(|candidate| candidate.name.as_str()),
        Some("Fiber")
    );

    app.handle_project_remove_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut config,
        &paths,
    );

    assert_eq!(config.repos.len(), 1);
    assert_eq!(config.repos[0].name, "ghr");
    assert!(app.project_remove_dialog.is_none());
    assert!(
        !app.sections
            .iter()
            .any(|section| section_view_key(section) == "repo:Fiber")
    );
    assert_ne!(app.active_view, "repo:Fiber");
    assert_eq!(app.status, "project removed: Fiber");

    let saved = Config::load_or_create(&paths.config_path).expect("load saved config");
    assert_eq!(saved.repos.len(), 1);
    assert_eq!(saved.repos[0].name, "ghr");
}

#[test]
fn command_palette_area_is_centered_in_viewport() {
    let area = Rect::new(0, 0, 120, 40);
    let palette = command_palette_area(area);

    assert_eq!(palette.width, 91);
    assert_eq!(palette.height, 18);
    assert_eq!(palette.x, 14);
    assert_eq!(palette.y, 11);
}

#[test]
fn modal_footer_is_below_dialog_and_same_width() {
    let area = Rect::new(0, 0, 120, 40);
    let dialog = Rect::new(14, 11, 91, 18);

    assert_eq!(
        modal_footer_area(area, dialog),
        Some(Rect::new(14, 29, 91, 1))
    );
}

#[test]
fn command_palette_enter_dispatches_selected_shortcut() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    app.command_palette = Some(CommandPalette {
        query: "help".to_string(),
        selected: 0,
    });

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Enter),
        &config,
        &store,
        &tx
    ));

    assert!(app.command_palette.is_none());
    assert!(app.help_dialog);
    assert_eq!(app.status, "help");
}

#[test]
fn command_palette_global_command_works_over_comment_editor() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    app.start_new_comment_dialog();
    app.comment_dialog.as_mut().unwrap().body.set_text("draft");
    app.command_palette = Some(CommandPalette {
        query: "help".to_string(),
        selected: 0,
    });

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Enter),
        &config,
        &store,
        &tx
    ));

    assert!(app.command_palette.is_none());
    assert!(app.help_dialog);
    assert_eq!(
        app.comment_dialog
            .as_ref()
            .map(|dialog| dialog.body.as_str()),
        Some("draft")
    );
}

#[test]
fn command_palette_escape_dismisses_without_dispatching() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    app.command_palette = Some(CommandPalette {
        query: "merge".to_string(),
        selected: 0,
    });

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Esc),
        &config,
        &store,
        &tx
    ));

    assert!(app.command_palette.is_none());
    assert!(!app.help_dialog);
    assert_eq!(app.status, "command palette dismissed");
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

    assert!(rendered.contains("Compiler diagnostics"));
    assert!(rendered.contains("A body with useful context"));
    assert!(!rendered.contains("Details:"));
    assert!(!rendered.contains('┃'));
    assert!(!rendered.contains("Funding state"));
    assert!(!rendered.contains("Updated"));
}

#[test]
fn light_theme_renders_background_and_active_tabs_from_palette() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.set_theme(ThemeName::Light);
    let backend = ratatui::backend::TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let paths = test_paths();

    terminal
        .draw(|frame| draw(frame, &app, &paths))
        .expect("draw");

    let theme = crate::theme::Theme::from_name(ThemeName::Light);
    let buffer = terminal.backend().buffer();
    assert_eq!(
        buffer.cell((0, 0)).expect("top-left cell").bg,
        theme.surface
    );

    let lines = buffer_lines(buffer);
    let (row, column) = lines
        .iter()
        .enumerate()
        .find_map(|(row, line)| line.find("Pull Requests").map(|column| (row, column)))
        .expect("active pull requests tab");
    let tab_cell = buffer
        .cell((column as u16, row as u16))
        .expect("active tab cell");

    assert_eq!(tab_cell.fg, theme.highlight_fg);
    assert_eq!(tab_cell.bg, theme.highlight_bg);
    assert!(tab_cell.modifier.contains(Modifier::BOLD));

    let rendered = lines.join("\n");
    assert!(rendered.contains("GHR"));
    assert!(!rendered.contains("ghr"));

    let (ghr_row, ghr_column) = lines
        .iter()
        .enumerate()
        .find_map(|(row, line)| line.find("GHR").map(|column| (row, column)))
        .expect("GHR tab group title");
    let ghr_title_cell = buffer
        .cell((ghr_column as u16, ghr_row as u16))
        .expect("GHR title cell");
    assert_eq!(ghr_title_cell.fg, theme.muted);

    let (sections_row, sections_column) = lines
        .iter()
        .enumerate()
        .find_map(|(row, line)| line.find("Sections").map(|column| (row, column)))
        .expect("sections tab group title");
    let sections_title_cell = buffer
        .cell((sections_column as u16, sections_row as u16))
        .expect("sections title cell");
    assert_eq!(sections_title_cell.fg, theme.muted);
}

#[test]
fn dark_theme_renders_explicit_background() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.set_theme(ThemeName::Dark);
    let backend = ratatui::backend::TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let paths = test_paths();

    terminal
        .draw(|frame| draw(frame, &app, &paths))
        .expect("draw");

    let theme = crate::theme::Theme::from_name(ThemeName::Dark);
    let buffer = terminal.backend().buffer();
    assert_ne!(theme.surface, Color::Reset);
    assert_eq!(
        buffer.cell((0, 0)).expect("top-left cell").bg,
        theme.surface
    );
    assert_eq!(
        buffer.cell((0, 0)).expect("top-left border").fg,
        theme.border
    );
    assert_ne!(theme.border, Color::DarkGray);
}

#[test]
fn light_theme_global_search_modal_uses_contrast_styles() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.set_theme(ThemeName::Light);
    app.start_global_search_input();
    {
        let dialog = app.global_search_dialog.as_mut().expect("search dialog");
        dialog.field = GlobalSearchField::Status;
        reset_global_search_dialog_suggestions(dialog);
    }
    let backend = ratatui::backend::TestBackend::new(180, 40);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let paths = test_paths();

    terminal
        .draw(|frame| draw(frame, &app, &paths))
        .expect("draw");

    let theme = crate::theme::Theme::from_name(ThemeName::Light);
    let buffer = terminal.backend().buffer();
    let lines = buffer_lines(buffer);

    let (sort_row, sort_col) = lines
        .iter()
        .enumerate()
        .find_map(|(row, line)| line.find("created_at").map(|column| (row, column)))
        .expect("sort value");
    let sort_cell = buffer
        .cell((sort_col as u16, sort_row as u16))
        .expect("sort value cell");
    assert_eq!(sort_cell.fg, theme.text);
    assert_eq!(sort_cell.bg, theme.surface);

    let (label_row, label_col) = lines
        .iter()
        .enumerate()
        .find_map(|(row, line)| line.find("Label").map(|column| (row, column)))
        .expect("inactive label");
    let label_cell = buffer
        .cell((label_col as u16, label_row as u16))
        .expect("inactive label cell");
    assert_eq!(label_cell.fg, theme.muted);

    let options_row = lines
        .iter()
        .position(|line| line.contains("Options"))
        .expect("options heading");
    let (candidate_row, candidate_col) = lines
        .iter()
        .enumerate()
        .skip(options_row + 1)
        .find_map(|(row, line)| {
            line.find("> open")
                .map(|column| (row, display_width(&line[..column]) + 2))
        })
        .expect("selected status candidate");
    let candidate_cell = buffer
        .cell((candidate_col as u16, candidate_row as u16))
        .expect("selected candidate cell");
    assert_eq!(candidate_cell.fg, theme.highlight_fg);
    assert_eq!(candidate_cell.bg, theme.highlight_bg);
}

#[test]
fn details_render_clears_stale_cells_when_scrolling_short_lines() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let item_id = app.current_item().expect("item").id.clone();
    let mut comment = comment("chenyukang", "short body", None);
    comment.reactions.heart = 1;
    app.details
        .insert(item_id, DetailState::Loaded(vec![comment]));
    app.focus_details();
    app.selected_comment_index = 0;
    let area = Rect::new(0, 0, 120, 32);
    let details_area = details_area_for(&app, area);
    let inner = block_inner(details_area);
    let document = build_details_document(&app, inner.width);
    let header_line = document
        .lines
        .iter()
        .position(|line| {
            let text = line.to_string();
            text.contains("+ react") && text.contains("reply")
        })
        .expect("comment header");
    let paths = test_paths();
    let backend = ratatui::backend::TestBackend::new(area.width, area.height);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    app.details_scroll = header_line.min(usize::from(u16::MAX)) as u16;
    terminal
        .draw(|frame| draw(frame, &app, &paths))
        .expect("draw header");

    app.details_scroll = header_line.saturating_add(1).min(usize::from(u16::MAX)) as u16;
    terminal
        .draw(|frame| draw(frame, &app, &paths))
        .expect("draw following short line");

    let top_details_line = &buffer_lines(terminal.backend().buffer())[inner.y as usize];
    assert!(
        !top_details_line.contains("+ react") && !top_details_line.contains("reply"),
        "stale comment header cells leaked after scroll: {top_details_line:?}"
    );
}

#[test]
fn list_table_renders_updated_next_to_meta() {
    let mut section = test_section();
    section.items[0].updated_at = Some(Utc::now() - chrono::Duration::days(2));
    let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
    app.list_width_percent = crate::state::MAX_LIST_WIDTH_PERCENT;
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
                && line.contains("#")
                && line.contains("Title")
                && line.contains("Updated")
                && line.contains("Meta")
        })
        .expect("list header");
    let repo_pos = header.find("Repo").expect("repo column");
    let number_pos = header.find("#").expect("number column");
    let title_pos = header.find("Title").expect("title column");
    let updated_pos = header.find("Updated").expect("updated column");
    let meta_pos = header.find("Meta").expect("meta column");
    assert_eq!(number_pos.saturating_sub(repo_pos), 21);
    assert!(repo_pos < number_pos);
    assert!(number_pos < title_pos);
    assert!(title_pos < updated_pos);
    assert!(
        updated_pos.saturating_sub(title_pos) >= 74,
        "header positions: {header:?}"
    );
    assert!(updated_pos < meta_pos);

    let row = lines
        .iter()
        .find(|line| line.contains("rust-lang/rust") && line.contains("Compiler diagnostics"))
        .expect("list row");
    let repo_pos = row.find("rust-lang/rust").expect("repo cell");
    let number_pos = row.find("#1").expect("number cell");
    let title_pos = row.find("Compiler diagnostics").expect("title cell");
    let updated_pos = row.find("2d").expect("updated cell");
    let meta_pos = row.find("open 0c").expect("meta cell");
    assert_eq!(number_pos.saturating_sub(repo_pos), 21);
    assert!(title_pos < updated_pos);
    assert!(updated_pos.saturating_sub(title_pos) >= 74);
    assert!(updated_pos < meta_pos);
}

#[test]
fn repo_list_table_hides_redundant_repo_column() {
    let view = repo_view_key("Rust");
    let mut section = SectionSnapshot::empty_for_view(
        &view,
        SectionKind::PullRequests,
        "Pull Requests",
        "repo:rust-lang/rust",
    );
    section.items = vec![work_item(
        "rust-lang/rust#1",
        "rust-lang/rust",
        1,
        "Compiler diagnostics",
        None,
    )];
    section.items[0].updated_at = Some(Utc::now() - chrono::Duration::hours(3));
    let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
    app.active_view = view;
    app.list_width_percent = crate::state::MAX_LIST_WIDTH_PERCENT;
    let area = Rect::new(0, 0, 180, 30);
    let list_area = layout::body_areas_with_ratio(body_area(area), app.list_width_percent)[0];
    let list_width = usize::from(list_area.x.saturating_add(list_area.width));
    let backend = ratatui::backend::TestBackend::new(area.width, area.height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let paths = test_paths();

    terminal
        .draw(|frame| draw(frame, &app, &paths))
        .expect("draw");

    let lines = buffer_lines(terminal.backend().buffer());
    let header = lines
        .iter()
        .find(|line| {
            line.contains("#")
                && line.contains("Title")
                && line.contains("Updated")
                && line.contains("Meta")
        })
        .expect("repo list header");
    let header = &header[..header.len().min(list_width)];
    assert!(!header.contains("Repo"));
    let number_pos = header.find("#").expect("number column");
    let title_pos = header.find("Title").expect("title column");
    let updated_pos = header.find("Updated").expect("updated column");
    assert!(number_pos < title_pos);
    assert!(title_pos < updated_pos);
    assert!(
        updated_pos.saturating_sub(title_pos) >= 94,
        "header positions: {header:?}"
    );

    let row = lines
        .iter()
        .find(|line| line.contains("#1") && line.contains("Compiler diagnostics"))
        .expect("repo list row");
    let row = &row[..row.len().min(list_width)];
    assert!(!row.contains("rust-lang/rust"));
    let number_pos = row.find("#1").expect("number cell");
    let title_pos = row.find("Compiler diagnostics").expect("title cell");
    let updated_pos = row.find("3h").expect("updated cell");
    assert!(number_pos < title_pos);
    assert!(title_pos < updated_pos);
    assert!(
        updated_pos.saturating_sub(title_pos) >= 94,
        "row positions: {row:?}"
    );
}

#[test]
fn list_rows_start_immediately_after_header() {
    let app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let backend = ratatui::backend::TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let paths = test_paths();

    terminal
        .draw(|frame| draw(frame, &app, &paths))
        .expect("draw");

    let lines = buffer_lines(terminal.backend().buffer());
    let header_index = lines
        .iter()
        .position(|line| {
            line.contains("#")
                && line.contains("Title")
                && line.contains("Updated")
                && line.contains("Meta")
        })
        .expect("list header");
    let selected_row_index = lines
        .iter()
        .position(|line| line.contains("Compiler diagnostics"))
        .expect("selected row");

    assert_eq!(selected_row_index, header_index + 1);
}

#[test]
fn notification_list_table_hides_meta_and_moves_updated_right() {
    let mut item = notification_item("thread-1", true);
    item.title = "Notification title".to_string();
    item.updated_at = Some(Utc::now() - chrono::Duration::days(1));
    let section = SectionSnapshot {
        key: "notifications:all".to_string(),
        kind: SectionKind::Notifications,
        title: "All".to_string(),
        filters: "is:all".to_string(),
        items: vec![item],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(SectionKind::Notifications, vec![section]);
    app.list_width_percent = crate::state::MAX_LIST_WIDTH_PERCENT;
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
                && line.contains("#")
                && line.contains("Title")
                && line.contains("Updated")
        })
        .expect("notification list header");
    assert!(!header.contains("Meta"));
    let title_pos = header.find("Title").expect("title column");
    let updated_pos = header.find("Updated").expect("updated column");
    assert!(
        updated_pos.saturating_sub(title_pos) >= 88,
        "header positions: {header:?}"
    );

    let row = lines
        .iter()
        .find(|line| line.contains("rust-lang/rust") && line.contains("Notification title"))
        .expect("notification list row");
    assert!(!row.contains("review-requested"));
    assert!(!row.contains("mention PullRequest"));
    let title_pos = row.find("Notification title").expect("title cell");
    let updated_pos = row.find("1d").expect("updated cell");
    assert!(
        updated_pos.saturating_sub(title_pos) >= 88,
        "row positions: {row:?}"
    );
}

#[test]
fn inbox_rows_dim_read_items_and_bold_unread_items() {
    let app = AppState::new(SectionKind::Notifications, vec![]);
    let unread = notification_item("thread-1", true);
    let read = notification_item("thread-2", false);

    let unread_style = list_item_row_style(&app, &unread);
    assert_eq!(unread_style.fg, Some(Color::White));
    assert!(unread_style.add_modifier.contains(Modifier::BOLD));

    let read_style = list_item_row_style(&app, &read);
    assert_eq!(
        read_style.fg,
        Some(crate::theme::Theme::from_name(ThemeName::Dark).subtle)
    );
    assert!(!read_style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn first_load_repo_section_renders_loading_hint_in_list() {
    let section = SectionSnapshot::empty_for_view(
        "repo:ghr",
        SectionKind::PullRequests,
        "Pull Requests",
        "repo:chenyukang/ghr is:open archived:false sort:created-desc",
    );
    let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
    app.switch_view("repo:ghr");
    app.refreshing = true;
    let backend = ratatui::backend::TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let paths = test_paths();

    terminal
        .draw(|frame| draw(frame, &app, &paths))
        .expect("draw");

    let rendered = buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(rendered.contains("Git repo is loading ..."));
}

#[test]
fn empty_loaded_repo_section_renders_empty_hint_not_loading() {
    let mut section = SectionSnapshot::empty_for_view(
        "repo:ghr",
        SectionKind::PullRequests,
        "Pull Requests",
        "repo:chenyukang/ghr is:open archived:false sort:created-desc",
    );
    section.refreshed_at = Some(Utc::now());
    section.total_count = Some(0);
    let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
    app.switch_view("repo:ghr");
    app.refreshing = true;
    let backend = ratatui::backend::TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let paths = test_paths();

    terminal
        .draw(|frame| draw(frame, &app, &paths))
        .expect("draw");

    let rendered = buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(rendered.contains("No items found."));
    assert!(!rendered.contains(
        "Git repo is loading, it will take for a while at the first time, please wait ..."
    ));
}

#[test]
fn footer_uses_list_shortcuts_and_status() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.focus_list();
    let paths = test_paths();
    let text = footer_line(&app, &paths).to_string();

    assert!(text.contains(
        "j/k/n/p move  [ ] page  tab Details  enter Details  / search  v diff  i ignore  e edit  a comment"
    ));
    assert!(!text.contains("List items"));
    assert!(!text.contains("Details content"));
    assert!(text.contains("/ search"));
    assert!(text.contains("v diff"));
    assert!(text.contains("i ignore"));
    assert!(text.contains("? help  : cmd  f filter  r refresh  q quit  o open"));
    assert!(text.contains("m text-select"));
    assert!(!text.contains("focus List"));
    assert!(!text.contains("refresh idle"));
    assert!(!text.contains("status:"));
    assert!(!text.contains("state list focused"));
    assert!(!text.contains("1 ghr  2 Sections  3 list  4 Details"));
    assert!(!text.contains("n/p comment"));
    assert!(!text.contains("db "));

    let compact = footer_line_for_width(&app, &paths, 80).to_string();
    assert!(display_width(&compact) <= 80);
    assert!(compact.contains("j/k/n/p move"));
    assert!(compact.contains("[ ] page"));
    assert!(compact.contains("tab Details"));
    assert!(compact.contains("enter Details"));
    assert!(compact.contains("/ search"));
    assert!(!compact.contains("M/C/D/U/E/O/F/X actions"));
    assert!(!compact.contains("1-4 focus"));

    app.refreshing = true;
    let refreshing = footer_line(&app, &paths).to_string();
    assert!(refreshing.contains("j/k/n/p move"));
    assert!(!refreshing.contains("status: refreshing"));
    assert_eq!(top_status_line(&app, 32).to_string(), "status: refreshing");
}

#[test]
fn section_page_loading_overrides_generic_refresh_status() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.refreshing = true;
    app.section_page_loading = Some(SectionPageLoading {
        section_key: "pull_requests:test".to_string(),
        title: "Pull Requests".to_string(),
        page_label: "2/20+".to_string(),
        started_at: Instant::now(),
    });

    assert_eq!(footer_status(&app), "loading Pull Requests page 2/20+");
    assert!(
        top_status_line(&app, 42)
            .to_string()
            .contains("loading Pull Requests")
    );
}

#[test]
fn status_age_labels_list_refresh_source() {
    let mut section = test_section();
    section.refreshed_at = Some(Utc::now() - chrono::Duration::seconds(34));
    let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
    app.status = "comment 4 focused".to_string();

    let status = footer_status(&app);

    assert!(status.contains("comment 4 focused"));
    assert!(status.contains("list refreshed"));
    assert!(!status.contains("details refreshed"));
}

#[test]
fn status_age_prefers_details_refresh_source_while_details_focused() {
    let mut section = test_section();
    section.refreshed_at = Some(Utc::now() - chrono::Duration::seconds(120));
    let item_id = section.items[0].id.clone();
    let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
    app.focus = FocusTarget::Details;
    app.details_mode = DetailsMode::Conversation;
    app.status = "comment 4 focused".to_string();
    app.details_refreshed_at
        .insert(item_id, Utc::now() - chrono::Duration::seconds(34));

    let status = footer_status(&app);

    assert!(status.contains("comment 4 focused"));
    assert!(status.contains("details refreshed"));
    assert!(!status.contains("list refreshed"));
}

#[test]
fn section_page_loading_dialog_renders_page_context() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.refreshing = true;
    app.section_page_loading = Some(SectionPageLoading {
        section_key: "pull_requests:test".to_string(),
        title: "Pull Requests".to_string(),
        page_label: "2/20+".to_string(),
        started_at: Instant::now() - Duration::from_secs(2),
    });
    let backend = ratatui::backend::TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let paths = test_paths();

    terminal
        .draw(|frame| draw(frame, &app, &paths))
        .expect("draw");

    let rendered = buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(rendered.contains("Loading Page"));
    assert!(rendered.contains("Fetching GitHub result page."));
    assert!(rendered.contains("section: Pull Requests"));
    assert!(rendered.contains("page: 2/20+"));
}

#[test]
fn footer_shows_merge_method_shortcuts_in_merge_confirmation() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_pr_action_dialog(PrAction::Merge);
    let paths = test_paths();
    let text = footer_line(&app, &paths).to_string();

    assert!(!text.contains("Confirm PR action"));
    assert!(text.contains("m/s/r method"));
    assert!(text.contains("y/enter run"));
    assert!(text.contains("esc cancel"));
    assert!(
        top_status_line(&app, 40)
            .to_string()
            .contains("confirm pull request merge")
    );
}

#[test]
fn footer_switches_shortcuts_for_each_focus_region() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let paths = test_paths();

    app.focus_ghr();
    let ghr = footer_line(&app, &paths).to_string();
    assert!(ghr.contains("tab/h/l/[ ] switch  j/n/enter Sections  esc List"));
    assert!(!ghr.contains("M/C/D/U/E/O/F/X actions"));
    assert!(!ghr.contains("t milestone"));

    app.focus_sections();
    let sections = footer_line(&app, &paths).to_string();
    assert!(sections.contains("tab/h/l/[ ] switch  k/p GHR  j/n/enter List"));
    assert!(!sections.contains("a comment"));

    app.focus_details();
    let details = footer_line(&app, &paths).to_string();
    assert!(details.contains("j/k scroll"));
    assert!(!details.contains("Details content"));
    assert!(details.contains("tab List  v diff  / search  c/a comment"));
    assert!(!details.contains("R reply"));
    assert!(details.contains("e edit"));
    assert!(!details.contains("enter expand"));
    assert!(details.contains("esc List"));
    assert!(!details.contains("g/G ends"));

    app.details.insert(
        "1".to_string(),
        DetailState::Loaded(vec![comment("alice", "Needs a reply", None)]),
    );
    let details_with_comment = footer_line(&app, &paths).to_string();
    assert!(details_with_comment.contains("n/p comment  enter expand"));
    assert!(details_with_comment.contains("R reply"));
    assert!(details_with_comment.contains("e edit"));

    app.details.insert(
        "1".to_string(),
        DetailState::Loaded(vec![own_comment(
            42,
            "chenyukang",
            "My editable comment",
            None,
        )]),
    );
    let details_with_own_comment = footer_line(&app, &paths).to_string();
    assert!(details_with_own_comment.contains("R reply  e edit"));

    app.sections[0].items[0].assignees = vec!["alice".to_string()];
    let details_with_assignee = footer_line(&app, &paths).to_string();
    assert!(!details_with_assignee.contains("@/- assign"));

    app.show_diff();
    app.focus_details();
    let diff = footer_line(&app, &paths).to_string();
    assert!(diff.contains("j/k line  tab files  n/p comment  h/l page"));
    assert!(!diff.contains("Details diff"));
    assert!(diff.contains("c inline  a comment"));
    assert!(!diff.contains("m/e range"));
    assert!(!diff.contains("M/C/D/U/E/O/F/X actions"));
    assert!(!diff.contains("m text-select"));
    assert!(diff.contains("q back"));
    assert!(!diff.contains("q quit"));
    assert!(!diff.contains("R reply"));

    app.focus_list();
    let diff_list = footer_line(&app, &paths).to_string();
    assert!(diff_list.contains("j/k file  tab diff  enter diff"));
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
fn conversation_details_p_at_first_comment_focuses_details_body() {
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

    assert_eq!(app.selected_comment_index, NO_SELECTED_COMMENT_INDEX);
    assert_eq!(app.details_scroll, 0);
    assert!(app.details_scroll < before);
    assert_eq!(app.status, "pull request details focused");
}

#[test]
fn top_status_label_uses_active_color() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);

    app.set_theme(ThemeName::Dark);
    app.focus_details();
    let line = top_status_line(&app, 40);

    let status = line
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "status:")
        .expect("status label");
    assert_eq!(status.style.fg, Some(Color::Cyan));
    assert!(status.style.add_modifier.contains(Modifier::BOLD));

    let value = line
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "details focused")
        .expect("status value");
    assert_eq!(
        value.style.fg,
        Some(crate::theme::Theme::from_name(ThemeName::Dark).success)
    );
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
    app.global_search_dialog
        .as_mut()
        .expect("search dialog")
        .title
        .set_text("borrow");
    let backend = ratatui::backend::TestBackend::new(220, 40);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let paths = test_paths();

    terminal
        .draw(|frame| draw(frame, &app, &paths))
        .expect("draw");

    let rendered = buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(rendered.contains("Repo Search in rust-lang/rust: dialog open"));
    assert!(rendered.contains("Search Issues and Pull Requests"));
    assert!(rendered.contains("Title / #"));
    assert!(rendered.contains("borrow"));
    assert!(rendered.contains("created_at"));
}

#[test]
fn quick_filter_query_replaces_state_and_adds_qualifiers_before_sort() {
    let filter = QuickFilter::parse("state:closed label:bug author:alice assignee:bob").unwrap();
    let filter = filter.expect("filter");

    assert_eq!(
        quick_filter_query(
            "repo:owner/repo is:open author:me label:old archived:false sort:updated-desc",
            &filter,
        ),
        "repo:owner/repo archived:false is:closed assignee:bob author:alice label:bug sort:updated-desc"
    );
}

#[test]
fn quick_filter_state_shortcuts_toggle_query_state() {
    let base = "repo:owner/repo is:open archived:false sort:updated-desc";
    let closed = QuickFilter::parse("closed").unwrap().expect("closed");
    let close = QuickFilter::parse("state:close").unwrap().expect("close");
    let merged = QuickFilter::parse("state:merged").unwrap().expect("merged");
    let draft = QuickFilter::parse("draft").unwrap().expect("draft");
    let all = QuickFilter::parse("all").unwrap().expect("all");

    assert!(quick_filter_query(base, &closed).contains("is:closed"));
    assert!(quick_filter_query(base, &close).contains("is:closed"));
    assert_eq!(close.display(), "state:closed");
    assert!(quick_filter_query(base, &merged).contains("is:merged"));
    assert!(quick_filter_query(base, &draft).contains("is:draft"));
    assert_eq!(
        quick_filter_query(base, &all),
        "repo:owner/repo archived:false sort:updated-desc"
    );
}

#[test]
fn quick_filter_applies_assignee_author_and_multiple_labels() {
    let filter = QuickFilter::parse("assignee:bob author:alice labels:bug,regression")
        .unwrap()
        .expect("filter");

    assert_eq!(
        quick_filter_query("is:open sort:updated-desc", &filter),
        "is:open assignee:bob author:alice label:bug label:regression sort:updated-desc"
    );
    assert_eq!(
        filter.display(),
        "assignee:bob author:alice label:bug label:regression"
    );
}

#[test]
fn quick_filter_clear_inputs_reset_overlay() {
    assert_eq!(QuickFilter::parse("").unwrap(), None);
    assert_eq!(QuickFilter::parse("clear").unwrap(), None);
    assert_eq!(QuickFilter::parse("reset").unwrap(), None);
}

#[test]
fn list_title_and_footer_show_active_quick_filter() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.quick_filters.insert(
        "pull_requests:test".to_string(),
        QuickFilter::parse("state:closed label:bug")
            .unwrap()
            .expect("filter"),
    );
    let backend = ratatui::backend::TestBackend::new(220, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let paths = test_paths();

    terminal
        .draw(|frame| draw(frame, &app, &paths))
        .expect("draw");

    let rendered = buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(rendered.contains("Test [state:closed label:bug]"));
    assert!(rendered.contains("filter: state:closed label:bug"));
}

#[test]
fn filter_input_prompt_is_discoverable_and_prefilled() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.quick_filters.insert(
        "pull_requests:test".to_string(),
        QuickFilter::parse("state:closed author:alice")
            .unwrap()
            .expect("filter"),
    );

    app.start_filter_input();

    assert!(app.filter_input_active);
    assert_eq!(app.filter_input_query, "state:closed author:alice");
    assert_eq!(
        active_list_input_prompt(&app).map(|(prompt, _)| prompt),
        Some(
            "Filter: fstate:closed author:alice_  Enter apply  empty/clear resets  Esc cancel"
                .to_string()
        )
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

    app.move_comment_in_view(-1, None);
    assert_eq!(app.selected_comment_index, 2);
    assert_eq!(app.status, "comment search: 2/2 for 'compiler'");
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
    let area = Rect::new(0, 0, 220, 40);
    let details_area = details_area_for(&app, area);
    let inner = block_inner(details_area);
    let document = build_details_document(&app, inner.width);
    let search_line = document
        .lines
        .iter()
        .position(|line| line.to_string().contains("Comment search: 1/1"))
        .expect("comment search summary line");
    app.details_scroll = search_line.saturating_sub(1).min(usize::from(u16::MAX)) as u16;
    let backend = ratatui::backend::TestBackend::new(area.width, area.height);
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
fn details_title_keeps_comment_search_prompt_after_enter() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let item_id = app.current_item().expect("item").id.clone();
    app.details.insert(
        item_id,
        DetailState::Loaded(vec![
            comment("alice", "borrow checker", None),
            comment("bob", "compiler diagnostic", None),
        ]),
    );
    app.focus_details();
    app.comment_search_active = false;
    app.comment_search_query = "borrow".to_string();
    let area = Rect::new(0, 0, 220, 40);
    let backend = ratatui::backend::TestBackend::new(area.width, area.height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let paths = test_paths();

    terminal
        .draw(|frame| draw(frame, &app, &paths))
        .expect("draw");

    let rendered = buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(rendered.contains("Comment Search: /borrow  n/p results  Esc clear"));
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
fn notification_section_tab_label_shows_only_unread_count() {
    let section = SectionSnapshot {
        key: "notifications:all".to_string(),
        kind: SectionKind::Notifications,
        title: "All".to_string(),
        filters: "is:all".to_string(),
        items: vec![
            notification_item("thread-1", true),
            notification_item("thread-2", false),
            notification_item("thread-3", true),
        ],
        total_count: Some(50),
        page: 1,
        page_size: 50,
        refreshed_at: None,
        error: None,
    };
    let app = AppState::new(SectionKind::Notifications, vec![section]);

    assert_eq!(
        section_tab_label(&app, app.visible_sections()[0]),
        "All (2)"
    );
}

#[test]
fn notification_section_tab_label_hides_zero_unread_count() {
    let section = SectionSnapshot {
        key: "notifications:mentioned".to_string(),
        kind: SectionKind::Notifications,
        title: "Mentioned".to_string(),
        filters: "reason:mention".to_string(),
        items: vec![notification_item("thread-1", false)],
        total_count: Some(50),
        page: 1,
        page_size: 50,
        refreshed_at: None,
        error: None,
    };
    let app = AppState::new(SectionKind::Notifications, vec![section]);

    assert_eq!(
        section_tab_label(&app, app.visible_sections()[0]),
        "Mentioned"
    );
}

#[test]
fn notification_section_tabs_show_each_section_unread_count() {
    let sections = vec![
        SectionSnapshot {
            key: "notifications:all".to_string(),
            kind: SectionKind::Notifications,
            title: "All".to_string(),
            filters: "is:all".to_string(),
            items: vec![
                notification_item("thread-1", true),
                notification_item("thread-2", false),
                notification_item("thread-3", true),
            ],
            total_count: Some(50),
            page: 1,
            page_size: 50,
            refreshed_at: None,
            error: None,
        },
        SectionSnapshot {
            key: "notifications:review-requested".to_string(),
            kind: SectionKind::Notifications,
            title: "Review Requested".to_string(),
            filters: "reason:review-requested".to_string(),
            items: vec![notification_item("thread-1", true)],
            total_count: Some(50),
            page: 1,
            page_size: 50,
            refreshed_at: None,
            error: None,
        },
        SectionSnapshot {
            key: "notifications:assigned".to_string(),
            kind: SectionKind::Notifications,
            title: "Assigned".to_string(),
            filters: "reason:assign".to_string(),
            items: vec![notification_item("thread-2", false)],
            total_count: Some(50),
            page: 1,
            page_size: 50,
            refreshed_at: None,
            error: None,
        },
        SectionSnapshot {
            key: "notifications:mentioned".to_string(),
            kind: SectionKind::Notifications,
            title: "Mentioned".to_string(),
            filters: "reason:mention".to_string(),
            items: vec![
                notification_item("thread-4", true),
                notification_item("thread-5", true),
                notification_item("thread-6", false),
            ],
            total_count: Some(50),
            page: 1,
            page_size: 50,
            refreshed_at: None,
            error: None,
        },
    ];
    let app = AppState::new(SectionKind::Notifications, sections);
    let labels = app
        .visible_sections()
        .into_iter()
        .map(|section| section_tab_label(&app, section))
        .collect::<Vec<_>>();

    assert_eq!(
        labels,
        vec![
            "All (2)",
            "Review Requested (1)",
            "Assigned",
            "Mentioned (2)"
        ]
    );
}

#[test]
fn inbox_top_tab_label_shows_unread_notification_count() {
    let section = SectionSnapshot {
        key: "notifications:all".to_string(),
        kind: SectionKind::Notifications,
        title: "All".to_string(),
        filters: "is:all".to_string(),
        items: vec![
            notification_item("thread-1", true),
            notification_item("thread-2", false),
            notification_item("thread-3", true),
        ],
        total_count: Some(50),
        page: 1,
        page_size: 50,
        refreshed_at: None,
        error: None,
    };
    let app = AppState::new(SectionKind::Notifications, vec![section]);

    let inbox = app
        .view_tabs()
        .into_iter()
        .find(|view| view.key == builtin_view_key(SectionKind::Notifications))
        .expect("inbox tab");

    assert_eq!(inbox.label, "Inbox (2)");
}

#[test]
fn inbox_top_tab_label_deduplicates_unread_threads() {
    let sections = vec![
        SectionSnapshot {
            key: "notifications:all".to_string(),
            kind: SectionKind::Notifications,
            title: "All".to_string(),
            filters: "is:all".to_string(),
            items: vec![
                notification_item("thread-1", true),
                notification_item("thread-2", true),
            ],
            total_count: Some(50),
            page: 1,
            page_size: 50,
            refreshed_at: None,
            error: None,
        },
        SectionSnapshot {
            key: "notifications:review-requested".to_string(),
            kind: SectionKind::Notifications,
            title: "Review Requested".to_string(),
            filters: "reason:review-requested".to_string(),
            items: vec![
                notification_item("thread-1", true),
                notification_item("thread-3", true),
            ],
            total_count: Some(50),
            page: 1,
            page_size: 50,
            refreshed_at: None,
            error: None,
        },
    ];
    let app = AppState::new(SectionKind::Notifications, sections);

    let inbox = app
        .view_tabs()
        .into_iter()
        .find(|view| view.key == builtin_view_key(SectionKind::Notifications))
        .expect("inbox tab");

    assert_eq!(inbox.label, "Inbox (3)");
}

#[test]
fn inbox_top_tab_label_hides_zero_unread_count() {
    let section = SectionSnapshot {
        key: "notifications:all".to_string(),
        kind: SectionKind::Notifications,
        title: "All".to_string(),
        filters: "is:all".to_string(),
        items: vec![notification_item("thread-1", false)],
        total_count: Some(50),
        page: 1,
        page_size: 50,
        refreshed_at: None,
        error: None,
    };
    let app = AppState::new(SectionKind::Notifications, vec![section]);

    let inbox = app
        .view_tabs()
        .into_iter()
        .find(|view| view.key == builtin_view_key(SectionKind::Notifications))
        .expect("inbox tab");

    assert_eq!(inbox.label, "Inbox");
}

#[test]
fn legacy_ansi_text_colors_use_readable_light_theme_palette() {
    let previous = crate::theme::active_theme_name();
    set_active_theme(ThemeName::Light);
    let theme = active_theme();

    assert_eq!(themed_fg_style(Color::White).fg, Some(theme.text));
    assert_eq!(themed_fg_style(Color::Gray).fg, Some(theme.muted));
    assert_eq!(themed_fg_style(Color::DarkGray).fg, Some(theme.subtle));

    set_active_theme(previous);
}

#[test]
fn key_value_labels_are_readable_in_light_theme() {
    let previous = crate::theme::active_theme_name();
    set_active_theme(ThemeName::Light);
    let theme = active_theme();

    let line = key_value_line("repo", "chenyukang/ghr".to_string());

    assert_eq!(line.spans[0].style.fg, Some(theme.muted));
    assert_eq!(line.spans[0].style.bg, Some(theme.surface));

    set_active_theme(previous);
}

#[test]
fn top_tab_highlights_use_high_contrast_blocks() {
    let view = active_view_tab_style();
    assert_eq!(view.fg, Some(Color::Black));
    assert_eq!(view.bg, Some(Color::LightCyan));
    assert!(view.add_modifier.contains(Modifier::BOLD));

    let section = active_section_tab_style();
    assert_eq!(section, view);
    assert_eq!(section.fg, Some(Color::Black));
    assert_eq!(section.bg, Some(Color::LightCyan));
    assert!(section.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn ghr_title_is_bold_when_unread_notifications_exist() {
    let notification_section = SectionSnapshot {
        key: "notifications:all".to_string(),
        kind: SectionKind::Notifications,
        title: "Notifications".to_string(),
        filters: "is:all".to_string(),
        items: vec![notification_item("thread-1", true)],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(
        SectionKind::PullRequests,
        vec![test_section(), notification_section],
    );
    app.focus_list();
    let base = themed_fg_style(Color::DarkGray);

    assert!(app.has_unread_notifications());
    assert!(
        view_tabs_title_style(&app, base)
            .add_modifier
            .contains(Modifier::BOLD)
    );

    app.sections[1].items[0].unread = Some(false);

    assert!(!app.has_unread_notifications());
    assert!(
        !view_tabs_title_style(&app, base)
            .add_modifier
            .contains(Modifier::BOLD)
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
    assert!(rendered.contains("showing 1-19/120 | page 1/3"));
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
    assert!(rendered.contains("showing 51-69/120 | page 2/3"));
}

#[test]
fn section_page_request_uses_total_count_and_configured_page_size() {
    let mut section = many_items_section(100);
    section.total_count = Some(250);
    section.page = 1;
    section.page_size = 100;
    section.filters = "repo:rust-lang/rust is:open archived:false sort:created-desc".to_string();
    let app = AppState::new(SectionKind::PullRequests, vec![section]);
    let request = app
        .section_page_request(1, &Config::default())
        .expect("next page request");

    assert_eq!(request.page, 2);
    assert_eq!(request.page_size, 100);
    assert_eq!(request.total_pages, 3);
    assert_eq!(
        request.filters,
        "repo:rust-lang/rust is:open archived:false sort:created-desc"
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
    assert!(!app.filter_input_active);
    assert_eq!(app.global_search_query, "");
    assert_eq!(
        app.global_search_dialog.as_ref().map(|dialog| dialog.field),
        Some(GlobalSearchField::Title)
    );
    assert_eq!(
        app.global_search_dialog
            .as_ref()
            .map(|dialog| dialog.sort.text()),
        Some("created_at")
    );
    assert_eq!(app.status, "repo search mode in rust-lang/rust");
}

#[test]
fn slash_in_pr_or_issue_list_opens_unified_repo_search_dialog() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('/')),
        &config,
        &store,
        &tx
    ));

    assert!(app.global_search_active);
    assert!(!app.search_active);
    assert!(app.global_search_dialog.is_some());
}

#[test]
fn global_search_enter_submits_query() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_global_search_input();
    let mut submitted = None;

    app.handle_global_search_key_with_submit(key(KeyCode::Char('f')), |query, _| {
        submitted = Some(query);
    });
    app.handle_global_search_key_with_submit(key(KeyCode::Char('i')), |query, _| {
        submitted = Some(query);
    });
    app.handle_global_search_key_with_submit(key(KeyCode::Char('b')), |query, _| {
        submitted = Some(query);
    });
    app.handle_global_search_key_with_submit(key(KeyCode::Enter), |query, _| {
        submitted = Some(query);
    });

    assert_eq!(submitted, Some("fib type:pr".to_string()));
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
fn global_search_dialog_builds_filter_query() {
    let mut dialog = GlobalSearchDialog::default();
    dialog.title.set_text("borrowck");
    dialog.status.set_text("open");
    dialog.label.set_text("T-compiler, good first issue");
    dialog.author.set_text("@alice");
    dialog.assignee.set_text("bob");
    dialog.sort.set_text("updated_at asc");

    assert_eq!(
        global_search_dialog_query(&dialog).expect("query"),
        "borrowck is:open label:T-compiler label:\"good first issue\" author:alice assignee:bob sort:updated-asc"
    );
}

#[test]
fn global_search_dialog_keeps_plain_number_lookup_query() {
    let mut dialog = GlobalSearchDialog::default();
    dialog.title.set_text("#149468");

    assert_eq!(
        global_search_dialog_query(&dialog).expect("query"),
        "149468"
    );
}

#[test]
fn global_search_dropdown_suggestions_fill_active_field() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.label_suggestions_cache.insert(
        "rust-lang/rust".to_string(),
        vec![
            "T-compiler".to_string(),
            "good first issue".to_string(),
            "good second issue".to_string(),
            "S-waiting-on-review".to_string(),
        ],
    );
    app.assignee_suggestions_cache.insert(
        "rust-lang/rust".to_string(),
        vec!["alice".to_string(), "bob".to_string(), "bobby".to_string()],
    );
    app.start_global_search_input();

    {
        let dialog = app.global_search_dialog.as_mut().expect("search dialog");
        dialog.field = GlobalSearchField::Status;
    }
    let mut submitted = None;
    app.handle_global_search_key_with_submit(key(KeyCode::Down), |_, _| {});
    assert_eq!(
        app.global_search_dialog
            .as_ref()
            .map(|dialog| dialog.status.text()),
        Some("")
    );
    app.handle_global_search_key_with_submit(key(KeyCode::Enter), |query, _| {
        submitted = Some(query);
    });
    assert_eq!(submitted, None);
    assert_eq!(
        app.global_search_dialog
            .as_ref()
            .map(|dialog| dialog.status.text()),
        Some("open")
    );
    app.handle_global_search_key_with_submit(key(KeyCode::Down), |_, _| {});
    assert_eq!(
        app.global_search_dialog
            .as_ref()
            .map(|dialog| dialog.status.text()),
        Some("open")
    );
    app.handle_global_search_key_with_submit(key(KeyCode::Enter), |query, _| {
        submitted = Some(query);
    });
    assert_eq!(submitted, None);
    assert_eq!(
        app.global_search_dialog
            .as_ref()
            .map(|dialog| dialog.status.text()),
        Some("closed")
    );

    {
        let dialog = app.global_search_dialog.as_mut().expect("search dialog");
        dialog.field = GlobalSearchField::Label;
        reset_global_search_dialog_suggestions(dialog);
        dialog.label.set_text("good");
        reset_global_search_dialog_suggestions(dialog);
    }
    app.handle_global_search_key_with_submit(key(KeyCode::Down), |_, _| {});
    assert_eq!(
        app.global_search_dialog
            .as_ref()
            .map(|dialog| dialog.label.text()),
        Some("good")
    );
    app.handle_global_search_key_with_submit(key(KeyCode::Down), |_, _| {});
    assert_eq!(
        app.global_search_dialog
            .as_ref()
            .map(|dialog| dialog.label.text()),
        Some("good")
    );
    app.handle_global_search_key_with_submit(key(KeyCode::Enter), |_, _| {});
    assert_eq!(
        app.global_search_dialog
            .as_ref()
            .map(|dialog| dialog.label.text()),
        Some("good second issue")
    );

    {
        let dialog = app.global_search_dialog.as_mut().expect("search dialog");
        dialog.field = GlobalSearchField::Author;
        reset_global_search_dialog_suggestions(dialog);
        dialog.author.set_text("bo");
        reset_global_search_dialog_suggestions(dialog);
    }
    app.handle_global_search_key_with_submit(key(KeyCode::Down), |_, _| {});
    assert_eq!(
        app.global_search_dialog
            .as_ref()
            .map(|dialog| dialog.author.text()),
        Some("bo")
    );
    app.handle_global_search_key_with_submit(key(KeyCode::Down), |_, _| {});
    assert_eq!(
        app.global_search_dialog
            .as_ref()
            .map(|dialog| dialog.author.text()),
        Some("bo")
    );
    app.handle_global_search_key_with_submit(key(KeyCode::Enter), |_, _| {});
    assert_eq!(
        app.global_search_dialog
            .as_ref()
            .map(|dialog| dialog.author.text()),
        Some("bobby")
    );

    {
        let dialog = app.global_search_dialog.as_mut().expect("search dialog");
        dialog.field = GlobalSearchField::Assignee;
        reset_global_search_dialog_suggestions(dialog);
        dialog.assignee_candidates = vec!["chenyukang".to_string()];
        dialog.assignee.set_text("chen");
        reset_global_search_dialog_suggestions(dialog);
    }
    app.handle_global_search_key_with_submit(key(KeyCode::Enter), |query, _| {
        submitted = Some(query);
    });
    assert_eq!(
        app.global_search_dialog
            .as_ref()
            .map(|dialog| dialog.assignee.text()),
        Some("chenyukang")
    );
    assert_eq!(submitted, None);

    app.handle_global_search_key_with_submit(key(KeyCode::Enter), |query, _| {
        submitted = Some(query);
    });
    assert_eq!(
        submitted,
        Some(
            "is:closed label:\"good second issue\" author:bobby assignee:chenyukang type:pr"
                .to_string()
        )
    );
}

#[test]
fn global_search_dynamic_candidates_support_substring_matching() {
    let mut dialog = GlobalSearchDialog {
        labels: vec![
            "T-compiler".to_string(),
            "good first issue".to_string(),
            "S-waiting-on-review".to_string(),
        ],
        author_candidates: vec![
            "chenyukang".to_string(),
            "bjorn3".to_string(),
            "rustbot".to_string(),
        ],
        assignee_candidates: vec![
            "compiler-errors".to_string(),
            "estebank".to_string(),
            "BoxyUwU".to_string(),
        ],
        ..GlobalSearchDialog::default()
    };

    dialog.field = GlobalSearchField::Label;
    dialog.label.set_text("compiler");
    assert_eq!(
        global_search_dialog_suggestion_matches(&dialog),
        vec!["T-compiler".to_string()]
    );

    dialog.field = GlobalSearchField::Author;
    dialog.author.set_text("yuk");
    assert_eq!(
        global_search_dialog_suggestion_matches(&dialog),
        vec!["chenyukang".to_string()]
    );

    dialog.field = GlobalSearchField::Assignee;
    dialog.assignee.set_text("uwu");
    assert_eq!(
        global_search_dialog_suggestion_matches(&dialog),
        vec!["BoxyUwU".to_string()]
    );
}

#[test]
fn global_search_reopens_with_last_repo_conditions() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_global_search_input();
    {
        let dialog = app.global_search_dialog.as_mut().expect("search dialog");
        dialog.title.set_text("borrowck");
        dialog.status.set_text("open");
        dialog.label.set_text("T-compiler");
        dialog.author.set_text("alice");
        dialog.assignee.set_text("bob");
        dialog.sort.set_text("updated_at");
        dialog.field = GlobalSearchField::Assignee;
    }

    let mut submitted = None;
    app.handle_global_search_key_with_submit(key(KeyCode::Enter), |query, _| {
        submitted = Some(query);
    });

    assert_eq!(
        submitted,
        Some(
            "borrowck is:open label:T-compiler author:alice assignee:bob sort:updated-desc type:pr"
                .to_string()
        )
    );
    assert!(!app.global_search_active);

    app.start_global_search_input();
    let dialog = app.global_search_dialog.as_ref().expect("search dialog");
    assert_eq!(dialog.title.text(), "borrowck");
    assert_eq!(dialog.status.text(), "open");
    assert_eq!(dialog.label.text(), "T-compiler");
    assert_eq!(dialog.author.text(), "alice");
    assert_eq!(dialog.assignee.text(), "bob");
    assert_eq!(dialog.sort.text(), "updated_at");
    assert_eq!(dialog.field, GlobalSearchField::Assignee);
}

#[test]
fn global_search_ctrl_u_clears_current_repo_conditions() {
    let mut saved_searches = HashMap::new();
    saved_searches.insert(
        "rust-lang/rust".to_string(),
        GlobalSearchState {
            title: "borrowck".to_string(),
            status: "open".to_string(),
            label: "T-compiler".to_string(),
            author: "alice".to_string(),
            assignee: "bob".to_string(),
            sort: "updated_at".to_string(),
            field: "author".to_string(),
        },
    );
    let mut app = AppState::with_ui_state(
        SectionKind::PullRequests,
        vec![test_section()],
        UiState {
            global_search_by_repo: saved_searches,
            ..UiState::default()
        },
    );

    app.start_global_search_input();
    assert_eq!(
        app.global_search_dialog
            .as_ref()
            .map(|dialog| dialog.author.text()),
        Some("alice")
    );

    let mut submitted = None;
    app.handle_global_search_key_with_submit(ctrl_key(KeyCode::Char('u')), |query, _| {
        submitted = Some(query);
    });

    let dialog = app.global_search_dialog.as_ref().expect("search dialog");
    assert_eq!(dialog.title.text(), "");
    assert_eq!(dialog.status.text(), "");
    assert_eq!(dialog.label.text(), "");
    assert_eq!(dialog.author.text(), "");
    assert_eq!(dialog.assignee.text(), "");
    assert_eq!(dialog.sort.text(), "created_at");
    assert_eq!(dialog.field, GlobalSearchField::Title);
    assert_eq!(submitted, None);
    assert!(!app.global_search_by_repo.contains_key("rust-lang/rust"));

    app.handle_global_search_key_with_submit(key(KeyCode::Esc), |_, _| {});
    app.start_global_search_input();
    let dialog = app.global_search_dialog.as_ref().expect("search dialog");
    assert_eq!(dialog.title.text(), "");
    assert_eq!(dialog.sort.text(), "created_at");
}

#[test]
fn global_search_ctrl_s_prompts_for_named_saved_search_filter() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let mut config = Config::default();
    let paths = unique_test_paths("saved-search-config");
    app.start_global_search_input();
    {
        let dialog = app.global_search_dialog.as_mut().expect("search dialog");
        dialog.status.set_text("open");
        dialog.author.set_text("chenyukang");
    }

    app.handle_global_search_key_with_submit(ctrl_key(KeyCode::Char('s')), |_, _| {});

    assert!(app.save_search_dialog.is_some());
    assert_eq!(app.status, "name saved search filter");
    {
        let dialog = app.save_search_dialog.as_mut().expect("save dialog");
        assert_eq!(dialog.repo, "rust-lang/rust");
        assert_eq!(dialog.kind, SectionKind::PullRequests);
        assert_eq!(dialog.search.status, "open");
        assert_eq!(dialog.search.author, "chenyukang");
        dialog.name.set_text("my rust prs");
    }

    app.handle_save_search_key(key(KeyCode::Enter), &mut config, &paths);

    assert_eq!(
        app.global_search_saved_by_repo
            .get("rust-lang/rust")
            .and_then(|items| items.first())
            .map(|item| (item.name.as_str(), item.repo.as_str(), item.kind.as_str())),
        Some(("my rust prs", "rust-lang/rust", "pull_requests"))
    );
    assert_eq!(
        config.saved_search_filters.first().map(|item| (
            item.name.as_str(),
            item.repo.as_str(),
            item.kind.as_str()
        )),
        Some(("my rust prs", "rust-lang/rust", "pull_requests"))
    );
    let saved = Config::load_or_create(&paths.config_path).expect("load saved config");
    assert_eq!(
        saved.saved_search_filters.first().map(|item| (
            item.name.as_str(),
            item.repo.as_str(),
            item.kind.as_str()
        )),
        Some(("my rust prs", "rust-lang/rust", "pull_requests"))
    );
    assert_eq!(
        app.status,
        format!(
            "saved search filter 'my rust prs' saved to {}",
            paths.config_path.display()
        )
    );
}

#[test]
fn saved_search_candidates_are_run_from_named_filter_dialog() {
    let mut section = test_section();
    section.key = "repo:Rust:pull_requests:Pull Requests".to_string();
    section.title = "Pull Requests".to_string();
    section.filters = "repo:rust-lang/rust is:open archived:false sort:created-desc".to_string();
    let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
    let mut config = Config::default();
    config.saved_search_filters.push(SavedSearchFilterConfig {
        name: "my rust prs".to_string(),
        repo: "rust-lang/rust".to_string(),
        kind: "pull_requests".to_string(),
        status: "open".to_string(),
        author: "chenyukang".to_string(),
        ..SavedSearchFilterConfig::default()
    });

    app.show_saved_search_dialog(&config);
    assert!(app.saved_search_dialog.is_some());
    assert_eq!(
        app.saved_search_candidates_for_query("rust")
            .first()
            .map(|candidate| candidate.name.as_str()),
        Some("my rust prs")
    );

    let candidate = app
        .saved_search_candidates_for_query("my rust")
        .into_iter()
        .next()
        .expect("candidate");
    let target = app
        .saved_search_section_target(&candidate, &Config::default())
        .expect("target");
    assert_eq!(
        target.filters,
        "repo:rust-lang/rust archived:false is:open author:chenyukang sort:created-desc"
    );
    let label = saved_search_active_filter_label(&candidate);
    assert_eq!(
        label,
        "saved: my rust prs (status:open | author:chenyukang)"
    );
    app.section_filter_overrides.insert(
        target.section_key.clone(),
        SectionFilterOverride {
            display: label.clone(),
            filters: target.filters.clone(),
        },
    );
    let section = app.current_section().expect("section");
    assert_eq!(app.effective_filters_for_section(section), target.filters);
    assert_eq!(app.current_filter_label().as_deref(), Some(label.as_str()));
}

#[test]
fn global_search_loaded_candidates_update_active_dialog() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_global_search_input();

    app.handle_msg(AppMsg::LabelSuggestionsLoaded {
        repo: "rust-lang/rust".to_string(),
        result: Ok(vec!["bug".to_string(), "T-compiler".to_string()]),
    });
    app.handle_msg(AppMsg::AssigneeSuggestionsLoaded {
        repo: "rust-lang/rust".to_string(),
        result: Ok(vec!["alice".to_string(), "bob".to_string()]),
    });

    let dialog = app.global_search_dialog.as_ref().expect("search dialog");
    assert_eq!(
        dialog.labels,
        vec!["bug".to_string(), "T-compiler".to_string()]
    );
    assert_eq!(
        dialog.assignee_candidates,
        vec!["alice".to_string(), "bob".to_string()]
    );
    assert!(dialog.author_candidates.contains(&"alice".to_string()));
    assert!(dialog.author_candidates.contains(&"bob".to_string()));
}

#[test]
fn global_search_scope_comes_from_current_item_or_repo_section() {
    let app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    assert_eq!(app.current_repo_scope().as_deref(), Some("rust-lang/rust"));

    let repo_section = SectionSnapshot {
        key: "repo:Fiber:pull_requests:Pull Requests".to_string(),
        kind: SectionKind::PullRequests,
        title: "Pull Requests".to_string(),
        filters: "repo:nervosnetwork/fiber is:open archived:false sort:created-desc".to_string(),
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
fn global_search_started_from_issues_prefers_issue_results() {
    let mut issue_section = SectionSnapshot::empty_for_view(
        builtin_view_key(SectionKind::Issues),
        SectionKind::Issues,
        "Issues",
        "repo:nervosnetwork/fiber is:open archived:false sort:created-desc",
    );
    let mut source_issue = work_item(
        "source-issue",
        "nervosnetwork/fiber",
        911,
        "Atomic MPP design",
        None,
    );
    source_issue.kind = ItemKind::Issue;
    source_issue.url = "https://github.com/nervosnetwork/fiber/issues/911".to_string();
    issue_section.items = vec![source_issue];

    let mut app = AppState::new(SectionKind::Issues, vec![issue_section]);
    app.start_global_search_input();
    assert_eq!(app.global_search_preferred_kind, Some(SectionKind::Issues));

    let mut pr_section = SectionSnapshot::empty_for_view(
        global_search_view_key(),
        SectionKind::PullRequests,
        "Pull Requests",
        "atomic",
    );
    pr_section.items = vec![work_item(
        "pr-827",
        "nervosnetwork/fiber",
        827,
        "Atomic mpp",
        None,
    )];
    let mut result_issue_section = SectionSnapshot::empty_for_view(
        global_search_view_key(),
        SectionKind::Issues,
        "Issues",
        "atomic",
    );
    let mut result_issue = work_item(
        "issue-911",
        "nervosnetwork/fiber",
        911,
        "Atomic MPP design",
        None,
    );
    result_issue.kind = ItemKind::Issue;
    result_issue.url = "https://github.com/nervosnetwork/fiber/issues/911".to_string();
    result_issue_section.items = vec![result_issue];

    app.handle_msg(AppMsg::GlobalSearchFinished {
        query: "atomic".to_string(),
        sections: vec![pr_section, result_issue_section],
    });

    assert_eq!(app.active_view, global_search_view_key());
    assert_eq!(
        app.current_section().map(|section| section.kind),
        Some(SectionKind::Issues)
    );
    assert_eq!(
        app.current_item().map(|item| item.url.as_str()),
        Some("https://github.com/nervosnetwork/fiber/issues/911")
    );
    assert_eq!(app.global_search_preferred_kind, None);
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
fn global_search_finished_selects_first_section_with_results() {
    let mut app = AppState::new(SectionKind::Issues, vec![test_section()]);
    app.global_search_running = true;
    let pr_section = SectionSnapshot::empty_for_view(
        global_search_view_key(),
        SectionKind::PullRequests,
        "Pull Requests",
        "#1 repo:rust-lang/rust",
    );
    let mut issue_section = SectionSnapshot::empty_for_view(
        global_search_view_key(),
        SectionKind::Issues,
        "Issues",
        "#1 repo:rust-lang/rust",
    );
    let mut issue = work_item("rust-1", "rust-lang/rust", 1, "Rust issue", None);
    issue.kind = ItemKind::Issue;
    issue.url = "https://github.com/rust-lang/rust/issues/1".to_string();
    issue_section.items = vec![issue];

    app.handle_msg(AppMsg::GlobalSearchFinished {
        query: "1".to_string(),
        sections: vec![pr_section, issue_section],
    });

    assert_eq!(app.active_view, global_search_view_key());
    assert_eq!(
        app.current_section().map(|section| section.title.as_str()),
        Some("Issues")
    );
    assert_eq!(
        app.current_item().map(|item| item.id.as_str()),
        Some("rust-1")
    );
}

#[test]
fn escape_on_search_results_returns_to_previous_list() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    app.start_global_search_input();
    app.global_search_dialog
        .as_mut()
        .expect("search dialog")
        .title
        .set_text("fiber");
    app.handle_global_search_key_with_submit(key(KeyCode::Enter), |_, _| {});

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
    let created_at = DateTime::parse_from_rfc3339("2026-05-04T01:02:03Z")
        .unwrap()
        .with_timezone(&Utc);
    item.created_at = Some(created_at);
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
    assert!(rendered.contains("status: READY"));
    assert!(rendered.contains("state: open"));
    assert!(rendered.contains(&format!("created: {}", local_datetime(Some(created_at)))));
    assert!(rendered.contains("author: chenyukang"));
    assert!(rendered.contains("comments: 3"));
    assert!(rendered.contains("milestone: -"));
    assert!(rendered.contains("labels: T-compiler×  +"));
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
    assert_document_action_for_text(
        &document,
        "×",
        DetailAction::RemoveLabel("T-compiler".to_string()),
    );
}

#[test]
fn pr_details_meta_places_status_after_number() {
    let app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let rendered = build_details_document(&app, 120)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let identity_line = rendered
        .lines()
        .find(|line| line.contains("repo:") && line.contains("number:"))
        .expect("identity metadata line");

    assert!(identity_line.contains("status: READY"));
    assert!(
        identity_line.find("number:").expect("number key")
            < identity_line.find("status:").expect("status key")
    );
}

#[test]
fn pr_details_meta_renders_ready_status_green() {
    let app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let style = details_status_value_style(&app, "READY");

    assert_eq!(style.fg, Some(Color::LightGreen));
    assert!(style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn pr_details_meta_renders_draft_status_red() {
    let mut section = test_section();
    section.items[0].extra = Some("draft".to_string());
    let app = AppState::new(SectionKind::PullRequests, vec![section]);
    let style = details_status_value_style(&app, "DRAFT");

    assert_eq!(style.fg, Some(Color::LightRed));
    assert!(style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn details_state_renders_open_green_bold() {
    let app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let style = details_state_value_style(&app, "open");

    assert_eq!(style.fg, Some(Color::LightGreen));
    assert!(style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn pr_details_state_renders_merged_blue_bold() {
    let mut section = test_section();
    section.items[0].state = Some("merged".to_string());
    let app = AppState::new(SectionKind::PullRequests, vec![section]);
    let style = details_state_value_style(&app, "merged");

    assert_eq!(style.fg, Some(Color::LightBlue));
    assert!(style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn details_state_renders_closed_red_bold() {
    let mut section = test_section();
    section.items[0].state = Some("closed".to_string());
    let app = AppState::new(SectionKind::PullRequests, vec![section]);
    let style = details_state_value_style(&app, "closed");

    assert_eq!(style.fg, Some(Color::LightRed));
    assert!(style.add_modifier.contains(Modifier::BOLD));
}

fn details_state_value_style(app: &AppState, value: &str) -> Style {
    let document = build_details_document(app, 120);
    let state_line = document
        .lines
        .iter()
        .find(|line| line.to_string().contains(&format!("state: {value}")))
        .expect("state metadata line");
    state_line
        .spans
        .iter()
        .find(|span| span.content.as_ref() == value)
        .expect("state value span")
        .style
}

fn details_status_value_style(app: &AppState, value: &str) -> Style {
    let document = build_details_document(app, 120);
    let status_line = document
        .lines
        .iter()
        .find(|line| line.to_string().contains(&format!("status: {value}")))
        .expect("status metadata line");
    status_line
        .spans
        .iter()
        .find(|span| span.content.as_ref() == value)
        .expect("status value span")
        .style
}

#[test]
fn details_title_and_metadata_spacing_are_ordered() {
    let mut item = work_item(
        "1",
        "chenyukang/ghr",
        1,
        "Aligned metadata title",
        Some("chenyukang"),
    );
    item.assignees = vec!["alice".to_string()];
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
    app.action_hints.insert(
        "1".to_string(),
        ActionHintState::Loaded(ActionHints {
            labels: vec!["Approvable".to_string()],
            checks: None,
            commits: Some(4),
            failed_check_runs: Vec::new(),
            note: Some("Merge blocked: checks pending".to_string()),
            head: Some(PullRequestBranch {
                repository: "chenyukang/ghr".to_string(),
                branch: "feature/checks".to_string(),
            }),
            ..ActionHints::default()
        }),
    );

    let lines = build_details_document(&app, 120)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let title_line = lines
        .iter()
        .position(|line| line == "Aligned metadata title")
        .expect("title line");
    assert_eq!(lines.get(title_line + 1), Some(&String::new()));

    let line_for = |key: &str| {
        let needle = format!("{key}:");
        lines
            .iter()
            .find(|line| line.contains(&needle))
            .unwrap_or_else(|| panic!("missing metadata key {key}: {lines:?}"))
    };
    let key_colon = |line: &str, key: &str| {
        let needle = format!("{key}:");
        line.find(&needle).expect("metadata key") + key.len()
    };

    let repo_line = line_for("repo");
    let state_line = line_for("state");
    assert!(repo_line.starts_with("  "));
    assert_ne!(
        repo_line, state_line,
        "state metadata should start on a separate line from repo/number"
    );
    let colon_column = key_colon(repo_line, "repo");
    for key in [
        "state",
        "author",
        "branch",
        "action",
        "action note",
        "url",
        "assignees",
        "labels",
    ] {
        let line = line_for(key);
        assert!(
            line.starts_with("  "),
            "metadata line should keep left padding: {line:?}"
        );
        assert_eq!(
            key_colon(line, key),
            colon_column,
            "metadata key {key} should align with repo key"
        );
    }
}

#[test]
fn pr_details_meta_shows_milestone_title() {
    let mut item = work_item("1", "chenyukang/ghr", 1, "More on tui", Some("chenyukang"));
    item.milestone = Some(Milestone {
        number: 9,
        title: "next-release".to_string(),
    });
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
    let rendered = build_details_document(&app, 120)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("milestone: next-release"));
}

#[test]
fn inbox_details_mark_new_since_last_read_updates() {
    let last_read = DateTime::parse_from_rfc3339("2026-05-05T01:00:00Z")
        .expect("last read timestamp")
        .with_timezone(&Utc);
    let updated = DateTime::parse_from_rfc3339("2026-05-05T02:00:00Z")
        .expect("updated timestamp")
        .with_timezone(&Utc);
    let mut item = notification_item("thread-1", true);
    item.body = Some("Notification body".to_string());
    item.updated_at = Some(updated);
    item.last_read_at = Some(last_read);
    let section = SectionSnapshot {
        key: "notifications:all".to_string(),
        kind: SectionKind::Notifications,
        title: "All".to_string(),
        filters: "is:all".to_string(),
        items: vec![item],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(SectionKind::Notifications, vec![section]);
    app.details.insert(
        "thread-1".to_string(),
        DetailState::Loaded(vec![CommentPreview {
            id: None,
            kind: CommentPreviewKind::Comment,
            author: "alice".to_string(),
            body: "new comment".to_string(),
            created_at: Some(updated),
            updated_at: None,
            url: None,
            parent_id: None,
            is_mine: false,
            viewer_can_update: None,
            reactions: ReactionSummary::default(),
            review: None,
        }]),
    );

    let rendered = build_details_document(&app, 120)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("New since last read"));
    assert!(rendered.contains("last read"));
}

#[test]
fn comments_loaded_updates_details_milestone_metadata() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);

    app.handle_msg(AppMsg::CommentsLoaded {
        item_id: "1".to_string(),
        comments: Ok(CommentFetchResult {
            item_metadata: None,
            item_reactions: Some(ReactionSummary::default()),
            item_milestone: Some(Some(Milestone {
                number: 9,
                title: "next-release".to_string(),
            })),
            comments: Vec::new(),
        }),
    });

    let rendered = build_details_document(&app, 120)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("milestone: next-release"));
}

#[test]
fn comments_loaded_preserves_item_reactions_and_milestone_when_details_are_unknown() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let cached_reactions = ReactionSummary {
        eyes: 2,
        ..ReactionSummary::default()
    };
    let cached_milestone = Milestone {
        number: 7,
        title: "cached-release".to_string(),
    };
    app.sections[0].items[0].reactions = cached_reactions.clone();
    app.sections[0].items[0].milestone = Some(cached_milestone.clone());

    app.handle_msg(AppMsg::CommentsLoaded {
        item_id: "1".to_string(),
        comments: Ok(CommentFetchResult {
            item_metadata: None,
            item_reactions: None,
            item_milestone: None,
            comments: Vec::new(),
        }),
    });

    let item = &app.sections[0].items[0];
    assert_eq!(item.reactions, cached_reactions);
    assert_eq!(item.milestone, Some(cached_milestone));
}

#[test]
fn comments_loaded_updates_inbox_notification_description_from_lazy_metadata() {
    let section = SectionSnapshot {
        key: "notifications:all".to_string(),
        kind: SectionKind::Notifications,
        title: "All".to_string(),
        filters: "is:all".to_string(),
        items: vec![notification_item("thread-1", true)],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(SectionKind::Notifications, vec![section]);

    assert_eq!(app.current_item().and_then(|item| item.body.as_ref()), None);

    app.handle_msg(AppMsg::CommentsLoaded {
        item_id: "thread-1".to_string(),
        comments: Ok(CommentFetchResult {
            item_metadata: Some(ItemDetailsMetadata {
                title: Some("Loaded notification PR".to_string()),
                body: Some("Loaded from the linked pull request.".to_string()),
                author: Some("rustbot".to_string()),
                state: Some("open".to_string()),
                url: Some("https://github.com/rust-lang/rust/pull/1".to_string()),
                created_at: None,
                updated_at: None,
                labels: Some(vec!["T-compiler".to_string()]),
                assignees: Some(vec!["alice".to_string()]),
                comments: Some(3),
                viewer_subscription: None,
            }),
            item_reactions: Some(ReactionSummary::default()),
            item_milestone: Some(None),
            comments: Vec::new(),
        }),
    });

    let item = app.current_item().expect("current item");
    assert_eq!(item.title, "Loaded notification PR");
    assert_eq!(
        item.body.as_deref(),
        Some("Loaded from the linked pull request.")
    );
    assert_eq!(item.reason.as_deref(), Some("mention"));
    assert_eq!(item.unread, Some(true));
    assert_eq!(item.comments, Some(3));

    let rendered = build_details_document(&app, 120)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("Loaded from the linked pull request."));
    assert!(!rendered.contains("No description."));
}

#[test]
fn issue_or_pr_description_renders_without_preview_truncation() {
    let body = format!(
        "{}\n\n{}final tail marker",
        (1..=31)
            .map(|index| format!("description line {index:02}"))
            .collect::<Vec<_>>()
            .join("\n"),
        "word ".repeat(520)
    );
    let mut item = notification_item("thread-1", true);
    item.body = Some(body.clone());
    let inbox_section = SectionSnapshot {
        key: "notifications:all".to_string(),
        kind: SectionKind::Notifications,
        title: "All".to_string(),
        filters: "is:all".to_string(),
        items: vec![item],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let inbox_app = AppState::new(SectionKind::Notifications, vec![inbox_section]);

    let inbox_rendered = build_details_document(&inbox_app, 120)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert!(
        inbox_rendered
            .iter()
            .any(|line| line.contains("description line 31")),
        "inbox linked issue/PR descriptions should render fully: {inbox_rendered:?}"
    );
    assert!(
        inbox_rendered
            .iter()
            .any(|line| line.contains("final tail marker")),
        "inbox linked issue/PR descriptions should render past the old character preview limit: {inbox_rendered:?}"
    );
    assert!(
        !inbox_rendered.iter().any(|line| line.trim() == "..."),
        "inbox linked issue/PR descriptions should not show preview ellipsis: {inbox_rendered:?}"
    );

    let mut normal_section = test_section();
    normal_section.items[0].body = Some(body);
    let normal_app = AppState::new(SectionKind::PullRequests, vec![normal_section]);
    let normal_rendered = build_details_document(&normal_app, 120)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert!(
        normal_rendered
            .iter()
            .any(|line| line.contains("description line 31")),
        "issue/PR descriptions should render fully outside inbox too: {normal_rendered:?}"
    );
    assert!(
        normal_rendered
            .iter()
            .any(|line| line.contains("final tail marker")),
        "issue/PR descriptions should render past the old character preview limit outside inbox too: {normal_rendered:?}"
    );
    assert!(
        !normal_rendered.iter().any(|line| line.trim() == "..."),
        "issue/PR descriptions should not show preview ellipsis: {normal_rendered:?}"
    );
}

#[test]
fn inbox_refresh_keeps_lazy_description_visible_while_details_reload() {
    let mut item = notification_item("thread-1", true);
    item.body = Some("Loaded from the linked pull request.".to_string());
    item.author = Some("rustbot".to_string());
    item.labels = vec!["T-compiler".to_string()];
    let section = SectionSnapshot {
        key: "notifications:all".to_string(),
        kind: SectionKind::Notifications,
        title: "All".to_string(),
        filters: "is:all".to_string(),
        items: vec![item],
        total_count: None,
        page: 1,
        page_size: 50,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(SectionKind::Notifications, vec![section]);
    app.details.insert(
        "thread-1".to_string(),
        DetailState::Loaded(vec![comment("alice", "cached comment", None)]),
    );

    let refreshed_section = SectionSnapshot {
        key: "notifications:all".to_string(),
        kind: SectionKind::Notifications,
        title: "All".to_string(),
        filters: "is:all".to_string(),
        items: vec![notification_item("thread-1", false)],
        total_count: None,
        page: 1,
        page_size: 50,
        refreshed_at: None,
        error: None,
    };
    app.handle_msg(AppMsg::RefreshFinished {
        sections: vec![refreshed_section],
        save_error: None,
    });

    let item = app.current_item().expect("current item");
    assert_eq!(
        item.body.as_deref(),
        Some("Loaded from the linked pull request.")
    );
    assert_eq!(item.author.as_deref(), Some("rustbot"));
    assert_eq!(item.labels, vec!["T-compiler".to_string()]);
    assert_eq!(item.unread, Some(false));
    assert!(app.details_stale.contains("thread-1"));
    assert!(
        matches!(
            app.details.get("thread-1"),
            Some(DetailState::Loaded(comments)) if comments[0].body == "cached comment"
        ),
        "refresh should keep old details visible while the async reload runs"
    );

    let rendered = build_details_document(&app, 120)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("Loaded from the linked pull request."));
    assert!(!rendered.contains("No description."));
}

#[test]
fn issue_details_meta_links_author() {
    let mut item = work_item("issue-1", "chenyukang/ghr", 1, "Bug report", Some("alice"));
    item.kind = ItemKind::Issue;
    item.url = "https://github.com/chenyukang/ghr/issues/1".to_string();
    let section = SectionSnapshot {
        key: "issues:test".to_string(),
        kind: SectionKind::Issues,
        title: "Issues".to_string(),
        filters: String::new(),
        items: vec![item],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let app = AppState::new(SectionKind::Issues, vec![section]);
    let document = build_details_document(&app, 120);

    assert_document_link_for_text(&document, "alice", "https://github.com/alice");
    assert_document_action_for_text_on_line(&document, "labels:", "+", DetailAction::AddLabel);
}

#[test]
fn issue_details_show_empty_labels_as_actionable() {
    let mut item = work_item("issue-1", "chenyukang/ghr", 1, "Bug report", Some("alice"));
    item.kind = ItemKind::Issue;
    item.url = "https://github.com/chenyukang/ghr/issues/1".to_string();
    item.labels.clear();
    let section = SectionSnapshot {
        key: "issues:test".to_string(),
        kind: SectionKind::Issues,
        title: "Issues".to_string(),
        filters: String::new(),
        items: vec![item],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let app = AppState::new(SectionKind::Issues, vec![section]);
    let document = build_details_document(&app, 120);
    let rendered = document
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("labels:  +"));
    assert!(!rendered.contains("labels: none"));
    assert!(rendered.contains("subscription: subscribe"));
    assert!(rendered.contains("  reactions:  + react"));
    assert!(!rendered.contains("reactions: none"));
    assert_document_action_for_text_on_line(&document, "labels:", "+", DetailAction::AddLabel);
    assert_document_action_for_text_on_line(
        &document,
        "subscription:",
        "subscribe",
        DetailAction::SubscribeItem,
    );
    assert!(
        !document
            .actions
            .iter()
            .any(|action| action.action == DetailAction::UnsubscribeItem)
    );
    assert_document_action_for_text_on_line(
        &document,
        "reactions:",
        "+ react",
        DetailAction::ReactItem,
    );
}

#[test]
fn issue_details_show_unsubscribe_for_subscribed_item() {
    let mut item = work_item("issue-1", "chenyukang/ghr", 1, "Bug report", Some("alice"));
    item.kind = ItemKind::Issue;
    item.viewer_subscription = Some("SUBSCRIBED".to_string());
    item.url = "https://github.com/chenyukang/ghr/issues/1".to_string();
    let section = SectionSnapshot {
        key: "issues:test".to_string(),
        kind: SectionKind::Issues,
        title: "Issues".to_string(),
        filters: String::new(),
        items: vec![item],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let app = AppState::new(SectionKind::Issues, vec![section]);
    let document = build_details_document(&app, 120);
    let rendered = document
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("subscription: unsubscribe"));
    assert_document_action_for_text_on_line(
        &document,
        "subscription:",
        "unsubscribe",
        DetailAction::UnsubscribeItem,
    );
    assert!(
        !document
            .actions
            .iter()
            .any(|action| action.action == DetailAction::SubscribeItem)
    );
}

#[test]
fn item_subscription_success_updates_visible_action() {
    let mut item = work_item("issue-1", "chenyukang/ghr", 1, "Bug report", Some("alice"));
    item.kind = ItemKind::Issue;
    let section = SectionSnapshot {
        key: "issues:test".to_string(),
        kind: SectionKind::Issues,
        title: "Issues".to_string(),
        filters: String::new(),
        items: vec![item],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(SectionKind::Issues, vec![section]);

    app.handle_msg(AppMsg::ItemSubscriptionUpdated {
        item_id: "issue-1".to_string(),
        item_kind: ItemKind::Issue,
        action: ItemSubscriptionAction::Subscribe,
        result: Ok(()),
    });

    assert_eq!(
        app.current_item()
            .and_then(|item| item.viewer_subscription.as_deref()),
        Some("SUBSCRIBED")
    );
    let document = build_details_document(&app, 120);
    assert_document_action_for_text_on_line(
        &document,
        "subscription:",
        "unsubscribe",
        DetailAction::UnsubscribeItem,
    );
}

#[test]
fn notification_details_hide_unavailable_item_reaction_action() {
    let mut item = work_item(
        "release-1",
        "chenyukang/ghr",
        0,
        "ghr v0.6.0",
        Some("chenyukang"),
    );
    item.kind = ItemKind::Notification;
    item.number = None;
    item.url = "https://github.com/chenyukang/ghr".to_string();
    item.labels.clear();
    item.state = None;
    item.body = None;
    item.extra = Some("Release".to_string());
    let section = SectionSnapshot {
        key: "notifications:all".to_string(),
        kind: SectionKind::Notifications,
        title: "All".to_string(),
        filters: String::new(),
        items: vec![item],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let app = AppState::new(SectionKind::Notifications, vec![section]);
    let document = build_details_document(&app, 120);
    let rendered = document
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!rendered.contains("+ react"));
    assert!(!rendered.contains("reactions:"));
    assert!(!rendered.contains("subscription:"));
    assert!(
        !document
            .actions
            .iter()
            .any(|action| matches!(&action.action, DetailAction::ReactItem))
    );
}

#[test]
fn notification_details_show_reaction_counts_without_unavailable_action() {
    let mut item = work_item(
        "release-1",
        "chenyukang/ghr",
        0,
        "ghr v0.6.0",
        Some("chenyukang"),
    );
    item.kind = ItemKind::Notification;
    item.number = None;
    item.url = "https://github.com/chenyukang/ghr".to_string();
    item.labels.clear();
    item.state = None;
    item.body = None;
    item.extra = Some("Release".to_string());
    item.reactions.eyes = 1;
    let section = SectionSnapshot {
        key: "notifications:all".to_string(),
        kind: SectionKind::Notifications,
        title: "All".to_string(),
        filters: String::new(),
        items: vec![item],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let app = AppState::new(SectionKind::Notifications, vec![section]);
    let document = build_details_document(&app, 120);
    let rendered = document
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("reactions: 👀 1"));
    assert!(!rendered.contains("+ react"));
    assert!(
        !document
            .actions
            .iter()
            .any(|action| matches!(&action.action, DetailAction::ReactItem))
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
            commits: Some(4),
            failed_check_runs: Vec::new(),
            note: Some("Merge blocked: checks pending".to_string()),
            head: Some(PullRequestBranch {
                repository: "chenyukang/ghr".to_string(),
                branch: "feature/checks".to_string(),
            }),
            ..ActionHints::default()
        }),
    );

    let document = build_details_document(&app, 120);
    let lines = document
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let rendered = lines.join("\n");

    assert!(rendered.contains("action: Approvable, Mergeable"));
    assert!(rendered.contains("checks: 10 pass, 2 fail, 1 pending"));
    assert!(rendered.contains("commits: 4"));
    assert_document_link_for_text(
        &document,
        "4",
        "https://github.com/chenyukang/ghr/pull/1/commits",
    );
    assert!(rendered.contains("branch: chenyukang/ghr:feature/checks"));
    assert_document_link_for_text(
        &document,
        "chenyukang/ghr:feature/checks",
        "https://github.com/chenyukang/ghr/tree/feature/checks",
    );
    assert!(rendered.contains("action note: Merge blocked: checks pending"));
    let branch_line = lines
        .iter()
        .position(|line| line.contains("branch: chenyukang/ghr:feature/checks"))
        .expect("branch line");
    let commits_line = lines
        .iter()
        .position(|line| line.contains("commits: 4"))
        .expect("commits line");
    assert!(
        branch_line > commits_line,
        "branch should render on its own line after compact metadata"
    );
    assert!(
        !lines[branch_line].contains("commits:"),
        "branch line should not share horizontal space with compact metadata"
    );
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
fn pr_details_render_merge_queue_and_review_summary() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.action_hints.insert(
        "1".to_string(),
        ActionHintState::Loaded(ActionHints {
            labels: vec!["In merge queue".to_string()],
            checks: Some(CheckSummary {
                passed: 43,
                failed: 0,
                pending: 0,
                skipped: 2,
                total: 45,
                incomplete: false,
            }),
            queue: Some(Box::new(crate::model::MergeQueueInfo {
                state: "AWAITING_CHECKS".to_string(),
                position: Some(1),
                enqueued_at: None,
                estimated_time_to_merge: None,
                url: Some("https://github.com/nervosnetwork/ckb/queue/develop".to_string()),
            })),
            reviews: Some(Box::new(crate::model::PullRequestReviewSummary {
                decision: Some("APPROVED".to_string()),
                approved: 1,
                changes_requested: 0,
                pending: 1,
                latest_reviews: vec![crate::model::PullRequestReviewActorState {
                    actor: PullRequestReviewActor {
                        label: "eval-exec".to_string(),
                        url: Some("https://github.com/eval-exec".to_string()),
                    },
                    state: "APPROVED".to_string(),
                }],
                pending_reviewers: vec![PullRequestReviewActor {
                    label: "zhangsoledad".to_string(),
                    url: Some("https://github.com/zhangsoledad".to_string()),
                }],
            })),
            ..ActionHints::default()
        }),
    );

    let document = build_details_document(&app, 120);
    let lines = document
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let rendered = lines.join("\n");

    assert!(rendered.contains("queue: #1 awaiting checks"));
    assert!(rendered.contains("action: In merge queue"));
    assert!(rendered.contains("checks: 43 pass, 0 fail, 2 skipped"));
    assert!(rendered.contains("reviews: approved 1, pending 1"));
    assert!(rendered.contains("reviewers: eval-exec approved, zhangsoledad pending"));

    let queue_line = lines
        .iter()
        .position(|line| line.contains("queue: #1 awaiting checks"))
        .expect("queue line");
    let queue_column =
        display_width(&lines[queue_line][..lines[queue_line].find("#1").expect("queue link")])
            as u16;
    assert_eq!(
        document.link_at(queue_line, queue_column).as_deref(),
        Some("https://github.com/nervosnetwork/ckb/queue/develop")
    );
    assert_document_link_for_text(&document, "eval-exec", "https://github.com/eval-exec");
    assert_document_link_for_text(&document, "zhangsoledad", "https://github.com/zhangsoledad");
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
fn positive_action_labels_are_rendered_green_bold() {
    let segments = action_label_segments(&[
        "Approvable".to_string(),
        "Mergeable".to_string(),
        "Auto-mergeable".to_string(),
        "Auto-merge on".to_string(),
        "In merge queue".to_string(),
        "Update branch".to_string(),
    ]);

    for label in [
        "Approvable",
        "Mergeable",
        "Auto-mergeable",
        "Auto-merge on",
        "In merge queue",
    ] {
        let segment = segments
            .iter()
            .find(|segment| segment.text == label)
            .unwrap_or_else(|| panic!("{label} segment"));
        assert_eq!(segment.style.fg, Some(Color::LightGreen));
        assert!(segment.style.add_modifier.contains(Modifier::BOLD));
    }

    let update_branch = segments
        .iter()
        .find(|segment| segment.text == "Update branch")
        .expect("update branch segment");
    assert_eq!(update_branch.style, Style::default());
}

#[test]
fn merge_conflict_action_note_is_rendered_red() {
    let segments = action_note_segments("Merge blocked: draft; merge conflicts must be resolved");

    let merge_blocked = segments
        .iter()
        .find(|segment| segment.text == "Merge blocked")
        .expect("merge blocked segment");
    assert_eq!(merge_blocked.style.fg, Some(Color::Yellow));
    assert!(merge_blocked.style.add_modifier.contains(Modifier::BOLD));

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
fn details_markdown_preserves_soft_line_breaks() {
    let mut builder = DetailsBuilder::new(120);
    builder.push_markdown_block_indented(
            "closes: https://github.com/rust-lang/rust/issues/143131\ncloses: https://github.com/rust-lang/rust/issues/155446\n\nWe don't need bounds.",
            "empty",
            usize::MAX,
            usize::MAX,
            0,
            0,
        );
    let rendered = builder
        .finish()
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(
        rendered[0],
        "closes: https://github.com/rust-lang/rust/issues/143131"
    );
    assert_eq!(
        rendered[1],
        "closes: https://github.com/rust-lang/rust/issues/155446"
    );
    assert!(rendered[2].is_empty());
    assert_eq!(rendered[3], "We don't need bounds.");
}

#[test]
fn markdown_tables_render_as_separated_rows() {
    let mut builder = DetailsBuilder::new(100);
    builder.push_markdown_block_indented(
            "| x | not const | const |\n| --- | ---- | --- |\n| not comptime | fn | const fn |\n| comptime | comptime fn | ??? |",
            "empty",
            usize::MAX,
            usize::MAX,
            0,
            0,
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
        Some(&"  A body with useful context".to_string())
    );

    let comments_index = rendered
        .iter()
        .position(|line| line == "Comments")
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
fn description_body_reserves_left_and_right_padding() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.sections[0].items[0].body =
        Some("I had a similar experience with you, the learning curve".to_string());

    let width = 31;
    let rendered = build_details_document(&app, width)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    let description_index = rendered
        .iter()
        .position(|line| line == "Description")
        .expect("description heading");
    let body_lines = &rendered[description_index + 2..description_index + 5];

    assert_eq!(
        body_lines,
        &[
            "  I had a similar experience".to_string(),
            "  with you, the learning".to_string(),
            "  curve".to_string()
        ]
    );
    assert!(
        body_lines
            .iter()
            .all(|line| display_width(line) <= usize::from(width - 2)),
        "description body should reserve right padding: {body_lines:?}"
    );
}

#[test]
fn selected_description_has_distinct_selection_style() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.focus_details();
    app.selected_comment_index = NO_SELECTED_COMMENT_INDEX;

    let document = build_details_document(&app, 100);
    let rendered = document
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let description_index = rendered
        .iter()
        .position(|line| line.contains("Description"))
        .expect("description heading");

    assert!(
        rendered[description_index].starts_with("┃ Description"),
        "selected description should have a left rail: {rendered:?}"
    );
    assert!(
        rendered
            .get(description_index.saturating_sub(1))
            .is_some_and(|line| line.starts_with('┏')),
        "selected description should have a top edge: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.starts_with("┃ A body with useful context")),
        "selected description body should keep the rail: {rendered:?}"
    );
    assert_ne!(
        description_selected_rail_style().fg,
        comment_selected_rail_style().fg,
        "description selection should use a different color from comment selection"
    );

    let mut issue_section = test_section();
    issue_section.kind = SectionKind::Issues;
    issue_section.key = "issues:test".to_string();
    issue_section.items[0].kind = ItemKind::Issue;
    issue_section.items[0].url = "https://github.com/rust-lang/rust/issues/1".to_string();
    let mut issue_app = AppState::new(SectionKind::Issues, vec![issue_section]);
    issue_app.focus_details();
    issue_app.selected_comment_index = NO_SELECTED_COMMENT_INDEX;
    let issue_rendered = build_details_document(&issue_app, 100)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert!(
        issue_rendered
            .iter()
            .any(|line| line.starts_with("┃ Description")),
        "issue description should use the same selected details-body treatment: {issue_rendered:?}"
    );
}

#[test]
fn mouse_clicking_description_selects_details_body_without_jumping_scroll() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.focus_details();
    app.details.insert(
        "1".to_string(),
        DetailState::Loaded(vec![comment("alice", "First comment", None)]),
    );
    app.selected_comment_index = 0;
    app.details_scroll = 1;
    let area = Rect::new(0, 0, 120, 40);
    let details = details_area_for(&app, area);
    let inner = block_inner(details);
    let document = build_details_document(&app, inner.width);
    let description_line = document
        .lines
        .iter()
        .position(|line| line.to_string().contains("A body with useful context"))
        .expect("description body line");
    assert!(document.description_at(description_line));
    let description_row = inner.y + description_line as u16 - app.details_scroll;

    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: inner.x + 2,
            row: description_row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
        area,
    );

    assert_eq!(app.focus, FocusTarget::Details);
    assert_eq!(app.selected_comment_index, NO_SELECTED_COMMENT_INDEX);
    assert_eq!(app.details_scroll, 1);
    assert_eq!(app.status, "pull request details focused");
}

#[test]
fn mouse_clicking_inbox_linked_description_selects_details_body() {
    let mut item = notification_item("thread-1", true);
    item.body = Some("Loaded notification description.".to_string());
    let section = SectionSnapshot {
        key: "notifications:all".to_string(),
        kind: SectionKind::Notifications,
        title: "All".to_string(),
        filters: "is:all".to_string(),
        items: vec![item],
        total_count: None,
        page: 1,
        page_size: 50,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(SectionKind::Notifications, vec![section]);
    app.focus_details();
    app.details.insert(
        "thread-1".to_string(),
        DetailState::Loaded(vec![comment("alice", "First comment", None)]),
    );
    app.selected_comment_index = 0;
    let area = Rect::new(0, 0, 120, 40);
    let details = details_area_for(&app, area);
    let inner = block_inner(details);
    let document = build_details_document(&app, inner.width);
    let description_line = document
        .lines
        .iter()
        .position(|line| {
            line.to_string()
                .contains("Loaded notification description.")
        })
        .expect("notification description line");
    assert!(document.description_at(description_line));
    let description_row = inner.y + description_line as u16 - app.details_scroll;

    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: inner.x + 2,
            row: description_row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
        area,
    );

    assert_eq!(app.focus, FocusTarget::Details);
    assert_eq!(app.selected_comment_index, NO_SELECTED_COMMENT_INDEX);
    assert_eq!(app.status, "pull request details focused");
}

#[test]
fn mouse_dragging_details_text_copies_selection_without_text_selection_mode() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.sections[0].items[0].body = Some("alpha beta gamma\nsecond line".to_string());
    app.focus_details();
    app.selected_comment_index = NO_SELECTED_COMMENT_INDEX;
    let area = Rect::new(0, 0, 120, 40);
    let details = details_area_for(&app, area);
    let inner = block_inner(details);
    let document = build_details_document(&app, inner.width);
    let line_index = document
        .lines
        .iter()
        .position(|line| line.to_string().contains("alpha beta gamma"))
        .expect("description line");
    let rendered = document.lines[line_index].to_string();
    let start_column = display_width(&rendered[..rendered.find("alpha").expect("alpha")]);
    let end_column =
        display_width(&rendered[..rendered.find("gamma").expect("gamma") + "gamma".len()]);
    let row = inner.y + line_index as u16 - app.details_scroll;

    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: inner.x + start_column as u16,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
        area,
    );
    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: inner.x + end_column as u16,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
        area,
    );

    assert!(app.mouse_capture_enabled);
    assert_eq!(app.status, "selecting details text; release to copy");
    let selection = app
        .details_text_selection
        .clone()
        .expect("active details text selection");
    assert_eq!(
        selected_details_text(&document, &selection),
        "alpha beta gamma"
    );

    let mut highlighted = build_details_document(&app, inner.width);
    apply_details_text_selection(&app, &mut highlighted);
    let selected_line = &highlighted.lines[line_index];
    assert!(
        selected_line
            .spans
            .iter()
            .any(|span| span.style.bg == Some(Color::LightCyan)
                && span.content.contains("alpha beta gamma")),
        "dragged text should be highlighted: {selected_line:?}"
    );

    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: inner.x + end_column as u16,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
        area,
    );

    assert!(app.mouse_capture_enabled);
    assert!(app.details_text_drag.is_none());
    assert_eq!(app.status, "copied selected details text");
    assert_eq!(
        selected_details_text(
            &build_details_document(&app, inner.width),
            app.details_text_selection
                .as_ref()
                .expect("selection remains visible after copy")
        ),
        "alpha beta gamma"
    );
}

#[test]
fn selected_details_text_strips_selection_rails_from_copy() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.sections[0].items[0].body =
        Some("trait Trait2: Sized {}\n\nimpl Trait2 for () {}".to_string());
    app.focus_details();
    app.selected_comment_index = NO_SELECTED_COMMENT_INDEX;

    let area = Rect::new(0, 0, 120, 40);
    let details = details_area_for(&app, area);
    let inner = block_inner(details);
    let document = build_details_document(&app, inner.width);
    let line_index = document
        .lines
        .iter()
        .position(|line| line.to_string().contains("trait Trait2"))
        .expect("description code line");
    let rendered = document.lines[line_index].to_string();
    assert!(
        rendered.trim_start().starts_with('┃'),
        "focused description should render a selection rail: {rendered:?}"
    );
    let selection = DetailsTextSelection {
        item_id: app.current_item().expect("current item").id.clone(),
        start: DetailsTextPosition {
            line: line_index,
            column: 0,
        },
        end: DetailsTextPosition {
            line: line_index,
            column: display_width(&rendered) as u16,
        },
    };

    assert_eq!(
        selected_details_text(&document, &selection),
        "trait Trait2: Sized {}"
    );

    app.details_text_selection = Some(selection);
    let mut highlighted = document.clone();
    apply_details_text_selection(&app, &mut highlighted);
    let highlighted_selection = highlighted.lines[line_index]
        .spans
        .iter()
        .filter(|span| span.style.bg == Some(Color::LightCyan))
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(
        !highlighted_selection.contains('┃'),
        "selection highlight should skip copy-excluded rails: {highlighted_selection:?}"
    );
    assert!(
        highlighted_selection.contains("trait Trait2"),
        "selection highlight should keep real content: {highlighted_selection:?}"
    );
}

#[test]
fn selected_details_text_uses_segment_copy_metadata() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let item_id = app.current_item().expect("current item").id.clone();
    let mut parent = comment("alice", "Parent comment", None);
    parent.id = Some(1);
    let mut child = comment("bob", "Thread reply with read | write", None);
    child.id = Some(2);
    child.parent_id = Some(1);
    let review = crate::model::ReviewCommentPreview {
        path: "src/lib.rs".to_string(),
        line: Some(1),
        original_line: None,
        start_line: None,
        original_start_line: None,
        side: Some("RIGHT".to_string()),
        start_side: None,
        diff_hunk: None,
        is_resolved: false,
        is_outdated: false,
    };
    parent.review = Some(review.clone());
    child.review = Some(review);
    app.details
        .insert(item_id.clone(), DetailState::Loaded(vec![parent, child]));
    app.focus_details();
    app.selected_comment_index = NO_SELECTED_COMMENT_INDEX;

    let document = build_details_document(&app, 120);
    let line_index = document
        .lines
        .iter()
        .position(|line| line.to_string().contains("Thread reply"))
        .expect("thread reply line");
    let rendered = document.lines[line_index].to_string();
    assert!(
        rendered.contains('↳'),
        "nested comment should render a thread marker: {rendered:?}"
    );
    let selection = DetailsTextSelection {
        item_id,
        start: DetailsTextPosition {
            line: line_index,
            column: 0,
        },
        end: DetailsTextPosition {
            line: line_index,
            column: display_width(&rendered) as u16,
        },
    };

    assert_eq!(
        selected_details_text(&document, &selection),
        "Thread reply with read | write"
    );
}

#[test]
fn selected_details_text_preserves_blank_comment_body_lines() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let item_id = app.current_item().expect("current item").id.clone();
    let body = "rustbot has assigned @fmease.\n\
They will have a look at your PR within the next two weeks and either review your PR or reassign to another reviewer.\n\
\n\
Use r? to explicitly pick a reviewer\n\
\n\
The reviewer was selected based on:\n\
\n\
- Owners of files modified in this PR: compiler\n\
- compiler expanded to 69 candidates\n\
- Random selection from 11 candidates";
    app.details.insert(
        item_id.clone(),
        DetailState::Loaded(vec![comment("rustbot", body, None)]),
    );
    app.focus_details();
    app.selected_comment_index = 0;

    let document = build_details_document(&app, 180);
    let start_line = document
        .lines
        .iter()
        .position(|line| line.to_string().contains("rustbot has assigned"))
        .expect("first body line");
    let end_line = document
        .lines
        .iter()
        .position(|line| line.to_string().contains("Random selection"))
        .expect("last body line");
    let end_column = display_width(&document.lines[end_line].to_string()) as u16;
    let selection = DetailsTextSelection {
        item_id,
        start: DetailsTextPosition {
            line: start_line,
            column: 0,
        },
        end: DetailsTextPosition {
            line: end_line,
            column: end_column,
        },
    };

    let copied = selected_details_text(&document, &selection);
    assert!(
        copied.contains("reviewer.\n\nUse r?"),
        "blank line before reviewer command should be preserved: {copied:?}"
    );
    assert!(
        copied.contains("reviewer\n\nThe reviewer"),
        "blank line before reviewer explanation should be preserved: {copied:?}"
    );
    assert!(
        copied.contains("based on:\n\n- Owners"),
        "blank line before list should be preserved: {copied:?}"
    );
    assert!(
        !copied.contains('┃'),
        "selection rails should still be excluded from copied text: {copied:?}"
    );
}

#[test]
fn markdown_blockquotes_render_with_quote_marker_on_wrapped_lines() {
    let mut builder = DetailsBuilder::new(12);
    builder.push_markdown_block_indented(
        "> quoted reply with enough text to wrap\n\nnormal reply",
        "empty",
        usize::MAX,
        usize::MAX,
        0,
        0,
    );
    let document = builder.finish();
    let rendered = rendered_lines_without_block_copy(&document);

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
        quote_lines.iter().all(|line| line.starts_with("┃ ")),
        "each wrapped quote line should keep the quote marker: {quote_lines:?}"
    );
    let quoted_text = document
        .lines
        .iter()
        .take(quote_lines.len())
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.contains("quote"))
        .expect("quoted text span");
    assert_eq!(
        quoted_text.style.fg,
        Some(crate::theme::Theme::from_name(ThemeName::Dark).quote)
    );
    assert_eq!(
        quoted_text.style.bg,
        Some(crate::theme::Theme::from_name(ThemeName::Dark).quote_bg)
    );
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
        document.lines[normal_index].spans.iter().all(
            |span| span.style.fg != Some(crate::theme::Theme::from_name(ThemeName::Dark).quote)
        ),
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
    let document = builder.finish();
    let rendered = rendered_lines_without_block_copy(&document);

    assert_eq!(rendered[0], "  Feature gate: #![feature(split_as_slice)]");
    assert!(rendered[1].trim().is_empty());
    assert_eq!(rendered[2], "  This is a tracking issue for:");
    assert!(rendered[3].trim().is_empty());
    assert_eq!(rendered[4], "  Public API");
    assert!(rendered[5].trim().is_empty());
    assert_eq!(rendered[6], "  ▏ let a = [1,2,3];");
    assert_eq!(rendered[7], "  ▏ let mut iter = a.split(|i| i == 2);");
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
    let rendered = rendered_lines_without_block_copy(&document);

    assert_eq!(rendered[0], "  ▏ fn places_alias<'tcx>(");
    assert_eq!(rendered[1], "  ▏     tcx: TyCtxt<'tcx>,");
    assert_eq!(rendered[3], "  ▏     return false; // conservative");

    let keyword = document.lines[0]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "fn")
        .expect("highlighted fn keyword");
    assert_eq!(keyword.style.fg, Some(Color::LightMagenta));
    assert_eq!(
        keyword.style.bg,
        Some(crate::theme::Theme::from_name(ThemeName::Dark).code_bg)
    );
    assert!(keyword.style.add_modifier.contains(Modifier::BOLD));

    let comment = document.lines[3]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "// conservative")
        .expect("highlighted comment");
    assert_eq!(
        comment.style.fg,
        Some(crate::theme::Theme::from_name(ThemeName::Dark).quote)
    );
    assert_eq!(
        comment.style.bg,
        Some(crate::theme::Theme::from_name(ThemeName::Dark).code_bg)
    );
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
    let rendered = rendered_lines_without_block_copy(&document);

    assert_eq!(rendered[0], "  ▏ ---");
    assert_eq!(
        rendered[1],
        "  ▏     Finished `dev` profile [unoptimized + debuginfo] target(s) in 31.62s"
    );
    assert_eq!(rendered[2], "  ▏ error[E0308]: mismatched types");

    let separator = document.lines[0]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "---")
        .expect("highlighted separator");
    assert_eq!(
        separator.style.fg,
        Some(crate::theme::Theme::from_name(ThemeName::Dark).subtle)
    );
    assert_eq!(
        separator.style.bg,
        Some(crate::theme::Theme::from_name(ThemeName::Dark).code_bg)
    );

    let error = document.lines[2]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "error[E0308]: mismatched types")
        .expect("highlighted error");
    assert_eq!(error.style.fg, Some(Color::LightRed));
    assert_eq!(
        error.style.bg,
        Some(crate::theme::Theme::from_name(ThemeName::Dark).code_bg)
    );
    assert!(error.style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn markdown_code_and_quote_blocks_have_distinct_chrome() {
    let mut builder = DetailsBuilder::new(88);
    builder.push_markdown_block_indented(
            "Intro\n\n```console\nerror[E0214]: parenthesized type parameters\n  1 | fn foo(_: Option()) {}\n```\n\n> quoted rationale\n> with another line",
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
    let theme = crate::theme::Theme::from_name(ThemeName::Dark);

    assert!(rendered[2].starts_with("  ▏ error[E0214]: parenthesized type parameters"));
    assert!(rendered[2].trim_end().ends_with("copy"));
    assert!(rendered[4].starts_with("  ┃ quoted rationale"));
    assert!(rendered[4].trim_end().ends_with("copy"));

    let code_rail = document.lines[2]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "▏ ")
        .expect("code rail");
    assert_eq!(code_rail.style.bg, Some(theme.code_bg));
    assert!(document.copy_exclusions.iter().any(|region| {
        region.line == 2 && region.start == COMMENT_LEFT_PADDING as u16 && region.end == 4
    }));

    let quote_rail = document.lines[4]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "┃ ")
        .expect("quote rail");
    assert_eq!(quote_rail.style.bg, Some(theme.quote_bg));
    assert!(document.copy_exclusions.iter().any(|region| {
        region.line == 4 && region.start == COMMENT_LEFT_PADDING as u16 && region.end == 4
    }));

    let code = document.lines[2]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "error[E0214]: parenthesized type parameters")
        .expect("code content");
    assert_eq!(code.style.bg, Some(theme.code_bg));

    let code_copy =
        "error[E0214]: parenthesized type parameters\n  1 | fn foo(_: Option()) {}".to_string();
    let code_copy_action = document
        .actions
        .iter()
        .find(|region| {
            region.line == 2
                && matches!(&region.action, DetailAction::CopyBlock(text) if text == &code_copy)
        })
        .expect("code copy action");
    assert_eq!(
        document.action_at(code_copy_action.line, code_copy_action.start),
        Some(DetailAction::CopyBlock(code_copy))
    );
    assert!(document.copy_exclusions.iter().any(|region| {
        region.line == 2
            && region.start <= code_copy_action.start
            && region.end >= code_copy_action.end
    }));

    let quote = document.lines[4]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "quoted rationale")
        .expect("quote content");
    assert_eq!(quote.style.fg, Some(theme.quote));
    assert_eq!(quote.style.bg, Some(theme.quote_bg));

    let quote_copy = "quoted rationale\nwith another line".to_string();
    let quote_copy_action = document
        .actions
        .iter()
        .find(|region| {
            region.line == 4
                && matches!(&region.action, DetailAction::CopyBlock(text) if text == &quote_copy)
        })
        .expect("quote copy action");
    assert_eq!(
        document.action_at(quote_copy_action.line, quote_copy_action.start),
        Some(DetailAction::CopyBlock(quote_copy))
    );
    assert!(document.copy_exclusions.iter().any(|region| {
        region.line == 4
            && region.start <= quote_copy_action.start
            && region.end >= quote_copy_action.end
    }));
}

#[test]
fn details_comments_have_separators_and_raw_urls_are_clickable() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.details.insert(
        "1".to_string(),
        DetailState::Loaded(vec![
            CommentPreview {
                id: None,
                kind: CommentPreviewKind::Comment,
                author: "alice".to_string(),
                body: "See https://example.com/one.".to_string(),
                created_at: None,
                updated_at: None,
                url: None,
                parent_id: None,
                is_mine: false,
                viewer_can_update: None,
                reactions: ReactionSummary::default(),
                review: None,
            },
            CommentPreview {
                id: None,
                kind: CommentPreviewKind::Comment,
                author: "bob".to_string(),
                body: "Second comment".to_string(),
                created_at: None,
                updated_at: None,
                url: Some("https://github.com/rust-lang/rust/pull/1#issuecomment-2".to_string()),
                parent_id: None,
                is_mine: false,
                viewer_can_update: None,
                reactions: ReactionSummary::default(),
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
    let comment_line_indices = document
        .comments
        .iter()
        .flat_map(|comment| comment.start_line..comment.end_line)
        .collect::<Vec<_>>();
    assert!(
        comment_line_indices
            .iter()
            .filter_map(|index| rendered_lines.get(*index))
            .filter(|line| !line.trim().is_empty())
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
fn details_activity_hides_comment_actions() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let mut activity = comment(
        "doitian",
        "pushed 3 commits\n\n- [ee8130a](https://github.com/owner/repo/commit/ee8130a) fix\n- [439a42b](https://github.com/owner/repo/commit/439a42b) feat",
        Some("https://github.com/owner/repo/pull/1/commits"),
    );
    activity.kind = CommentPreviewKind::Activity;
    app.details
        .insert("1".to_string(), DetailState::Loaded(vec![activity]));

    let rendered = build_details_document(&app, 120)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("pushed 3 commits"));
    assert!(
        rendered.lines().any(|line| line.contains("- ee8130a fix")),
        "first commit should render as its own list line: {rendered:?}"
    );
    assert!(
        rendered.lines().any(|line| line.contains("- 439a42b feat")),
        "second commit should render as its own list line: {rendered:?}"
    );
    assert!(
        !rendered
            .lines()
            .any(|line| line.contains("ee8130a") && line.contains("439a42b")),
        "commit list items should not collapse into one paragraph: {rendered:?}"
    );
    let activity_header = rendered
        .lines()
        .find(|line| line.contains("doitian"))
        .expect("activity header");
    assert!(!activity_header.contains("+ react"));
    assert!(!activity_header.contains("reply"));
}

#[test]
fn long_clickable_markdown_tokens_are_truncated_instead_of_hard_wrapped() {
    let url =
        "https://example.com/some/really/long/path/that/would/otherwise/leave/a/dangling/character";
    let mut builder = DetailsBuilder::new(24);
    builder.push_markdown_block_indented(
        &format!("See {url} for details."),
        "empty",
        usize::MAX,
        usize::MAX,
        0,
        0,
    );
    let document = builder.finish();
    let rendered = document
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let link_regions = document
        .links
        .iter()
        .filter(|link| link.url == url)
        .collect::<Vec<_>>();

    assert_eq!(link_regions.len(), 1);
    let link_line = &rendered[link_regions[0].line];
    assert!(link_line.contains("..."));
    assert!(display_width(link_line) <= 24);
    assert_eq!(
        document.link_at(link_regions[0].line, link_regions[0].start),
        Some(url.to_string())
    );
    assert!(
        rendered.iter().all(|line| line.trim().chars().count() != 1),
        "long clickable token should not leave orphan single-character lines: {rendered:?}"
    );
}

#[test]
fn markdown_images_render_as_clickable_image_links() {
    let url = "https://example.com/architecture.png";
    let mut builder = DetailsBuilder::new(80);
    builder.push_markdown_block_indented(
        &format!("Screenshot:\n\n![Architecture *diagram*]({url})"),
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

    let line_index = rendered
        .iter()
        .position(|line| line.contains("[image: Architecture diagram]"))
        .expect("rendered image label");
    let column = rendered[line_index]
        .find("[image: Architecture diagram]")
        .expect("image label column") as u16;
    assert_eq!(document.link_at(line_index, column), Some(url.to_string()));
    assert!(
        !rendered.iter().any(|line| line.contains("![")),
        "raw markdown image syntax should not leak: {rendered:?}"
    );
}

#[test]
fn github_html_image_tags_render_as_clickable_image_links() {
    let url = "https://github.com/user-attachments/assets/c84fe1a4-44bc-4b62-bc58-ca0aa3c437fd";
    let mut builder = DetailsBuilder::new(100);
    builder.push_markdown_block_indented(
        &format!(r#"<img width="1403" height="988" alt="Image" src="{url}" />"#),
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

    let line_index = rendered
        .iter()
        .position(|line| line.contains("[image 1403x988]"))
        .expect("rendered GitHub attachment label");
    let column = rendered[line_index]
        .find("[image 1403x988]")
        .expect("image label column") as u16;
    assert_eq!(document.link_at(line_index, column), Some(url.to_string()));
    assert!(
        !rendered.iter().any(|line| line.contains("<img")),
        "raw html image tag should not leak: {rendered:?}"
    );
}

#[test]
fn html_image_attrs_decode_entities_for_label_and_url() {
    let url = "https://example.com/shot.png?one=1&two=2";
    let mut builder = DetailsBuilder::new(100);
    builder.push_markdown_block_indented(
        r#"<img alt="Packet &amp; claim" src="https://example.com/shot.png?one=1&amp;two=2">"#,
        "empty",
        usize::MAX,
        usize::MAX,
        0,
        0,
    );
    let document = builder.finish();
    let rendered = document
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    let line_index = rendered
        .iter()
        .position(|line| line.contains("[image: Packet & claim]"))
        .expect("rendered decoded image label");
    let column = rendered[line_index]
        .find("[image: Packet & claim]")
        .expect("image label column") as u16;
    assert_eq!(document.link_at(line_index, column), Some(url.to_string()));
}

#[test]
fn pr_and_issue_detail_comment_authors_are_clickable() {
    let mut pr_app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    pr_app.details.insert(
        "1".to_string(),
        DetailState::Loaded(vec![comment("alice", "PR comment", None)]),
    );
    let pr_document = build_details_document(&pr_app, 120);
    assert_document_link_for_text(&pr_document, "alice", "https://github.com/alice");

    let mut issue = work_item("issue-1", "chenyukang/ghr", 1, "Bug report", Some("bob"));
    issue.kind = ItemKind::Issue;
    issue.url = "https://github.com/chenyukang/ghr/issues/1".to_string();
    let issue_section = SectionSnapshot {
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
    };
    let mut issue_app = AppState::new(SectionKind::Issues, vec![issue_section]);
    issue_app.details.insert(
        "issue-1".to_string(),
        DetailState::Loaded(vec![comment("carol", "Issue comment", None)]),
    );
    let issue_document = build_details_document(&issue_app, 120);
    assert_document_link_for_text(&issue_document, "carol", "https://github.com/carol");
}

#[test]
fn details_render_description_and_comment_reactions() {
    let mut section = test_section();
    section.items[0].reactions.heart = 1;
    section.items[0].reactions.eyes = 1;
    let mut comment = comment("alice", "A reacted comment", None);
    comment.reactions.rocket = 2;
    comment.reactions.eyes = 1;

    let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
    app.details
        .insert("1".to_string(), DetailState::Loaded(vec![comment]));

    let rendered_lines = build_details_document(&app, 100)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let rendered = rendered_lines.join("\n");

    assert!(rendered.contains("  reactions: ❤️ 1  👀 1  + react"));
    assert!(rendered.contains("alice - -  🚀 2  👀 1"));
    let header_index = rendered_lines
        .iter()
        .position(|line| line.contains("alice - -  🚀 2  👀 1"))
        .expect("comment header");
    assert!(
        rendered_lines
            .get(header_index + 1)
            .is_some_and(|line| line.trim().is_empty()),
        "comment header and body should be separated by a blank line: {rendered_lines:?}"
    );
    assert!(
        rendered_lines
            .get(header_index + 2)
            .is_some_and(|line| line.contains("A reacted comment")),
        "comment body should follow the blank spacer: {rendered_lines:?}"
    );
}

#[test]
fn display_width_counts_reaction_emoji_columns() {
    assert_eq!(display_width("😄"), 2);
    assert_eq!(display_width("❤️"), 2);
    assert_eq!(display_width("👀"), 2);
    assert_eq!(display_width("❤️ 1  👀 1  react"), 17);
}

#[test]
fn details_renderer_marks_terminal_wide_symbols_as_skip_cells() {
    let line = Line::from(vec![
        Span::raw("Zhangcy0x3 - 16d open "),
        Span::raw("❤️"),
        Span::raw(" 1  "),
        Span::styled("+ react", active_theme().action),
    ]);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 3));

    DetailsLines::new(std::slice::from_ref(&line), 0, active_theme().panel())
        .render(buffer.area, &mut buffer);

    let heart_column = display_width("Zhangcy0x3 - 16d open ") as u16;
    assert_eq!(buffer[(heart_column, 0)].symbol(), "❤️");
    assert!(
        buffer[(heart_column + 1, 0)].skip,
        "the second cell occupied by the heart emoji should be skipped by terminal diffing"
    );
    assert_eq!(buffer[(heart_column + 2, 0)].symbol(), " ");
    assert_eq!(buffer[(heart_column + 3, 0)].symbol(), "1");
}

#[test]
fn plus_in_pr_details_opens_item_reaction_when_no_comment_is_visible() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    let area = Rect::new(0, 0, 120, 36);

    app.focus_details();
    assert!(!handle_key_in_area(
        &mut app,
        KeyEvent::new(KeyCode::Char('='), KeyModifiers::SHIFT),
        &config,
        &store,
        &tx,
        Some(area),
    ));

    assert!(matches!(
        app.reaction_dialog.as_ref().map(|dialog| &dialog.target),
        Some(ReactionTarget::Item)
    ));
}

#[test]
fn plus_in_pr_details_reacts_to_visible_focused_comment() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.details.insert(
        "1".to_string(),
        DetailState::Loaded(vec![own_comment(42, "alice", "visible", None)]),
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    let area = Rect::new(0, 0, 120, 40);

    app.focus_details();
    app.scroll_selected_comment_into_view(Some(area));
    assert!(!handle_key_in_area(
        &mut app,
        key(KeyCode::Char('+')),
        &config,
        &store,
        &tx,
        Some(area),
    ));

    assert!(matches!(
        app.reaction_dialog.as_ref().map(|dialog| &dialog.target),
        Some(ReactionTarget::IssueComment {
            index: 0,
            comment_id: 42
        })
    ));
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
        is_resolved: false,
        is_outdated: false,
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

    app.handle_detail_action(DetailAction::ToggleCommentExpanded(0), None, None);
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
            is_resolved: false,
            is_outdated: false,
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
    let header_line = rendered
        .lines()
        .find(|line| line.contains("chenyukang") && line.contains("open"))
        .expect("inline comment header");
    assert!(
        !header_line.contains("inline src/github.rs:876 right"),
        "review metadata should stay off the author/action header: {rendered}"
    );
    let metadata_line = rendered
        .lines()
        .find(|line| line.contains("inline src/github.rs:876 right"))
        .expect("inline review metadata line");
    assert!(
        !metadata_line.contains("+ react") && !metadata_line.contains("reply"),
        "review metadata line should not carry comment actions: {rendered}"
    );
    assert!(rendered.contains("GH_NO_UPDATE_NOTIFIER"));
    assert!(rendered.lines().any(|line| {
        line.contains('>') && line.contains('+') && line.contains("GH_NO_UPDATE_NOTIFIER")
    }));
    assert!(rendered.contains("This is a review comment?"));
    assert!(rendered.contains("comments: 1"));
}

#[test]
fn details_comments_keep_long_inline_review_metadata_on_own_line() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let mut inline = comment(
        "mejrs",
        "It seems wrong to suggest this.",
        Some("https://github.com/rust-lang/rust/pull/156000#discussion_r99"),
    );
    inline.review = Some(crate::model::ReviewCommentPreview {
        path: "tests/ui/drop/explicit-drop-call-error.stderr".to_string(),
        line: Some(10),
        original_line: None,
        start_line: Some(9),
        original_start_line: None,
        side: Some("RIGHT".to_string()),
        start_side: Some("RIGHT".to_string()),
        diff_hunk: None,
        is_resolved: false,
        is_outdated: true,
    });
    app.details
        .insert("1".to_string(), DetailState::Loaded(vec![inline]));
    app.focus_details();

    let rendered = build_details_document(&app, 120)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let header_index = rendered
        .iter()
        .position(|line| line.contains("mejrs") && line.contains("+ react"))
        .expect("comment action header");
    let metadata_index = rendered
        .iter()
        .position(|line| {
            line.contains("inline tests/ui/drop/explicit-drop-call-error.stderr:9-10 right")
        })
        .expect("inline review metadata line");

    assert_eq!(
        metadata_index,
        header_index + 1,
        "review metadata should be directly below the author/action header: {rendered:?}"
    );
    assert!(
        rendered[metadata_index].contains("outdated"),
        "outdated state should stay with review metadata: {rendered:?}"
    );
    assert!(
        !rendered[header_index].contains("tests/ui/drop"),
        "long review metadata should not share the author/action header: {rendered:?}"
    );
    assert!(
        !rendered[metadata_index].contains("+ react")
            && !rendered[metadata_index].contains("reply"),
        "comment actions should stay on the header: {rendered:?}"
    );
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
        is_resolved: false,
        is_outdated: false,
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
    assert!(
        rendered.lines().any(|line| {
            line.contains('>') && line.contains('+') && line.contains("outputsData")
        })
    );
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
    let rendered = document
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
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
        .filter(|line| {
            let line = line.to_string();
            (line.starts_with('┏') && line.ends_with('┓'))
                || (line.starts_with('┗') && line.ends_with('┛'))
        })
        .count();
    assert_eq!(selected_border_count, 2);
    assert!(
        rendered
            .iter()
            .all(|line| !line.starts_with("┃ ━") && !line.starts_with("┃━")),
        "selected comment borders should use closed corners: {rendered:?}"
    );
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
    let theme = crate::theme::active_theme();
    assert!(document.lines.iter().all(|line| {
        line.spans
            .iter()
            .all(|span| span.style.bg.is_none() || span.style.bg == Some(theme.surface))
    }));
    assert_eq!(
        document.action_at(bob_line_index, reply_column),
        Some(DetailAction::ReplyComment(1))
    );
    assert_eq!(document.comment_at(bob_line_index), Some(1));
}

#[test]
fn comment_gap_lines_are_padded_to_clear_stale_header_cells() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.focus_details();
    app.clear_selected_comment();
    let mut reacted = comment("Zhangcy0x3", "pushed 1 commit", None);
    reacted.reactions.heart = 1;
    app.details
        .insert("1".to_string(), DetailState::Loaded(vec![reacted]));

    let document = build_details_document(&app, 100);
    let rendered = document
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let header_index = rendered
        .iter()
        .position(|line| line.contains("Zhangcy0x3") && line.contains("+ react"))
        .expect("comment header");
    let gap = rendered
        .get(header_index + 1)
        .expect("comment body gap line");

    assert!(
        gap.trim().is_empty(),
        "gap should stay visually blank: {gap:?}"
    );
    assert_eq!(
        display_width(gap),
        100,
        "gap should overwrite the full details row: {rendered:?}"
    );
}

#[test]
fn selected_comment_right_border_stays_aligned_with_reactions() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.focus_details();
    app.selected_comment_index = 0;
    let mut reacted = comment("Zhangcy0x3", "pushed 1 commit", None);
    reacted.reactions.heart = 1;
    app.details
        .insert("1".to_string(), DetailState::Loaded(vec![reacted]));

    let width = 100;
    let document = build_details_document(&app, width);
    let border_width = comment_right_border_column(usize::from(width)) + 1;
    let rendered = document
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let top = rendered
        .iter()
        .find(|line| line.starts_with('┏'))
        .expect("selected comment top border");
    let header = rendered
        .iter()
        .find(|line| line.contains("Zhangcy0x3") && line.contains("+ react"))
        .expect("selected comment header");
    let body = rendered
        .iter()
        .find(|line| line.contains("pushed 1 commit"))
        .expect("selected comment body");

    for line in [top, header, body] {
        assert_eq!(
            display_width(line),
            border_width,
            "selected comment line should end at the shared right border: {line:?}"
        );
        assert!(
            line.ends_with('┓') || line.ends_with('┃'),
            "selected comment line should have a visible right edge: {line:?}"
        );
    }
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
fn comment_with_github_update_permission_has_edit_action() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.focus_details();
    let mut editable = comment("alice", "Editable by viewer permissions", None);
    editable.id = Some(42);
    editable.viewer_can_update = Some(true);
    app.details
        .insert("1".to_string(), DetailState::Loaded(vec![editable]));

    let document = build_details_document(&app, 100);
    let header_index = document
        .lines
        .iter()
        .position(|line| line.to_string().contains("alice"))
        .expect("comment header");
    let header = document.lines[header_index].to_string();
    let edit_column = display_width(&header[..header.find("edit").expect("edit button")]) as u16;

    assert_eq!(
        document.action_at(header_index, edit_column),
        Some(DetailAction::EditComment(0))
    );
}

#[test]
fn edit_action_opens_dialog_prefilled_with_comment_body() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.details.insert(
        "1".to_string(),
        DetailState::Loaded(vec![own_comment(42, "chenyukang", "Original body", None)]),
    );

    app.handle_detail_action(DetailAction::EditComment(0), None, None);

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

    app.handle_detail_action(DetailAction::ReplyComment(0), None, None);

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
fn assignee_actions_are_rendered_in_details() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);

    let document = build_details_document(&app, 100);
    let assignee_line = document
        .lines
        .iter()
        .position(|line| line.to_string().contains("assignees: -"))
        .expect("empty assignee row");
    let empty_line = document.lines[assignee_line].to_string();
    let assign_column = empty_line.find("@ assign").expect("assign action") as u16;
    assert!(!empty_line.contains("- unassign"));
    assert_eq!(
        document.action_at(assignee_line, assign_column),
        Some(DetailAction::AssignAssignee)
    );

    app.sections[0].items[0].assignees = vec!["alice".to_string()];

    let document = build_details_document(&app, 100);
    let assignee_line = document
        .lines
        .iter()
        .position(|line| line.to_string().contains("assignees: alice"))
        .expect("assignee row");
    let assign_column = document.lines[assignee_line]
        .to_string()
        .find("@ assign")
        .expect("assign action") as u16;
    let unassign_column = document.lines[assignee_line]
        .to_string()
        .find("- unassign")
        .expect("unassign action") as u16;

    assert_eq!(
        document.action_at(assignee_line, assign_column),
        Some(DetailAction::AssignAssignee)
    );
    assert_eq!(
        document.action_at(assignee_line, unassign_column),
        Some(DetailAction::UnassignAssignee)
    );
}

#[test]
fn at_and_minus_open_assignee_dialogs() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('@')),
        &config,
        &store,
        &tx
    ));
    assert_eq!(
        app.assignee_dialog.as_ref().map(|dialog| dialog.action),
        Some(AssigneeAction::Assign)
    );
    assert_eq!(app.status, "enter assignee to add");

    app.assignee_dialog = None;
    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('-')),
        &config,
        &store,
        &tx
    ));
    assert!(app.assignee_dialog.is_none());
    assert_eq!(app.status, "selected item has no assignees");

    app.sections[0].items[0].assignees = vec!["alice".to_string()];
    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('-')),
        &config,
        &store,
        &tx
    ));
    assert_eq!(
        app.assignee_dialog.as_ref().map(|dialog| dialog.action),
        Some(AssigneeAction::Unassign)
    );
    assert_eq!(app.status, "enter assignee to remove");
}

#[test]
fn assignee_dialog_enter_submits_parsed_logins() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_assignee_dialog(AssigneeAction::Assign, None);
    app.assignee_dialog.as_mut().unwrap().input = "@alice, bob alice".to_string();
    let mut submitted = None;

    app.handle_assignee_dialog_key_with_submit(key(KeyCode::Enter), |item, action, assignees| {
        submitted = Some((item.id, action, assignees));
    });

    assert!(app.assignee_action_running);
    assert_eq!(app.status, "assigning assignee");
    assert_eq!(
        submitted,
        Some((
            "1".to_string(),
            AssigneeAction::Assign,
            vec!["alice".to_string(), "bob".to_string()]
        ))
    );
}

#[test]
fn assignee_dialog_enter_uses_prefix_candidate_for_last_login() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_assignee_dialog(AssigneeAction::Assign, None);
    let dialog = app.assignee_dialog.as_mut().expect("dialog");
    dialog.input = "alice, bo".to_string();
    dialog.suggestions = vec!["bob".to_string(), "bobby".to_string(), "carol".to_string()];
    let mut submitted = None;

    app.handle_assignee_dialog_key_with_submit(key(KeyCode::Enter), |item, action, assignees| {
        submitted = Some((item.id, action, assignees));
    });

    assert_eq!(
        submitted,
        Some((
            "1".to_string(),
            AssigneeAction::Assign,
            vec!["alice".to_string(), "bob".to_string()]
        ))
    );
}

#[test]
fn assignee_dialog_can_cycle_prefix_candidates() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_assignee_dialog(AssigneeAction::Assign, None);
    let dialog = app.assignee_dialog.as_mut().expect("dialog");
    dialog.input = "bo".to_string();
    dialog.suggestions = vec!["bob".to_string(), "bobby".to_string()];
    let mut submitted = None;

    app.handle_assignee_dialog_key_with_submit(key(KeyCode::Down), |_, _, _| {});
    app.handle_assignee_dialog_key_with_submit(key(KeyCode::Enter), |item, action, assignees| {
        submitted = Some((item.id, action, assignees));
    });

    assert_eq!(
        submitted,
        Some((
            "1".to_string(),
            AssigneeAction::Assign,
            vec!["bobby".to_string()]
        ))
    );
}

#[test]
fn empty_unassign_dialog_defaults_to_the_only_current_assignee() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.sections[0].items[0].assignees = vec!["chenyukang".to_string()];
    app.start_assignee_dialog(AssigneeAction::Unassign, None);
    let mut submitted = None;

    app.handle_assignee_dialog_key_with_submit(key(KeyCode::Enter), |item, action, assignees| {
        submitted = Some((item.id, action, assignees));
    });

    assert!(app.assignee_action_running);
    assert_eq!(app.status, "removing assignee");
    assert_eq!(
        submitted,
        Some((
            "1".to_string(),
            AssigneeAction::Unassign,
            vec!["chenyukang".to_string()]
        ))
    );
}

#[test]
fn unassign_dialog_uses_current_assignees_as_prefix_candidates() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.sections[0].items[0].assignees = vec!["alice".to_string(), "bob".to_string()];
    app.start_assignee_dialog(AssigneeAction::Unassign, None);
    app.assignee_dialog.as_mut().unwrap().input = "bo".to_string();
    let mut submitted = None;

    app.handle_assignee_dialog_key_with_submit(key(KeyCode::Enter), |item, action, assignees| {
        submitted = Some((item.id, action, assignees));
    });

    assert_eq!(
        submitted,
        Some((
            "1".to_string(),
            AssigneeAction::Unassign,
            vec!["bob".to_string()]
        ))
    );
}

#[test]
fn assignee_suggestions_loaded_updates_active_assign_dialog() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_assignee_dialog(AssigneeAction::Assign, None);
    app.assignee_dialog.as_mut().unwrap().suggestions_loading = true;

    app.handle_msg(AppMsg::AssigneeSuggestionsLoaded {
        repo: "rust-lang/rust".to_string(),
        result: Ok(vec!["alice".to_string(), "bob".to_string()]),
    });

    let dialog = app.assignee_dialog.as_ref().expect("dialog");
    assert!(!dialog.suggestions_loading);
    assert_eq!(dialog.suggestions, vec!["alice", "bob"]);
    assert_eq!(
        app.assignee_suggestions_cache.get("rust-lang/rust"),
        Some(&vec!["alice".to_string(), "bob".to_string()])
    );
    assert_eq!(app.status, "assignee candidates loaded");
}

#[test]
fn assignee_dialog_uses_cached_candidates_immediately() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.assignee_suggestions_cache.insert(
        "rust-lang/rust".to_string(),
        vec!["cached-user".to_string(), "chenyukang".to_string()],
    );

    app.start_assignee_dialog(AssigneeAction::Assign, None);

    let dialog = app.assignee_dialog.as_ref().expect("dialog");
    assert!(!dialog.suggestions_loading);
    assert_eq!(dialog.suggestions, vec!["cached-user", "chenyukang"]);
}

#[test]
fn assignee_suggestions_loaded_updates_cache_without_active_dialog() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);

    app.handle_msg(AppMsg::AssigneeSuggestionsLoaded {
        repo: "rust-lang/rust".to_string(),
        result: Ok(vec!["alice".to_string()]),
    });

    assert!(app.assignee_dialog.is_none());
    assert_eq!(
        app.assignee_suggestions_cache.get("rust-lang/rust"),
        Some(&vec!["alice".to_string()])
    );
}

#[test]
fn assignee_suggestion_refresh_error_keeps_cached_candidates() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.assignee_suggestions_cache.insert(
        "rust-lang/rust".to_string(),
        vec!["cached-user".to_string()],
    );
    app.start_assignee_dialog(AssigneeAction::Assign, None);
    app.assignee_dialog.as_mut().unwrap().suggestions_loading = true;

    app.handle_msg(AppMsg::AssigneeSuggestionsLoaded {
        repo: "rust-lang/rust".to_string(),
        result: Err("network failed".to_string()),
    });

    let dialog = app.assignee_dialog.as_ref().expect("dialog");
    assert!(!dialog.suggestions_loading);
    assert_eq!(dialog.suggestions, vec!["cached-user"]);
    assert!(dialog.suggestions_error.is_none());
    assert_eq!(
        app.status,
        "assignee candidates refresh failed; using cache"
    );
}

#[test]
fn assignee_update_success_refreshes_item_state() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_assignee_dialog(AssigneeAction::Assign, None);
    app.assignee_action_running = true;
    let mut updated = app.current_item().cloned().expect("item");
    updated.assignees = vec!["alice".to_string()];

    app.handle_msg(AppMsg::AssigneesUpdated {
        item_id: "1".to_string(),
        action: AssigneeAction::Assign,
        result: Ok(updated),
    });

    assert!(!app.assignee_action_running);
    assert!(app.assignee_dialog.is_none());
    assert_eq!(app.current_item().unwrap().assignees, vec!["alice"]);
    assert!(app.details_stale.contains("1"));
    assert_eq!(app.status, "assignee added");
    assert_eq!(
        app.message_dialog
            .as_ref()
            .map(|dialog| dialog.title.as_str()),
        Some("Assignee Added")
    );
}

#[test]
fn assignee_update_failure_opens_message_dialog() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_assignee_dialog(AssigneeAction::Unassign, None);
    app.assignee_action_running = true;

    app.handle_msg(AppMsg::AssigneesUpdated {
        item_id: "1".to_string(),
        action: AssigneeAction::Unassign,
        result: Err("gh api repos/owner/repo/issues/1/assignees failed: HTTP 422".to_string()),
    });

    assert!(!app.assignee_action_running);
    assert_eq!(app.status, "unassign failed");
    let dialog = app.message_dialog.as_ref().expect("message dialog");
    assert_eq!(dialog.title, "Unassign Failed");
    assert_eq!(dialog.body, "HTTP 422");
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
fn e_key_opens_item_edit_dialog_from_details() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    app.focus_details();
    app.details.insert(
        "1".to_string(),
        DetailState::Loaded(vec![own_comment(42, "chenyukang", "Original body", None)]),
    );
    app.clear_selected_comment();

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('e')),
        &config,
        &store,
        &tx
    ));

    assert!(app.item_edit_dialog.is_some());
    assert!(app.comment_dialog.is_none());
    assert_eq!(app.status, "editing item");
}

#[test]
fn e_key_edits_selected_comment_from_details() {
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

    assert!(app.item_edit_dialog.is_none());
    assert_eq!(
        app.comment_dialog.as_ref().map(|dialog| &dialog.mode),
        Some(&CommentDialogMode::Edit {
            comment_index: 0,
            comment_id: 42,
            is_review: false,
        })
    );
    assert_eq!(app.status, "editing comment");
}

#[test]
fn e_key_edits_selected_issue_comment_from_details() {
    let mut section = test_section();
    section.key = "issues:test".to_string();
    section.kind = SectionKind::Issues;
    section.items[0].kind = ItemKind::Issue;
    let mut app = AppState::new(SectionKind::Issues, vec![section]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    app.focus_details();
    app.details.insert(
        "1".to_string(),
        DetailState::Loaded(vec![own_comment(84, "chenyukang", "Issue comment", None)]),
    );

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('e')),
        &config,
        &store,
        &tx
    ));

    assert!(app.item_edit_dialog.is_none());
    assert_eq!(
        app.comment_dialog.as_ref().map(|dialog| &dialog.mode),
        Some(&CommentDialogMode::Edit {
            comment_index: 0,
            comment_id: 84,
            is_review: false,
        })
    );
    assert_eq!(app.status, "editing comment");
}

#[test]
fn start_edit_selected_comment_dialog_uses_github_update_permission() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.focus_details();
    let mut comment = comment("alice", "Editable by repository permission", None);
    comment.id = Some(42);
    comment.viewer_can_update = Some(true);
    app.details
        .insert("1".to_string(), DetailState::Loaded(vec![comment]));

    app.start_edit_selected_comment_dialog();

    assert_eq!(
        app.comment_dialog.as_ref().map(|dialog| &dialog.mode),
        Some(&CommentDialogMode::Edit {
            comment_index: 0,
            comment_id: 42,
            is_review: false,
        })
    );
}

#[test]
fn start_edit_selected_comment_dialog_rejects_without_update_permission() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.focus_details();
    let mut comment = comment("alice", "Not editable", None);
    comment.id = Some(42);
    app.details
        .insert("1".to_string(), DetailState::Loaded(vec![comment]));

    app.start_edit_selected_comment_dialog();

    assert!(app.comment_dialog.is_none());
    assert_eq!(app.status, "selected comment cannot be edited");
}

#[test]
fn github_update_permission_overrides_comment_ownership() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.focus_details();
    let mut comment = own_comment(42, "chenyukang", "Owned but locked", None);
    comment.viewer_can_update = Some(false);
    app.details
        .insert("1".to_string(), DetailState::Loaded(vec![comment]));

    app.start_edit_selected_comment_dialog();

    assert!(app.comment_dialog.is_none());
    assert_eq!(app.status, "selected comment cannot be edited");
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
fn copy_github_link_prefers_selected_comment_when_details_focused() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.details.insert(
        "1".to_string(),
        DetailState::Loaded(vec![comment(
            "alice",
            "looks good",
            Some("https://github.com/rust-lang/rust/pull/1#issuecomment-1"),
        )]),
    );
    app.focus_details();

    assert_eq!(
        app.selected_github_link(),
        Some((
            "https://github.com/rust-lang/rust/pull/1#issuecomment-1".to_string(),
            "comment"
        ))
    );

    app.copy_github_link();

    assert_eq!(app.status, "copied comment link");
}

#[test]
fn copy_github_link_uses_item_link_without_selected_comment() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);

    assert_eq!(
        app.selected_github_link(),
        Some((
            "https://github.com/rust-lang/rust/pull/1".to_string(),
            "pull request"
        ))
    );

    app.copy_github_link();

    assert_eq!(app.status, "copied pull request link");
}

#[test]
fn copy_content_prefers_selected_comment_when_details_focused() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.details.insert(
        "1".to_string(),
        DetailState::Loaded(vec![comment(
            "alice",
            "comment body\nwith markdown",
            Some("https://github.com/rust-lang/rust/pull/1#issuecomment-1"),
        )]),
    );
    app.focus_details();

    assert_eq!(
        app.selected_copy_content(),
        Some(("comment body\nwith markdown".to_string(), "comment content"))
    );

    app.copy_content();

    assert_eq!(app.status, "copied comment content");
}

#[test]
fn copy_content_uses_item_description_without_selected_comment() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);

    assert_eq!(
        app.selected_copy_content(),
        Some((
            "A body with useful context".to_string(),
            "pull request description"
        ))
    );

    app.copy_content();

    assert_eq!(app.status, "copied pull request description");
}

#[test]
fn copy_content_reports_missing_description() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.sections[0].items[0].body = None;

    assert_eq!(app.selected_copy_content(), None);

    app.copy_content();

    assert_eq!(app.status, "no content selected");
}

#[test]
fn linux_clipboard_commands_require_matching_display_env() {
    let wayland = UNIX_CLIPBOARD_COMMANDS
        .iter()
        .find(|command| command.command == "wl-copy")
        .copied()
        .expect("wl-copy command");
    let xclip = UNIX_CLIPBOARD_COMMANDS
        .iter()
        .find(|command| command.command == "xclip")
        .copied()
        .expect("xclip command");

    assert!(clipboard_command_enabled(wayland, |name| name == "WAYLAND_DISPLAY"));
    assert!(!clipboard_command_enabled(wayland, |name| name == "DISPLAY"));
    assert!(clipboard_command_enabled(xclip, |name| name == "DISPLAY"));
    assert!(!clipboard_command_enabled(xclip, |name| name == "WAYLAND_DISPLAY"));
}

#[test]
fn osc52_base64_encoding_handles_padding() {
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_encode(b"f"), "Zg==");
    assert_eq!(base64_encode(b"fo"), "Zm8=");
    assert_eq!(base64_encode(b"foo"), "Zm9v");
    assert_eq!(
        base64_encode(&[0xe5, 0xa4, 0x8d, 0xe5, 0x88, 0xb6]),
        "5aSN5Yi2"
    );
}

#[test]
fn clipboard_command_failure_message_includes_stderr() {
    assert_eq!(
        command_failure_message("xclip", "exit status: 1", b"Error: Can't open display\n"),
        "xclip failed (exit status: 1): Error: Can't open display"
    );
    assert_eq!(
        command_failure_message("xsel", "exit status: 1", b""),
        "xsel failed (exit status: 1)"
    );
}

#[test]
fn clipboard_copy_error_reports_attempted_targets_concisely() {
    let errors = vec![
        "wl-copy skipped; WAYLAND_DISPLAY is not set".to_string(),
        "xclip failed (exit status: 1): Error: Can't open display".to_string(),
        "xsel not found".to_string(),
        "tmux failed (exit status: 1): clipboard unavailable".to_string(),
        "OSC 52 requires terminal stdout".to_string(),
    ];

    assert_eq!(
        clipboard_copy_error(&errors).to_string(),
        "no usable clipboard target: wl-copy skipped; WAYLAND_DISPLAY is not set; xclip failed (exit status: 1): Error: Can't open display; xsel not found; tmux failed (exit status: 1): clipboard unavailable; OSC 52 requires terminal stdout"
    );
}

#[test]
fn command_palette_copy_github_link_copies_current_item_link() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    app.command_palette = Some(CommandPalette {
        query: "copy github".to_string(),
        selected: 0,
    });

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Enter),
        &config,
        &store,
        &tx
    ));

    assert_eq!(app.status, "copied pull request link");
    assert!(app.command_palette.is_none());
}

#[test]
fn command_palette_copy_content_copies_current_item_description() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    app.command_palette = Some(CommandPalette {
        query: "copy content".to_string(),
        selected: 0,
    });

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Enter),
        &config,
        &store,
        &tx
    ));

    assert_eq!(app.status, "copied pull request description");
    assert!(app.command_palette.is_none());
}

#[test]
fn label_dialog_submits_add_label() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_add_label_dialog(None);

    app.handle_label_dialog_key_with_submit(key(KeyCode::Char('b')), |_| {});
    app.handle_label_dialog_key_with_submit(key(KeyCode::Char('u')), |_| {});
    app.handle_label_dialog_key_with_submit(key(KeyCode::Char('g')), |_| {});

    let mut pending = None;
    app.handle_label_dialog_key_with_submit(key(KeyCode::Enter), |update| {
        pending = Some(update);
    });
    let pending = pending.expect("pending label update");

    assert!(app.label_updating);
    assert!(app.label_dialog.is_none());
    assert_eq!(pending.item.id, "1");
    assert_eq!(pending.action, LabelAction::Add("bug".to_string()));
}

#[test]
fn label_dialog_prefix_matches_repo_labels() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_add_label_dialog(None);
    let dialog = app.label_dialog.as_mut().expect("label dialog");
    dialog.suggestions = vec![
        "bug".to_string(),
        "good first issue".to_string(),
        "T-compiler".to_string(),
    ];

    app.handle_label_dialog_key_with_submit(key(KeyCode::Char('g')), |_| {});
    assert_eq!(
        label_dialog_suggestion_matches(app.label_dialog.as_ref().unwrap()),
        vec!["good first issue"]
    );

    let mut pending = None;
    app.handle_label_dialog_key_with_submit(key(KeyCode::Enter), |update| {
        pending = Some(update);
    });
    let pending = pending.expect("pending label update");
    assert_eq!(
        pending.action,
        LabelAction::Add("good first issue".to_string())
    );
}

#[test]
fn label_dialog_uses_cached_suggestions_immediately() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.label_suggestions_cache.insert(
        "rust-lang/rust".to_string(),
        vec!["T-compiler".to_string(), "A-diagnostics".to_string()],
    );

    app.start_add_label_dialog(None);

    let dialog = app.label_dialog.as_ref().expect("label dialog");
    assert!(!dialog.suggestions_loading);
    assert_eq!(dialog.suggestions, vec!["T-compiler", "A-diagnostics"]);
}

#[test]
fn label_suggestions_loaded_updates_cache_without_active_dialog() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);

    app.handle_msg(AppMsg::LabelSuggestionsLoaded {
        repo: "rust-lang/rust".to_string(),
        result: Ok(vec!["bug".to_string()]),
    });

    assert!(app.label_dialog.is_none());
    assert_eq!(
        app.label_suggestions_cache.get("rust-lang/rust"),
        Some(&vec!["bug".to_string()])
    );
}

#[test]
fn label_suggestion_refresh_error_keeps_cached_suggestions() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.label_suggestions_cache.insert(
        "rust-lang/rust".to_string(),
        vec!["cached-label".to_string()],
    );
    app.start_add_label_dialog(None);
    app.label_dialog.as_mut().unwrap().suggestions_loading = true;

    app.handle_msg(AppMsg::LabelSuggestionsLoaded {
        repo: "rust-lang/rust".to_string(),
        result: Err("network failed".to_string()),
    });

    let dialog = app.label_dialog.as_ref().expect("label dialog");
    assert!(!dialog.suggestions_loading);
    assert_eq!(dialog.suggestions, vec!["cached-label"]);
    assert!(dialog.suggestions_error.is_none());
    assert_eq!(app.status, "label suggestions refresh failed; using cache");
}

#[test]
fn issue_label_input_parses_comma_separated_labels() {
    assert_eq!(
        parse_issue_labels("bug, needs review, bug, , T-compiler"),
        vec!["bug", "needs review", "T-compiler"]
    );
}

#[test]
fn issue_dialog_renders_field_colons_and_separators() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_new_issue_dialog();
    if let Some(dialog) = &mut app.issue_dialog {
        dialog.labels.set_text("bug, T-compiler");
        dialog.body.set_text("Steps to reproduce");
    }

    let backend = ratatui::backend::TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let paths = test_paths();

    terminal
        .draw(|frame| draw(frame, &app, &paths))
        .expect("draw");

    let rendered = buffer_lines(terminal.backend().buffer()).join("\n");

    assert!(rendered.contains("Repo: rust-lang/rust"));
    assert!(rendered.contains("Title:"));
    assert!(rendered.contains("Labels: bug, T-compiler"));
    assert!(rendered.contains("Body:"));
    assert!(rendered.lines().any(|line| line.contains("────────")));
}

#[test]
fn issue_dialog_submits_metadata() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_new_issue_dialog();

    for ch in "Crash in parser".chars() {
        app.handle_issue_dialog_key_with_submit(key(KeyCode::Char(ch)), None, |_| {});
    }
    app.handle_issue_dialog_key_with_submit(key(KeyCode::Tab), None, |_| {});
    for ch in "bug, T-compiler".chars() {
        app.handle_issue_dialog_key_with_submit(key(KeyCode::Char(ch)), None, |_| {});
    }
    app.handle_issue_dialog_key_with_submit(key(KeyCode::Tab), None, |_| {});
    for ch in "Steps to reproduce".chars() {
        app.handle_issue_dialog_key_with_submit(key(KeyCode::Char(ch)), None, |_| {});
    }

    let mut pending = None;
    app.handle_issue_dialog_key_with_submit(ctrl_key(KeyCode::Enter), None, |issue| {
        pending = Some(issue);
    });
    let pending = pending.expect("pending issue create");

    assert!(app.issue_creating);
    assert!(app.issue_dialog.is_none());
    assert_eq!(pending.repo, "rust-lang/rust");
    assert_eq!(pending.title, "Crash in parser");
    assert_eq!(pending.body, "Steps to reproduce");
    assert_eq!(pending.labels, vec!["bug", "T-compiler"]);
}

#[test]
fn capital_n_in_pr_repo_with_local_dir_opens_pr_create_dialog() {
    let local_dir = checkout_test_repo_dir_on_branch("feature/new-pr");
    let section = SectionSnapshot {
        key: "repo:ghr:pull_requests:Pull Requests".to_string(),
        kind: SectionKind::PullRequests,
        title: "Pull Requests".to_string(),
        filters: "repo:chenyukang/ghr is:open archived:false".to_string(),
        items: Vec::new(),
        total_count: Some(0),
        page: 1,
        page_size: 50,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
    app.active_view = "repo:ghr".to_string();
    let mut config = Config::default();
    config.repos.push(crate::config::RepoConfig {
        name: "ghr".to_string(),
        repo: "chenyukang/ghr".to_string(),
        remote: None,
        local_dir: Some(local_dir.display().to_string()),
        show_prs: true,
        show_issues: true,
        labels: Vec::new(),
        pr_labels: Vec::new(),
        issue_labels: Vec::new(),
    });

    app.start_new_issue_or_pull_request_dialog(&config);
    let dialog = app.pr_create_dialog.as_ref().expect("pr create dialog");

    assert!(app.issue_dialog.is_none());
    assert_eq!(dialog.repo, "chenyukang/ghr");
    assert_eq!(dialog.local_dir, local_dir);
    assert_eq!(dialog.branch, "feature/new-pr");
    assert_eq!(dialog.head_ref, "feature/new-pr");
    assert_eq!(dialog.field, PrCreateField::Title);
    assert_eq!(app.status, "new pull request");
}

#[test]
fn capital_n_in_pr_repo_with_fork_local_dir_uses_owner_qualified_head() {
    let branch = "fix/mpp-force-close-preimage-retention";
    let local_dir =
        checkout_test_fork_repo_dir_on_branch(branch, "chenyukang/fiber", "nervosnetwork/fiber");
    let section = SectionSnapshot {
        key: "repo:fiber:pull_requests:Pull Requests".to_string(),
        kind: SectionKind::PullRequests,
        title: "Pull Requests".to_string(),
        filters: "repo:nervosnetwork/fiber is:open archived:false".to_string(),
        items: Vec::new(),
        total_count: Some(0),
        page: 1,
        page_size: 50,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
    app.active_view = "repo:fiber".to_string();
    let mut config = Config::default();
    config.repos.push(crate::config::RepoConfig {
        name: "Fiber".to_string(),
        repo: "nervosnetwork/fiber".to_string(),
        remote: None,
        local_dir: Some(local_dir.display().to_string()),
        show_prs: true,
        show_issues: true,
        labels: Vec::new(),
        pr_labels: Vec::new(),
        issue_labels: Vec::new(),
    });

    app.start_new_issue_or_pull_request_dialog(&config);
    let dialog = app.pr_create_dialog.as_ref().expect("pr create dialog");

    assert_eq!(dialog.repo, "nervosnetwork/fiber");
    assert_eq!(dialog.branch, branch);
    assert_eq!(
        dialog.head_ref,
        "chenyukang:fix/mpp-force-close-preimage-retention"
    );
    assert_eq!(app.status, "new pull request");
}

#[test]
fn configured_repo_remote_is_used_when_validating_local_dir() {
    let local_dir = checkout_test_fork_repo_dir_on_branch(
        "feature/upstream-base",
        "Officeyutong/tentacle",
        "nervosnetwork/tentacle",
    );
    let mut config = Config::default();
    config.repos.push(crate::config::RepoConfig {
        name: "tentacle".to_string(),
        repo: "nervosnetwork/tentacle".to_string(),
        remote: Some("origin".to_string()),
        local_dir: Some(local_dir.display().to_string()),
        show_prs: true,
        show_issues: true,
        labels: Vec::new(),
        pr_labels: Vec::new(),
        issue_labels: Vec::new(),
    });

    let error = resolve_pr_checkout_directory(&config, "nervosnetwork/tentacle")
        .expect_err("origin points at the fork, not the configured base repo");
    assert!(error.contains("remote origin points at Officeyutong/tentacle"));

    config.repos[0].remote = Some("upstream".to_string());

    assert_eq!(
        resolve_pr_checkout_directory(&config, "nervosnetwork/tentacle").expect("upstream remote"),
        local_dir
    );
}

#[test]
fn capital_n_in_pr_repo_without_local_dir_shows_hint() {
    let section = SectionSnapshot {
        key: "repo:ghr:pull_requests:Pull Requests".to_string(),
        kind: SectionKind::PullRequests,
        title: "Pull Requests".to_string(),
        filters: "repo:chenyukang/ghr is:open archived:false".to_string(),
        items: Vec::new(),
        total_count: Some(0),
        page: 1,
        page_size: 50,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
    app.active_view = "repo:ghr".to_string();

    app.start_new_issue_or_pull_request_dialog(&Config::default());

    assert!(app.pr_create_dialog.is_none());
    assert!(app.issue_dialog.is_none());
    assert_eq!(app.status, "pull request creation unavailable");
    let dialog = app.message_dialog.as_ref().expect("message dialog");
    assert_eq!(dialog.title, "New Pull Request Unavailable");
    assert_eq!(dialog.kind, MessageDialogKind::Error);
    assert!(dialog.body.contains("repo chenyukang/ghr has no local_dir"));
    assert!(dialog.body.contains("[[repos]].local_dir"));
}

#[test]
fn capital_n_in_pr_repo_with_invalid_local_dir_opens_message_dialog() {
    let section = SectionSnapshot {
        key: "repo:ghr:pull_requests:Pull Requests".to_string(),
        kind: SectionKind::PullRequests,
        title: "Pull Requests".to_string(),
        filters: "repo:chenyukang/ghr is:open archived:false".to_string(),
        items: Vec::new(),
        total_count: Some(0),
        page: 1,
        page_size: 50,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
    app.active_view = "repo:ghr".to_string();
    let local_dir =
        std::env::temp_dir().join(format!("ghr-missing-pr-local-dir-{}", std::process::id()));
    let mut config = Config::default();
    config.repos.push(crate::config::RepoConfig {
        name: "ghr".to_string(),
        repo: "chenyukang/ghr".to_string(),
        remote: None,
        local_dir: Some(local_dir.display().to_string()),
        show_prs: true,
        show_issues: true,
        labels: Vec::new(),
        pr_labels: Vec::new(),
        issue_labels: Vec::new(),
    });

    app.start_new_issue_or_pull_request_dialog(&config);

    assert!(app.pr_create_dialog.is_none());
    assert_eq!(app.status, "pull request creation unavailable");
    let dialog = app.message_dialog.as_ref().expect("message dialog");
    assert_eq!(dialog.title, "New Pull Request Unavailable");
    assert_eq!(dialog.kind, MessageDialogKind::Error);
    assert!(
        dialog
            .body
            .contains("Configured local_dir for chenyukang/ghr cannot be used")
    );
    assert!(dialog.body.contains("is not a directory"));
}

#[test]
fn capital_n_in_issue_section_still_opens_issue_dialog() {
    let section = SectionSnapshot {
        key: "repo:ghr:issues:Issues".to_string(),
        kind: SectionKind::Issues,
        title: "Issues".to_string(),
        filters: "repo:chenyukang/ghr is:open archived:false".to_string(),
        items: Vec::new(),
        total_count: Some(0),
        page: 1,
        page_size: 50,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(SectionKind::Issues, vec![section]);
    app.active_view = "repo:ghr".to_string();

    app.start_new_issue_or_pull_request_dialog(&Config::default());

    assert!(app.pr_create_dialog.is_none());
    assert_eq!(
        app.issue_dialog.as_ref().map(|dialog| dialog.repo.as_str()),
        Some("chenyukang/ghr")
    );
}

#[test]
fn capital_n_in_issue_list_opens_issue_dialog() {
    let section = SectionSnapshot {
        key: "repo:ghr:issues:Issues".to_string(),
        kind: SectionKind::Issues,
        title: "Issues".to_string(),
        filters: "repo:chenyukang/ghr is:open archived:false".to_string(),
        items: Vec::new(),
        total_count: Some(0),
        page: 1,
        page_size: 50,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(SectionKind::Issues, vec![section]);
    app.active_view = "repo:ghr".to_string();
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('N')),
        &config,
        &store,
        &tx
    ));

    assert!(app.pr_create_dialog.is_none());
    assert_eq!(
        app.issue_dialog.as_ref().map(|dialog| dialog.repo.as_str()),
        Some("chenyukang/ghr")
    );
    assert_eq!(app.status, "new issue");
}

#[test]
fn capital_n_in_pr_details_opens_pr_create_dialog() {
    let local_dir = checkout_test_repo_dir_on_branch("feature/details-pr");
    let section = SectionSnapshot {
        key: "repo:ghr:pull_requests:Pull Requests".to_string(),
        kind: SectionKind::PullRequests,
        title: "Pull Requests".to_string(),
        filters: "repo:chenyukang/ghr is:open archived:false".to_string(),
        items: vec![work_item("8", "chenyukang/ghr", 8, "Add diff UI", None)],
        total_count: Some(1),
        page: 1,
        page_size: 50,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
    app.active_view = "repo:ghr".to_string();
    app.focus_details();
    let mut config = Config::default();
    config.repos.push(crate::config::RepoConfig {
        name: "ghr".to_string(),
        repo: "chenyukang/ghr".to_string(),
        remote: None,
        local_dir: Some(local_dir.display().to_string()),
        show_prs: true,
        show_issues: true,
        labels: Vec::new(),
        pr_labels: Vec::new(),
        issue_labels: Vec::new(),
    });
    let (tx, _rx) = mpsc::unbounded_channel();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('N')),
        &config,
        &store,
        &tx
    ));

    assert!(app.issue_dialog.is_none());
    let dialog = app.pr_create_dialog.as_ref().expect("pr create dialog");
    assert_eq!(dialog.repo, "chenyukang/ghr");
    assert_eq!(dialog.local_dir, local_dir);
    assert_eq!(dialog.branch, "feature/details-pr");
    assert_eq!(dialog.head_ref, "feature/details-pr");
    assert_eq!(app.status, "new pull request");
}

#[test]
fn capital_n_in_issue_details_opens_issue_dialog() {
    let mut issue = work_item("issue-8", "chenyukang/ghr", 8, "Bug report", None);
    issue.kind = ItemKind::Issue;
    issue.url = "https://github.com/chenyukang/ghr/issues/8".to_string();
    let section = SectionSnapshot {
        key: "repo:ghr:issues:Issues".to_string(),
        kind: SectionKind::Issues,
        title: "Issues".to_string(),
        filters: "repo:chenyukang/ghr is:open archived:false".to_string(),
        items: vec![issue],
        total_count: Some(1),
        page: 1,
        page_size: 50,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(SectionKind::Issues, vec![section]);
    app.active_view = "repo:ghr".to_string();
    app.focus_details();
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('N')),
        &config,
        &store,
        &tx
    ));

    assert!(app.pr_create_dialog.is_none());
    let dialog = app.issue_dialog.as_ref().expect("issue dialog");
    assert_eq!(dialog.repo, "chenyukang/ghr");
    assert_eq!(dialog.field, IssueDialogField::Title);
    assert_eq!(app.status, "new issue");
}

#[test]
fn pr_create_dialog_submits_title_body_and_branch() {
    let local_dir = checkout_test_repo_dir_with_main_and_branch("feature/pr-body", true);
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.pr_create_dialog = Some(PrCreateDialog {
        repo: "chenyukang/ghr".to_string(),
        local_dir: local_dir.clone(),
        branch: "feature/pr-body".to_string(),
        head_ref: "feature/pr-body".to_string(),
        title: EditorText::from_text("Add PR creation"),
        body: EditorText::from_text("Created from the TUI"),
        field: PrCreateField::Body,
        body_scroll: 0,
    });

    let pending = app.prepare_pr_create().expect("pending pr create");

    assert!(app.pr_creating);
    assert!(app.pr_create_dialog.is_none());
    assert_eq!(pending.repo, "chenyukang/ghr");
    assert_eq!(pending.local_dir, local_dir);
    assert_eq!(pending.head_ref, "feature/pr-body");
    assert_eq!(pending.title, "Add PR creation");
    assert_eq!(pending.body, "Created from the TUI");
}

#[test]
fn pr_create_preflight_failure_restores_dialog_and_shows_modal() {
    let local_dir = checkout_test_repo_dir_with_main_and_branch("feature/no-diff", false);
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.pr_create_dialog = Some(PrCreateDialog {
        repo: "chenyukang/ghr".to_string(),
        local_dir,
        branch: "feature/no-diff".to_string(),
        head_ref: "feature/no-diff".to_string(),
        title: EditorText::from_text("No diff"),
        body: EditorText::from_text("This should not submit"),
        field: PrCreateField::Body,
        body_scroll: 0,
    });

    assert!(app.prepare_pr_create().is_none());

    assert!(!app.pr_creating);
    assert!(app.pending_pr_create.is_none());
    assert!(app.pr_create_dialog.is_some());
    assert_eq!(app.status, "pull request preflight failed");
    let message = app.message_dialog.as_ref().expect("preflight dialog");
    assert_eq!(message.title, "Pull Request Preflight Failed");
    assert_eq!(message.kind, MessageDialogKind::Error);
    assert!(
        message
            .body
            .contains("No commits between main and feature/no-diff")
    );
}

#[test]
fn pr_create_preflight_blocks_dirty_worktree_before_submit() {
    let local_dir = checkout_test_repo_dir_with_main_and_branch("feature/dirty", true);
    std::fs::write(local_dir.join("dirty.txt"), "uncommitted\n").expect("write dirty file");
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.pr_create_dialog = Some(PrCreateDialog {
        repo: "chenyukang/ghr".to_string(),
        local_dir,
        branch: "feature/dirty".to_string(),
        head_ref: "feature/dirty".to_string(),
        title: EditorText::from_text("Dirty"),
        body: EditorText::from_text("This should not submit"),
        field: PrCreateField::Body,
        body_scroll: 0,
    });

    assert!(app.prepare_pr_create().is_none());

    let message = app.message_dialog.as_ref().expect("preflight dialog");
    assert_eq!(message.title, "Pull Request Preflight Failed");
    assert!(
        message
            .body
            .contains("Working tree has uncommitted changes")
    );
    assert!(message.body.contains("dirty.txt"));
}

#[test]
fn issue_create_failure_restores_dialog_for_retry() {
    let mut app = AppState::new(SectionKind::Issues, vec![test_section()]);
    let dialog = IssueDialog {
        repo: EditorText::from_text("chenyukang/ghr"),
        title: EditorText::from_text("Crash in parser"),
        labels: EditorText::from_text("bug, T-compiler"),
        body: EditorText::from_text("Steps to reproduce"),
        field: IssueDialogField::Body,
        body_scroll: 1,
    };
    app.issue_creating = true;
    app.pending_issue_create = Some(PendingIssueCreate {
        repo: "chenyukang/ghr".to_string(),
        title: "Crash in parser".to_string(),
        body: "Steps to reproduce".to_string(),
        labels: vec!["bug".to_string(), "T-compiler".to_string()],
        dialog,
        draft_key: None,
    });

    app.handle_msg(AppMsg::IssueCreated {
            result: Err("failed to create issue in chenyukang/ghr: gh api repos/chenyukang/ghr/issues failed: validation failed".to_string()),
        });

    assert!(!app.issue_creating);
    assert_eq!(app.status, "issue create failed");
    let message = app.message_dialog.as_ref().expect("failure dialog");
    assert_eq!(message.title, "Issue Create Failed");
    assert_eq!(message.kind, MessageDialogKind::RetryableError);
    assert!(message.body.contains("validation failed"));
    let restored = app.issue_dialog.as_ref().expect("restored issue dialog");
    assert_eq!(restored.title, "Crash in parser");
    assert_eq!(restored.labels, "bug, T-compiler");
    assert_eq!(restored.body, "Steps to reproduce");
    assert_eq!(restored.field, IssueDialogField::Body);
    assert_eq!(restored.body_scroll, 1);
}

#[test]
fn pull_request_create_failure_shows_gh_detail_and_restores_dialog() {
    let local_dir = checkout_test_repo_dir_on_branch("feature/pr-body");
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let dialog = PrCreateDialog {
        repo: "chenyukang/ghr".to_string(),
        local_dir: local_dir.clone(),
        branch: "feature/pr-body".to_string(),
        head_ref: "feature/pr-body".to_string(),
        title: EditorText::from_text("Add PR creation"),
        body: EditorText::from_text("Created from the TUI"),
        field: PrCreateField::Body,
        body_scroll: 2,
    };
    app.pr_creating = true;
    app.pending_pr_create = Some(PendingPrCreate {
        repo: "chenyukang/ghr".to_string(),
        local_dir,
        head_ref: "feature/pr-body".to_string(),
        title: "Add PR creation".to_string(),
        body: "Created from the TUI".to_string(),
        dialog,
        draft_key: None,
    });

    app.handle_msg(AppMsg::PullRequestCreated {
            result: Err("failed to create pull request in chenyukang/ghr: gh pr create --repo chenyukang/ghr --head feature/pr-body failed: a pull request for branch \"feature/pr-body\" already exists: https://github.com/chenyukang/ghr/pull/42".to_string()),
        });

    assert!(!app.pr_creating);
    assert_eq!(app.status, "pull request create failed");
    let message = app.message_dialog.as_ref().expect("failure dialog");
    assert_eq!(message.title, "Pull Request Create Failed");
    assert_eq!(message.kind, MessageDialogKind::RetryableError);
    assert!(message.body.contains("already exists"));
    assert!(
        message
            .body
            .contains("https://github.com/chenyukang/ghr/pull/42")
    );
    let restored = app
        .pr_create_dialog
        .as_ref()
        .expect("restored pr create dialog");
    assert_eq!(restored.title, "Add PR creation");
    assert_eq!(restored.body, "Created from the TUI");
    assert_eq!(restored.field, PrCreateField::Body);
    assert_eq!(restored.body_scroll, 2);
}

#[test]
fn created_issue_is_inserted_into_matching_repo_issue_section() {
    let pr_section = SectionSnapshot {
        key: "repo:Fiber:pull_requests:Pull Requests".to_string(),
        kind: SectionKind::PullRequests,
        title: "Pull Requests".to_string(),
        filters: "repo:nervosnetwork/fiber is:open archived:false".to_string(),
        items: vec![work_item("pr-1", "nervosnetwork/fiber", 10, "A PR", None)],
        total_count: Some(1),
        page: 1,
        page_size: 50,
        refreshed_at: None,
        error: None,
    };
    let issue_section = SectionSnapshot {
        key: "repo:Fiber:issues:Issues".to_string(),
        kind: SectionKind::Issues,
        title: "Issues".to_string(),
        filters: "repo:nervosnetwork/fiber is:open archived:false".to_string(),
        items: Vec::new(),
        total_count: Some(0),
        page: 1,
        page_size: 50,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(SectionKind::PullRequests, vec![pr_section, issue_section]);
    app.active_view = "repo:Fiber".to_string();
    app.clamp_positions();

    let mut issue = work_item(
        "nervosnetwork/fiber#42",
        "nervosnetwork/fiber",
        42,
        "New issue",
        Some("chenyukang"),
    );
    issue.kind = ItemKind::Issue;
    issue.url = "https://github.com/nervosnetwork/fiber/issues/42".to_string();
    issue.labels = vec!["bug".to_string()];

    assert!(app.insert_created_issue(issue));
    assert_eq!(app.active_view, "repo:Fiber");
    assert_eq!(
        app.current_section().map(|section| section.key.as_str()),
        Some("repo:Fiber:issues:Issues")
    );
    assert_eq!(
        app.current_item().map(|item| item.id.as_str()),
        Some("nervosnetwork/fiber#42")
    );
    assert_eq!(app.focus, FocusTarget::Details);
}

#[test]
fn remove_label_action_opens_confirmation_and_updates_local_item() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);

    app.handle_detail_action(
        DetailAction::RemoveLabel("T-compiler".to_string()),
        None,
        None,
    );
    assert_eq!(
        app.label_dialog.as_ref().map(|dialog| &dialog.mode),
        Some(&LabelDialogMode::Remove {
            label: "T-compiler".to_string()
        })
    );

    let mut pending = None;
    app.handle_label_dialog_key_with_submit(key(KeyCode::Enter), |update| {
        pending = Some(update);
    });
    let pending = pending.expect("pending label update");
    assert_eq!(
        pending.action,
        LabelAction::Remove("T-compiler".to_string())
    );

    app.apply_label_update_local("1", &pending.action);
    assert_eq!(
        app.current_item().map(|item| item.labels.as_slice()),
        Some(&[][..])
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
    assert_eq!(dialog.merge_method, MergeMethod::Merge);
    assert_eq!(app.status, "confirm pull request merge (method: merge)");
}

#[test]
fn merge_confirmation_switches_methods() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_pr_action_dialog(PrAction::Merge);

    app.handle_pr_action_dialog_key_with_submit(key(KeyCode::Char('s')), |_, _, _, _| {
        panic!("method switch should not submit");
    });

    assert_eq!(
        app.pr_action_dialog
            .as_ref()
            .expect("merge dialog")
            .merge_method,
        MergeMethod::Squash
    );
    assert_eq!(app.status, "merge method: squash");

    app.handle_pr_action_dialog_key_with_submit(key(KeyCode::Char('r')), |_, _, _, _| {
        panic!("method switch should not submit");
    });

    assert_eq!(
        app.pr_action_dialog
            .as_ref()
            .expect("merge dialog")
            .merge_method,
        MergeMethod::Rebase
    );
    assert_eq!(app.status, "merge method: rebase");
}

#[test]
fn merge_confirmation_cycles_methods_with_tab() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_pr_action_dialog(PrAction::Merge);

    app.handle_pr_action_dialog_key_with_submit(key(KeyCode::Tab), |_, _, _, _| {
        panic!("method switch should not submit");
    });
    assert_eq!(
        app.pr_action_dialog
            .as_ref()
            .expect("merge dialog")
            .merge_method,
        MergeMethod::Squash
    );

    app.handle_pr_action_dialog_key_with_submit(key(KeyCode::Tab), |_, _, _, _| {
        panic!("method switch should not submit");
    });
    assert_eq!(
        app.pr_action_dialog
            .as_ref()
            .expect("merge dialog")
            .merge_method,
        MergeMethod::Rebase
    );
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
fn capital_c_key_opens_reopen_confirmation_for_closed_pull_request() {
    let mut section = test_section();
    section.items[0].state = Some("closed".to_string());
    let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
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

    let dialog = app.pr_action_dialog.as_ref().expect("reopen dialog");
    assert_eq!(dialog.action, PrAction::Reopen);
    assert_eq!(dialog.item.id, "1");
    assert_eq!(app.status, "confirm pull request reopen");
}

#[test]
fn capital_c_key_rejects_merged_pull_request_reopen() {
    let mut section = test_section();
    section.items[0].state = Some("merged".to_string());
    let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
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

    assert!(app.pr_action_dialog.is_none());
    assert_eq!(app.status, "merged pull requests cannot be reopened");
}

#[test]
fn capital_c_key_opens_issue_close_or_reopen_confirmation() {
    let mut open_issue = work_item("issue-1", "rust-lang/rust", 1, "Open issue", None);
    open_issue.kind = ItemKind::Issue;
    open_issue.state = Some("open".to_string());
    let mut closed_issue = work_item("issue-2", "rust-lang/rust", 2, "Closed issue", None);
    closed_issue.kind = ItemKind::Issue;
    closed_issue.state = Some("CLOSED".to_string());
    let section = SectionSnapshot {
        key: "issues:test".to_string(),
        kind: SectionKind::Issues,
        title: "Test".to_string(),
        filters: String::new(),
        items: vec![open_issue, closed_issue],
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
        key(KeyCode::Char('C')),
        &config,
        &store,
        &tx
    ));
    let dialog = app.pr_action_dialog.as_ref().expect("issue close dialog");
    assert_eq!(dialog.action, PrAction::Close);
    assert_eq!(dialog.item.kind, ItemKind::Issue);
    assert_eq!(app.status, "confirm issue close");

    app.pr_action_dialog = None;
    app.move_selection(1);
    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('C')),
        &config,
        &store,
        &tx
    ));
    let dialog = app.pr_action_dialog.as_ref().expect("issue reopen dialog");
    assert_eq!(dialog.action, PrAction::Reopen);
    assert_eq!(dialog.item.id, "issue-2");
    assert_eq!(app.status, "confirm issue reopen");
}

#[test]
fn capital_a_key_opens_approve_review_summary_for_pull_request_details() {
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

    let dialog = app
        .review_submit_dialog
        .as_ref()
        .expect("approve review dialog");
    assert_eq!(dialog.event, PullRequestReviewEvent::Approve);
    assert_eq!(dialog.item.id, "1");
    assert_eq!(app.status, "review summary: approve");
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
            commits: None,
            failed_check_runs: Vec::new(),
            note: None,
            head: Some(PullRequestBranch {
                repository: "chenyukang/ghr".to_string(),
                branch: "codex/pr-checkout-local".to_string(),
            }),
            ..ActionHints::default()
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
fn capital_f_key_opens_rerun_failed_checks_confirmation() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.action_hints.insert(
        "1".to_string(),
        ActionHintState::Loaded(ActionHints {
            checks: Some(CheckSummary {
                passed: 2,
                failed: 1,
                pending: 0,
                skipped: 0,
                total: 3,
                incomplete: false,
            }),
            failed_check_runs: vec![FailedCheckRunSummary {
                run_id: 123,
                workflow: Some("CI".to_string()),
                checks: vec!["test".to_string()],
            }],
            ..ActionHints::default()
        }),
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('F')),
        &config,
        &store,
        &tx
    ));

    let dialog = app.pr_action_dialog.as_ref().expect("rerun dialog");
    assert_eq!(dialog.action, PrAction::RerunFailedChecks);
    assert_eq!(dialog.item.id, "1");
    assert_eq!(app.status, "confirm failed check rerun");
    assert!(
        dialog
            .summary
            .iter()
            .any(|(_, value)| value.contains("CI #123"))
    );
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
            commits: None,
            failed_check_runs: Vec::new(),
            note: None,
            head: Some(PullRequestBranch {
                repository: "chenyukang/ghr".to_string(),
                branch: "codex/pr-checkout-local".to_string(),
            }),
            ..ActionHints::default()
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
        |item, action, checkout, _merge_method| {
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
fn capital_e_key_opens_enable_auto_merge_confirmation_for_pull_request() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('E')),
        &config,
        &store,
        &tx
    ));

    let dialog = app.pr_action_dialog.as_ref().expect("enable dialog");
    assert_eq!(dialog.action, PrAction::EnableAutoMerge);
    assert_eq!(dialog.item.id, "1");
    assert_eq!(app.status, "confirm pull request auto-merge enable");
}

#[test]
fn capital_o_key_opens_disable_auto_merge_confirmation_for_pull_request_details() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    app.focus_details();

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('O')),
        &config,
        &store,
        &tx
    ));

    let dialog = app.pr_action_dialog.as_ref().expect("disable dialog");
    assert_eq!(dialog.action, PrAction::DisableAutoMerge);
    assert_eq!(dialog.item.id, "1");
    assert_eq!(app.status, "confirm pull request auto-merge disable");
}

#[test]
fn auto_merge_action_dialog_prompt_is_clear() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_pr_action_dialog(PrAction::EnableAutoMerge);
    let backend = ratatui::backend::TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let paths = test_paths();

    terminal
        .draw(|frame| draw(frame, &app, &paths))
        .expect("draw");

    let rendered = buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(rendered.contains("Enable auto-merge for this pull request on GitHub?"));
    assert!(rendered.contains("y/Enter: yes, enable auto-merge for PR"));
}

#[test]
fn capital_u_key_opens_update_branch_confirmation_for_pull_request() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('U')),
        &config,
        &store,
        &tx
    ));

    let dialog = app.pr_action_dialog.as_ref().expect("update branch dialog");
    assert_eq!(dialog.action, PrAction::UpdateBranch);
    assert_eq!(dialog.item.id, "1");
    assert_eq!(app.status, "confirm pull request branch update");
}

#[test]
fn update_branch_dialog_prompt_names_base_branch_update() {
    let mut section = test_section();
    let item = section.items.remove(0);
    let dialog = PrActionDialog {
        item,
        action: PrAction::UpdateBranch,
        checkout: None,
        summary: Vec::new(),
        merge_method: MergeMethod::default(),
    };
    let backend = ratatui::backend::TestBackend::new(100, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    terminal
        .draw(|frame| draw_pr_action_dialog(frame, &dialog, false, frame.area()))
        .expect("draw");

    let rendered = buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(rendered.contains("Update Pull Request Branch"));
    assert!(rendered.contains("Update this pull request branch from its base branch?"));
    assert!(rendered.contains("y/Enter: yes, update PR"));
}

#[test]
fn p_key_opens_request_reviewers_dialog_for_pull_request() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('P')),
        &config,
        &store,
        &tx
    ));

    let dialog = app
        .reviewer_dialog
        .as_ref()
        .expect("request reviewers dialog");
    assert_eq!(dialog.action, ReviewerAction::Request);
    assert_eq!(dialog.item.id, "1");
    assert_eq!(app.status, "enter reviewer logins to request");
}

#[test]
fn y_key_opens_remove_reviewers_dialog_for_pull_request_details() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    app.focus_details();

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('Y')),
        &config,
        &store,
        &tx
    ));

    let dialog = app
        .reviewer_dialog
        .as_ref()
        .expect("remove reviewers dialog");
    assert_eq!(dialog.action, ReviewerAction::Remove);
    assert_eq!(dialog.item.id, "1");
    assert_eq!(app.status, "enter reviewer logins to remove");
}

#[test]
fn reviewer_dialog_submission_parses_comma_separated_logins() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_reviewer_dialog(ReviewerAction::Request);
    app.reviewer_dialog.as_mut().unwrap().input = " alice, bob, Alice ,, ".to_string();
    let mut submitted = None;

    app.handle_reviewer_dialog_key_with_submit(key(KeyCode::Enter), |item, action, reviewers| {
        submitted = Some((item.id, action, reviewers));
    });

    assert!(app.reviewer_action_running);
    assert_eq!(app.status, "requesting review from alice, bob");
    assert_eq!(
        submitted,
        Some((
            "1".to_string(),
            ReviewerAction::Request,
            vec!["alice".to_string(), "bob".to_string()]
        ))
    );
}

#[test]
fn reviewer_dialog_uses_cached_candidates_immediately() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.reviewer_suggestions_cache.insert(
        "rust-lang/rust".to_string(),
        vec!["chenyukang".to_string(), "reviewer-b".to_string()],
    );

    app.start_reviewer_dialog(ReviewerAction::Request);

    let dialog = app.reviewer_dialog.as_ref().expect("reviewer dialog");
    assert!(!dialog.suggestions_loading);
    assert_eq!(dialog.suggestions, vec!["chenyukang", "reviewer-b"]);
}

#[test]
fn reviewer_dialog_falls_back_to_assignee_candidates() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.assignee_suggestions_cache.insert(
        "rust-lang/rust".to_string(),
        vec!["assignable-user".to_string()],
    );

    app.start_reviewer_dialog(ReviewerAction::Request);

    let dialog = app.reviewer_dialog.as_ref().expect("reviewer dialog");
    assert_eq!(dialog.suggestions, vec!["assignable-user"]);
}

#[test]
fn reviewer_dialog_enter_uses_prefix_candidate_for_last_login() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_reviewer_dialog(ReviewerAction::Request);
    let dialog = app.reviewer_dialog.as_mut().expect("reviewer dialog");
    dialog.input = "alice, ch".to_string();
    dialog.suggestions = vec![
        "bob".to_string(),
        "chenyukang".to_string(),
        "chris".to_string(),
    ];
    let mut submitted = None;

    app.handle_reviewer_dialog_key_with_submit(key(KeyCode::Enter), |item, action, reviewers| {
        submitted = Some((item.id, action, reviewers));
    });

    assert_eq!(
        submitted,
        Some((
            "1".to_string(),
            ReviewerAction::Request,
            vec!["alice".to_string(), "chenyukang".to_string()]
        ))
    );
}

#[test]
fn reviewer_dialog_can_cycle_prefix_candidates() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_reviewer_dialog(ReviewerAction::Request);
    let dialog = app.reviewer_dialog.as_mut().expect("reviewer dialog");
    dialog.input = "ch".to_string();
    dialog.suggestions = vec![
        "chenyukang".to_string(),
        "chris".to_string(),
        "reviewer".to_string(),
    ];
    let mut submitted = None;

    app.handle_reviewer_dialog_key_with_submit(key(KeyCode::Down), |_, _, _| {});
    app.handle_reviewer_dialog_key_with_submit(key(KeyCode::Enter), |item, action, reviewers| {
        submitted = Some((item.id, action, reviewers));
    });

    assert_eq!(
        submitted,
        Some((
            "1".to_string(),
            ReviewerAction::Request,
            vec!["chris".to_string()]
        ))
    );
}

#[test]
fn reviewer_suggestions_loaded_updates_cache_without_active_dialog() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);

    app.handle_msg(AppMsg::ReviewerSuggestionsLoaded {
        repo: "rust-lang/rust".to_string(),
        result: Ok(vec!["chenyukang".to_string()]),
    });

    assert!(app.reviewer_dialog.is_none());
    assert_eq!(
        app.reviewer_suggestions_cache.get("rust-lang/rust"),
        Some(&vec!["chenyukang".to_string()])
    );
}

#[test]
fn reviewer_suggestion_refresh_error_keeps_cached_candidates() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.reviewer_suggestions_cache
        .insert("rust-lang/rust".to_string(), vec!["chenyukang".to_string()]);
    app.start_reviewer_dialog(ReviewerAction::Request);
    app.reviewer_dialog.as_mut().unwrap().suggestions_loading = true;

    app.handle_msg(AppMsg::ReviewerSuggestionsLoaded {
        repo: "rust-lang/rust".to_string(),
        result: Err("network failed".to_string()),
    });

    let dialog = app.reviewer_dialog.as_ref().expect("reviewer dialog");
    assert!(!dialog.suggestions_loading);
    assert_eq!(dialog.suggestions, vec!["chenyukang"]);
    assert!(dialog.suggestions_error.is_none());
    assert_eq!(
        app.status,
        "reviewer candidates refresh failed; using cache"
    );
}

#[test]
fn reviewer_dialog_empty_input_stays_open() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_reviewer_dialog(ReviewerAction::Remove);
    let mut submitted = false;

    app.handle_reviewer_dialog_key_with_submit(
        key(KeyCode::Enter),
        |_item, _action, _reviewers| {
            submitted = true;
        },
    );

    assert!(!submitted);
    assert!(!app.reviewer_action_running);
    assert!(app.reviewer_dialog.is_some());
    assert_eq!(app.status, "enter at least one reviewer login");
}

#[test]
fn reviewer_action_rejects_non_pull_request() {
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
        key(KeyCode::Char('P')),
        &config,
        &store,
        &tx
    ));

    assert!(app.reviewer_dialog.is_none());
    assert_eq!(app.status, "selected item is not a pull request");
}

#[test]
fn reviewer_action_finished_refreshes_details_and_hints() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_reviewer_dialog(ReviewerAction::Request);
    app.reviewer_action_running = true;
    app.action_hints
        .insert("1".to_string(), ActionHintState::Loading);

    app.handle_msg(AppMsg::ReviewerActionFinished {
        item_id: "1".to_string(),
        action: ReviewerAction::Request,
        reviewers: vec!["alice".to_string()],
        result: Ok(()),
    });

    assert!(app.reviewer_dialog.is_none());
    assert!(!app.reviewer_action_running);
    assert!(app.details_stale.contains("1"));
    assert!(matches!(
        app.action_hints.get("1"),
        Some(ActionHintState::Loading)
    ));
    assert!(app.action_hints_stale.contains("1"));
    assert_eq!(app.status, "requested review from alice; refreshing");
    let dialog = app.message_dialog.as_ref().expect("success dialog");
    assert_eq!(dialog.title, "Reviewers Requested");
    assert!(dialog.auto_close_at.is_some());
}

#[test]
fn capital_d_key_opens_convert_to_draft_confirmation_for_ready_pr() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('D')),
        &config,
        &store,
        &tx
    ));

    let dialog = app.pr_action_dialog.as_ref().expect("draft dialog");
    assert_eq!(dialog.action, PrAction::ConvertToDraft);
    assert_eq!(dialog.item.id, "1");
    assert_eq!(app.status, "confirm convert pull request to draft");

    let backend = ratatui::backend::TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let paths = test_paths();
    terminal
        .draw(|frame| draw(frame, &app, &paths))
        .expect("draw");
    let rendered = buffer_lines(terminal.backend().buffer()).join("\n");

    assert!(rendered.contains("Convert this pull request to draft on GitHub?"));
    assert!(rendered.contains("y/Enter: yes, convert PR"));
}

#[test]
fn capital_d_key_opens_ready_confirmation_for_draft_pr_details() {
    let mut section = test_section();
    section.items[0].extra = Some("draft".to_string());
    let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    app.focus_details();

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('D')),
        &config,
        &store,
        &tx
    ));

    let dialog = app.pr_action_dialog.as_ref().expect("ready dialog");
    assert_eq!(dialog.action, PrAction::MarkReadyForReview);
    assert_eq!(dialog.item.id, "1");
    assert_eq!(app.status, "confirm mark pull request ready");

    let backend = ratatui::backend::TestBackend::new(110, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let paths = test_paths();
    terminal
        .draw(|frame| draw(frame, &app, &paths))
        .expect("draw");
    let rendered = buffer_lines(terminal.backend().buffer()).join("\n");

    assert!(rendered.contains("Mark this pull request ready for review on GitHub?"));
    assert!(rendered.contains("y/Enter: yes, mark ready PR"));
}

#[test]
fn pr_action_confirmation_submits_selected_action() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_pr_action_dialog(PrAction::Approve);
    let mut submitted = None;

    app.handle_pr_action_dialog_key_with_submit(
        key(KeyCode::Enter),
        |item, action, _checkout, merge_method| {
            submitted = Some((item.id, action, merge_method));
        },
    );

    assert!(app.pr_action_running);
    assert_eq!(app.status, "approving pull request");
    assert_eq!(submitted, Some(("1".to_string(), PrAction::Approve, None)));
}

#[test]
fn merge_confirmation_submits_selected_method() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_pr_action_dialog(PrAction::Merge);
    app.select_merge_method(MergeMethod::Squash);
    let mut submitted = None;

    app.handle_pr_action_dialog_key_with_submit(
        key(KeyCode::Enter),
        |item, action, _checkout, merge_method| {
            submitted = Some((item.id, action, merge_method));
        },
    );

    assert!(app.pr_action_running);
    assert_eq!(app.status, "merging pull request with squash");
    assert_eq!(
        submitted,
        Some(("1".to_string(), PrAction::Merge, Some(MergeMethod::Squash)))
    );
}

#[test]
fn merge_confirmation_submits_default_merge_method() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_pr_action_dialog(PrAction::Merge);
    let mut submitted = None;

    app.handle_pr_action_dialog_key_with_submit(
        key(KeyCode::Enter),
        |item, action, _checkout, merge_method| {
            submitted = Some((item.id, action, merge_method));
        },
    );

    assert!(app.pr_action_running);
    assert_eq!(app.status, "merging pull request with merge");
    assert_eq!(
        submitted,
        Some(("1".to_string(), PrAction::Merge, Some(MergeMethod::Merge)))
    );
}

#[test]
fn update_branch_confirmation_submits_selected_action() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_pr_action_dialog(PrAction::UpdateBranch);
    let mut submitted = None;

    app.handle_pr_action_dialog_key_with_submit(
        key(KeyCode::Char('y')),
        |item, action, _checkout, merge_method| {
            submitted = Some((item.id, action, merge_method));
        },
    );

    assert!(app.pr_action_running);
    assert_eq!(app.status, "updating pull request branch");
    assert_eq!(
        submitted,
        Some(("1".to_string(), PrAction::UpdateBranch, None))
    );
}

#[test]
fn draft_ready_confirmation_submits_selected_action() {
    let mut section = test_section();
    section.items[0].extra = Some("draft".to_string());
    let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
    app.start_pr_draft_ready_dialog();
    let mut submitted = None;

    app.handle_pr_action_dialog_key_with_submit(
        key(KeyCode::Enter),
        |item, action, _checkout, _merge_method| {
            submitted = Some((item.id, action));
        },
    );

    assert!(app.pr_action_running);
    assert_eq!(app.status, "marking pull request ready for review");
    assert_eq!(
        submitted,
        Some(("1".to_string(), PrAction::MarkReadyForReview))
    );
}

#[test]
fn issue_state_action_confirmation_submits_selected_action() {
    let mut item = work_item("issue-1", "rust-lang/rust", 1, "Compiler diagnostics", None);
    item.kind = ItemKind::Issue;
    item.state = Some("closed".to_string());
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
    app.start_close_or_reopen_dialog();
    let mut submitted = None;

    app.handle_pr_action_dialog_key_with_submit(
        key(KeyCode::Enter),
        |item, action, _checkout, _merge_method| {
            submitted = Some((item.id, item.kind, action));
        },
    );

    assert!(app.pr_action_running);
    assert_eq!(app.status, "reopening issue");
    assert_eq!(
        submitted,
        Some(("issue-1".to_string(), ItemKind::Issue, PrAction::Reopen))
    );
}

#[test]
fn pr_action_dialog_escape_cancels() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_pr_action_dialog(PrAction::Merge);

    app.handle_pr_action_dialog_key_with_submit(
        key(KeyCode::Esc),
        |_item, _action, _checkout, _merge_method| {
            panic!("escape should not submit the action");
        },
    );

    assert!(app.pr_action_dialog.is_none());
    assert!(!app.pr_action_running);
    assert_eq!(app.status, "item action cancelled");
}

#[test]
fn rerun_failed_checks_rejects_pr_without_failed_checks() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.action_hints.insert(
        "1".to_string(),
        ActionHintState::Loaded(ActionHints {
            checks: Some(CheckSummary {
                passed: 3,
                failed: 0,
                pending: 0,
                skipped: 0,
                total: 3,
                incomplete: false,
            }),
            ..ActionHints::default()
        }),
    );

    app.start_pr_action_dialog(PrAction::RerunFailedChecks);

    assert!(app.pr_action_dialog.is_none());
    assert_eq!(app.status, "no failed checks to rerun");
    let dialog = app.message_dialog.as_ref().expect("message dialog");
    assert_eq!(dialog.title, "No Failed Checks");
}

#[test]
fn rerun_failed_checks_confirmation_submits_action() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.action_hints.insert(
        "1".to_string(),
        ActionHintState::Loaded(ActionHints {
            checks: Some(CheckSummary {
                passed: 0,
                failed: 1,
                pending: 0,
                skipped: 0,
                total: 1,
                incomplete: false,
            }),
            ..ActionHints::default()
        }),
    );
    app.start_pr_action_dialog(PrAction::RerunFailedChecks);
    let mut submitted = None;

    app.handle_pr_action_dialog_key_with_submit(
        key(KeyCode::Char('y')),
        |item, action, _checkout, _merge_method| {
            submitted = Some((item.id, action));
        },
    );

    assert!(app.pr_action_running);
    assert_eq!(app.status, "rerunning failed checks");
    assert_eq!(
        submitted,
        Some(("1".to_string(), PrAction::RerunFailedChecks))
    );
}

#[test]
fn review_submit_dialog_selects_event_and_edits_summary() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_review_submit_dialog(PullRequestReviewEvent::Comment);

    app.handle_review_submit_dialog_key_with_submit(
        key(KeyCode::Char('2')),
        None,
        |_pending| panic!("event selection should not submit"),
        |_item, _body| panic!("event selection should not create a draft"),
    );
    app.handle_review_submit_dialog_key_with_submit(
        key(KeyCode::Char('o')),
        None,
        |_pending| panic!("typing should not submit"),
        |_item, _body| panic!("typing should not create a draft"),
    );
    app.handle_review_submit_dialog_key_with_submit(
        key(KeyCode::Char('k')),
        None,
        |_pending| panic!("typing should not submit"),
        |_item, _body| panic!("typing should not create a draft"),
    );

    let dialog = app.review_submit_dialog.as_ref().expect("review dialog");
    assert_eq!(dialog.event, PullRequestReviewEvent::RequestChanges);
    assert_eq!(dialog.body, "ok");
    assert_eq!(app.status, "review event: request changes");
}

#[test]
fn review_submit_dialog_submits_selected_event() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_review_submit_dialog(PullRequestReviewEvent::Comment);
    app.review_submit_dialog
        .as_mut()
        .unwrap()
        .body
        .set_text("looks good overall");
    let mut submitted = None;

    app.handle_review_submit_dialog_key_with_submit(
        key(KeyCode::Char('3')),
        None,
        |pending| submitted = Some((pending.item.id, pending.event, pending.body, pending.mode)),
        |_item, _body| panic!("submit should not create a draft"),
    );
    app.handle_review_submit_dialog_key_with_submit(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
        None,
        |pending| submitted = Some((pending.item.id, pending.event, pending.body, pending.mode)),
        |_item, _body| panic!("submit should not create a draft"),
    );

    assert!(app.review_submit_dialog.is_none());
    assert!(app.review_submit_running);
    assert_eq!(app.status, "submitting review: approve");
    assert_eq!(
        submitted,
        Some((
            "1".to_string(),
            PullRequestReviewEvent::Approve,
            "looks good overall".to_string(),
            ReviewSubmitMode::New
        ))
    );
}

#[test]
fn approve_shortcut_submits_empty_approve_review() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('A')),
        &config,
        &store,
        &tx
    ));
    let mut submitted = None;
    app.handle_review_submit_dialog_key_with_submit(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
        None,
        |pending| submitted = Some((pending.item.id, pending.event, pending.body)),
        |_item, _body| panic!("approve should submit, not create a draft"),
    );

    assert_eq!(
        submitted,
        Some((
            "1".to_string(),
            PullRequestReviewEvent::Approve,
            String::new()
        ))
    );
}

#[test]
fn pending_review_submit_and_discard_use_local_pending_review() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.pending_reviews.insert(
        "1".to_string(),
        PendingReviewState {
            review_id: 77,
            body: "draft summary".to_string(),
        },
    );

    app.start_review_submit_dialog(PullRequestReviewEvent::Comment);
    let dialog = app.review_submit_dialog.as_ref().expect("pending dialog");
    assert_eq!(dialog.body, "draft summary");
    assert_eq!(dialog.mode, ReviewSubmitMode::Pending { review_id: 77 });
    app.review_submit_dialog.as_mut().unwrap().event = PullRequestReviewEvent::RequestChanges;
    let mut submitted = None;
    app.handle_review_submit_dialog_key_with_submit(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
        None,
        |pending| submitted = Some((pending.item.id, pending.event, pending.body, pending.mode)),
        |_item, _body| panic!("submit should not create a draft"),
    );

    assert_eq!(
        submitted,
        Some((
            "1".to_string(),
            PullRequestReviewEvent::RequestChanges,
            "draft summary".to_string(),
            ReviewSubmitMode::Pending { review_id: 77 }
        ))
    );

    app.review_submit_running = false;
    app.review_submit_dialog = None;
    let mut discarded = None;
    app.discard_pending_review_with_submit(|item, review_id| {
        discarded = Some((item.id, review_id));
    });

    assert!(app.review_submit_running);
    assert_eq!(app.status, "discarding pending review");
    assert_eq!(discarded, Some(("1".to_string(), 77)));
}

#[test]
fn ctrl_p_creates_pending_review_draft_from_summary() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_review_submit_dialog(PullRequestReviewEvent::Comment);
    app.review_submit_dialog
        .as_mut()
        .unwrap()
        .body
        .set_text("hold these notes");
    let mut created = None;

    app.handle_review_submit_dialog_key_with_submit(
        KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
        None,
        |_pending| panic!("draft create should not submit a review"),
        |item, body| created = Some((item.id, body)),
    );

    assert!(app.review_submit_dialog.is_none());
    assert!(app.review_submit_running);
    assert_eq!(app.status, "creating pending review");
    assert_eq!(
        created,
        Some(("1".to_string(), "hold these notes".to_string()))
    );
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
fn auto_merge_action_rejects_non_pull_request() {
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
        key(KeyCode::Char('E')),
        &config,
        &store,
        &tx
    ));

    assert!(app.pr_action_dialog.is_none());
    assert_eq!(app.status, "selected item is not a pull request");
}

#[test]
fn item_edit_key_opens_form_from_list_and_details() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('e')),
        &config,
        &store,
        &tx
    ));

    assert!(app.item_edit_dialog.is_some());
    assert_eq!(app.status, "editing item");

    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.focus_details();

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('e')),
        &config,
        &store,
        &tx
    ));

    assert!(app.item_edit_dialog.is_some());
    assert_eq!(app.status, "editing item");
}

#[test]
fn item_edit_form_prefills_editable_fields_without_repo_or_number() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);

    app.start_item_edit_dialog();

    let dialog = app.item_edit_dialog.as_ref().expect("item edit dialog");
    assert_eq!(dialog.title.text(), "Compiler diagnostics");
    assert_eq!(dialog.body.text(), "A body with useful context");
    assert_eq!(dialog.labels, vec!["T-compiler".to_string()]);
    assert_eq!(dialog.assignees, Vec::<String>::new());

    let backend = ratatui::backend::TestBackend::new(100, 32);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let paths = test_paths();
    terminal.draw(|frame| draw(frame, &app, &paths)).unwrap();
    let rendered = buffer_lines(terminal.backend().buffer()).join("\n");
    let title_index = rendered.find("Title:").expect("title field");
    let assign_index = rendered.find("Assign:").expect("assign field");
    let labels_index = rendered.find("Labels:").expect("labels field");
    let body_index = rendered.find("Body:").expect("body field");
    assert!(title_index < assign_index);
    assert!(assign_index < labels_index);
    assert!(labels_index < body_index);
    assert!(!rendered.contains("Assignees:"));
    assert!(!rendered.contains("Assignee:"));
    assert!(!rendered.contains("Label:"));
    assert!(!rendered.contains("repo:"));
    assert!(!rendered.contains("pull request:"));
    assert!(!rendered.contains("item: #"));
}

#[test]
fn item_edit_cursor_positions_match_rendered_field_rows() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_item_edit_dialog();
    let area = Rect::new(0, 0, 120, 34);
    let dialog_area = item_edit_dialog_area(area);
    let inner = block_inner(dialog_area);
    let editor_width = inner.width.max(1);
    let editor_height = item_edit_body_editor_height(dialog_area, ItemEditField::Labels);

    app.item_edit_dialog.as_mut().expect("dialog").field = ItemEditField::Labels;
    app.item_edit_dialog.as_mut().expect("dialog").label_input = "bu".to_string();

    let backend = ratatui::backend::TestBackend::new(area.width, area.height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let paths = test_paths();
    terminal.draw(|frame| draw(frame, &app, &paths)).unwrap();
    let lines = buffer_lines(terminal.backend().buffer());
    let labels_y = lines
        .iter()
        .position(|line| line.contains("Labels:"))
        .expect("labels line") as u16;
    let labels_cursor = item_edit_dialog_cursor_position(
        app.item_edit_dialog.as_ref().expect("dialog"),
        0,
        dialog_area,
        editor_width,
        editor_height,
    )
    .expect("labels cursor");
    assert_eq!(labels_cursor.y, labels_y);

    let dialog = app.item_edit_dialog.as_mut().expect("dialog");
    dialog.field = ItemEditField::Body;
    dialog.body.set_text("first\nsecond");
    dialog.body.set_cursor_byte("first\nse".len());

    let backend = ratatui::backend::TestBackend::new(area.width, area.height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| draw(frame, &app, &paths)).unwrap();
    let lines = buffer_lines(terminal.backend().buffer());
    let body_first_y = lines
        .iter()
        .position(|line| line.contains("first"))
        .expect("body first line") as u16;
    let body_cursor = item_edit_dialog_cursor_position(
        app.item_edit_dialog.as_ref().expect("dialog"),
        0,
        dialog_area,
        editor_width,
        item_edit_body_editor_height(dialog_area, ItemEditField::Body),
    )
    .expect("body cursor");
    assert_eq!(body_cursor.y, body_first_y + 1);
}

#[test]
fn mouse_dragging_item_edit_body_copies_selection() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.sections[0].items[0].body = Some("alpha beta gamma\nsecond line".to_string());
    app.start_item_edit_dialog();
    let area = Rect::new(0, 0, 120, 34);
    let dialog = app.item_edit_dialog.as_ref().unwrap().clone();
    let dialog_area = item_edit_dialog_area(area);
    let inner = block_inner(dialog_area);
    let layout = item_edit_layout_rows(dialog.field);
    let body_lines = comment_dialog_body_lines(dialog.body.text(), inner.width.max(1));
    let row = inner.y + layout.body_text;
    let end_column = display_width("alpha beta").min(usize::from(u16::MAX)) as u16;

    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: inner.x,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
        area,
    );
    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: inner.x + end_column,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
        area,
    );

    let selection = app
        .dialog_text_selection
        .clone()
        .expect("active item edit body selection");
    assert_eq!(selected_dialog_text(&body_lines, &selection), "alpha beta");
    assert_eq!(app.status, "selecting item body text; release to copy");

    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: inner.x + end_column,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
        area,
    );

    assert_eq!(app.status, "copied selected item body text");
    assert_eq!(
        selected_dialog_text(
            &body_lines,
            app.dialog_text_selection
                .as_ref()
                .expect("selection remains visible")
        ),
        "alpha beta"
    );
}

#[test]
fn mouse_releasing_item_edit_body_selection_outside_dialog_copies_selection() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.sections[0].items[0].body = Some("alpha beta gamma\nsecond line".to_string());
    app.start_item_edit_dialog();
    let area = Rect::new(0, 0, 120, 34);
    let dialog = app.item_edit_dialog.as_ref().unwrap().clone();
    let dialog_area = item_edit_dialog_area(area);
    let inner = block_inner(dialog_area);
    let layout = item_edit_layout_rows(dialog.field);
    let body_lines = comment_dialog_body_lines(dialog.body.text(), inner.width.max(1));
    let row = inner.y + layout.body_text;
    let end_column = display_width("alpha beta").min(usize::from(u16::MAX)) as u16;

    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: inner.x,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
        area,
    );
    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: inner.x + end_column,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
        area,
    );
    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
        area,
    );

    assert_eq!(app.status, "copied selected item body text");
    assert_eq!(
        selected_dialog_text(
            &body_lines,
            app.dialog_text_selection
                .as_ref()
                .expect("selection remains visible")
        ),
        "alpha beta"
    );
}

#[test]
fn item_edit_candidate_lists_only_show_for_focused_collection_field() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_item_edit_dialog();
    let area = Rect::new(0, 0, 120, 34);
    let paths = test_paths();

    let render = |app: &AppState| {
        let backend = ratatui::backend::TestBackend::new(area.width, area.height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal.draw(|frame| draw(frame, app, &paths)).unwrap();
        buffer_lines(terminal.backend().buffer()).join("\n")
    };

    {
        let dialog = app.item_edit_dialog.as_mut().expect("dialog");
        dialog.assignee_suggestions = vec!["alice".to_string()];
        dialog.label_suggestions = vec!["bug".to_string()];
    }

    let rendered = render(&app);
    assert!(!rendered.contains("Assignee candidates"));
    assert!(!rendered.contains("Label candidates"));

    app.item_edit_dialog.as_mut().expect("dialog").field = ItemEditField::Assignees;
    let rendered = render(&app);
    assert!(rendered.contains("Assignee candidates"));
    assert!(!rendered.contains("Label candidates"));

    app.item_edit_dialog.as_mut().expect("dialog").field = ItemEditField::Labels;
    let rendered = render(&app);
    assert!(!rendered.contains("Assignee candidates"));
    assert!(rendered.contains("Label candidates"));
}

#[test]
fn item_edit_assignee_and_label_fields_use_candidates() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_item_edit_dialog();
    let dialog = app.item_edit_dialog.as_mut().expect("dialog");
    dialog.field = ItemEditField::Assignees;
    dialog.assignee_suggestions = vec!["alice".to_string(), "bob".to_string()];
    dialog.assignee_input = "bo".to_string();

    app.handle_item_edit_dialog_key_with_submit(key(KeyCode::Enter), None, |_| {});

    let dialog = app.item_edit_dialog.as_mut().expect("dialog");
    assert_eq!(dialog.assignees, vec!["bob".to_string()]);
    assert!(dialog.assignee_input.is_empty());
    dialog.assignee_input = "bob".to_string();

    app.handle_item_edit_dialog_key_with_submit(key(KeyCode::Enter), None, |_| {});

    let dialog = app.item_edit_dialog.as_mut().expect("dialog");
    assert!(dialog.assignees.is_empty());
    dialog.assignee_input = "alice bo".to_string();

    app.handle_item_edit_dialog_key_with_submit(key(KeyCode::Enter), None, |_| {});

    let dialog = app.item_edit_dialog.as_mut().expect("dialog");
    assert_eq!(
        dialog.assignees,
        vec!["alice".to_string(), "bob".to_string()]
    );
    dialog.field = ItemEditField::Labels;
    dialog.label_suggestions = vec!["bug".to_string(), "T-compiler".to_string()];
    dialog.label_input = "bu".to_string();

    app.handle_item_edit_dialog_key_with_submit(key(KeyCode::Enter), None, |_| {});

    let dialog = app.item_edit_dialog.as_mut().expect("dialog");
    assert_eq!(
        dialog.labels,
        vec!["T-compiler".to_string(), "bug".to_string()]
    );
    assert!(dialog.label_input.is_empty());
    dialog.label_input = "T-compiler".to_string();

    app.handle_item_edit_dialog_key_with_submit(key(KeyCode::Enter), None, |_| {});

    assert_eq!(
        app.item_edit_dialog.as_ref().unwrap().labels,
        vec!["bug".to_string()]
    );
    app.item_edit_dialog.as_mut().unwrap().label_input = "help wanted, T-".to_string();

    app.handle_item_edit_dialog_key_with_submit(key(KeyCode::Enter), None, |_| {});

    assert_eq!(
        app.item_edit_dialog.as_ref().unwrap().labels,
        vec![
            "bug".to_string(),
            "help wanted".to_string(),
            "T-compiler".to_string()
        ]
    );
}

#[test]
fn item_edit_submit_sends_all_editable_fields() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_item_edit_dialog();
    let dialog = app.item_edit_dialog.as_mut().expect("dialog");
    dialog.title.set_text("New title");
    dialog.body.clear();
    dialog.assignees = vec!["alice".to_string(), "bob".to_string()];
    dialog.labels = vec!["bug".to_string(), "T-compiler".to_string()];
    let mut submitted = None;

    app.handle_item_edit_dialog_key_with_submit(
        KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::CONTROL),
        None,
        |pending| {
            submitted = Some((
                pending.item.id,
                pending.title,
                pending.body,
                pending.assignees,
                pending.labels,
            ))
        },
    );

    assert!(app.item_edit_running);
    assert_eq!(app.status, "updating item");
    assert_eq!(
        submitted,
        Some((
            "1".to_string(),
            "New title".to_string(),
            String::new(),
            vec!["alice".to_string(), "bob".to_string()],
            vec!["bug".to_string(), "T-compiler".to_string()],
        ))
    );
}

#[test]
fn item_edit_update_success_refreshes_cached_item() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.item_edit_running = true;
    app.start_item_edit_dialog();

    app.handle_msg(AppMsg::ItemEdited {
        item_id: "1".to_string(),
        result: Ok(ItemEditUpdate {
            title: "New title".to_string(),
            body: Some("New body".to_string()),
            labels: vec!["bug".to_string()],
            assignees: vec!["alice".to_string()],
            updated_at: None,
        }),
    });

    assert!(!app.item_edit_running);
    assert!(app.item_edit_dialog.is_none());
    assert_eq!(app.sections[0].items[0].title, "New title");
    assert_eq!(app.sections[0].items[0].body.as_deref(), Some("New body"));
    assert_eq!(app.sections[0].items[0].labels, vec!["bug".to_string()]);
    assert_eq!(
        app.sections[0].items[0].assignees,
        vec!["alice".to_string()]
    );
    assert!(app.details_stale.contains("1"));
    assert_eq!(app.status, "item updated");
    assert_eq!(
        app.message_dialog
            .as_ref()
            .map(|dialog| (dialog.title.as_str(), dialog.body.as_str())),
        Some(("Item Updated", "GitHub accepted the item update."))
    );
}

#[test]
fn item_edit_update_failure_reports_status() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.item_edit_running = true;

    app.handle_msg(AppMsg::ItemEdited {
        item_id: "1".to_string(),
        result: Err("gh api repos/owner/repo/issues/1 failed: validation failed".to_string()),
    });

    assert!(!app.item_edit_running);
    assert_eq!(app.status, "item update failed");
    let dialog = app.message_dialog.as_ref().expect("failure dialog");
    assert_eq!(dialog.title, "Item Update Failed");
    assert_eq!(dialog.kind, MessageDialogKind::Error);
    assert!(dialog.body.contains("validation failed"));
    assert!(dialog.auto_close_at.is_none());
}

#[test]
fn item_edit_rejects_non_issue_or_pull_request_items() {
    let section = SectionSnapshot {
        key: "notifications:test".to_string(),
        kind: SectionKind::Notifications,
        title: "Test".to_string(),
        filters: String::new(),
        items: vec![WorkItem {
            id: "thread-1".to_string(),
            kind: ItemKind::Notification,
            repo: "rust-lang/rust".to_string(),
            number: None,
            title: "Notification only".to_string(),
            body: None,
            author: None,
            state: None,
            url: "https://github.com/rust-lang/rust".to_string(),
            created_at: None,
            updated_at: None,
            last_read_at: None,
            labels: Vec::new(),
            reactions: ReactionSummary::default(),
            milestone: None,
            assignees: Vec::new(),
            comments: None,
            unread: Some(true),
            reason: Some("mention".to_string()),
            extra: Some("Commit".to_string()),
            viewer_subscription: None,
        }],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(SectionKind::Notifications, vec![section]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('T')),
        &config,
        &store,
        &tx
    ));

    assert!(app.item_edit_dialog.is_none());
    assert_eq!(app.status, "selected item is not an issue or pull request");
}

#[test]
fn update_branch_rejects_closed_pull_request() {
    let mut section = test_section();
    section.items[0].state = Some("closed".to_string());
    let mut app = AppState::new(SectionKind::PullRequests, vec![section]);

    app.start_pr_action_dialog(PrAction::UpdateBranch);

    assert!(app.pr_action_dialog.is_none());
    assert_eq!(app.status, "selected pull request is not open");
}

#[test]
fn draft_ready_action_rejects_closed_pull_request() {
    let mut section = test_section();
    section.items[0].state = Some("closed".to_string());
    let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('D')),
        &config,
        &store,
        &tx
    ));

    assert!(app.pr_action_dialog.is_none());
    assert_eq!(app.status, "selected pull request is not open");
}

#[test]
fn pr_action_finished_marks_item_state_and_closes_dialog() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_pr_action_dialog(PrAction::Merge);
    app.pr_action_running = true;

    app.handle_msg(AppMsg::PrActionFinished {
        item_id: "1".to_string(),
        item_kind: ItemKind::PullRequest,
        action: PrAction::Merge,
        merge_method: Some(MergeMethod::Merge),
        result: Ok(()),
    });

    assert!(app.pr_action_dialog.is_none());
    assert!(!app.pr_action_running);
    assert_eq!(app.sections[0].items[0].state.as_deref(), Some("merged"));
    assert!(app.details_stale.contains("1"));
    assert_eq!(app.status, "pull request merged using merge; refreshing");
    let dialog = app.message_dialog.as_ref().expect("success dialog");
    assert_eq!(dialog.title, "Pull Request Merged");
    assert_eq!(dialog.kind, MessageDialogKind::Success);
    assert_eq!(message_dialog_accent(dialog), Color::LightGreen);
    assert!(dialog.auto_close_at.is_some());
}

#[test]
fn reopen_action_finished_marks_item_open_and_refreshes_details() {
    let mut section = test_section();
    section.items[0].state = Some("closed".to_string());
    let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
    app.start_close_or_reopen_dialog();
    app.pr_action_running = true;

    app.handle_msg(AppMsg::PrActionFinished {
        item_id: "1".to_string(),
        item_kind: ItemKind::PullRequest,
        action: PrAction::Reopen,
        merge_method: None,
        result: Ok(()),
    });

    assert!(app.pr_action_dialog.is_none());
    assert!(!app.pr_action_running);
    assert_eq!(app.sections[0].items[0].state.as_deref(), Some("open"));
    assert!(app.details_stale.contains("1"));
    assert_eq!(app.status, "pull request reopened; refreshing");
    let dialog = app.message_dialog.as_ref().expect("success dialog");
    assert_eq!(dialog.title, "Pull Request Reopened");
    assert!(dialog.auto_close_at.is_some());
}

#[test]
fn approve_action_finished_keeps_item_open_and_refreshes_details() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_pr_action_dialog(PrAction::Approve);
    app.pr_action_running = true;

    app.handle_msg(AppMsg::PrActionFinished {
        item_id: "1".to_string(),
        item_kind: ItemKind::PullRequest,
        action: PrAction::Approve,
        merge_method: None,
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
fn auto_merge_action_finished_refreshes_details_and_action_hints() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.action_hints.insert(
        "1".to_string(),
        ActionHintState::Loaded(ActionHints {
            labels: vec!["Auto-mergeable".to_string()],
            checks: None,
            note: None,
            ..ActionHints::default()
        }),
    );
    app.start_pr_action_dialog(PrAction::EnableAutoMerge);
    app.pr_action_running = true;

    app.handle_msg(AppMsg::PrActionFinished {
        item_id: "1".to_string(),
        item_kind: ItemKind::PullRequest,
        action: PrAction::EnableAutoMerge,
        merge_method: None,
        result: Ok(()),
    });

    assert!(app.pr_action_dialog.is_none());
    assert!(!app.pr_action_running);
    assert_eq!(app.sections[0].items[0].state.as_deref(), Some("open"));
    assert!(app.details_stale.contains("1"));
    assert!(matches!(
        app.action_hints.get("1"),
        Some(ActionHintState::Loaded(_))
    ));
    assert!(app.action_hints_stale.contains("1"));
    assert_eq!(app.status, "pull request auto-merge enabled; refreshing");
    let dialog = app.message_dialog.as_ref().expect("success dialog");
    assert_eq!(dialog.title, "Auto-Merge Enabled");
    assert!(dialog.auto_close_at.is_some());
}

#[test]
fn update_branch_finished_keeps_item_open_and_refreshes_details_and_hints() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.action_hints.insert(
        "1".to_string(),
        ActionHintState::Loaded(ActionHints {
            labels: vec!["Update branch".to_string()],
            checks: None,
            note: None,
            ..ActionHints::default()
        }),
    );
    app.diffs.insert(
        "1".to_string(),
        DiffState::Loaded(PullRequestDiff {
            files: Vec::new(),
            additions: 0,
            deletions: 0,
        }),
    );
    app.start_pr_action_dialog(PrAction::UpdateBranch);
    app.pr_action_running = true;

    app.handle_msg(AppMsg::PrActionFinished {
        item_id: "1".to_string(),
        item_kind: ItemKind::PullRequest,
        action: PrAction::UpdateBranch,
        merge_method: None,
        result: Ok(()),
    });

    assert!(app.pr_action_dialog.is_none());
    assert!(!app.pr_action_running);
    assert_eq!(app.sections[0].items[0].state.as_deref(), Some("open"));
    assert!(app.details_stale.contains("1"));
    assert!(matches!(
        app.action_hints.get("1"),
        Some(ActionHintState::Loaded(_))
    ));
    assert!(app.action_hints_stale.contains("1"));
    assert!(!app.diffs.contains_key("1"));
    assert_eq!(
        app.status,
        "pull request branch update accepted; refreshing"
    );
    let dialog = app.message_dialog.as_ref().expect("success dialog");
    assert_eq!(dialog.title, "Pull Request Branch Updated");
    assert!(dialog.auto_close_at.is_some());
}

#[test]
fn auto_merge_action_failure_reports_action_specific_status() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_pr_action_dialog(PrAction::DisableAutoMerge);
    app.pr_action_running = true;

    app.handle_msg(AppMsg::PrActionFinished {
        item_id: "1".to_string(),
        item_kind: ItemKind::PullRequest,
        action: PrAction::DisableAutoMerge,
        merge_method: None,
        result: Err("auto-merge is already disabled for owner/repo#1".to_string()),
    });

    assert!(app.pr_action_dialog.is_none());
    assert!(!app.pr_action_running);
    assert_eq!(app.status, "pull request auto-merge disable failed");
    let dialog = app.message_dialog.as_ref().expect("message dialog");
    assert_eq!(dialog.title, "Disable Auto-Merge Failed");
    assert!(dialog.body.contains("auto-merge is already disabled"));
    assert!(dialog.auto_close_at.is_none());
}

#[test]
fn draft_ready_action_finished_updates_item_extra_and_refreshes_action_hints() {
    let mut section = test_section();
    section.items[0].extra = Some("draft".to_string());
    let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
    app.action_hints.insert(
        "1".to_string(),
        ActionHintState::Loaded(ActionHints {
            labels: vec!["Draft".to_string()],
            checks: None,
            note: Some("Merge blocked: draft".to_string()),
            ..ActionHints::default()
        }),
    );
    app.start_pr_draft_ready_dialog();
    app.pr_action_running = true;

    app.handle_msg(AppMsg::PrActionFinished {
        item_id: "1".to_string(),
        item_kind: ItemKind::PullRequest,
        action: PrAction::MarkReadyForReview,
        merge_method: None,
        result: Ok(()),
    });

    assert!(app.pr_action_dialog.is_none());
    assert!(!app.pr_action_running);
    assert_eq!(app.sections[0].items[0].state.as_deref(), Some("open"));
    assert_eq!(app.sections[0].items[0].extra, None);
    assert!(matches!(
        app.action_hints.get("1"),
        Some(ActionHintState::Loaded(_))
    ));
    assert!(app.action_hints_stale.contains("1"));
    assert!(app.details_stale.contains("1"));
    assert_eq!(app.status, "pull request marked ready; refreshing");
    let dialog = app.message_dialog.as_ref().expect("success dialog");
    assert_eq!(dialog.title, "Pull Request Ready for Review");
    assert!(dialog.auto_close_at.is_some());

    app.handle_msg(AppMsg::PrActionFinished {
        item_id: "1".to_string(),
        item_kind: ItemKind::PullRequest,
        action: PrAction::ConvertToDraft,
        merge_method: None,
        result: Ok(()),
    });

    assert_eq!(app.sections[0].items[0].extra.as_deref(), Some("draft"));
    assert_eq!(app.status, "pull request converted to draft; refreshing");
}

#[test]
fn pr_action_failure_opens_message_dialog() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_pr_action_dialog(PrAction::Merge);
    app.pr_action_running = true;

    app.handle_msg(AppMsg::PrActionFinished {
        item_id: "1".to_string(),
        item_kind: ItemKind::PullRequest,
        action: PrAction::Merge,
        merge_method: Some(MergeMethod::Squash),
        result: Err(
            "merge blocked for owner/repo#1: review approval required; 1 check(s) failing"
                .to_string(),
        ),
    });

    assert!(app.pr_action_dialog.is_none());
    assert!(!app.pr_action_running);
    assert_eq!(app.status, "pull request squash merge failed");
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
fn milestone_prefix_matching_is_case_insensitive() {
    let milestone = Milestone {
        number: 1,
        title: "Release Train".to_string(),
    };

    assert!(milestone_matches_prefix(&milestone, "rel"));
    assert!(milestone_matches_prefix(&milestone, "RELEASE"));
    assert!(!milestone_matches_prefix(&milestone, "train"));
}

#[test]
fn t_key_opens_milestone_dialog_for_issue() {
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
        key(KeyCode::Char('t')),
        &config,
        &store,
        &tx
    ));

    let dialog = app.milestone_dialog.as_ref().expect("milestone dialog");
    assert_eq!(dialog.item.id, "1");
    assert!(matches!(dialog.state, MilestoneDialogState::Loading));
    assert_eq!(app.status, "loading milestones");
}

#[test]
fn milestone_dialog_filters_by_prefix_and_submits_selection() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    app.start_milestone_dialog(&tx);
    app.handle_msg(AppMsg::MilestonesLoaded {
        item_id: "1".to_string(),
        result: Ok(vec![
            Milestone {
                number: 1,
                title: "alpha".to_string(),
            },
            Milestone {
                number: 2,
                title: "beta".to_string(),
            },
        ]),
    });

    let mut submitted = None;
    app.handle_milestone_dialog_key_with_submit(key(KeyCode::Char('b')), |item, milestone| {
        submitted = Some((item.id, milestone));
    });
    app.handle_milestone_dialog_key_with_submit(key(KeyCode::Enter), |item, milestone| {
        submitted = Some((item.id, milestone));
    });

    assert!(app.milestone_action_running);
    assert_eq!(app.status, "changing milestone");
    assert_eq!(
        submitted,
        Some((
            "1".to_string(),
            MilestoneChoice::Set(Milestone {
                number: 2,
                title: "beta".to_string(),
            })
        ))
    );
}

#[test]
fn milestone_dialog_can_submit_clear_choice() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    app.start_milestone_dialog(&tx);
    app.handle_msg(AppMsg::MilestonesLoaded {
        item_id: "1".to_string(),
        result: Ok(vec![Milestone {
            number: 1,
            title: "alpha".to_string(),
        }]),
    });
    let mut submitted = None;

    app.handle_milestone_dialog_key_with_submit(key(KeyCode::Enter), |item, milestone| {
        submitted = Some((item.id, milestone));
    });

    assert_eq!(submitted, Some(("1".to_string(), MilestoneChoice::Clear)));
    assert_eq!(app.status, "clearing milestone");
}

#[test]
fn milestone_dialog_can_create_missing_prefix_milestone() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    app.start_milestone_dialog(&tx);
    app.handle_msg(AppMsg::MilestonesLoaded {
        item_id: "1".to_string(),
        result: Ok(vec![Milestone {
            number: 1,
            title: "alpha".to_string(),
        }]),
    });

    let mut submitted = None;
    for value in "next-release".chars() {
        app.handle_milestone_dialog_key_with_submit(key(KeyCode::Char(value)), |item, choice| {
            submitted = Some((item.id, choice));
        });
    }
    let choices = milestone_choices(app.milestone_dialog.as_ref().expect("milestone dialog"));
    assert_eq!(
        choices,
        vec![
            MilestoneChoice::Clear,
            MilestoneChoice::Create("next-release".to_string())
        ]
    );

    app.handle_milestone_dialog_key_with_submit(key(KeyCode::Enter), |item, choice| {
        submitted = Some((item.id, choice));
    });

    assert_eq!(
        submitted,
        Some((
            "1".to_string(),
            MilestoneChoice::Create("next-release".to_string())
        ))
    );
    assert!(app.milestone_action_running);
    assert_eq!(app.status, "creating milestone");
}

#[test]
fn milestone_action_rejects_non_issue_items() {
    let section = SectionSnapshot {
        key: "notifications:test".to_string(),
        kind: SectionKind::Notifications,
        title: "Test".to_string(),
        filters: String::new(),
        items: vec![WorkItem {
            kind: ItemKind::Notification,
            ..notification_item("thread-1", true)
        }],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    };
    let mut app = AppState::new(SectionKind::Notifications, vec![section]);
    let (tx, _rx) = mpsc::unbounded_channel();

    app.start_milestone_dialog(&tx);

    assert!(app.milestone_dialog.is_none());
    assert_eq!(app.status, "selected item is not an issue or pull request");
}

#[test]
fn milestone_changed_marks_item_and_details_stale() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.milestone_dialog = Some(MilestoneDialog {
        item: app.sections[0].items[0].clone(),
        state: MilestoneDialogState::Loaded(Vec::new()),
        input: String::new(),
        selected: 0,
    });
    app.milestone_action_running = true;
    let milestone = Milestone {
        number: 9,
        title: "v1".to_string(),
    };

    app.handle_msg(AppMsg::MilestoneChanged {
        item_id: "1".to_string(),
        milestone: Some(milestone.clone()),
        result: Ok(()),
    });

    assert!(app.milestone_dialog.is_none());
    assert!(!app.milestone_action_running);
    assert_eq!(app.sections[0].items[0].milestone, Some(milestone));
    assert!(app.details_stale.contains("1"));
    assert_eq!(app.status, "milestone changed; refreshing");
}

#[test]
fn milestone_change_failure_opens_message_dialog() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.milestone_action_running = true;

    app.handle_msg(AppMsg::MilestoneChanged {
        item_id: "1".to_string(),
        milestone: None,
        result: Err("gh api failed: HTTP 403".to_string()),
    });

    assert!(!app.milestone_action_running);
    assert_eq!(app.status, "milestone change failed");
    let dialog = app.message_dialog.as_ref().expect("message dialog");
    assert_eq!(dialog.title, "Milestone Failed");
    assert!(dialog.body.contains("HTTP 403"));
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
fn retryable_message_dialog_esc_edits_and_enter_cancels() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    let pr_dialog = PrCreateDialog {
        repo: "chenyukang/ghr".to_string(),
        local_dir: PathBuf::from("/tmp/ghr"),
        branch: "dev-next".to_string(),
        head_ref: "dev-next".to_string(),
        title: EditorText::from_text("Retry title"),
        body: EditorText::from_text("Retry body"),
        field: PrCreateField::Body,
        body_scroll: 0,
    };
    app.pr_create_dialog = Some(pr_dialog.clone());
    app.message_dialog = Some(retryable_message_dialog(
        "Pull Request Create Failed",
        "a pull request already exists",
    ));

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Esc),
        &config,
        &store,
        &tx
    ));

    assert!(app.message_dialog.is_none());
    assert_eq!(app.pr_create_dialog, Some(pr_dialog.clone()));
    assert_eq!(app.status, "edit and retry");

    app.message_dialog = Some(retryable_message_dialog(
        "Pull Request Create Failed",
        "a pull request already exists",
    ));
    assert!(!handle_key(
        &mut app,
        key(KeyCode::Enter),
        &config,
        &store,
        &tx
    ));

    assert!(app.message_dialog.is_none());
    assert!(app.pr_create_dialog.is_none());
    assert_eq!(app.status, "retry cancelled");
}

#[test]
fn retryable_message_dialog_footer_shows_cancel_and_retry_keys() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.message_dialog = Some(retryable_message_dialog(
        "Pull Request Create Failed",
        "a pull request already exists",
    ));
    let backend = ratatui::backend::TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let paths = test_paths();

    terminal
        .draw(|frame| draw(frame, &app, &paths))
        .expect("draw");

    let rendered = buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(rendered.contains("Enter: cancel  Esc: edit and retry"));
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
    {
        let dialog = app.comment_dialog.as_mut().unwrap();
        dialog.body.set_text("hello");
    }

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
fn comment_editor_supports_mid_text_insert_and_word_line_deletes() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    app.start_new_comment_dialog();
    for ch in "hello world".chars() {
        app.handle_comment_dialog_key(key(KeyCode::Char(ch)), &tx, None);
    }
    for _ in 0..5 {
        app.handle_comment_dialog_key(key(KeyCode::Left), &tx, None);
    }
    for ch in "brave ".chars() {
        app.handle_comment_dialog_key(key(KeyCode::Char(ch)), &tx, None);
    }

    assert_eq!(
        app.comment_dialog
            .as_ref()
            .map(|dialog| (dialog.body.as_str(), dialog.body.cursor_byte())),
        Some(("hello brave world", "hello brave ".len()))
    );

    app.handle_comment_dialog_key(ctrl_key(KeyCode::Char('w')), &tx, None);
    assert_eq!(
        app.comment_dialog
            .as_ref()
            .map(|dialog| (dialog.body.as_str(), dialog.body.cursor_byte())),
        Some(("hello world", "hello ".len()))
    );

    app.handle_comment_dialog_key(ctrl_key(KeyCode::Char('k')), &tx, None);
    assert_eq!(
        app.comment_dialog
            .as_ref()
            .map(|dialog| (dialog.body.as_str(), dialog.body.cursor_byte())),
        Some(("hello ", "hello ".len()))
    );
}

#[test]
fn comment_editor_up_down_follow_rendered_lines() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    app.start_new_comment_dialog();
    {
        let dialog = app.comment_dialog.as_mut().unwrap();
        dialog.body.set_text("abc\ndef");
    }

    app.handle_comment_dialog_key(key(KeyCode::Up), &tx, None);
    app.handle_comment_dialog_key(key(KeyCode::Char('!')), &tx, None);

    assert_eq!(
        app.comment_dialog
            .as_ref()
            .map(|dialog| dialog.body.as_str()),
        Some("abc!\ndef")
    );
}

#[test]
fn comment_editor_can_delete_current_line() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    app.start_new_comment_dialog();
    {
        let dialog = app.comment_dialog.as_mut().unwrap();
        dialog.body.set_text("one\ntwo\nthree");
        dialog.body.set_cursor_byte("one\nt".len());
    }

    app.handle_comment_dialog_key(ctrl_key(KeyCode::Char('x')), &tx, None);

    assert_eq!(
        app.comment_dialog
            .as_ref()
            .map(|dialog| (dialog.body.as_str(), dialog.body.cursor_byte())),
        Some(("one\nthree", "one\n".len()))
    );
}

#[test]
fn mac_command_z_undoes_editor_change() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    app.start_new_comment_dialog();
    app.comment_dialog.as_mut().unwrap().body.set_text("hello");

    app.handle_comment_dialog_key(key(KeyCode::Char('!')), &tx, None);
    app.handle_comment_dialog_key(cmd_key(KeyCode::Char('z')), &tx, None);

    assert_eq!(
        app.comment_dialog
            .as_ref()
            .map(|dialog| dialog.body.as_str()),
        Some("hello")
    );

    app.handle_comment_dialog_key(cmd_shift_key(KeyCode::Char('Z')), &tx, None);

    assert_eq!(
        app.comment_dialog
            .as_ref()
            .map(|dialog| dialog.body.as_str()),
        Some("hello!")
    );
}

#[test]
fn mouse_clicking_comment_editor_moves_cursor() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let area = Rect::new(0, 0, 100, 30);
    app.start_new_comment_dialog();
    {
        let dialog = app.comment_dialog.as_mut().unwrap();
        dialog.body.set_text("hello world");
    }
    let dialog_area = comment_dialog_area(app.comment_dialog.as_ref().unwrap(), area);
    let inner = block_inner(dialog_area);

    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: inner.x + 6,
            row: inner.y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
        area,
    );
    app.handle_comment_dialog_key(key(KeyCode::Char('X')), &tx, Some(area));

    assert_eq!(
        app.comment_dialog
            .as_ref()
            .map(|dialog| (dialog.body.as_str(), dialog.body.cursor_byte())),
        Some(("hello Xworld", "hello X".len()))
    );
}

#[test]
fn mouse_dragging_comment_editor_text_copies_selection() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let area = Rect::new(0, 0, 100, 30);
    app.start_new_comment_dialog();
    {
        let dialog = app.comment_dialog.as_mut().unwrap();
        dialog.body.set_text("hello world\nsecond line");
    }
    let dialog = app.comment_dialog.as_ref().unwrap().clone();
    let dialog_area = comment_dialog_area(&dialog, area);
    let inner = block_inner(dialog_area);
    let body_lines = comment_dialog_body_lines(dialog.body.text(), inner.width.max(1));
    let row = inner.y;

    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: inner.x,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
        area,
    );
    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: inner.x + 5,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
        area,
    );

    let selection = app
        .dialog_text_selection
        .clone()
        .expect("active dialog text selection");
    assert_eq!(selected_dialog_text(&body_lines, &selection), "hello");
    assert_eq!(app.status, "selecting comment text; release to copy");

    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: inner.x + 5,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
        area,
    );

    assert_eq!(app.status, "copied selected comment text");
    assert_eq!(
        selected_dialog_text(
            &body_lines,
            app.dialog_text_selection
                .as_ref()
                .expect("selection remains visible")
        ),
        "hello"
    );
}

#[test]
fn comment_dialog_loads_existing_draft_on_open() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let key = new_comment_draft_key(app.current_item().expect("current item"));
    app.load_editor_drafts(HashMap::from([(
        key.clone(),
        EditorDraft {
            key,
            body: "saved draft".to_string(),
            updated_at: Utc::now(),
        },
    )]));

    app.start_new_comment_dialog();

    assert_eq!(
        app.comment_dialog
            .as_ref()
            .map(|dialog| dialog.body.as_str()),
        Some("saved draft")
    );
    assert_eq!(app.status, "loaded comment draft");
}

#[test]
fn mention_query_tracks_github_login_prefix_at_cursor() {
    assert_eq!(
        super::mentions::mention_query_at_cursor("ping @al", 8),
        Some((5, "al".to_string()))
    );
    assert_eq!(
        super::mentions::mention_query_at_cursor("@", 1),
        Some((0, String::new()))
    );
    assert_eq!(
        super::mentions::mention_query_at_cursor("cc @alice-bot", 13),
        Some((3, "alice-bot".to_string()))
    );
    assert_eq!(
        super::mentions::mention_query_at_cursor("ping @al ", 9),
        None
    );
    assert_eq!(
        super::mentions::mention_query_at_cursor("ping @al!", 9),
        None
    );
}

#[test]
fn comment_editor_accepts_mention_candidate() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.assignee_suggestions_cache.insert(
        "rust-lang/rust".to_string(),
        vec!["alice".to_string(), "bob".to_string()],
    );
    let (tx, _rx) = mpsc::unbounded_channel();

    app.start_new_comment_dialog();
    for ch in "ping @bo".chars() {
        app.handle_comment_dialog_key(key(KeyCode::Char(ch)), &tx, None);
    }

    let view = app
        .mention_candidate_view_for_target(MentionTarget::Comment)
        .expect("mention candidates");
    assert_eq!(view.candidates, vec!["bob"]);

    app.handle_comment_dialog_key(key(KeyCode::Enter), &tx, None);

    assert_eq!(
        app.comment_dialog
            .as_ref()
            .map(|dialog| dialog.body.as_str()),
        Some("ping @bob ")
    );
}

#[test]
fn edit_comment_renders_mention_candidates_after_at_trigger() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.assignee_suggestions_cache.insert(
        "rust-lang/rust".to_string(),
        vec!["alice".to_string(), "bob".to_string()],
    );
    app.details.insert(
        "1".to_string(),
        DetailState::Loaded(vec![own_comment(42, "chenyukang", "Original body", None)]),
    );
    app.handle_detail_action(DetailAction::EditComment(0), None, None);
    if let Some(dialog) = &mut app.comment_dialog {
        dialog.body.set_text("@");
    }

    let dialog = app.comment_dialog.as_ref().expect("edit comment").clone();
    let backend = ratatui::backend::TestBackend::new(120, 28);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| draw_comment_dialog(frame, &app, &dialog, frame.area()))
        .expect("draw edit comment dialog");
    let rendered = buffer_lines(terminal.backend().buffer()).join("\n");

    assert!(rendered.contains("Mention candidates"));
    assert!(rendered.contains("@alice"));
}

#[test]
fn mention_candidates_include_github_user_search_results() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.mention_user_search_cache.insert(
        "fe".to_string(),
        vec!["ferris".to_string(), "ferrocene".to_string()],
    );
    let (tx, _rx) = mpsc::unbounded_channel();

    app.start_new_comment_dialog();
    for ch in "cc @fe".chars() {
        app.handle_comment_dialog_key(key(KeyCode::Char(ch)), &tx, None);
    }

    let view = app
        .mention_candidate_view_for_target(MentionTarget::Comment)
        .expect("mention candidates");
    assert_eq!(view.candidates, vec!["ferris", "ferrocene"]);
}

#[test]
fn mention_user_search_loaded_updates_active_candidates() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();

    app.start_new_comment_dialog();
    for ch in "@swa".chars() {
        app.handle_comment_dialog_key(key(KeyCode::Char(ch)), &tx, None);
    }
    app.handle_msg(AppMsg::MentionUserSearchLoaded {
        query: "swa".to_string(),
        result: Ok(vec!["swananan".to_string()]),
    });

    let view = app
        .mention_candidate_view_for_target(MentionTarget::Comment)
        .expect("mention candidates");
    assert_eq!(view.candidates, vec!["swananan"]);
    assert!(!view.loading);
}

#[test]
fn issue_title_accepts_mention_candidate_before_field_advance() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.assignee_suggestions_cache.insert(
        "rust-lang/rust".to_string(),
        vec!["alice".to_string(), "bob".to_string()],
    );

    app.start_new_issue_dialog();
    for ch in "@bo".chars() {
        app.handle_issue_dialog_key_with_submit(key(KeyCode::Char(ch)), None, |_| {});
    }
    app.handle_issue_dialog_key_with_submit(key(KeyCode::Enter), None, |_| {});

    let dialog = app.issue_dialog.as_ref().expect("issue dialog");
    assert_eq!(dialog.title.as_str(), "@bob ");
    assert_eq!(dialog.field, IssueDialogField::Title);
}

#[test]
fn review_submit_editor_accepts_mention_candidate() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.assignee_suggestions_cache.insert(
        "rust-lang/rust".to_string(),
        vec!["alice".to_string(), "bob".to_string()],
    );
    let (tx, _rx) = mpsc::unbounded_channel();

    app.start_review_submit_dialog(PullRequestReviewEvent::Comment);
    for ch in "thanks @bo".chars() {
        app.handle_review_submit_dialog_key(key(KeyCode::Char(ch)), &tx, None);
    }
    app.handle_review_submit_dialog_key(key(KeyCode::Enter), &tx, None);

    assert_eq!(
        app.review_submit_dialog
            .as_ref()
            .map(|dialog| dialog.body.as_str()),
        Some("thanks @bob ")
    );
}

#[test]
fn item_edit_body_renders_and_accepts_mention_candidate() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.assignee_suggestions_cache.insert(
        "rust-lang/rust".to_string(),
        vec!["chenyukang".to_string(), "alice".to_string()],
    );
    app.start_item_edit_dialog();
    if let Some(dialog) = &mut app.item_edit_dialog {
        dialog.field = ItemEditField::Body;
        dialog.body.set_text("cc @che");
    }

    let dialog = app.item_edit_dialog.as_ref().expect("item edit").clone();
    let backend = ratatui::backend::TestBackend::new(100, 32);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| {
            draw_item_edit_dialog(frame, &app, &dialog, false, frame.area());
        })
        .expect("draw item edit dialog");
    let rendered = buffer_lines(terminal.backend().buffer()).join("\n");
    assert!(rendered.contains("Mention candidates"));
    assert!(rendered.contains("@chenyukang"));

    app.handle_item_edit_dialog_key_with_submit(key(KeyCode::Enter), None, |_| {});

    assert_eq!(
        app.item_edit_dialog
            .as_ref()
            .map(|dialog| dialog.body.as_str()),
        Some("cc @chenyukang ")
    );
}

#[test]
fn saving_comment_draft_persists_and_reopens_it() {
    let paths = unique_test_paths("comment-draft-save");
    let store = SnapshotStore::new(paths.db_path.clone());
    store.init().expect("init store");
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);

    app.start_new_comment_dialog();
    app.comment_dialog
        .as_mut()
        .unwrap()
        .body
        .set_text("remember me");
    app.save_active_comment_draft(&store, DraftSaveTrigger::Manual, Instant::now());

    assert_eq!(app.status, "draft saved");
    assert_eq!(
        store
            .load_editor_drafts()
            .expect("load drafts")
            .values()
            .next()
            .map(|draft| draft.body.as_str()),
        Some("remember me")
    );

    app.comment_dialog = None;
    app.start_new_comment_dialog();
    assert_eq!(
        app.comment_dialog
            .as_ref()
            .map(|dialog| dialog.body.as_str()),
        Some("remember me")
    );
}

#[test]
fn comment_draft_auto_save_waits_for_interval() {
    let paths = unique_test_paths("comment-draft-auto-save");
    let store = SnapshotStore::new(paths.db_path.clone());
    store.init().expect("init store");
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);

    app.start_new_comment_dialog();
    let opened_at = app.comment_draft_last_auto_save_at;
    app.comment_dialog
        .as_mut()
        .unwrap()
        .body
        .set_text("autosaved body");

    app.auto_save_active_comment_draft(
        &store,
        opened_at + EDITOR_DRAFT_AUTO_SAVE_INTERVAL - Duration::from_millis(1),
    );
    assert!(
        store
            .load_editor_drafts()
            .expect("load drafts before interval")
            .is_empty()
    );

    app.auto_save_active_comment_draft(&store, opened_at + EDITOR_DRAFT_AUTO_SAVE_INTERVAL);
    assert_eq!(
        store
            .load_editor_drafts()
            .expect("load drafts after interval")
            .values()
            .next()
            .map(|draft| draft.body.as_str()),
        Some("autosaved body")
    );
}

#[test]
fn clicking_comment_dialog_footer_saves_draft() {
    let paths = unique_test_paths("comment-draft-click");
    let store = SnapshotStore::new(paths.db_path.clone());
    store.init().expect("init store");
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let area = Rect::new(0, 0, 100, 30);
    app.start_new_comment_dialog();
    app.comment_dialog
        .as_mut()
        .unwrap()
        .body
        .set_text("clicked draft");
    let dialog_area = comment_dialog_area(app.comment_dialog.as_ref().unwrap(), area);
    let footer = modal_footer_area(area, dialog_area).expect("footer area");

    handle_mouse_with_sync(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: footer.x,
            row: footer.y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
        area,
        Some(&store),
        None,
    );

    assert_eq!(app.status, "draft saved");
    assert_eq!(
        store
            .load_editor_drafts()
            .expect("load drafts")
            .values()
            .next()
            .map(|draft| draft.body.as_str()),
        Some("clicked draft")
    );
}

#[test]
fn successful_comment_post_clears_saved_draft_state() {
    let paths = unique_test_paths("comment-draft-clear");
    let store = SnapshotStore::new(paths.db_path.clone());
    store.init().expect("init store");
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);

    app.start_new_comment_dialog();
    app.comment_dialog
        .as_mut()
        .unwrap()
        .body
        .set_text("draft body");
    app.save_active_comment_draft(&store, DraftSaveTrigger::Manual, Instant::now());
    let key = app.comment_draft_key.clone().expect("draft key");

    app.handle_comment_dialog_key_with_submit(ctrl_key(KeyCode::Enter), None, |_| {});
    app.handle_msg(AppMsg::CommentPosted {
        item_id: "1".to_string(),
        result: Ok(comment("chenyukang", "posted", None)),
    });
    clear_editor_draft_after_success(draft_clear_task(Some(key.clone()), Some(store.clone())));

    assert!(!app.editor_drafts.contains_key(&key));
    assert!(store.load_editor_drafts().expect("load drafts").is_empty());
}

#[test]
fn new_issue_dialog_loads_existing_repo_draft() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let key = new_issue_draft_key("rust-lang/rust");
    let body = encode_editor_draft(&NewIssueDraft {
        repo: "rust-lang/rust".to_string(),
        title: "draft issue".to_string(),
        labels: "bug, help wanted".to_string(),
        body: "draft body".to_string(),
    })
    .expect("encode draft");
    app.load_editor_drafts(HashMap::from([(
        key.clone(),
        EditorDraft {
            key,
            body,
            updated_at: Utc::now(),
        },
    )]));

    app.start_new_issue_dialog();

    let dialog = app.issue_dialog.as_ref().expect("issue dialog");
    assert_eq!(dialog.repo, "rust-lang/rust");
    assert_eq!(dialog.title, "draft issue");
    assert_eq!(dialog.labels, "bug, help wanted");
    assert_eq!(dialog.body, "draft body");
    assert_eq!(app.status, "loaded issue draft");
}

#[test]
fn saving_new_issue_draft_uses_repo_key() {
    let paths = unique_test_paths("issue-draft-save");
    let store = SnapshotStore::new(paths.db_path.clone());
    store.init().expect("init store");
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);

    app.start_new_issue_dialog();
    {
        let dialog = app.issue_dialog.as_mut().expect("issue dialog");
        dialog.title.set_text("draft issue");
        dialog.labels.set_text("bug");
        dialog.body.set_text("draft body");
    }
    app.save_active_issue_draft(&store, DraftSaveTrigger::Manual, Instant::now());

    let drafts = store.load_editor_drafts().expect("load drafts");
    let draft = drafts
        .get(&new_issue_draft_key("rust-lang/rust"))
        .expect("repo issue draft");
    let payload: NewIssueDraft = serde_json::from_str(&draft.body).expect("decode draft");
    assert_eq!(app.status, "draft saved");
    assert_eq!(payload.repo, "rust-lang/rust");
    assert_eq!(payload.title, "draft issue");
    assert_eq!(payload.labels, "bug");
    assert_eq!(payload.body, "draft body");
}

#[test]
fn new_pr_dialog_loads_existing_repo_draft() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let key = new_pr_draft_key("rust-lang/rust");
    let body = encode_editor_draft(&NewPrDraft {
        repo: "rust-lang/rust".to_string(),
        title: "draft PR".to_string(),
        body: "draft PR body".to_string(),
    })
    .expect("encode draft");
    app.load_editor_drafts(HashMap::from([(
        key.clone(),
        EditorDraft {
            key,
            body,
            updated_at: Utc::now(),
        },
    )]));

    let loaded = app.open_pr_create_dialog_with_draft(
        "rust-lang/rust".to_string(),
        PathBuf::from("/tmp/rust"),
        "feature/draft".to_string(),
    );

    let dialog = app.pr_create_dialog.as_ref().expect("pr create dialog");
    assert!(loaded);
    assert_eq!(dialog.repo, "rust-lang/rust");
    assert_eq!(dialog.title, "draft PR");
    assert_eq!(dialog.body, "draft PR body");
}

#[test]
fn saving_new_pr_draft_uses_repo_key() {
    let paths = unique_test_paths("pr-draft-save");
    let store = SnapshotStore::new(paths.db_path.clone());
    store.init().expect("init store");
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);

    app.open_pr_create_dialog_with_draft(
        "rust-lang/rust".to_string(),
        PathBuf::from("/tmp/rust"),
        "feature/draft".to_string(),
    );
    {
        let dialog = app.pr_create_dialog.as_mut().expect("pr create dialog");
        dialog.title.set_text("draft PR");
        dialog.body.set_text("draft PR body");
    }
    app.save_active_pr_create_draft(&store, DraftSaveTrigger::Manual, Instant::now());

    let drafts = store.load_editor_drafts().expect("load drafts");
    let draft = drafts
        .get(&new_pr_draft_key("rust-lang/rust"))
        .expect("repo PR draft");
    let payload: NewPrDraft = serde_json::from_str(&draft.body).expect("decode draft");
    assert_eq!(app.status, "draft saved");
    assert_eq!(payload.repo, "rust-lang/rust");
    assert_eq!(payload.title, "draft PR");
    assert_eq!(payload.body, "draft PR body");
}

#[test]
fn successful_issue_create_clears_saved_draft_state() {
    let paths = unique_test_paths("issue-draft-clear");
    let store = SnapshotStore::new(paths.db_path.clone());
    store.init().expect("init store");
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);

    app.start_new_issue_dialog();
    app.issue_dialog
        .as_mut()
        .unwrap()
        .title
        .set_text("draft issue");
    app.save_active_issue_draft(&store, DraftSaveTrigger::Manual, Instant::now());
    let key = app.issue_draft_key.clone().expect("draft key");
    app.handle_issue_dialog_key_with_submit(ctrl_key(KeyCode::Enter), None, |_| {});

    let mut item = work_item(
        "rust-lang/rust#99",
        "rust-lang/rust",
        99,
        "draft issue",
        None,
    );
    item.kind = ItemKind::Issue;
    item.url = "https://github.com/rust-lang/rust/issues/99".to_string();
    app.handle_msg(AppMsg::IssueCreated { result: Ok(item) });
    clear_editor_draft_after_success(draft_clear_task(Some(key.clone()), Some(store.clone())));

    assert!(!app.editor_drafts.contains_key(&key));
    assert!(store.load_editor_drafts().expect("load drafts").is_empty());
}

#[test]
fn successful_pr_create_clears_saved_draft_state() {
    let paths = unique_test_paths("pr-draft-clear");
    let store = SnapshotStore::new(paths.db_path.clone());
    store.init().expect("init store");
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let local_dir = checkout_test_repo_dir_with_main_and_branch("feature/draft", true);

    app.open_pr_create_dialog_with_draft(
        "chenyukang/ghr".to_string(),
        local_dir,
        "feature/draft".to_string(),
    );
    app.pr_create_dialog
        .as_mut()
        .unwrap()
        .title
        .set_text("draft PR");
    app.save_active_pr_create_draft(&store, DraftSaveTrigger::Manual, Instant::now());
    let key = app.pr_create_draft_key.clone().expect("draft key");
    app.prepare_pr_create().expect("pending PR create");

    let item = work_item(
        "chenyukang/ghr#100",
        "chenyukang/ghr",
        100,
        "draft PR",
        None,
    );
    app.handle_msg(AppMsg::PullRequestCreated { result: Ok(item) });
    clear_editor_draft_after_success(draft_clear_task(Some(key.clone()), Some(store.clone())));

    assert!(!app.editor_drafts.contains_key(&key));
    assert!(store.load_editor_drafts().expect("load drafts").is_empty());
}

#[test]
fn mouse_clicking_issue_title_moves_field_cursor() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let area = Rect::new(0, 0, 100, 30);
    app.start_new_issue_dialog();
    {
        let dialog = app.issue_dialog.as_mut().unwrap();
        dialog.title.set_text("hello world");
    }
    let dialog_area = issue_dialog_area(area);
    let inner = block_inner(dialog_area);
    let input_start = inner.x
        + display_width(&issue_dialog_field_prefix("Title")).min(usize::from(u16::MAX)) as u16;

    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: input_start + 6,
            row: inner.y + 2,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
        area,
    );
    for ch in "brave ".chars() {
        app.handle_issue_dialog_key_with_submit(key(KeyCode::Char(ch)), Some(area), |_| {});
    }

    let dialog = app.issue_dialog.as_ref().unwrap();
    assert_eq!(dialog.field, IssueDialogField::Title);
    assert_eq!(dialog.title, "hello brave world");
    assert_eq!(dialog.title.cursor_byte(), "hello brave ".len());
}

#[test]
fn ctrl_enter_in_comment_dialog_submits() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_new_comment_dialog();
    app.comment_dialog.as_mut().unwrap().body.set_text("hello");
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
    app.comment_dialog
        .as_mut()
        .unwrap()
        .body
        .set_text("updated");
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
        is_resolved: false,
        is_outdated: false,
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
        is_resolved: false,
        is_outdated: false,
    });
    app.details
        .insert("1".to_string(), DetailState::Loaded(vec![inline]));
    app.start_reply_to_selected_comment();
    app.comment_dialog
        .as_mut()
        .unwrap()
        .body
        .set_text("reply inline");
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
        app.review_submit_dialog.as_ref().map(|dialog| dialog.event),
        Some(PullRequestReviewEvent::Approve)
    ));

    app.review_submit_dialog = None;
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

    app.pr_action_dialog = None;
    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('E')),
        &config,
        &store,
        &tx
    ));
    assert!(matches!(
        app.pr_action_dialog.as_ref().map(|dialog| dialog.action),
        Some(PrAction::EnableAutoMerge)
    ));

    app.pr_action_dialog = None;
    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('O')),
        &config,
        &store,
        &tx
    ));
    assert!(matches!(
        app.pr_action_dialog.as_ref().map(|dialog| dialog.action),
        Some(PrAction::DisableAutoMerge)
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
    app.comment_dialog
        .as_mut()
        .unwrap()
        .body
        .set_text("please tighten this");
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
        result: Ok(comment("alice", "posted", None)),
    });

    assert!(!app.posting_comment);
    assert_eq!(app.status, "comment posted");
    assert_eq!(
        app.message_dialog
            .as_ref()
            .map(|dialog| (dialog.title.as_str(), dialog.body.as_str())),
        Some((
            "Comment Posted",
            "GitHub accepted the comment; comments will refresh in the background."
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
fn stale_comments_refresh_keeps_optimistic_posted_comment_until_api_returns_it() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let mut existing = comment("bob", "old comment", None);
    existing.id = Some(1);
    app.details
        .insert("1".to_string(), DetailState::Loaded(vec![existing.clone()]));

    let mut posted = comment("alice", "posted comment", None);
    posted.id = Some(99);
    app.handle_msg(AppMsg::CommentPosted {
        item_id: "1".to_string(),
        result: Ok(posted.clone()),
    });

    app.handle_msg(AppMsg::CommentsLoaded {
        item_id: "1".to_string(),
        comments: Ok(CommentFetchResult {
            item_metadata: None,
            item_reactions: Some(ReactionSummary::default()),
            item_milestone: Some(None),
            comments: vec![existing.clone()],
        }),
    });

    let comments = match app.details.get("1") {
        Some(DetailState::Loaded(comments)) => comments,
        other => panic!("expected loaded comments, got {other:?}"),
    };
    assert_eq!(
        comments
            .iter()
            .map(|comment| comment.body.as_str())
            .collect::<Vec<_>>(),
        vec!["old comment", "posted comment"]
    );
    assert!(
        app.optimistic_comment_ids
            .get("1")
            .is_some_and(|ids| ids.contains(&99))
    );

    app.handle_msg(AppMsg::CommentsLoaded {
        item_id: "1".to_string(),
        comments: Ok(CommentFetchResult {
            item_metadata: None,
            item_reactions: Some(ReactionSummary::default()),
            item_milestone: Some(None),
            comments: vec![existing, posted],
        }),
    });

    assert!(!app.optimistic_comment_ids.contains_key("1"));
}

#[test]
fn comment_post_failure_opens_result_dialog() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.posting_comment = true;
    app.message_dialog = Some(comment_pending_dialog(&PendingCommentMode::Post));
    app.pending_comment_submit = Some(PendingCommentSubmit {
        item: app.sections[0].items[0].clone(),
        body: "draft body".to_string(),
        mode: PendingCommentMode::Post,
        dialog: CommentDialog {
            mode: CommentDialogMode::New,
            body: EditorText::from_text("draft body"),
            scroll: 0,
        },
        draft_key: None,
    });

    app.handle_msg(AppMsg::CommentPosted {
        item_id: "1".to_string(),
        result: Err("gh api repos/owner/repo/issues/1/comments failed: HTTP 403".to_string()),
    });

    assert!(!app.posting_comment);
    assert_eq!(app.status, "comment post failed");
    let dialog = app.message_dialog.as_ref().expect("failure dialog");
    assert_eq!(dialog.title, "Comment Failed");
    assert_eq!(dialog.kind, MessageDialogKind::RetryableError);
    assert!(dialog.body.contains("HTTP 403"));
    assert!(dialog.auto_close_at.is_none());
    assert_eq!(
        app.comment_dialog
            .as_ref()
            .map(|dialog| dialog.body.as_str()),
        Some("draft body")
    );
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
        result: Ok(CommentFetchResult {
            item_metadata: None,
            item_reactions: Some(ReactionSummary::default()),
            item_milestone: Some(None),
            comments: vec![own_comment(42, "chenyukang", "updated", None)],
        }),
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
    assert_eq!(dialog.kind, MessageDialogKind::RetryableError);
    assert!(dialog.body.contains("validation failed"));
    assert!(dialog.auto_close_at.is_none());
}

#[test]
fn review_comment_result_dialog_reports_success_and_failure() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.posting_comment = true;

    app.handle_msg(AppMsg::ReviewCommentPosted {
        item_id: "1".to_string(),
        result: Ok(comment("alice", "review comment", None)),
    });

    assert!(!app.posting_comment);
    assert!(!app.details_stale.contains("1"));
    assert!(matches!(
        app.details.get("1"),
        Some(DetailState::Loaded(comments)) if comments[0].body == "review comment"
    ));
    assert_eq!(app.status, "review comment posted");
    assert_eq!(
        app.message_dialog
            .as_ref()
            .map(|dialog| (dialog.title.as_str(), dialog.body.as_str())),
        Some((
            "Review Comment Posted",
            "GitHub accepted the review comment; comments will refresh in the background."
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
    assert_eq!(dialog.kind, MessageDialogKind::RetryableError);
    assert!(dialog.body.contains("validation failed"));
    assert!(dialog.auto_close_at.is_none());
}

#[test]
fn posted_review_comment_is_rendered_locally_in_diff_mode() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.details_mode = DetailsMode::Diff;
    let diff = parse_pull_request_diff(
        r#"diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,2 @@
 fn main() {
+    println!("new");
 }
"#,
    )
    .expect("diff");
    app.diffs.insert("1".to_string(), DiffState::Loaded(diff));
    let mut posted = comment("alice", "please tighten this", None);
    posted.id = Some(99);
    posted.is_mine = true;
    posted.review = Some(crate::model::ReviewCommentPreview {
        path: "src/lib.rs".to_string(),
        line: Some(2),
        original_line: Some(2),
        start_line: None,
        original_start_line: None,
        side: Some("RIGHT".to_string()),
        start_side: None,
        diff_hunk: None,
        is_resolved: false,
        is_outdated: false,
    });

    app.handle_msg(AppMsg::ReviewCommentPosted {
        item_id: "1".to_string(),
        result: Ok(posted),
    });

    let rendered = build_details_document(&app, 120)
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("please tighten this"));
    assert!(!rendered.contains("loading comments"));
    assert!(!app.details_stale.contains("1"));
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
        body: EditorText::from_text("hello\nworld"),
        scroll: 0,
    };
    let area = Rect::new(10, 5, 30, 10);

    assert_eq!(
        comment_dialog_cursor_position(
            dialog.body.as_str(),
            dialog.body.cursor_byte(),
            dialog.scroll,
            area,
            28,
            6,
        ),
        Some(Position::new(16, 7))
    );
}

#[test]
fn comment_dialog_cursor_uses_display_width_for_chinese_input() {
    let area = Rect::new(10, 5, 30, 10);

    assert_eq!(
        comment_dialog_cursor_position("你好", "你好".len(), 0, area, 28, 6),
        Some(Position::new(15, 6))
    );
    assert_eq!(comment_dialog_body_lines("你好ab", 5), vec!["你好a", "b"]);
    assert_eq!(
        comment_dialog_cursor_position("你好ab", "你好ab".len(), 0, area, 5, 6),
        Some(Position::new(12, 7))
    );
}

#[test]
fn comment_dialog_cursor_is_hidden_when_scrolled_away_from_input() {
    let body = (1..=80)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let area = Rect::new(10, 5, 30, 10);

    assert_eq!(
        comment_dialog_cursor_position(&body, body.len(), 20, area, 28, 6),
        None
    );
}

#[test]
fn comment_dialog_cursor_wraps_to_next_line_at_exact_width() {
    let area = Rect::new(10, 5, 7, 10);

    assert_eq!(
        comment_dialog_cursor_position("abcde", "abcde".len(), 0, area, 5, 6),
        Some(Position::new(11, 7))
    );
}

#[test]
fn issue_dialog_single_line_cursor_uses_display_width_for_chinese_input() {
    let dialog = IssueDialog {
        repo: EditorText::from_text("rust-lang/rust"),
        title: EditorText::from_text("中文"),
        labels: EditorText::from_text("标签"),
        body: EditorText::empty(),
        field: IssueDialogField::Title,
        body_scroll: 0,
    };
    let area = Rect::new(10, 5, 80, 22);

    assert_eq!(
        issue_dialog_cursor_position(&dialog, 0, area, 78, 12),
        Some(Position::new(22, 8))
    );
}

#[test]
fn comment_dialog_scroll_tracks_cursor_for_long_body() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    app.start_new_comment_dialog();
    let body = (1..=20)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let dialog = app.comment_dialog.as_mut().unwrap();
    dialog.body.set_text(body);

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
    app.comment_dialog.as_mut().unwrap().body.set_text(
        (1..=80)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );

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
        body: EditorText::empty(),
        scroll: 0,
    };
    let medium_body = (1..=18)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let medium = CommentDialog {
        mode: CommentDialogMode::New,
        body: EditorText::from_text(medium_body),
        scroll: 0,
    };
    let long_body = (1..=100)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let long = CommentDialog {
        mode: CommentDialogMode::New,
        body: EditorText::from_text(long_body),
        scroll: 0,
    };

    let short_area = comment_dialog_area(&short, area);
    let medium_area = comment_dialog_area(&medium, area);
    let long_area = comment_dialog_area(&long, area);

    assert!(medium_area.height > short_area.height);
    assert_eq!(long_area.height, comment_dialog_max_height(area));

    let inner = block_inner(long_area);
    let editor_height = inner.height.max(1);
    assert!(max_comment_dialog_scroll(long.body.as_str(), inner.width, editor_height) > 0);
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
            kind: CommentPreviewKind::Comment,
            author: "alice".to_string(),
            body,
            created_at: None,
            updated_at: None,
            url: None,
            parent_id: None,
            is_mine: false,
            viewer_can_update: None,
            reactions: ReactionSummary::default(),
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

    for index in 1..=12 {
        assert!(rendered.contains(&format!("line {index}")));
    }
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
    assert_eq!(app.status, "GHR focused");
}

#[test]
fn repo_sections_create_top_level_repo_tab_with_generic_section_titles() {
    let sections = vec![
        test_section(),
        SectionSnapshot::empty_for_view(
            "repo:fiber",
            SectionKind::Issues,
            "Issues",
            "repo:nervosnetwork/fiber is:open archived:false sort:created-desc",
        ),
        SectionSnapshot::empty_for_view(
            "repo:fiber",
            SectionKind::PullRequests,
            "Pull Requests",
            "repo:nervosnetwork/fiber is:open archived:false sort:created-desc",
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
        vec!["Issues", "Pull Requests"]
    );
}

#[test]
fn builtin_top_level_tabs_put_inbox_before_pull_requests() {
    let config = Config::default();
    let app = AppState::new(SectionKind::PullRequests, configured_sections(&config));

    assert_eq!(
        app.view_tabs()
            .iter()
            .map(|view| view.label.as_str())
            .collect::<Vec<_>>(),
        vec!["Inbox", "Pull Requests", "Issues"]
    );
}

#[test]
fn current_repo_tab_can_lead_configured_repo_tabs() {
    let mut config = Config::default();
    config.repos.push(crate::config::RepoConfig {
        name: "runnel".to_string(),
        repo: "chenyukang/runnel".to_string(),
        remote: None,
        local_dir: None,
        show_prs: true,
        show_issues: true,
        labels: Vec::new(),
        pr_labels: Vec::new(),
        issue_labels: Vec::new(),
    });
    config.repos.push(crate::config::RepoConfig {
        name: "Fiber".to_string(),
        repo: "nervosnetwork/fiber".to_string(),
        remote: None,
        local_dir: None,
        show_prs: true,
        show_issues: true,
        labels: Vec::new(),
        pr_labels: Vec::new(),
        issue_labels: Vec::new(),
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
fn repo_top_tab_shows_unseen_issue_and_pr_counts_until_opened() {
    let now = Utc::now();
    let mut issue_one = work_item("issue-1", "chenyukang/ghr", 1, "Issue one", None);
    issue_one.kind = ItemKind::Issue;
    issue_one.url = "https://github.com/chenyukang/ghr/issues/1".to_string();
    let mut issue_two = work_item("issue-2", "chenyukang/ghr", 2, "Issue two", None);
    issue_two.kind = ItemKind::Issue;
    issue_two.url = "https://github.com/chenyukang/ghr/issues/2".to_string();
    let pr_one = work_item("pr-1", "chenyukang/ghr", 11, "PR one", None);
    let pr_two = work_item("pr-2", "chenyukang/ghr", 12, "PR two", None);
    let pr_three = work_item("pr-3", "chenyukang/ghr", 13, "PR three", None);
    let issue_section = SectionSnapshot {
        key: "repo:ghr:issues:Issues".to_string(),
        kind: SectionKind::Issues,
        title: "Issues".to_string(),
        filters: "repo:chenyukang/ghr is:open".to_string(),
        items: vec![issue_one.clone()],
        total_count: Some(1),
        page: 1,
        page_size: 50,
        refreshed_at: Some(now),
        error: None,
    };
    let pr_section = SectionSnapshot {
        key: "repo:ghr:pull_requests:Pull Requests".to_string(),
        kind: SectionKind::PullRequests,
        title: "Pull Requests".to_string(),
        filters: "repo:chenyukang/ghr is:open".to_string(),
        items: vec![pr_one.clone()],
        total_count: Some(1),
        page: 1,
        page_size: 50,
        refreshed_at: Some(now),
        error: None,
    };
    let mut app = AppState::new(
        SectionKind::PullRequests,
        vec![test_section(), issue_section.clone(), pr_section.clone()],
    );

    app.apply_idle_refreshed_sections(vec![
        SectionSnapshot {
            items: vec![issue_two, issue_one],
            total_count: Some(2),
            refreshed_at: Some(now + chrono::Duration::minutes(1)),
            ..issue_section
        },
        SectionSnapshot {
            items: vec![pr_three, pr_two, pr_one],
            total_count: Some(3),
            refreshed_at: Some(now + chrono::Duration::minutes(1)),
            ..pr_section
        },
    ]);

    let repo_tab = app
        .view_tabs()
        .into_iter()
        .find(|view| view.key == "repo:ghr")
        .expect("repo tab");
    assert_eq!(repo_tab.label, "ghr(1|2)");

    app.switch_view("repo:ghr");

    let repo_tab = app
        .view_tabs()
        .into_iter()
        .find(|view| view.key == "repo:ghr")
        .expect("repo tab");
    assert_eq!(repo_tab.label, "ghr");
}

#[test]
fn repo_unseen_counts_survive_restart_and_active_refresh_until_opened() {
    let now = Utc::now();
    let mut issue_one = work_item("issue-1", "chenyukang/ghr", 1, "Issue one", None);
    issue_one.kind = ItemKind::Issue;
    issue_one.url = "https://github.com/chenyukang/ghr/issues/1".to_string();
    let pr_one = work_item("pr-1", "chenyukang/ghr", 11, "PR one", None);
    let pr_two = work_item("pr-2", "chenyukang/ghr", 12, "PR two", None);
    let pr_three = work_item("pr-3", "chenyukang/ghr", 13, "PR three", None);
    let issue_section = SectionSnapshot {
        key: "repo:ghr:issues:Issues".to_string(),
        kind: SectionKind::Issues,
        title: "Issues".to_string(),
        filters: "repo:chenyukang/ghr is:open".to_string(),
        items: vec![issue_one],
        total_count: Some(1),
        page: 1,
        page_size: 50,
        refreshed_at: Some(now),
        error: None,
    };
    let pr_section = SectionSnapshot {
        key: "repo:ghr:pull_requests:Pull Requests".to_string(),
        kind: SectionKind::PullRequests,
        title: "Pull Requests".to_string(),
        filters: "repo:chenyukang/ghr is:open".to_string(),
        items: vec![pr_one.clone()],
        total_count: Some(1),
        page: 1,
        page_size: 50,
        refreshed_at: Some(now),
        error: None,
    };
    let mut app = AppState::with_ui_state(
        SectionKind::PullRequests,
        vec![test_section(), issue_section, pr_section.clone()],
        UiState {
            active_view: "repo:ghr".to_string(),
            repo_unseen_items: HashMap::from([(
                "repo:ghr".to_string(),
                RepoUnseenItemsState {
                    issues: vec!["issue-2".to_string()],
                    pull_requests: vec!["pr-2".to_string()],
                },
            )]),
            ..UiState::default()
        },
    );

    app.apply_refreshed_section(
        SectionSnapshot {
            items: vec![pr_three, pr_two, pr_one],
            total_count: Some(3),
            refreshed_at: Some(now + chrono::Duration::minutes(1)),
            ..pr_section
        },
        None,
    );

    let repo_tab = app
        .view_tabs()
        .into_iter()
        .find(|view| view.key == "repo:ghr")
        .expect("repo tab");
    assert_eq!(repo_tab.label, "ghr(1|2)");
    assert_eq!(
        app.ui_state()
            .repo_unseen_items
            .get("repo:ghr")
            .map(|unseen| unseen.pull_requests.clone()),
        Some(vec!["pr-2".to_string(), "pr-3".to_string()])
    );

    app.switch_view("repo:ghr");

    let repo_tab = app
        .view_tabs()
        .into_iter()
        .find(|view| view.key == "repo:ghr")
        .expect("repo tab");
    assert_eq!(repo_tab.label, "ghr");
    assert!(!app.ui_state().repo_unseen_items.contains_key("repo:ghr"));
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
fn list_brackets_load_github_result_pages_instead_of_moving_selection() {
    let mut section = many_items_section(30);
    section.total_count = Some(30);
    section.page_size = 50;
    let mut app = AppState::new(SectionKind::PullRequests, vec![section]);
    app.refreshing = true;
    app.set_selection(10);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    let area = Rect::new(0, 0, 120, 20);

    assert!(!handle_key_in_area(
        &mut app,
        key(KeyCode::Char(']')),
        &config,
        &store,
        &tx,
        Some(area)
    ));
    assert_eq!(app.current_selected_position(), 10);
    assert_eq!(app.status, "only one result page");

    app.status.clear();
    assert!(!handle_key_in_area(
        &mut app,
        key(KeyCode::Char('[')),
        &config,
        &store,
        &tx,
        Some(area)
    ));
    assert_eq!(app.current_selected_position(), 10);
    assert_eq!(app.status, "only one result page");
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
        usize::from(MOUSE_LIST_SCROLL_LINES).min(max_table_viewport_offset(30, visible_rows))
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
    app.details_scroll = bob_line_index
        .saturating_sub(usize::from(inner.height.saturating_sub(2)))
        .min(usize::from(u16::MAX)) as u16;
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
    assert_eq!(details_title(), "Details");
}

#[test]
fn focus_panel_title_marks_active_panel() {
    assert_eq!(
        focus_panel_title("Details", "Details", true),
        "[Focus] Details"
    );
    assert_eq!(focus_panel_title("Details", "Details", false), "Details");
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
                .map(|index| comment(&format!("user{index}"), &format!("comment {index}"), None))
                .collect(),
        ),
    );
    let area = Rect::new(0, 0, 100, 16);
    let details = details_area_for(&app, area);
    let inner = block_inner(details);

    let scroll_attempts = usize::from(max_details_scroll(&app, details_area_for(&app, area)))
        + usize::from(inner.height)
        + 16;
    for _ in 0..scroll_attempts {
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
fn n_and_p_mirror_j_and_k_between_ghr_sections_and_list_focus() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

    app.focus_ghr();
    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('p')),
        &config,
        &store,
        &tx
    ));
    assert_eq!(app.focus, FocusTarget::Ghr);
    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('n')),
        &config,
        &store,
        &tx
    ));
    assert_eq!(app.focus, FocusTarget::Sections);

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('p')),
        &config,
        &store,
        &tx
    ));
    assert_eq!(app.focus, FocusTarget::Ghr);

    app.focus_sections();
    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('n')),
        &config,
        &store,
        &tx
    ));
    assert_eq!(app.focus, FocusTarget::List);
}

#[test]
fn h_l_and_brackets_switch_only_the_focused_tab_group() {
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

    app.focus_ghr();
    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char(']')),
        &config,
        &store,
        &tx
    ));
    assert_eq!(app.active_view, builtin_view_key(SectionKind::Issues));
    assert_eq!(app.focus, FocusTarget::Ghr);

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('[')),
        &config,
        &store,
        &tx
    ));
    assert_eq!(app.active_view, builtin_view_key(SectionKind::PullRequests));
    assert_eq!(app.focus, FocusTarget::Ghr);

    app.section_index
        .insert(builtin_view_key(SectionKind::PullRequests), 0);
    app.focus_sections();
    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char(']')),
        &config,
        &store,
        &tx
    ));
    assert_eq!(app.current_section_position(), 1);
    assert_eq!(app.focus, FocusTarget::Sections);

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('[')),
        &config,
        &store,
        &tx
    ));
    assert_eq!(app.current_section_position(), 0);
    assert_eq!(app.focus, FocusTarget::Sections);

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
fn ghr_tab_navigation_preserves_focus_when_target_view_snapshot_was_details() {
    let fiber_key = "repo:Fiber:pull_requests:Pull Requests";
    let ghr_key = "repo:ghr:pull_requests:Pull Requests";
    let sections = vec![
        test_section(),
        SectionSnapshot {
            key: fiber_key.to_string(),
            kind: SectionKind::PullRequests,
            title: "Pull Requests".to_string(),
            filters: String::new(),
            items: vec![
                work_item("fiber-1", "nervosnetwork/fiber", 1, "First", None),
                work_item("fiber-2", "nervosnetwork/fiber", 2, "Second", None),
            ],
            total_count: None,
            page: 1,
            page_size: 0,
            refreshed_at: None,
            error: None,
        },
        SectionSnapshot {
            key: ghr_key.to_string(),
            kind: SectionKind::PullRequests,
            title: "Pull Requests".to_string(),
            filters: String::new(),
            items: vec![work_item("ghr-1", "chenyukang/ghr", 1, "Ghr", None)],
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
    let fiber_view = repo_view_key("Fiber");
    let tabs = app.view_tabs();
    let fiber_pos = tabs
        .iter()
        .position(|view| view.key == fiber_view)
        .expect("fiber repo view should be a top tab");
    let start_pos = if fiber_pos == 0 {
        tabs.len() - 1
    } else {
        fiber_pos - 1
    };
    let start_view = tabs[start_pos].key.clone();

    app.switch_view(fiber_view.clone());
    app.set_selection(1);
    app.details.insert(
        "fiber-2".to_string(),
        DetailState::Loaded(vec![
            comment("alice", "first", None),
            comment("bob", "second", None),
        ]),
    );
    app.focus_details();
    app.details_scroll = 9;
    app.selected_comment_index = 1;
    app.set_current_list_scroll_offset(4);

    app.switch_view(start_view.clone());
    app.focus_ghr();
    assert_eq!(app.active_view, start_view);
    assert_eq!(app.focus, FocusTarget::Ghr);

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('l')),
        &config,
        &store,
        &tx
    ));
    assert_eq!(app.active_view, fiber_view);
    assert_eq!(app.focus, FocusTarget::Ghr);
    assert_eq!(
        app.current_item().map(|item| item.id.as_str()),
        Some("fiber-2")
    );
    assert_eq!(app.details_scroll, 9);
    assert_eq!(app.selected_comment_index, 1);
    assert_eq!(app.list_scroll_offset.get(fiber_key), Some(&4));

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('l')),
        &config,
        &store,
        &tx
    ));
    assert_ne!(app.active_view, fiber_view);
    assert_eq!(app.focus, FocusTarget::Ghr);
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

    app.switch_view(builtin_view_key(SectionKind::PullRequests));
    app.show_diff();
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
    app.show_diff();
    app.focus_sections();
    assert_eq!(app.current_section_position(), 0);
    assert!(!handle_key(
        &mut app,
        key(KeyCode::Tab),
        &config,
        &store,
        &tx
    ));
    assert_eq!(app.details_mode, DetailsMode::Diff);
    assert_eq!(app.active_view, builtin_view_key(SectionKind::PullRequests));
    assert_eq!(app.current_section_position(), 1);
    assert_eq!(app.focus, FocusTarget::Sections);
}

#[test]
fn tab_toggles_between_list_and_details_in_conversation_mode() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    app.focus_list();

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Tab),
        &config,
        &store,
        &tx
    ));
    assert_eq!(app.details_mode, DetailsMode::Conversation);
    assert_eq!(app.focus, FocusTarget::Details);
    assert_eq!(app.active_view, builtin_view_key(SectionKind::PullRequests));
    assert_eq!(app.current_section_position(), 0);

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Tab),
        &config,
        &store,
        &tx
    ));
    assert_eq!(app.details_mode, DetailsMode::Conversation);
    assert_eq!(app.focus, FocusTarget::List);
    assert_eq!(app.active_view, builtin_view_key(SectionKind::PullRequests));
    assert_eq!(app.current_section_position(), 0);

    assert!(!handle_key(
        &mut app,
        key(KeyCode::BackTab),
        &config,
        &store,
        &tx
    ));
    assert_eq!(app.details_mode, DetailsMode::Conversation);
    assert_eq!(app.focus, FocusTarget::Details);
    assert_eq!(app.active_view, builtin_view_key(SectionKind::PullRequests));
    assert_eq!(app.current_section_position(), 0);
}

#[test]
fn tab_toggles_between_diff_files_and_diff_details_in_diff_mode() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    app.show_diff();
    app.focus_list();

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Tab),
        &config,
        &store,
        &tx
    ));
    assert_eq!(app.details_mode, DetailsMode::Diff);
    assert_eq!(app.focus, FocusTarget::Details);
    assert_eq!(app.status, "details focused");

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Tab),
        &config,
        &store,
        &tx
    ));
    assert_eq!(app.details_mode, DetailsMode::Diff);
    assert_eq!(app.focus, FocusTarget::List);
    assert_eq!(app.status, "files focused");

    assert!(!handle_key(
        &mut app,
        key(KeyCode::BackTab),
        &config,
        &store,
        &tx
    ));
    assert_eq!(app.details_mode, DetailsMode::Diff);
    assert_eq!(app.focus, FocusTarget::Details);
    assert_eq!(app.active_view, builtin_view_key(SectionKind::PullRequests));
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
fn notification_done_local_removes_item_from_all_notification_sections() {
    let done_item = notification_item("thread-1", false);
    let other_item = notification_item("thread-2", true);
    let sections = vec![
        SectionSnapshot {
            key: "notifications:unread".to_string(),
            kind: SectionKind::Notifications,
            title: "Unread".to_string(),
            filters: "is:unread".to_string(),
            items: vec![done_item.clone(), other_item.clone()],
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
            items: vec![done_item],
            total_count: None,
            page: 1,
            page_size: 0,
            refreshed_at: None,
            error: None,
        },
    ];
    let mut app = AppState::new(SectionKind::Notifications, sections);

    assert!(app.apply_notification_done_local("thread-1"));

    assert_eq!(app.sections[0].items.len(), 1);
    assert_eq!(app.sections[0].items[0].id, "thread-2");
    assert!(app.sections[1].items.is_empty());
    assert_eq!(
        app.current_item().map(|item| item.id.as_str()),
        Some("thread-2")
    );
}

#[test]
fn all_notifications_read_local_clears_unread_sections_and_updates_all_items() {
    let before = Utc::now();
    let unread_item = notification_item("thread-1", true);
    let read_item = notification_item("thread-2", false);
    let sections = vec![
        SectionSnapshot {
            key: "notifications:unread".to_string(),
            kind: SectionKind::Notifications,
            title: "Unread".to_string(),
            filters: "is:unread".to_string(),
            items: vec![unread_item.clone()],
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
            items: vec![unread_item, read_item],
            total_count: None,
            page: 1,
            page_size: 0,
            refreshed_at: None,
            error: None,
        },
    ];
    let mut app = AppState::new(SectionKind::Notifications, sections);

    assert!(app.apply_all_notifications_read_local());

    assert!(app.sections[0].items.is_empty());
    assert!(
        app.sections[1]
            .items
            .iter()
            .all(|item| item.unread == Some(false))
    );
    assert!(app.sections[1].items.iter().all(|item| {
        item.last_read_at
            .as_ref()
            .is_some_and(|last_read_at| last_read_at >= &before)
    }));
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
fn notification_done_finished_updates_local_state_and_clears_pending() {
    let sections = vec![SectionSnapshot {
        key: "notifications:all".to_string(),
        kind: SectionKind::Notifications,
        title: "All".to_string(),
        filters: "is:all".to_string(),
        items: vec![notification_item("thread-1", false)],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    }];
    let mut app = AppState::new(SectionKind::Notifications, sections);
    app.notification_done_pending.insert("thread-1".to_string());

    app.handle_msg(AppMsg::NotificationDoneFinished {
        thread_id: "thread-1".to_string(),
        result: Ok(None),
    });

    assert!(!app.notification_done_pending.contains("thread-1"));
    assert!(app.sections[0].items.is_empty());
    assert_eq!(app.status, "notification marked done");
}

#[test]
fn inbox_keyboard_navigation_does_not_mark_notification_read() {
    let sections = vec![SectionSnapshot {
        key: "notifications:all".to_string(),
        kind: SectionKind::Notifications,
        title: "All".to_string(),
        filters: "is:all".to_string(),
        items: vec![
            notification_item("thread-1", true),
            notification_item("thread-2", true),
        ],
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

    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('j')),
        &config,
        &store,
        &tx
    ));

    assert_eq!(
        app.current_item().map(|item| item.id.as_str()),
        Some("thread-2")
    );
    assert!(app.notification_read_pending.is_empty());
    assert!(
        app.sections[0]
            .items
            .iter()
            .all(|item| item.unread == Some(true))
    );
}

#[test]
fn n_and_p_mirror_j_and_k_in_primary_lists() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

    for kind in [
        SectionKind::PullRequests,
        SectionKind::Issues,
        SectionKind::Notifications,
    ] {
        let (key_prefix, title, items) = match kind {
            SectionKind::PullRequests => (
                "pull_requests",
                "Pull Requests",
                vec![
                    work_item("pr-1", "rust-lang/rust", 1, "First PR", None),
                    work_item("pr-2", "rust-lang/rust", 2, "Second PR", None),
                ],
            ),
            SectionKind::Issues => {
                let mut first = work_item("issue-1", "rust-lang/rust", 1, "First issue", None);
                first.kind = ItemKind::Issue;
                let mut second = work_item("issue-2", "rust-lang/rust", 2, "Second issue", None);
                second.kind = ItemKind::Issue;
                ("issues", "Issues", vec![first, second])
            }
            SectionKind::Notifications => (
                "notifications",
                "All",
                vec![
                    notification_item("thread-1", true),
                    notification_item("thread-2", true),
                ],
            ),
        };
        let sections = vec![SectionSnapshot {
            key: format!("{key_prefix}:test"),
            kind,
            title: title.to_string(),
            filters: String::new(),
            items,
            total_count: None,
            page: 1,
            page_size: 0,
            refreshed_at: None,
            error: None,
        }];
        let mut app = AppState::new(kind, sections);

        assert_eq!(app.current_selected_position(), 0);
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('n')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.current_selected_position(), 1, "{kind:?} n moves down");
        assert!(!handle_key(
            &mut app,
            key(KeyCode::Char('p')),
            &config,
            &store,
            &tx
        ));
        assert_eq!(app.current_selected_position(), 0, "{kind:?} p moves up");
        assert_eq!(app.focus, FocusTarget::List, "{kind:?} p stays in list");

        if matches!(kind, SectionKind::Notifications) {
            assert!(app.notification_read_pending.is_empty());
            assert!(
                app.sections[0]
                    .items
                    .iter()
                    .all(|item| item.unread == Some(true))
            );
        }
    }
}

#[test]
fn k_and_p_leave_primary_list_at_first_item_then_continue_to_ghr() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

    for code in [KeyCode::Char('k'), KeyCode::Char('p')] {
        let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
        app.focus_list();
        assert_eq!(app.current_selected_position(), 0);

        assert!(!handle_key(&mut app, key(code), &config, &store, &tx));
        assert_eq!(app.focus, FocusTarget::Sections);
        assert_eq!(app.current_selected_position(), 0);

        assert!(!handle_key(&mut app, key(code), &config, &store, &tx));
        assert_eq!(app.focus, FocusTarget::Ghr);
    }
}

#[test]
fn inbox_table_hover_does_not_mark_notification_read() {
    let sections = vec![SectionSnapshot {
        key: "notifications:all".to_string(),
        kind: SectionKind::Notifications,
        title: "All".to_string(),
        filters: "is:all".to_string(),
        items: vec![notification_item("thread-1", true)],
        total_count: None,
        page: 1,
        page_size: 0,
        refreshed_at: None,
        error: None,
    }];
    let mut app = AppState::new(SectionKind::Notifications, sections);
    let (tx, _rx) = mpsc::unbounded_channel();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));
    let area = Rect::new(0, 0, 100, 40);
    let table = body_areas(body_area(area))[0];
    let inner = block_inner(table);

    handle_mouse_with_sync(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Moved,
            column: inner.x + 4,
            row: inner.y + TABLE_HEADER_HEIGHT,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
        area,
        Some(&store),
        Some(&tx),
    );

    assert!(app.notification_read_pending.is_empty());
    assert_eq!(app.sections[0].items[0].unread, Some(true));
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
    assert_eq!(app.details_scroll, 3);
    assert_eq!(
        app.current_section().map(|section| section.title.as_str()),
        Some("Test")
    );
}

#[test]
fn ignore_key_hides_selected_issue_or_pull_request_and_persists() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let (tx, _rx) = mpsc::unbounded_channel();
    let config = Config::default();
    let store = SnapshotStore::new(std::path::PathBuf::from("/tmp/ghr-test-unused.db"));

    assert_eq!(app.current_item().map(|item| item.id.as_str()), Some("1"));
    assert!(!handle_key(
        &mut app,
        key(KeyCode::Char('i')),
        &config,
        &store,
        &tx
    ));

    assert!(app.ignored_items.contains("1"));
    assert_eq!(app.current_item().map(|item| item.id.as_str()), Some("2"));
    assert_eq!(
        app.filtered_indices(app.current_section().unwrap()),
        vec![1]
    );
    assert_eq!(app.ui_state().ignored_items, vec!["1"]);
    assert!(app.status.contains("ignored pull request #1"));
}

#[test]
fn ignored_items_restore_from_ui_state() {
    let state = UiState {
        ignored_items: vec!["1".to_string()],
        ..UiState::default()
    };

    let app = AppState::with_ui_state(SectionKind::PullRequests, vec![test_section()], state);

    assert_eq!(app.current_item().map(|item| item.id.as_str()), Some("2"));
    assert_eq!(
        section_count_label(&app, app.current_section().unwrap()),
        "1"
    );
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
}

fn ctrl_key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, crossterm::event::KeyModifiers::CONTROL)
}

fn cmd_key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, crossterm::event::KeyModifiers::SUPER)
}

fn cmd_shift_key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(
        code,
        crossterm::event::KeyModifiers::SUPER | crossterm::event::KeyModifiers::SHIFT,
    )
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

fn unique_test_paths(prefix: &str) -> Paths {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("ghr-{prefix}-{}-{unique}", std::process::id()));
    std::fs::create_dir_all(&root).expect("create test root");
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

#[test]
fn comments_auto_refresh_starts_after_interval_when_details_loaded() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let item = app.current_item().expect("current item").clone();
    let now = Instant::now();
    app.focus = FocusTarget::Details;
    app.details
        .insert(item.id.clone(), DetailState::Loaded(Vec::new()));
    app.comments_refresh_requested_at
        .insert(item.id.clone(), now - COMMENTS_AUTO_REFRESH_INTERVAL);

    assert!(app.start_comments_auto_refresh_if_due(&item, now));

    assert!(app.details_refreshing.contains(&item.id));
    assert_eq!(app.comments_refresh_requested_at.get(&item.id), Some(&now));
}

#[test]
fn comments_auto_refresh_waits_until_interval_and_skips_in_flight_loads() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let item = app.current_item().expect("current item").clone();
    let now = Instant::now();
    app.focus = FocusTarget::Details;
    app.details
        .insert(item.id.clone(), DetailState::Loaded(Vec::new()));
    app.comments_refresh_requested_at.insert(
        item.id.clone(),
        now - COMMENTS_AUTO_REFRESH_INTERVAL + Duration::from_millis(1),
    );

    assert!(!app.start_comments_auto_refresh_if_due(&item, now));

    app.comments_refresh_requested_at
        .insert(item.id.clone(), now - COMMENTS_AUTO_REFRESH_INTERVAL);
    app.details_refreshing.insert(item.id.clone());
    assert!(!app.start_comments_auto_refresh_if_due(&item, now));
}

#[test]
fn comments_auto_refresh_honors_post_refresh_delay() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let item = app.current_item().expect("current item").clone();
    let now = Instant::now();
    app.focus = FocusTarget::Details;
    app.details
        .insert(item.id.clone(), DetailState::Loaded(Vec::new()));
    app.comments_refresh_requested_at
        .insert(item.id.clone(), now);
    app.comments_refresh_after
        .insert(item.id.clone(), now + COMMENTS_POST_REFRESH_DELAY);

    assert!(!app.start_comments_auto_refresh_if_due(
        &item,
        now + COMMENTS_POST_REFRESH_DELAY - Duration::from_millis(1)
    ));
    assert!(app.start_comments_auto_refresh_if_due(&item, now + COMMENTS_POST_REFRESH_DELAY));
    assert!(!app.comments_refresh_after.contains_key(&item.id));
}

#[test]
fn comments_auto_refresh_skips_when_details_not_focused() {
    let mut app = AppState::new(SectionKind::PullRequests, vec![test_section()]);
    let item = app.current_item().expect("current item").clone();
    let now = Instant::now();
    app.focus = FocusTarget::List;
    app.details
        .insert(item.id.clone(), DetailState::Loaded(Vec::new()));
    app.comments_refresh_requested_at
        .insert(item.id.clone(), now - COMMENTS_AUTO_REFRESH_INTERVAL);

    assert!(!app.start_comments_auto_refresh_if_due(&item, now));
    assert!(!app.details_refreshing.contains(&item.id));
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

fn test_diff_file(path: &str, additions: usize, deletions: usize) -> DiffFile {
    DiffFile {
        old_path: path.to_string(),
        new_path: path.to_string(),
        metadata: Vec::new(),
        hunks: Vec::new(),
        additions,
        deletions,
    }
}

fn test_review_comment(id: u64, path: &str) -> CommentPreview {
    let mut comment = comment("alice", "inline", None);
    comment.id = Some(id);
    comment.review = Some(crate::model::ReviewCommentPreview {
        path: path.to_string(),
        line: Some(1),
        original_line: None,
        start_line: None,
        original_start_line: None,
        side: Some("RIGHT".to_string()),
        start_side: None,
        diff_hunk: None,
        is_resolved: false,
        is_outdated: false,
    });
    comment
}

fn token_end_column(line: &str, token: &str) -> usize {
    let start = line.find(token).expect("token column");
    display_width(&line[..start]) + display_width(token)
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
        remote: None,
        local_dir: Some(local_dir.display().to_string()),
        show_prs: true,
        show_issues: true,
        labels: Vec::new(),
        pr_labels: Vec::new(),
        issue_labels: Vec::new(),
    });
    config
}

fn checkout_test_repo_dir() -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let counter = CHECKOUT_TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "ghr-checkout-test-{}-{unique}-{counter}",
        std::process::id()
    ));
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

fn checkout_test_repo_dir_on_branch(branch: &str) -> PathBuf {
    let dir = checkout_test_repo_dir();
    let checkout = Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["checkout", "-q", "-b", branch])
        .output()
        .expect("run git checkout -b");
    assert!(
        checkout.status.success(),
        "git checkout -b failed: {}",
        command_output_text(&checkout.stdout, &checkout.stderr)
    );
    dir
}

fn checkout_test_repo_dir_with_main_and_branch(branch: &str, branch_commit: bool) -> PathBuf {
    let dir = checkout_test_repo_dir();
    run_checkout_test_git(&dir, &["config", "user.email", "ghr@example.com"]);
    run_checkout_test_git(&dir, &["config", "user.name", "ghr test"]);
    run_checkout_test_git(&dir, &["checkout", "-q", "-b", "main"]);
    std::fs::write(dir.join("README.md"), "base\n").expect("write base file");
    run_checkout_test_git(&dir, &["add", "README.md"]);
    run_checkout_test_git(&dir, &["commit", "-q", "-m", "base"]);
    run_checkout_test_git(&dir, &["update-ref", "refs/remotes/origin/main", "main"]);
    run_checkout_test_git(
        &dir,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    run_checkout_test_git(&dir, &["checkout", "-q", "-b", branch]);
    if branch_commit {
        std::fs::write(dir.join("feature.txt"), "feature\n").expect("write feature file");
        run_checkout_test_git(&dir, &["add", "feature.txt"]);
        run_checkout_test_git(&dir, &["commit", "-q", "-m", "feature"]);
    }
    dir
}

fn checkout_test_fork_repo_dir_on_branch(
    branch: &str,
    fork_repo: &str,
    base_repo: &str,
) -> PathBuf {
    let dir = checkout_test_repo_dir_on_branch(branch);
    run_checkout_test_git(
        &dir,
        &[
            "remote",
            "set-url",
            "origin",
            &format!("https://github.com/{fork_repo}.git"),
        ],
    );
    run_checkout_test_git(
        &dir,
        &[
            "remote",
            "add",
            "upstream",
            &format!("https://github.com/{base_repo}.git"),
        ],
    );
    run_checkout_test_git(
        &dir,
        &["config", &format!("branch.{branch}.remote"), "origin"],
    );
    run_checkout_test_git(
        &dir,
        &[
            "config",
            &format!("branch.{branch}.merge"),
            &format!("refs/heads/{branch}"),
        ],
    );
    dir
}

fn run_checkout_test_git(dir: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        command_output_text(&output.stdout, &output.stderr)
    );
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

fn assert_document_link_for_text(document: &DetailsDocument, text: &str, expected_url: &str) {
    let rendered = document
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let line_index = rendered
        .iter()
        .position(|line| line.contains(text))
        .expect("link text line");
    let column = display_width(
        &rendered[line_index][..rendered[line_index].find(text).expect("link text column")],
    ) as u16;
    assert_eq!(
        document.link_at(line_index, column).as_deref(),
        Some(expected_url)
    );
}

fn assert_document_action_for_text(
    document: &DetailsDocument,
    text: &str,
    expected_action: DetailAction,
) {
    assert_document_action_for_text_on_line(document, text, text, expected_action);
}

fn assert_document_action_for_text_on_line(
    document: &DetailsDocument,
    line_text: &str,
    text: &str,
    expected_action: DetailAction,
) {
    let rendered = document
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let line_index = rendered
        .iter()
        .position(|line| line.contains(line_text))
        .expect("action text line");
    let column = display_width(
        &rendered[line_index][..rendered[line_index].find(text).expect("action text column")],
    ) as u16;
    assert_eq!(
        document.action_at(line_index, column),
        Some(expected_action)
    );
}

fn comment(author: &str, body: &str, url: Option<&str>) -> CommentPreview {
    CommentPreview {
        id: None,
        kind: CommentPreviewKind::Comment,
        author: author.to_string(),
        body: body.to_string(),
        created_at: None,
        updated_at: None,
        url: url.map(str::to_string),
        parent_id: None,
        is_mine: false,
        viewer_can_update: None,
        reactions: ReactionSummary::default(),
        review: None,
    }
}

fn own_comment(id: u64, author: &str, body: &str, url: Option<&str>) -> CommentPreview {
    CommentPreview {
        id: Some(id),
        kind: CommentPreviewKind::Comment,
        author: author.to_string(),
        body: body.to_string(),
        created_at: None,
        updated_at: None,
        url: url.map(str::to_string),
        parent_id: None,
        is_mine: true,
        viewer_can_update: None,
        reactions: ReactionSummary::default(),
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
        created_at: None,
        updated_at: None,
        last_read_at: None,
        labels: vec!["T-compiler".to_string()],
        reactions: ReactionSummary::default(),
        milestone: None,
        assignees: Vec::new(),
        comments: Some(0),
        unread: None,
        reason: None,
        extra: None,
        viewer_subscription: None,
    }
}

fn recent_item(
    id: &str,
    kind: ItemKind,
    repo: &str,
    number: u64,
    title: &str,
    visited_at: i64,
) -> RecentItem {
    let path_kind = match kind {
        ItemKind::Issue => "issues",
        _ => "pull",
    };
    RecentItem {
        id: id.to_string(),
        kind,
        repo: repo.to_string(),
        number,
        title: title.to_string(),
        url: format!("https://github.com/{repo}/{path_kind}/{number}"),
        visited_at: DateTime::from_timestamp(visited_at, 0).unwrap(),
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
        created_at: None,
        updated_at: None,
        last_read_at: None,
        labels: Vec::new(),
        reactions: ReactionSummary::default(),
        milestone: None,
        assignees: Vec::new(),
        comments: None,
        unread: Some(unread),
        reason: Some("mention".to_string()),
        extra: Some("PullRequest".to_string()),
        viewer_subscription: None,
    }
}
