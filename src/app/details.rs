use pulldown_cmark::{CodeBlockKind, Event as MarkdownEvent, Options, Parser, Tag, TagEnd};

use super::*;
use crate::theme::active_theme;

const DETAILS_METADATA_PADDING: usize = 2;
const DETAILS_METADATA_KEY_WIDTH: usize = 11;
const DESCRIPTION_BODY_PADDING: usize = 2;
const INLINE_COMMENT_MARKER: &str = "💬 ";
const INLINE_COMMENT_MULTIPLE_MARKER: &str = "💬* ";

#[derive(Debug, Clone)]
pub(super) struct DetailsDocument {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) links: Vec<LinkRegion>,
    pub(super) actions: Vec<ActionRegion>,
    pub(super) copy_exclusions: Vec<CopyExclusionRegion>,
    pub(super) copy_skip_lines: Vec<usize>,
    pub(super) description: Option<DescriptionRegion>,
    pub(super) comments: Vec<CommentRegion>,
    pub(super) diff_files: Vec<usize>,
    pub(super) diff_lines: Vec<DiffLineRegion>,
    pub(super) inline_comment_markers: Vec<DiffInlineCommentMarkerRegion>,
    pub(super) selected_diff_line: Option<usize>,
}

impl DetailsDocument {
    pub(super) fn link_at(&self, line: usize, column: u16) -> Option<String> {
        self.links
            .iter()
            .find(|link| link.line == line && column >= link.start && column < link.end)
            .map(|link| link.url.clone())
    }

    pub(super) fn action_at(&self, line: usize, column: u16) -> Option<DetailAction> {
        self.actions
            .iter()
            .find(|action| action.line == line && column >= action.start && column < action.end)
            .map(|action| action.action.clone())
    }

    pub(super) fn comment_at(&self, line: usize) -> Option<usize> {
        self.comments
            .iter()
            .find(|comment| line >= comment.start_line && line < comment.end_line)
            .map(|comment| comment.index)
    }

    pub(super) fn comment_region(&self, index: usize) -> Option<&CommentRegion> {
        self.comments.iter().find(|comment| comment.index == index)
    }

    pub(super) fn description_at(&self, line: usize) -> bool {
        self.description
            .as_ref()
            .is_some_and(|description| description.contains(line))
    }

    pub(super) fn diff_line_at(&self, line: usize) -> Option<usize> {
        self.diff_lines
            .iter()
            .find(|diff_line| diff_line.line == line)
            .map(|diff_line| diff_line.review_index)
    }

    pub(super) fn inline_comment_marker_at(&self, line: usize) -> Option<&[usize]> {
        self.inline_comment_markers
            .iter()
            .find(|marker| marker.line == line)
            .map(|marker| marker.comment_indices.as_slice())
    }
}

#[derive(Debug, Clone)]
pub(super) struct LinkRegion {
    pub(super) line: usize,
    pub(super) start: u16,
    pub(super) end: u16,
    pub(super) url: String,
}

#[derive(Debug, Clone)]
pub(super) struct ActionRegion {
    pub(super) line: usize,
    pub(super) start: u16,
    pub(super) end: u16,
    pub(super) action: DetailAction,
}

#[derive(Debug, Clone)]
pub(super) struct CopyExclusionRegion {
    pub(super) line: usize,
    pub(super) start: u16,
    pub(super) end: u16,
}

#[derive(Debug, Clone)]
pub(super) struct DescriptionRegion {
    pub(super) start_line: usize,
    pub(super) end_line: usize,
}

impl DescriptionRegion {
    fn contains(&self, line: usize) -> bool {
        line >= self.start_line && line < self.end_line
    }
}

#[derive(Debug, Clone)]
pub(super) struct CommentRegion {
    pub(super) index: usize,
    pub(super) start_line: usize,
    pub(super) end_line: usize,
}

impl CommentRegion {
    pub(super) fn focus_line(&self) -> usize {
        self.start_line
            .saturating_add(1)
            .min(self.end_line.saturating_sub(1))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CommentDisplayEntry {
    pub(super) index: usize,
    pub(super) depth: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CommentCollapseState {
    pub(super) long: bool,
    pub(super) collapsed: bool,
    pub(super) line_count: usize,
    pub(super) char_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CommentRenderOptions {
    pub(super) selected: bool,
    pub(super) search_match: bool,
    pub(super) depth: usize,
    pub(super) collapse: CommentCollapseState,
    pub(super) new_since_last_read: bool,
}

#[derive(Debug, Clone)]
pub(super) struct DiffLineRegion {
    pub(super) line: usize,
    pub(super) review_index: usize,
}

#[derive(Debug, Clone)]
pub(super) struct DiffInlineCommentMarkerRegion {
    pub(super) line: usize,
    pub(super) comment_indices: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DiffTreeEntry {
    pub(super) file_index: Option<usize>,
    pub(super) label: String,
    pub(super) stats: Option<DiffFileStats>,
    pub(super) depth: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DiffFileStats {
    pub(super) status: &'static str,
    pub(super) additions: usize,
    pub(super) deletions: usize,
    pub(super) comments: usize,
}

impl DiffFileStats {
    #[cfg(test)]
    pub(super) fn label(&self) -> String {
        let mut stats = format!("{} +{} -{}", self.status, self.additions, self.deletions);
        if self.comments > 0 {
            stats.push_str(&format!(" {}c", self.comments));
        }
        stats
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum DiffReviewSide {
    Left,
    Right,
}

impl DiffReviewSide {
    pub(super) fn as_api_value(self) -> &'static str {
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
pub(super) struct DiffReviewTarget {
    pub(super) path: String,
    pub(super) line: usize,
    pub(super) side: DiffReviewSide,
    pub(super) start_line: Option<usize>,
    pub(super) start_side: Option<DiffReviewSide>,
    pub(super) preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct DiffInlineCommentKey {
    pub(super) path: String,
    pub(super) line: usize,
    pub(super) side: DiffReviewSide,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct DiffInlineCommentSummary {
    pub(super) count: usize,
    pub(super) has_resolved: bool,
    pub(super) has_outdated: bool,
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

pub(super) struct DiffRenderContext<'a> {
    item_id: &'a str,
    comments: Option<&'a [CommentPreview]>,
    expanded_comments: &'a HashSet<String>,
    diff_inline_comments_visible: bool,
    revealed_diff_inline_comments: Option<&'a HashSet<usize>>,
    details_focused: bool,
    selected_comment_index: usize,
    selected_file: usize,
    selected_line: usize,
    selected_range: Option<(usize, usize)>,
    file_link_base: Option<DiffFileLinkBase>,
    show_thread_markers: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiffFileLinkBase {
    repository: String,
    branch: String,
}

impl DiffReviewTarget {
    pub(super) fn location_label(&self) -> String {
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

    pub(super) fn is_range(&self) -> bool {
        self.start_line.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DetailAction {
    ReplyComment(usize),
    EditComment(usize),
    ToggleCommentExpanded(usize),
    ReactItem,
    ReactComment(usize),
    AddLabel,
    RemoveLabel(String),
    AssignAssignee,
    UnassignAssignee,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DetailSegment {
    pub(super) text: String,
    pub(super) style: Style,
    pub(super) link: Option<String>,
    pub(super) action: Option<DetailAction>,
    pub(super) copyable: bool,
}

impl DetailSegment {
    fn raw(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: Style::default(),
            link: None,
            action: None,
            copyable: true,
        }
    }

    fn styled(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
            link: None,
            action: None,
            copyable: true,
        }
    }

    fn chrome(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: Style::default(),
            link: None,
            action: None,
            copyable: false,
        }
    }

    fn styled_chrome(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
            link: None,
            action: None,
            copyable: false,
        }
    }

    fn link(text: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: link_style(),
            link: Some(url.into()),
            action: None,
            copyable: true,
        }
    }

    fn styled_link(text: impl Into<String>, url: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
            link: Some(url.into()),
            action: None,
            copyable: true,
        }
    }

    fn action(text: impl Into<String>, action: DetailAction) -> Self {
        Self {
            text: text.into(),
            style: action_style(),
            link: None,
            action: Some(action),
            copyable: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MarkdownBlock {
    pub(super) quote_depth: u8,
    pub(super) kind: MarkdownBlockKind,
    pub(super) gap_before: bool,
    pub(super) segments: Vec<DetailSegment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MarkdownBlockKind {
    Text,
    ListItem,
    TableRow,
    Code { language: CodeLanguage },
}

#[derive(Debug, Clone, Default)]
pub(super) struct MarkdownTable {
    pub(super) rows: Vec<MarkdownTableRow>,
}

#[derive(Debug, Clone)]
pub(super) struct MarkdownTableRow {
    pub(super) cells: Vec<Vec<DetailSegment>>,
    pub(super) header: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct MarkdownImage {
    pub(super) url: String,
    pub(super) alt: Option<String>,
    pub(super) title: Option<String>,
    pub(super) width: Option<String>,
    pub(super) height: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CodeLanguage {
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
pub(super) enum WrapTokenKind {
    Word,
    Whitespace,
    Newline,
}

#[derive(Debug, Clone)]
pub(super) struct WrapToken {
    pub(super) kind: WrapTokenKind,
    pub(super) segments: Vec<DetailSegment>,
    pub(super) width: usize,
}

pub(super) struct MarkdownRenderOptions {
    pub(super) prefix: Vec<DetailSegment>,
    pub(super) right_padding: usize,
}

pub(super) struct DetailsBuilder {
    pub(super) document: DetailsDocument,
    pub(super) width: usize,
}

impl DetailsBuilder {
    pub(super) fn new(width: u16) -> Self {
        Self {
            document: DetailsDocument {
                lines: Vec::new(),
                links: Vec::new(),
                actions: Vec::new(),
                copy_exclusions: Vec::new(),
                copy_skip_lines: Vec::new(),
                description: None,
                comments: Vec::new(),
                diff_files: Vec::new(),
                diff_lines: Vec::new(),
                inline_comment_markers: Vec::new(),
                selected_diff_line: None,
            },
            width: usize::from(width.max(1)),
        }
    }

    pub(super) fn finish(self) -> DetailsDocument {
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

    fn mark_inline_comment_marker(&mut self, comment_indices: Vec<usize>) {
        if comment_indices.is_empty() {
            return;
        }
        let line = self.document.lines.len();
        self.document
            .inline_comment_markers
            .push(DiffInlineCommentMarkerRegion {
                line,
                comment_indices,
            });
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
            if !segment.copyable && width > 0 {
                self.document.copy_exclusions.push(CopyExclusionRegion {
                    line: line_index,
                    start: column,
                    end: column.saturating_add(width),
                });
            }
            column = column.saturating_add(width);
            spans.push(Span::styled(segment.text, segment.style));
        }
        self.document.lines.push(Line::from(spans));
    }

    fn push_chrome_line(&mut self, segments: Vec<DetailSegment>) {
        let line_index = self.document.lines.len();
        self.push_line(segments);
        self.document.copy_skip_lines.push(line_index);
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
        self.push_styled_key_value_limited(key, value, 1);
    }

    fn push_styled_key_value_limited(
        &mut self,
        key: &str,
        value: Vec<DetailSegment>,
        max_lines: usize,
    ) {
        let mut segments = vec![metadata_key_segment(key)];
        segments.extend(value);
        self.push_metadata_wrapped_limited(key, segments, max_lines);
    }

    fn push_link_value(&mut self, key: &str, url: &str) {
        self.push_metadata_wrapped_limited(
            key,
            vec![
                metadata_key_segment(key),
                DetailSegment::link(url.to_string(), url.to_string()),
            ],
            3,
        );
    }

    fn push_meta_line(&mut self, fields: Vec<(&str, Vec<DetailSegment>)>) {
        let first_key = fields.first().map(|(key, _)| *key).unwrap_or("");
        let mut segments = Vec::new();
        for (index, (key, mut value)) in fields.into_iter().enumerate() {
            if index > 0 {
                segments.push(DetailSegment::raw("  "));
                if let Some(padding) = metadata_inline_key_padding(key) {
                    segments.push(padding);
                }
            }
            segments.push(metadata_key_segment(key));
            segments.append(&mut value);
        }
        self.push_metadata_wrapped_limited(first_key, segments, 2);
    }

    fn push_metadata_wrapped_limited(
        &mut self,
        first_key: &str,
        segments: Vec<DetailSegment>,
        max_lines: usize,
    ) {
        self.push_prefixed_wrapped_limited(
            segments,
            metadata_padding_prefix(first_key),
            DETAILS_METADATA_PADDING,
            max_lines,
        );
    }

    #[cfg(test)]
    pub(super) fn push_markdown_block_indented(
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

                    if let Some(segments) =
                        truncated_clickable_token_segments(&token, content_width)
                    {
                        append_segments(&mut current, &segments);
                        column += segments_width(&segments);
                        wrote_content = true;
                    } else if !self.push_hard_wrapped_token(
                        &token,
                        prefix,
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
                let char_width = display_width_char(ch);
                if column > prefix_width && column.saturating_add(char_width) > self.width {
                    if !self.flush_wrapped_line(&mut current, emitted, max_lines) {
                        return false;
                    }
                    current = prefix.to_vec();
                    column = prefix_width;
                }
                push_char_segment(&mut current, segment, ch);
                column = column.saturating_add(char_width);
            }
        }

        self.flush_wrapped_line(&mut current, emitted, max_lines)
    }

    #[allow(clippy::too_many_arguments)]
    fn push_hard_wrapped_token(
        &mut self,
        token: &WrapToken,
        prefix: &[DetailSegment],
        current: &mut Vec<DetailSegment>,
        column: &mut usize,
        wrote_content: &mut bool,
        emitted: &mut usize,
        max_lines: usize,
    ) -> bool {
        let prefix_width = segments_width(prefix);
        for segment in &token.segments {
            for ch in segment.text.chars() {
                let char_width = display_width_char(ch);
                if *column > prefix_width && (*column).saturating_add(char_width) > self.width {
                    if !self.flush_wrapped_line(current, emitted, max_lines) {
                        return false;
                    }
                    *current = prefix.to_vec();
                    *column = prefix_width;
                    *wrote_content = false;
                }

                push_char_segment(current, segment, ch);
                *column = (*column).saturating_add(char_width);
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

pub(super) fn wrap_tokens(segments: &[DetailSegment]) -> Vec<WrapToken> {
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

pub(super) fn push_wrap_token_char(
    tokens: &mut Vec<WrapToken>,
    template: &DetailSegment,
    ch: char,
    kind: WrapTokenKind,
) {
    if let Some(last) = tokens.last_mut()
        && last.kind == kind
    {
        push_char_segment(&mut last.segments, template, ch);
        last.width = last.width.saturating_add(display_width_char(ch));
        return;
    }

    let mut segments = Vec::new();
    push_char_segment(&mut segments, template, ch);
    let width = display_width_char(ch);
    tokens.push(WrapToken {
        kind,
        segments,
        width,
    });
}

pub(super) fn append_token_segments(current: &mut Vec<DetailSegment>, token: &WrapToken) {
    for segment in &token.segments {
        push_text_segment(current, segment, &segment.text);
    }
}

pub(super) fn append_segments(current: &mut Vec<DetailSegment>, segments: &[DetailSegment]) {
    for segment in segments {
        push_text_segment(current, segment, &segment.text);
    }
}

pub(super) fn truncated_clickable_token_segments(
    token: &WrapToken,
    max_width: usize,
) -> Option<Vec<DetailSegment>> {
    let first = token.segments.first()?;
    if first.link.is_none() && first.action.is_none() {
        return None;
    }
    if token.segments.iter().any(|segment| {
        segment.style != first.style || segment.link != first.link || segment.action != first.action
    }) {
        return None;
    }

    let text = token
        .segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<String>();
    Some(vec![DetailSegment {
        text: truncate_inline(&text, max_width),
        style: first.style,
        link: first.link.clone(),
        action: first.action.clone(),
        copyable: first.copyable,
    }])
}

pub(super) fn push_text_segment(
    current: &mut Vec<DetailSegment>,
    template: &DetailSegment,
    text: &str,
) {
    if text.is_empty() {
        return;
    }

    if let Some(last) = current.last_mut()
        && last.style == template.style
        && last.link == template.link
        && last.action == template.action
        && last.copyable == template.copyable
    {
        last.text.push_str(text);
        return;
    }

    current.push(DetailSegment {
        text: text.to_string(),
        style: template.style,
        link: template.link.clone(),
        action: template.action.clone(),
        copyable: template.copyable,
    });
}

pub(super) fn trim_trailing_wrap_whitespace(
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

pub(super) fn reserved_width(width: usize, right_padding: usize) -> usize {
    width.saturating_sub(right_padding).max(1)
}

pub(super) fn padding_prefix(width: usize) -> Vec<DetailSegment> {
    if width == 0 {
        Vec::new()
    } else {
        vec![DetailSegment::raw(" ".repeat(width))]
    }
}

fn metadata_padding_prefix(key: &str) -> Vec<DetailSegment> {
    padding_prefix(DETAILS_METADATA_PADDING + metadata_key_alignment_padding(key))
}

fn metadata_key_segment(key: &str) -> DetailSegment {
    DetailSegment::styled(format!("{key}: "), active_theme().muted())
}

fn metadata_inline_key_padding(key: &str) -> Option<DetailSegment> {
    let padding = metadata_key_alignment_padding(key);
    (padding > 0).then(|| DetailSegment::raw(" ".repeat(padding)))
}

fn metadata_key_alignment_padding(key: &str) -> usize {
    DETAILS_METADATA_KEY_WIDTH.saturating_sub(display_width(key))
}

fn pull_request_status_segments(item: &WorkItem) -> Vec<DetailSegment> {
    if item_is_draft_pull_request(item) {
        vec![DetailSegment::styled(
            "DRAFT",
            active_theme()
                .panel()
                .fg(active_theme().error)
                .add_modifier(Modifier::BOLD),
        )]
    } else {
        vec![DetailSegment::styled(
            "READY",
            active_theme()
                .panel()
                .fg(active_theme().success)
                .add_modifier(Modifier::BOLD),
        )]
    }
}

fn item_state_segments(item: &WorkItem) -> Vec<DetailSegment> {
    let state = item.state.clone().unwrap_or_else(|| "-".to_string());
    let style = match state.to_ascii_lowercase().as_str() {
        "open" => Some(
            active_theme()
                .panel()
                .fg(active_theme().success)
                .add_modifier(Modifier::BOLD),
        ),
        "merged" => Some(
            active_theme()
                .panel()
                .fg(active_theme().link)
                .add_modifier(Modifier::BOLD),
        ),
        "closed" => Some(
            active_theme()
                .panel()
                .fg(active_theme().error)
                .add_modifier(Modifier::BOLD),
        ),
        _ => None,
    };

    match style {
        Some(style) => vec![DetailSegment::styled(state, style)],
        None => vec![DetailSegment::raw(state)],
    }
}

pub(super) fn build_details_document(app: &AppState, width: u16) -> DetailsDocument {
    if app.details_mode == DetailsMode::Diff {
        return build_diff_document(app, width);
    }
    build_conversation_document(app, width)
}

pub(super) fn build_conversation_document(app: &AppState, width: u16) -> DetailsDocument {
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
            active_theme()
                .panel()
                .fg(active_theme().focus)
                .add_modifier(Modifier::BOLD),
        )],
        3,
    );
    builder.push_blank();
    if notification_has_new_since_last_read(item) {
        builder.push_wrapped_limited(notification_new_since_last_read_segments(item), 2);
    }

    let mut identity_meta = vec![
        ("repo", vec![DetailSegment::raw(item.repo.clone())]),
        (
            "number",
            vec![DetailSegment::raw(
                item.number
                    .map(|number| format!("#{number}"))
                    .unwrap_or_else(|| "-".to_string()),
            )],
        ),
    ];
    if matches!(item.kind, ItemKind::PullRequest) {
        identity_meta.push(("status", pull_request_status_segments(item)));
    }
    builder.push_meta_line(identity_meta);

    let mut state_meta = vec![("state", item_state_segments(item))];
    if matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest) {
        state_meta.push((
            "created",
            vec![DetailSegment::raw(local_datetime(item.created_at))],
        ));
    }
    state_meta.push((
        "updated",
        vec![DetailSegment::raw(relative_time(item.updated_at))],
    ));
    builder.push_meta_line(state_meta);

    let mut secondary_meta = Vec::new();
    let mut branch_meta = None;
    let mut queue_meta = None;
    let mut action_meta = Vec::new();
    let mut action_note = None;
    let mut reviewer_meta = None;
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
    if matches!(item.kind, ItemKind::PullRequest) {
        secondary_meta.push((
            "commits",
            commit_count_segments(app.action_hints.get(&item.id), item),
        ));
    }
    if matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest) {
        secondary_meta.push((
            "milestone",
            vec![DetailSegment::raw(
                item.milestone
                    .as_ref()
                    .map(|milestone| milestone.title.clone())
                    .unwrap_or_else(|| "-".to_string()),
            )],
        ));
    }
    if let Some(reason) = useful_meta_value(item.reason.as_deref()) {
        secondary_meta.push(("reason", vec![DetailSegment::raw(reason.to_string())]));
    }
    if matches!(item.kind, ItemKind::PullRequest) {
        let (action_segments, note) = action_hint_segments(app.action_hints.get(&item.id));
        branch_meta = Some(branch_hint_segments(app.action_hints.get(&item.id)));
        queue_meta = merge_queue_hint_segments(app.action_hints.get(&item.id));
        action_meta.push(("action", action_segments));
        action_meta.push((
            "checks",
            check_hint_segments(app.action_hints.get(&item.id)),
        ));
        if let Some(review_segments) = review_hint_segments(app.action_hints.get(&item.id)) {
            action_meta.push(("reviews", review_segments));
        }
        action_note = note;
        reviewer_meta = review_actor_hint_segments(app.action_hints.get(&item.id));
    }
    if !secondary_meta.is_empty() {
        builder.push_meta_line(secondary_meta);
    }
    if let Some(branch_segments) = branch_meta {
        builder.push_styled_key_value("branch", branch_segments);
    }
    if let Some(queue_segments) = queue_meta {
        builder.push_styled_key_value("queue", queue_segments);
    }
    if !action_meta.is_empty() {
        builder.push_meta_line(action_meta);
    }
    if let Some(note) = action_note {
        builder.push_styled_key_value("action note", action_note_segments(&note));
    }
    if let Some(reviewer_segments) = reviewer_meta {
        builder.push_styled_key_value_limited("reviewers", reviewer_segments, 2);
    }
    builder.push_link_value("url", &item.url);

    if matches!(item.kind, ItemKind::Issue | ItemKind::PullRequest) {
        builder.push_blank();
        builder.push_meta_line(vec![("assignees", assignee_detail_segments(item))]);
        push_label_controls(&mut builder, &item.labels);
    }

    if let Some(extra) = &item.extra {
        builder.push_blank();
        builder.push_key_value("extra", extra.clone());
    }

    builder.push_blank();
    push_description_block(&mut builder, app, item);
    push_reactions_line(&mut builder, &item.reactions, item.supports_reactions());

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
                        CommentRenderOptions {
                            selected: app.mouse_capture_enabled
                                && app.focus == FocusTarget::Details
                                && index == app.selected_comment_index,
                            search_match,
                            depth: if app.mouse_capture_enabled {
                                entry.depth
                            } else {
                                0
                            },
                            collapse,
                            new_since_last_read: comment_new_since_last_read(
                                comment,
                                item.last_read_at.as_ref(),
                            ),
                        },
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

pub(super) fn push_label_controls(builder: &mut DetailsBuilder, labels: &[String]) {
    let mut segments = Vec::new();
    if !labels.is_empty() {
        for (index, label) in labels.iter().enumerate() {
            if index > 0 {
                segments.push(DetailSegment::raw("  "));
            }
            segments.push(DetailSegment::styled(label.clone(), label_style()));
            segments.push(DetailSegment::action(
                "×",
                DetailAction::RemoveLabel(label.clone()),
            ));
        }
    }
    segments.push(DetailSegment::raw(if labels.is_empty() {
        " "
    } else {
        "  "
    }));
    segments.push(DetailSegment::action("+", DetailAction::AddLabel));
    builder.push_styled_key_value_limited("labels", segments, 3);
}

pub(super) fn push_description_block(
    builder: &mut DetailsBuilder,
    app: &AppState,
    item: &WorkItem,
) {
    let selected = app.mouse_capture_enabled
        && app.focus == FocusTarget::Details
        && app.comment_selection_cleared();
    let start_line = builder.document.lines.len();
    if !selected {
        builder.push_heading("Description");
        builder.push_blank();
        builder.push_markdown_block_prefixed(
            item.body.as_deref().unwrap_or(""),
            "No description.",
            usize::MAX,
            usize::MAX,
            MarkdownRenderOptions {
                prefix: padding_prefix(DESCRIPTION_BODY_PADDING),
                right_padding: DESCRIPTION_BODY_PADDING,
            },
        );
        builder.document.description = Some(DescriptionRegion {
            start_line,
            end_line: builder.document.lines.len(),
        });
        return;
    }

    push_selected_description_box_edge(builder, CommentBoxEdge::Top);
    let content_start_line = builder.document.lines.len();
    builder.push_prefixed_wrapped_limited(
        vec![DetailSegment::styled(
            "Description",
            description_selected_heading_style(),
        )],
        selected_description_prefix(),
        comment_right_padding(true),
        1,
    );
    builder.push_line(selected_description_prefix());
    builder.push_markdown_block_prefixed(
        item.body.as_deref().unwrap_or(""),
        "No description.",
        usize::MAX,
        usize::MAX,
        MarkdownRenderOptions {
            prefix: selected_description_prefix(),
            right_padding: comment_right_padding(true),
        },
    );
    add_selected_description_text_weight(builder, content_start_line, builder.document.lines.len());
    push_selected_description_box_edge(builder, CommentBoxEdge::Bottom);
    add_description_right_border(
        builder,
        content_start_line,
        builder.document.lines.len() - 1,
    );
    builder.document.description = Some(DescriptionRegion {
        start_line,
        end_line: builder.document.lines.len(),
    });
}

pub(super) fn build_diff_document(app: &AppState, width: u16) -> DetailsDocument {
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
            active_theme()
                .panel()
                .fg(active_theme().focus)
                .add_modifier(Modifier::BOLD),
        )],
        3,
    );
    builder.push_blank();
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
                    diff_inline_comments_visible: app.diff_inline_comments_visible,
                    revealed_diff_inline_comments: app.revealed_diff_inline_comments.get(&item.id),
                    details_focused: app.focus == FocusTarget::Details,
                    selected_comment_index: app.selected_comment_index,
                    selected_file,
                    selected_line,
                    selected_range: app.diff_mark_range_for(&item.id),
                    file_link_base: diff_file_link_base(item, app.action_hints.get(&item.id)),
                    show_thread_markers: app.mouse_capture_enabled,
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

pub(super) fn push_details_mode_tabs(builder: &mut DetailsBuilder, active: DetailsMode) {
    let tab = |label: &'static str, mode: DetailsMode| {
        let style = if active == mode {
            active_theme()
                .panel()
                .fg(active_theme().warning)
                .add_modifier(Modifier::BOLD)
        } else {
            active_theme().muted()
        };
        DetailSegment::styled(label, style)
    };
    builder.push_line(vec![
        tab("Conversation", DetailsMode::Conversation),
        DetailSegment::styled(" | ", active_theme().subtle()),
        tab("Diff", DetailsMode::Diff),
    ]);
}

pub(super) fn assignee_detail_segments(item: &WorkItem) -> Vec<DetailSegment> {
    let mut segments = Vec::new();
    if item.assignees.is_empty() {
        segments.push(DetailSegment::raw("-"));
    } else {
        for (index, assignee) in item.assignees.iter().enumerate() {
            if index > 0 {
                segments.push(DetailSegment::raw(", "));
            }
            segments.push(DetailSegment::link(
                assignee.clone(),
                github_profile_url(assignee),
            ));
        }
    }
    segments.push(DetailSegment::raw("  "));
    segments.push(DetailSegment::action(
        "@ assign",
        DetailAction::AssignAssignee,
    ));
    if !item.assignees.is_empty() {
        segments.push(DetailSegment::raw("  "));
        segments.push(DetailSegment::action(
            "- unassign",
            DetailAction::UnassignAssignee,
        ));
    }
    segments
}

pub(super) fn push_diff(
    builder: &mut DetailsBuilder,
    diff: &PullRequestDiff,
    context: DiffRenderContext<'_>,
) {
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
    let mut rendered_inline_comment_indices = HashSet::new();

    builder.push_line(vec![DetailSegment::styled(
        format!("file {}/{}", selected_file + 1, diff.files.len()),
        diff_metadata_style(),
    )]);

    builder.push_blank();
    builder.mark_diff_file();
    push_diff_file_header(
        builder,
        file,
        context.file_link_base.as_ref(),
        context.selected_line,
    );
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
            for entry in inline_entries {
                rendered_inline_comment_indices.insert(entry.index);
            }
            let line_review_index = target.as_ref().map(|_| {
                let index = review_index;
                review_index += 1;
                index
            });
            let inline_summary = diff_inline_comment_summary(context.comments, inline_entries);
            if inline_summary.count > 0 {
                builder.mark_inline_comment_marker(
                    inline_entries.iter().map(|entry| entry.index).collect(),
                );
            }
            push_diff_line(
                builder,
                line,
                line_review_index,
                line_review_index.is_some_and(|index| {
                    index == context.selected_line || index_in_range(index, context.selected_range)
                }),
                inline_summary,
            );
            if let Some(comments) = context.comments {
                if context.diff_inline_comments_visible {
                    push_diff_inline_comments(
                        builder,
                        context.item_id,
                        comments,
                        inline_entries,
                        context.expanded_comments,
                        context.details_focused,
                        context.selected_comment_index,
                        context.show_thread_markers,
                    );
                } else if let Some(revealed) = context.revealed_diff_inline_comments {
                    let revealed_entries = inline_entries
                        .iter()
                        .filter(|entry| revealed.contains(&entry.index))
                        .cloned()
                        .collect::<Vec<_>>();
                    push_diff_inline_comments(
                        builder,
                        context.item_id,
                        comments,
                        &revealed_entries,
                        context.expanded_comments,
                        context.details_focused,
                        context.selected_comment_index,
                        context.show_thread_markers,
                    );
                }
            }
        }
    }
    if context.diff_inline_comments_visible
        && let Some(comments) = context.comments
    {
        let unplaced_entries =
            diff_unplaced_review_comment_entries(comments, file, &rendered_inline_comment_indices);
        if !unplaced_entries.is_empty() {
            builder.push_blank();
            builder.push_line(vec![DetailSegment::styled(
                "Resolved/outdated comments not attached to a current diff line",
                diff_metadata_style(),
            )]);
            push_diff_inline_comments(
                builder,
                context.item_id,
                comments,
                &unplaced_entries,
                context.expanded_comments,
                context.details_focused,
                context.selected_comment_index,
                context.show_thread_markers,
            );
        }
    }
}

fn push_diff_file_header(
    builder: &mut DetailsBuilder,
    file: &DiffFile,
    link_base: Option<&DiffFileLinkBase>,
    selected_line: usize,
) {
    let path = if file.old_path == file.new_path {
        file.new_path.clone()
    } else {
        format!("{} -> {}", file.old_path, file.new_path)
    };
    let path = truncate_inline(&path, builder.width.saturating_sub(16).max(1));
    let path_segment = diff_file_blob_url(file, link_base, selected_line)
        .map(|url| DetailSegment::styled_link(path.clone(), url, diff_file_link_style()))
        .unwrap_or_else(|| DetailSegment::styled(path, diff_file_style()));
    builder.push_line(vec![
        DetailSegment::styled("▾ ", diff_file_style()),
        path_segment,
        DetailSegment::raw("  "),
        DetailSegment::styled(format!("+{}", file.additions), diff_added_style()),
        DetailSegment::raw(" "),
        DetailSegment::styled(format!("-{}", file.deletions), diff_removed_style()),
    ]);
}

fn diff_file_link_base(
    item: &WorkItem,
    action_hints: Option<&ActionHintState>,
) -> Option<DiffFileLinkBase> {
    if let Some(ActionHintState::Loaded(hints)) = action_hints
        && let Some(head) = &hints.head
    {
        return Some(DiffFileLinkBase {
            repository: head.repository.clone(),
            branch: head.branch.clone(),
        });
    }

    (!item.repo.trim().is_empty()).then(|| DiffFileLinkBase {
        repository: item.repo.clone(),
        branch: "HEAD".to_string(),
    })
}

fn diff_file_blob_url(
    file: &DiffFile,
    link_base: Option<&DiffFileLinkBase>,
    selected_line: usize,
) -> Option<String> {
    let link_base = link_base?;
    let path = diff_display_path(file);
    if path == "/dev/null" {
        return None;
    }
    let line = diff_file_nearby_line(file, selected_line)
        .map(|line| format!("#L{line}"))
        .unwrap_or_default();
    Some(format!(
        "https://github.com/{}/blob/{}/{}{}",
        link_base.repository,
        github_url_path(&link_base.branch),
        github_url_path(&path),
        line
    ))
}

fn diff_file_nearby_line(file: &DiffFile, selected_line: usize) -> Option<usize> {
    let lines = diff_reviewable_lines(file);
    if lines.is_empty() {
        return None;
    }

    let selected_line = selected_line.min(lines.len().saturating_sub(1));
    let changed = if file.new_path == "/dev/null" {
        nearest_diff_line(&lines, selected_line, |line| {
            matches!(line.kind, DiffLineKind::Removed)
        })
        .and_then(|line| line.old_line)
    } else {
        nearest_diff_line(&lines, selected_line, |line| {
            matches!(line.kind, DiffLineKind::Added)
        })
        .and_then(|line| line.new_line)
    };
    if changed.is_some() {
        return changed;
    }

    lines[selected_line]
        .new_line
        .or(lines[selected_line].old_line)
}

fn diff_reviewable_lines(file: &DiffFile) -> Vec<&DiffLine> {
    file.hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .filter(|line| !matches!(line.kind, DiffLineKind::Metadata))
        .collect()
}

fn nearest_diff_line<'a>(
    lines: &'a [&DiffLine],
    selected_line: usize,
    matches: impl Fn(&DiffLine) -> bool,
) -> Option<&'a DiffLine> {
    if matches(lines[selected_line]) {
        return Some(lines[selected_line]);
    }

    let max_distance = selected_line.max(lines.len().saturating_sub(selected_line + 1));
    for distance in 1..=max_distance {
        if let Some(line) = lines.get(selected_line + distance)
            && matches(line)
        {
            return Some(line);
        }
        if let Some(index) = selected_line.checked_sub(distance)
            && let Some(line) = lines.get(index)
            && matches(line)
        {
            return Some(line);
        }
    }
    None
}

fn github_url_path(path: &str) -> String {
    path.split('/')
        .map(github_url_component)
        .collect::<Vec<_>>()
        .join("/")
}

fn github_url_component(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

pub(super) fn push_diff_line(
    builder: &mut DetailsBuilder,
    line: &DiffLine,
    review_index: Option<usize>,
    selected: bool,
    inline_comment_summary: DiffInlineCommentSummary,
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
        active_theme()
            .panel()
            .fg(active_theme().highlight_fg)
            .bg(active_theme().focus_alt)
            .add_modifier(Modifier::BOLD)
    } else {
        diff_gutter_style()
    };
    if selected {
        style = style
            .bg(active_theme().selected_bg)
            .add_modifier(Modifier::BOLD);
    }
    let inline_comment_marker = if inline_comment_summary.count > 0 {
        Some(diff_inline_comment_marker(inline_comment_summary))
    } else {
        None
    };
    let comment_marker_width = inline_comment_marker
        .as_ref()
        .map(|(marker, _)| display_width(marker))
        .unwrap_or(0);
    let prefix_width = display_width(&gutter) + comment_marker_width + display_width(marker) + 1;
    let content_width = builder.width.saturating_sub(prefix_width).max(1);
    let mut segments = vec![DetailSegment::styled(gutter, gutter_style)];
    if let Some((marker, style)) = inline_comment_marker {
        segments.push(DetailSegment::styled(marker, style));
    }
    segments.extend([
        DetailSegment::styled(marker, style),
        DetailSegment::styled(" ", style),
        DetailSegment::styled(truncate_inline(&line.text, content_width), style),
    ]);
    builder.push_line(segments);
}

pub(super) fn push_diff_inline_comments(
    builder: &mut DetailsBuilder,
    item_id: &str,
    comments: &[CommentPreview],
    entries: &[CommentDisplayEntry],
    expanded_comments: &HashSet<String>,
    details_focused: bool,
    selected_comment_index: usize,
    show_thread_markers: bool,
) {
    for entry in entries {
        let Some(comment) = comments.get(entry.index) else {
            continue;
        };
        let selected = details_focused && entry.index == selected_comment_index;
        let collapse = comment_collapse_state_for(item_id, entry.index, comment, expanded_comments);
        let depth = if show_thread_markers { entry.depth } else { 0 };
        push_diff_inline_comment(builder, entry.index, comment, selected, depth, collapse);
    }
}

pub(super) fn push_diff_inline_comment(
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
    push_diff_inline_comment_separator(builder, selected, depth, CommentBoxEdge::Top);
    let content_start_line = builder.document.lines.len();

    let mut header = vec![
        DetailSegment::styled(
            comment_header_marker(comment, selected, true),
            comment_status_marker_style(comment, selected),
        ),
        comment_author_link_segment(&comment.author, selected),
        DetailSegment::raw(format!(" - {}", relative_time(timestamp))),
    ];
    if let Some(url) = &comment.url {
        header.push(DetailSegment::raw("  "));
        header.push(DetailSegment::link("open", url.clone()));
    }
    if let Some(review) = &comment.review {
        append_review_state_segments(&mut header, review);
    }
    append_reaction_segments(&mut header, &comment.reactions);
    if !comment.kind.is_activity() {
        header.push(DetailSegment::raw("  "));
        header.push(DetailSegment::action(
            "+ react",
            DetailAction::ReactComment(index),
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
    builder.push_prefixed_wrapped_limited(
        header,
        prefix.clone(),
        comment_right_padding(selected),
        2,
    );
    push_comment_body_gap(builder, &prefix);
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
        push_diff_inline_comment_expand_line(builder, index, selected, depth, collapse);
    }
    if selected {
        add_selected_comment_text_weight(builder, content_start_line, builder.document.lines.len());
        push_diff_inline_comment_separator(builder, true, depth, CommentBoxEdge::Bottom);
        add_comment_right_border(builder, start_line + 1, builder.document.lines.len() - 1);
    }
    builder.document.comments.push(CommentRegion {
        index,
        start_line,
        end_line: builder.document.lines.len(),
    });
}

#[derive(Clone, Copy)]
pub(super) enum CommentBoxEdge {
    Top,
    Bottom,
}

pub(super) fn push_diff_inline_comment_separator(
    builder: &mut DetailsBuilder,
    selected: bool,
    depth: usize,
    edge: CommentBoxEdge,
) {
    if selected {
        push_selected_comment_box_edge(builder, DIFF_INLINE_COMMENT_GUTTER_WIDTH, edge);
        return;
    }

    let mut segments = diff_inline_comment_prefix(selected, depth);
    let prefix_width = segments_width(&segments);
    let width = builder
        .width
        .saturating_sub(prefix_width + comment_right_padding(selected))
        .max(12);
    let line = if selected { "━" } else { "─" };
    segments.push(DetailSegment::styled_chrome(
        line.repeat(width),
        comment_separator_style(selected),
    ));
    builder.push_chrome_line(segments);
}

pub(super) fn push_selected_comment_box_edge(
    builder: &mut DetailsBuilder,
    left_padding: usize,
    edge: CommentBoxEdge,
) {
    let (left_corner, right_corner) = match edge {
        CommentBoxEdge::Top => ("┏", "┓"),
        CommentBoxEdge::Bottom => ("┗", "┛"),
    };
    let border_column = comment_right_border_column(builder.width);
    let horizontal_width = border_column.saturating_sub(left_padding + 1).max(1);
    let style = comment_separator_style(true);
    let mut segments = Vec::new();
    if left_padding > 0 {
        segments.push(DetailSegment::chrome(" ".repeat(left_padding)));
    }
    segments.extend([
        DetailSegment::styled_chrome(left_corner, style),
        DetailSegment::styled_chrome("━".repeat(horizontal_width), style),
        DetailSegment::styled_chrome(right_corner, style),
    ]);
    builder.push_chrome_line(segments);
}

pub(super) fn push_diff_inline_comment_expand_line(
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
        comment_right_padding(selected),
        2,
    );
}

pub(super) fn diff_inline_comment_prefix(selected: bool, depth: usize) -> Vec<DetailSegment> {
    let depth = depth.min(4);
    let mut prefix = " ".repeat(DIFF_INLINE_COMMENT_GUTTER_WIDTH);
    prefix.push_str(if selected { "┃ " } else { "│ " });
    if depth > 0 {
        prefix.push_str(&"  ".repeat(depth.saturating_sub(1)));
        prefix.push_str("↳ ");
    }
    vec![DetailSegment::styled_chrome(
        prefix,
        if selected {
            comment_selected_rail_style()
        } else {
            comment_thread_style()
        },
    )]
}

pub(super) fn diff_tree_entries(diff: &PullRequestDiff) -> Vec<DiffTreeEntry> {
    diff_tree_entries_with_comment_counts(diff, &HashMap::new())
}

pub(super) fn diff_tree_entries_with_comment_counts(
    diff: &PullRequestDiff,
    comment_counts: &HashMap<usize, usize>,
) -> Vec<DiffTreeEntry> {
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
                stats: Some(diff_file_stats(
                    &diff.files[file_index],
                    comment_counts.get(&file_index).copied().unwrap_or(0),
                )),
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
                    stats: None,
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
            stats: Some(diff_file_stats(
                &diff.files[file_index],
                comment_counts.get(&file_index).copied().unwrap_or(0),
            )),
            depth: parts.len().saturating_sub(1),
        });
    }

    entries
}

pub(super) fn diff_tree_row_index_for_file(
    entries: &[DiffTreeEntry],
    file_index: usize,
) -> Option<usize> {
    entries
        .iter()
        .position(|entry| entry.file_index == Some(file_index))
}

pub(super) fn diff_display_path(file: &DiffFile) -> String {
    if file.new_path != "/dev/null" {
        file.new_path.clone()
    } else {
        file.old_path.clone()
    }
}

pub(super) fn diff_file_details_scroll_key(item_id: &str, file: &DiffFile) -> String {
    format!("{item_id}::{}", diff_display_path(file))
}

pub(super) fn diff_file_stats(file: &DiffFile, comment_count: usize) -> DiffFileStats {
    DiffFileStats {
        status: diff_file_status(file),
        additions: file.additions,
        deletions: file.deletions,
        comments: comment_count,
    }
}

pub(super) fn diff_file_status(file: &DiffFile) -> &'static str {
    match (file.old_path.as_str(), file.new_path.as_str()) {
        ("/dev/null", _) => "A",
        (_, "/dev/null") => "D",
        (old, new) if old != new => "R",
        _ => "M",
    }
}

pub(super) fn diff_review_targets(file: &DiffFile) -> Vec<DiffReviewTarget> {
    file.hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .filter_map(|line| diff_review_target(file, line))
        .collect()
}

pub(super) fn ordered_range(left: usize, right: usize) -> (usize, usize) {
    if left <= right {
        (left, right)
    } else {
        (right, left)
    }
}

pub(super) fn index_in_range(index: usize, range: Option<(usize, usize)>) -> bool {
    let Some((start, end)) = range.map(|(start, end)| ordered_range(start, end)) else {
        return false;
    };
    index >= start && index <= end
}

pub(super) fn diff_review_target_from_range(
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

pub(super) fn diff_review_target(file: &DiffFile, line: &DiffLine) -> Option<DiffReviewTarget> {
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

pub(super) fn diff_review_path(file: &DiffFile, side: DiffReviewSide) -> String {
    match side {
        DiffReviewSide::Left if file.new_path == "/dev/null" => file.old_path.clone(),
        _ if file.new_path != "/dev/null" => file.new_path.clone(),
        _ => file.old_path.clone(),
    }
}

pub(super) fn diff_inline_comment_map(
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

pub(super) fn diff_inline_comment_summary(
    comments: Option<&[CommentPreview]>,
    entries: &[CommentDisplayEntry],
) -> DiffInlineCommentSummary {
    let Some(comments) = comments else {
        return DiffInlineCommentSummary::default();
    };
    let mut summary = DiffInlineCommentSummary {
        count: entries.len(),
        ..DiffInlineCommentSummary::default()
    };
    for entry in entries {
        let Some(review) = comments
            .get(entry.index)
            .and_then(|comment| comment.review.as_ref())
        else {
            continue;
        };
        summary.has_resolved |= review.is_resolved;
        summary.has_outdated |= review.is_outdated;
    }
    summary
}

pub(super) fn diff_inline_comment_marker(
    summary: DiffInlineCommentSummary,
) -> (&'static str, Style) {
    let marker = if summary.count > 9 {
        INLINE_COMMENT_MULTIPLE_MARKER
    } else {
        INLINE_COMMENT_MARKER
    };
    let style = if summary.has_outdated {
        review_outdated_style()
    } else if summary.has_resolved {
        review_resolved_style()
    } else {
        diff_inline_comment_marker_style()
    };
    (marker, style)
}

pub(super) fn diff_unplaced_review_comment_entries(
    comments: &[CommentPreview],
    file: &DiffFile,
    rendered_indices: &HashSet<usize>,
) -> Vec<CommentDisplayEntry> {
    comment_display_entries(comments)
        .into_iter()
        .filter(|entry| !rendered_indices.contains(&entry.index))
        .filter(|entry| {
            comments
                .get(entry.index)
                .and_then(|comment| comment.review.as_ref())
                .is_some_and(|review| {
                    (review.is_resolved || review.is_outdated)
                        && review_comment_path_matches_file(review, file)
                })
        })
        .collect()
}

pub(super) fn review_comment_path_matches_file(
    review: &crate::model::ReviewCommentPreview,
    file: &DiffFile,
) -> bool {
    review.path == file.new_path
        || review.path == file.old_path
        || review.path == diff_display_path(file)
}

pub(super) fn diff_file_comment_counts(
    diff: &PullRequestDiff,
    comments: &[CommentPreview],
) -> HashMap<usize, usize> {
    let mut counts = HashMap::new();
    let mut counted = HashSet::new();
    for (key, entries) in diff_inline_comment_map(comments) {
        let Some(file_index) = diff_file_index_for_comment_path(diff, &key.path) else {
            continue;
        };
        for entry in entries {
            if counted.insert(entry.index) {
                *counts.entry(file_index).or_insert(0) += 1;
            }
        }
    }

    for (index, comment) in comments.iter().enumerate() {
        if counted.contains(&index) {
            continue;
        }
        let Some(review) = comment.review.as_ref() else {
            continue;
        };
        let Some(file_index) = diff_file_index_for_comment_path(diff, &review.path) else {
            continue;
        };
        counted.insert(index);
        *counts.entry(file_index).or_insert(0) += 1;
    }

    counts
}

pub(super) fn diff_file_index_for_comment_path(
    diff: &PullRequestDiff,
    path: &str,
) -> Option<usize> {
    diff.files.iter().position(|file| {
        path == file.new_path || path == file.old_path || path == diff_display_path(file)
    })
}

pub(super) fn diff_inline_comment_key_for_comment(
    comment: &CommentPreview,
) -> Option<DiffInlineCommentKey> {
    let review = comment.review.as_ref()?;
    let line = usize::try_from(review_display_line(review)?).ok()?;
    let side = diff_review_side_from_label(review_display_side(review)?)?;
    Some(DiffInlineCommentKey {
        path: review.path.clone(),
        line,
        side,
    })
}

pub(super) fn diff_review_side_from_label(label: &str) -> Option<DiffReviewSide> {
    match label {
        label if label.eq_ignore_ascii_case("LEFT") => Some(DiffReviewSide::Left),
        label if label.eq_ignore_ascii_case("RIGHT") => Some(DiffReviewSide::Right),
        _ => None,
    }
}

pub(super) fn diff_gutter(old_line: Option<usize>, new_line: Option<usize>) -> String {
    let old = old_line
        .map(|line| format!("{line:>4}"))
        .unwrap_or_else(|| "    ".to_string());
    let new = new_line
        .map(|line| format!("{line:>4}"))
        .unwrap_or_else(|| "    ".to_string());
    format!("{old} {new} │ ")
}

pub(super) fn useful_meta_value(value: Option<&str>) -> Option<&str> {
    let value = value?.trim();
    if value.is_empty() || value == "-" {
        None
    } else {
        Some(value)
    }
}

pub(super) fn github_profile_url(author: &str) -> String {
    format!("https://github.com/{author}")
}

pub(super) fn comment_author_link_segment(author: &str, selected: bool) -> DetailSegment {
    DetailSegment::styled_link(
        author.to_string(),
        github_profile_url(author),
        comment_author_style(selected).add_modifier(Modifier::UNDERLINED),
    )
}

pub(super) fn comment_header_marker(
    comment: &CommentPreview,
    selected: bool,
    inline: bool,
) -> &'static str {
    if selected {
        return "▸ ";
    }
    let Some(review) = &comment.review else {
        return if inline { INLINE_COMMENT_MARKER } else { "  " };
    };
    if inline {
        return INLINE_COMMENT_MARKER;
    }
    if review.is_outdated {
        "◌ "
    } else if review.is_resolved {
        "✓ "
    } else {
        "  "
    }
}

pub(super) fn comment_status_marker_style(comment: &CommentPreview, selected: bool) -> Style {
    if selected {
        return comment_marker_style(true);
    }
    let Some(review) = &comment.review else {
        return comment_marker_style(false);
    };
    if review.is_outdated {
        review_outdated_style()
    } else if review.is_resolved {
        review_resolved_style()
    } else {
        diff_inline_comment_marker_style()
    }
}

pub(super) fn append_review_state_segments(
    segments: &mut Vec<DetailSegment>,
    review: &crate::model::ReviewCommentPreview,
) {
    if review.is_resolved {
        segments.push(DetailSegment::raw("  "));
        segments.push(DetailSegment::styled("resolved", review_resolved_style()));
    }
    if review.is_outdated {
        segments.push(DetailSegment::raw("  "));
        segments.push(DetailSegment::styled("outdated", review_outdated_style()));
    }
}

pub(super) fn push_reactions_line(
    builder: &mut DetailsBuilder,
    reactions: &ReactionSummary,
    can_react: bool,
) {
    if reactions.is_empty() && !can_react {
        return;
    }
    builder.push_blank();
    let mut segments = vec![DetailSegment::styled("reactions: ", active_theme().muted())];
    if !reactions.is_empty() {
        for (index, segment) in reaction_segments(reactions).into_iter().enumerate() {
            if index > 0 {
                segments.push(DetailSegment::raw("  "));
            }
            segments.push(segment);
        }
    }
    if can_react {
        segments.push(DetailSegment::raw(if reactions.is_empty() {
            " "
        } else {
            "  "
        }));
        segments.push(DetailSegment::action("+ react", DetailAction::ReactItem));
    }
    builder.push_prefixed_wrapped_limited(
        segments,
        padding_prefix(DESCRIPTION_BODY_PADDING),
        DESCRIPTION_BODY_PADDING,
        2,
    );
}

pub(super) fn notification_has_new_since_last_read(item: &WorkItem) -> bool {
    if item.unread.unwrap_or(false) {
        return true;
    }
    match (item.updated_at.as_ref(), item.last_read_at.as_ref()) {
        (Some(updated_at), Some(last_read_at)) => updated_at > last_read_at,
        _ => false,
    }
}

pub(super) fn notification_new_since_last_read_segments(item: &WorkItem) -> Vec<DetailSegment> {
    let mut details = Vec::new();
    if item.unread.unwrap_or(false) {
        details.push("unread".to_string());
    }
    if let Some(updated_at) = item.updated_at.as_ref().cloned() {
        details.push(format!("updated {}", relative_time(Some(updated_at))));
    }
    if let Some(last_read_at) = item.last_read_at.as_ref().cloned() {
        details.push(format!("last read {}", local_datetime(Some(last_read_at))));
    }

    let mut segments = vec![DetailSegment::styled(
        "New since last read".to_string(),
        new_since_last_read_style(),
    )];
    if !details.is_empty() {
        segments.push(DetailSegment::raw(format!(" - {}", details.join("; "))));
    }
    segments
}

pub(super) fn comment_new_since_last_read(
    comment: &CommentPreview,
    last_read_at: Option<&DateTime<Utc>>,
) -> bool {
    let Some(last_read_at) = last_read_at else {
        return false;
    };
    comment
        .updated_at
        .as_ref()
        .or(comment.created_at.as_ref())
        .is_some_and(|timestamp| timestamp > last_read_at)
}

pub(super) fn append_reaction_segments(
    segments: &mut Vec<DetailSegment>,
    reactions: &ReactionSummary,
) {
    if reactions.is_empty() {
        return;
    }
    for segment in reaction_segments(reactions) {
        segments.push(DetailSegment::raw("  "));
        segments.push(segment);
    }
}

pub(super) fn reaction_segments(reactions: &ReactionSummary) -> Vec<DetailSegment> {
    [
        ("👍", reactions.plus_one),
        ("👎", reactions.minus_one),
        ("😄", reactions.laugh),
        ("🎉", reactions.hooray),
        ("😕", reactions.confused),
        ("❤️", reactions.heart),
        ("🚀", reactions.rocket),
        ("👀", reactions.eyes),
    ]
    .into_iter()
    .filter(|(_, count)| *count > 0)
    .map(|(emoji, count)| DetailSegment::styled(format!("{emoji} {count}"), reaction_style()))
    .collect()
}

pub(super) fn push_comment(
    builder: &mut DetailsBuilder,
    index: usize,
    comment: &CommentPreview,
    options: CommentRenderOptions,
) {
    let timestamp = comment
        .updated_at
        .as_ref()
        .or(comment.created_at.as_ref())
        .cloned();
    let start_line = builder.document.lines.len();
    push_comment_separator(
        builder,
        options.selected,
        options.depth,
        CommentBoxEdge::Top,
    );
    let content_start_line = builder.document.lines.len();

    let mut header = vec![
        DetailSegment::styled(
            comment_header_marker(comment, options.selected, false),
            comment_status_marker_style(comment, options.selected),
        ),
        comment_author_link_segment(&comment.author, options.selected),
        DetailSegment::raw(format!(" - {}", relative_time(timestamp))),
    ];
    if let Some(url) = &comment.url {
        header.push(DetailSegment::raw("  "));
        header.push(DetailSegment::link("open", url.clone()));
    }
    append_reaction_segments(&mut header, &comment.reactions);
    if !comment.kind.is_activity() {
        header.push(DetailSegment::raw("  "));
        header.push(DetailSegment::action(
            "+ react",
            DetailAction::ReactComment(index),
        ));
    }
    if options.search_match {
        header.push(DetailSegment::styled(
            "  match",
            comment_search_match_style(),
        ));
    }
    if options.new_since_last_read {
        header.push(DetailSegment::styled(
            "  New since last read",
            new_since_last_read_style(),
        ));
    }
    if options.collapse.long {
        header.push(DetailSegment::raw("  "));
        header.push(DetailSegment::action(
            if options.collapse.collapsed {
                "▸ collapsed"
            } else {
                "▾ expanded"
            },
            DetailAction::ToggleCommentExpanded(index),
        ));
    }
    if !comment.kind.is_activity() {
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
    }
    let prefix = comment_line_prefix(options.selected, options.depth);
    builder.push_prefixed_wrapped_limited(
        header,
        prefix.clone(),
        comment_right_padding(options.selected),
        2,
    );
    if let Some(review) = &comment.review {
        builder.push_prefixed_wrapped_limited(
            review_comment_metadata_segments(review),
            prefix.clone(),
            comment_right_padding(options.selected),
            2,
        );
    }
    push_comment_body_gap(builder, &prefix);
    if options.selected
        && let Some(review) = &comment.review
    {
        push_inline_review_context(builder, review, options.selected, options.depth);
    }
    let collapsed_body;
    let body = if options.collapse.collapsed {
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
            right_padding: comment_right_padding(options.selected),
        },
    );
    if options.collapse.collapsed {
        push_comment_expand_line(
            builder,
            index,
            options.selected,
            options.depth,
            options.collapse,
        );
    }
    if options.selected {
        add_selected_comment_text_weight(builder, content_start_line, builder.document.lines.len());
        push_comment_separator(builder, true, options.depth, CommentBoxEdge::Bottom);
        add_comment_right_border(builder, start_line + 1, builder.document.lines.len() - 1);
    }
    builder.document.comments.push(CommentRegion {
        index,
        start_line,
        end_line: builder.document.lines.len(),
    });
}

pub(super) fn push_comment_body_gap(builder: &mut DetailsBuilder, prefix: &[DetailSegment]) {
    builder.push_line(prefix.to_vec());
}

pub(super) fn add_selected_comment_text_weight(
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

pub(super) fn add_selected_description_text_weight(
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

pub(super) fn push_selected_description_box_edge(
    builder: &mut DetailsBuilder,
    edge: CommentBoxEdge,
) {
    let (left_corner, right_corner) = match edge {
        CommentBoxEdge::Top => ("┏", "┓"),
        CommentBoxEdge::Bottom => ("┗", "┛"),
    };
    let border_column = comment_right_border_column(builder.width);
    let horizontal_width = border_column.saturating_sub(1).max(1);
    let style = description_selected_separator_style();
    builder.push_chrome_line(vec![
        DetailSegment::styled_chrome(left_corner, style),
        DetailSegment::styled_chrome("━".repeat(horizontal_width), style),
        DetailSegment::styled_chrome(right_corner, style),
    ]);
}

pub(super) fn long_comment_metrics(comment: &CommentPreview) -> (usize, usize) {
    let text = normalize_text(&comment.body);
    let line_count = if text.is_empty() {
        0
    } else {
        text.lines().count()
    };
    (line_count, text.chars().count())
}

pub(super) fn is_long_comment(comment: &CommentPreview) -> bool {
    let (line_count, char_count) = long_comment_metrics(comment);
    line_count > COMMENT_COLLAPSE_MIN_LINES || char_count > COMMENT_COLLAPSE_MIN_CHARS
}

pub(super) fn comment_collapse_state_for(
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

pub(super) fn comment_expand_key(item_id: &str, index: usize, comment: &CommentPreview) -> String {
    match comment.id {
        Some(id) => format!("{item_id}:comment:{id}"),
        None => format!("{item_id}:comment-index:{index}"),
    }
}

pub(super) fn collapsed_comment_body(body: &str) -> String {
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

pub(super) fn push_comment_expand_line(
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

pub(super) fn push_inline_review_context(
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

pub(super) fn inline_diff_focus_span(
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

pub(super) fn inline_diff_focus_range_candidates(
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

pub(super) fn push_inline_diff_focus_range_candidate<'a>(
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

pub(super) fn inline_diff_focus_line(
    lines: &[DiffLine],
    review: &crate::model::ReviewCommentPreview,
) -> Option<usize> {
    inline_diff_focus_candidates(review)
        .into_iter()
        .find_map(|(target, side)| find_inline_diff_line(lines, target, side))
}

pub(super) fn inline_diff_focus_candidates(
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

pub(super) fn push_inline_diff_focus_candidate<'a>(
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

pub(super) fn find_inline_diff_line(
    lines: &[DiffLine],
    target: usize,
    side: Option<&str>,
) -> Option<usize> {
    lines
        .iter()
        .position(|line| diff_line_matches_side(line, target, side))
}

pub(super) fn diff_line_matches_side(line: &DiffLine, target: usize, side: Option<&str>) -> bool {
    match side.unwrap_or_default() {
        side if side.eq_ignore_ascii_case("LEFT") => line.old_line == Some(target),
        side if side.eq_ignore_ascii_case("RIGHT") => line.new_line == Some(target),
        _ => line.new_line == Some(target) || line.old_line == Some(target),
    }
}

pub(super) fn review_display_line(review: &crate::model::ReviewCommentPreview) -> Option<u64> {
    review.line.or(review.original_line)
}

pub(super) fn review_display_start_line(
    review: &crate::model::ReviewCommentPreview,
) -> Option<u64> {
    review.start_line.or(review.original_start_line)
}

pub(super) fn review_display_side(review: &crate::model::ReviewCommentPreview) -> Option<&str> {
    review.side.as_deref().or(review.start_side.as_deref())
}

pub(super) fn review_line_label(review: &crate::model::ReviewCommentPreview) -> String {
    let end = review_display_line(review);
    let start = review_display_start_line(review);
    match (start, end) {
        (Some(start), Some(end)) if start != end => format!("{start}-{end}"),
        (_, Some(end)) => end.to_string(),
        (Some(start), None) => start.to_string(),
        (None, None) => "-".to_string(),
    }
}

pub(super) fn inline_diff_context_range(
    len: usize,
    focus_span: Option<(usize, usize)>,
) -> (usize, usize) {
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

pub(super) fn push_inline_diff_ellipsis(builder: &mut DetailsBuilder, prefix: &[DetailSegment]) {
    let mut segments = prefix.to_vec();
    segments.push(DetailSegment::styled("      ⋮", diff_metadata_style()));
    builder.push_line(segments);
}

pub(super) fn push_inline_diff_line(
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
        style = style
            .fg(active_theme().warning)
            .add_modifier(Modifier::BOLD);
    }
    let gutter_style = if focused {
        active_theme()
            .panel()
            .fg(active_theme().warning)
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

pub(super) fn compact_diff_gutter(old_line: Option<usize>, new_line: Option<usize>) -> String {
    let old = old_line
        .map(|line| format!("{line:>4}"))
        .unwrap_or_else(|| "    ".to_string());
    let new = new_line
        .map(|line| format!("{line:>4}"))
        .unwrap_or_else(|| "    ".to_string());
    format!("{old} {new} │ ")
}

pub(super) fn details_comment_count(app: &AppState, item: &WorkItem) -> Option<usize> {
    match app.details.get(&item.id) {
        Some(DetailState::Loaded(comments)) => Some(comments.len()),
        _ => item.comments.map(|comments| comments as usize),
    }
}

pub(super) fn review_comment_label(review: &crate::model::ReviewCommentPreview) -> String {
    let line = review_line_label(review);
    let side = review_display_side(review)
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| "line".to_string());
    format!("inline {}:{line} {side}", review.path)
}

pub(super) fn review_comment_metadata_segments(
    review: &crate::model::ReviewCommentPreview,
) -> Vec<DetailSegment> {
    let mut segments = vec![DetailSegment::styled(
        review_comment_label(review),
        diff_metadata_style(),
    )];
    append_review_state_segments(&mut segments, review);
    segments
}

pub(super) fn push_comment_separator(
    builder: &mut DetailsBuilder,
    selected: bool,
    depth: usize,
    edge: CommentBoxEdge,
) {
    if selected {
        push_selected_comment_box_edge(builder, 0, edge);
        return;
    }

    let prefix = comment_line_prefix(selected, depth);
    let prefix_width = segments_width(&prefix);
    let width = builder
        .width
        .saturating_sub(prefix_width + comment_right_padding(selected))
        .max(12);
    let line = if selected { "━" } else { "─" };
    let mut segments = prefix;
    segments.push(DetailSegment::styled_chrome(
        line.repeat(width),
        comment_separator_style(selected),
    ));
    builder.push_chrome_line(segments);
}

pub(super) fn comment_line_prefix(selected: bool, depth: usize) -> Vec<DetailSegment> {
    let depth = depth.min(4);
    if selected {
        let mut prefix = "┃ ".to_string();
        if depth > 0 {
            prefix.push_str("  ");
            prefix.push_str(&"  ".repeat(depth.saturating_sub(1)));
            prefix.push_str("↳ ");
        }
        vec![DetailSegment::styled_chrome(
            prefix,
            comment_selected_rail_style(),
        )]
    } else if depth > 0 {
        let mut prefix = "    ".to_string();
        prefix.push_str(&"  ".repeat(depth.saturating_sub(1)));
        prefix.push_str("↳ ");
        vec![DetailSegment::styled_chrome(prefix, comment_thread_style())]
    } else {
        padding_prefix(COMMENT_LEFT_PADDING)
    }
}

pub(super) fn selected_description_prefix() -> Vec<DetailSegment> {
    vec![DetailSegment::styled_chrome(
        "┃ ".to_string(),
        description_selected_rail_style(),
    )]
}

pub(super) fn add_comment_right_border(
    builder: &mut DetailsBuilder,
    start_line: usize,
    end_line: usize,
) {
    let border_column = comment_right_border_column(builder.width);
    for line_index in start_line..end_line.min(builder.document.lines.len()) {
        let line = &mut builder.document.lines[line_index];
        let width = display_width(&line.to_string());
        if width < border_column {
            line.spans
                .push(Span::raw(" ".repeat(border_column - width)));
        }
        line.spans
            .push(Span::styled("┃", comment_selected_rail_style()));
        builder.document.copy_exclusions.push(CopyExclusionRegion {
            line: line_index,
            start: width.min(usize::from(u16::MAX)) as u16,
            end: border_column.saturating_add(1).min(usize::from(u16::MAX)) as u16,
        });
    }
}

pub(super) fn add_description_right_border(
    builder: &mut DetailsBuilder,
    start_line: usize,
    end_line: usize,
) {
    let border_column = comment_right_border_column(builder.width);
    for line_index in start_line..end_line.min(builder.document.lines.len()) {
        let line = &mut builder.document.lines[line_index];
        let width = display_width(&line.to_string());
        if width < border_column {
            line.spans
                .push(Span::raw(" ".repeat(border_column - width)));
        }
        line.spans
            .push(Span::styled("┃", description_selected_rail_style()));
        builder.document.copy_exclusions.push(CopyExclusionRegion {
            line: line_index,
            start: width.min(usize::from(u16::MAX)) as u16,
            end: border_column.saturating_add(1).min(usize::from(u16::MAX)) as u16,
        });
    }
}

pub(super) fn comment_right_border_column(width: usize) -> usize {
    width
        .saturating_sub(COMMENT_RIGHT_PADDING + 1)
        .max(COMMENT_LEFT_PADDING)
}

pub(super) fn comment_right_padding(selected: bool) -> usize {
    COMMENT_RIGHT_PADDING + usize::from(selected)
}

pub(super) fn action_hint_segments(
    state: Option<&ActionHintState>,
) -> (Vec<DetailSegment>, Option<String>) {
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

pub(super) fn merge_queue_hint_segments(
    state: Option<&ActionHintState>,
) -> Option<Vec<DetailSegment>> {
    let Some(ActionHintState::Loaded(hints)) = state else {
        return None;
    };
    let queue = hints.queue.as_ref()?;
    let mut segments = Vec::new();
    if let Some(position) = queue.position {
        let label = format!("#{position}");
        if let Some(url) = &queue.url {
            segments.push(DetailSegment::link(label, url.clone()));
        } else {
            segments.push(DetailSegment::styled(
                label,
                queue_state_style(&queue.state),
            ));
        }
        segments.push(DetailSegment::raw(" ".to_string()));
    }
    segments.push(DetailSegment::styled(
        queue_state_label(&queue.state),
        queue_state_style(&queue.state),
    ));
    if let Some(enqueued_at) = queue.enqueued_at {
        segments.push(DetailSegment::raw(format!(
            ", queued {}",
            relative_time(Some(enqueued_at))
        )));
    }
    if let Some(seconds) = queue.estimated_time_to_merge {
        segments.push(DetailSegment::raw(format!(
            ", eta {}",
            merge_queue_eta_label(seconds)
        )));
    }
    Some(segments)
}

pub(super) fn review_hint_segments(state: Option<&ActionHintState>) -> Option<Vec<DetailSegment>> {
    let Some(ActionHintState::Loaded(hints)) = state else {
        return None;
    };
    let reviews = hints.reviews.as_ref()?;
    let mut segments = Vec::new();
    if reviews.approved > 0 {
        push_review_part(
            &mut segments,
            format!("approved {}", reviews.approved),
            review_state_style("APPROVED"),
        );
    }
    if reviews.changes_requested > 0 {
        push_review_part(
            &mut segments,
            format!("changes requested {}", reviews.changes_requested),
            review_state_style("CHANGES_REQUESTED"),
        );
    }
    if reviews.pending > 0 {
        push_review_part(
            &mut segments,
            format!("pending {}", reviews.pending),
            review_state_style("PENDING"),
        );
    }
    if segments.is_empty()
        && let Some(decision) = reviews.decision.as_deref()
    {
        push_review_part(
            &mut segments,
            review_decision_label(decision),
            review_state_style(decision),
        );
    }
    (!segments.is_empty()).then_some(segments)
}

pub(super) fn review_actor_hint_segments(
    state: Option<&ActionHintState>,
) -> Option<Vec<DetailSegment>> {
    let Some(ActionHintState::Loaded(hints)) = state else {
        return None;
    };
    let reviews = hints.reviews.as_ref()?;
    let mut segments = Vec::new();
    for review in &reviews.latest_reviews {
        push_review_actor_state(&mut segments, &review.actor, &review.state);
    }
    for actor in &reviews.pending_reviewers {
        push_review_actor_state(&mut segments, actor, "PENDING");
    }
    if reviews.pending > reviews.pending_reviewers.len() {
        push_review_part(
            &mut segments,
            format!(
                "+{} more pending",
                reviews.pending - reviews.pending_reviewers.len()
            ),
            review_state_style("PENDING"),
        );
    }
    (!segments.is_empty()).then_some(segments)
}

fn push_review_actor_state(
    segments: &mut Vec<DetailSegment>,
    actor: &PullRequestReviewActor,
    state: &str,
) {
    if !segments.is_empty() {
        segments.push(DetailSegment::raw(", "));
    }
    if let Some(url) = &actor.url {
        segments.push(DetailSegment::link(actor.label.clone(), url.clone()));
    } else {
        segments.push(DetailSegment::raw(actor.label.clone()));
    }
    segments.push(DetailSegment::raw(" "));
    segments.push(DetailSegment::styled(
        review_state_label(state),
        review_state_style(state),
    ));
}

fn push_review_part(segments: &mut Vec<DetailSegment>, text: String, style: Style) {
    if !segments.is_empty() {
        segments.push(DetailSegment::raw(", "));
    }
    segments.push(DetailSegment::styled(text, style));
}

fn queue_state_label(state: &str) -> String {
    state.to_ascii_lowercase().replace('_', " ")
}

fn queue_state_style(state: &str) -> Style {
    match state {
        "MERGEABLE" => active_theme()
            .panel()
            .fg(active_theme().success)
            .add_modifier(Modifier::BOLD),
        "UNMERGEABLE" => active_theme()
            .panel()
            .fg(active_theme().error)
            .add_modifier(Modifier::BOLD),
        "LOCKED" => active_theme().muted().add_modifier(Modifier::BOLD),
        "QUEUED" | "AWAITING_CHECKS" => active_theme()
            .panel()
            .fg(active_theme().warning)
            .add_modifier(Modifier::BOLD),
        _ => active_theme().panel().fg(active_theme().warning),
    }
}

fn merge_queue_eta_label(seconds: usize) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}h", seconds / 3600)
    }
}

fn review_decision_label(decision: &str) -> String {
    match decision {
        "APPROVED" => "approved".to_string(),
        "CHANGES_REQUESTED" => "changes requested".to_string(),
        "REVIEW_REQUIRED" => "review required".to_string(),
        other => other.to_ascii_lowercase().replace('_', " "),
    }
}

fn review_state_label(state: &str) -> String {
    match state {
        "APPROVED" => "approved".to_string(),
        "CHANGES_REQUESTED" => "changes requested".to_string(),
        "COMMENTED" => "commented".to_string(),
        "PENDING" => "pending".to_string(),
        other => other.to_ascii_lowercase().replace('_', " "),
    }
}

fn review_state_style(state: &str) -> Style {
    match state {
        "APPROVED" => active_theme()
            .panel()
            .fg(active_theme().success)
            .add_modifier(Modifier::BOLD),
        "CHANGES_REQUESTED" => active_theme()
            .panel()
            .fg(active_theme().error)
            .add_modifier(Modifier::BOLD),
        "REVIEW_REQUIRED" | "PENDING" => active_theme()
            .panel()
            .fg(active_theme().warning)
            .add_modifier(Modifier::BOLD),
        _ => active_theme().muted(),
    }
}

pub(super) fn action_label_segments(labels: &[String]) -> Vec<DetailSegment> {
    let mut segments = Vec::new();
    for label in labels {
        if !segments.is_empty() {
            segments.push(DetailSegment::raw(", "));
        }
        let style = if positive_action_label(label) {
            active_theme()
                .panel()
                .fg(active_theme().success)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        segments.push(DetailSegment::styled(label, style));
    }
    segments
}

fn positive_action_label(label: &str) -> bool {
    label == "Approvable" || label.to_ascii_lowercase().contains("merge")
}

pub(super) fn action_note_segments(note: &str) -> Vec<DetailSegment> {
    let mut segments = Vec::new();
    let mut rest = note;

    while let Some((index, text, style)) = next_action_note_highlight(rest) {
        if index > 0 {
            segments.push(DetailSegment::raw(rest[..index].to_string()));
        }
        segments.push(DetailSegment::styled(text, style));
        rest = &rest[index + text.len()..];
    }
    if !rest.is_empty() {
        segments.push(DetailSegment::raw(rest.to_string()));
    }
    if segments.is_empty() {
        segments.push(DetailSegment::raw(note.to_string()));
    }
    segments
}

fn next_action_note_highlight(rest: &str) -> Option<(usize, &'static str, Style)> {
    const MERGE_BLOCKED: &str = "Merge blocked";
    const CONFLICTS: &str = "merge conflicts must be resolved";
    let mut next = rest.find(MERGE_BLOCKED).map(|index| {
        (
            index,
            MERGE_BLOCKED,
            log_warning_style().add_modifier(Modifier::BOLD),
        )
    });
    if let Some(index) = rest.find(CONFLICTS) {
        let conflict = (
            index,
            CONFLICTS,
            log_error_style().add_modifier(Modifier::BOLD),
        );
        if next
            .as_ref()
            .is_none_or(|(next_index, _, _)| index < *next_index)
        {
            next = Some(conflict);
        }
    }
    next
}

pub(super) fn branch_hint_segments(state: Option<&ActionHintState>) -> Vec<DetailSegment> {
    match state {
        Some(ActionHintState::Loaded(hints)) => hints
            .head
            .as_ref()
            .map(|branch| {
                vec![DetailSegment::link(
                    pull_request_branch_label(branch),
                    pull_request_branch_url(branch),
                )]
            })
            .unwrap_or_else(|| vec![DetailSegment::raw("unavailable")]),
        Some(ActionHintState::Loading) | None => vec![DetailSegment::raw("loading...")],
        Some(ActionHintState::Error(_)) => vec![DetailSegment::raw("unavailable")],
    }
}

pub(super) fn pull_request_branch_label(branch: &PullRequestBranch) -> String {
    format!("{}:{}", branch.repository, branch.branch)
}

pub(super) fn pull_request_branch_url(branch: &PullRequestBranch) -> String {
    format!(
        "https://github.com/{}/tree/{}",
        branch.repository, branch.branch
    )
}

pub(super) fn check_hint_segments(state: Option<&ActionHintState>) -> Vec<DetailSegment> {
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

pub(super) fn commit_count_segments(
    state: Option<&ActionHintState>,
    item: &WorkItem,
) -> Vec<DetailSegment> {
    match state {
        Some(ActionHintState::Loaded(hints)) => hints
            .commits
            .map(|commits| {
                vec![DetailSegment::link(
                    commits.to_string(),
                    pull_request_commits_url(item),
                )]
            })
            .unwrap_or_else(|| vec![DetailSegment::raw("-")]),
        Some(ActionHintState::Loading) | None => vec![DetailSegment::raw("loading...")],
        Some(ActionHintState::Error(_)) => vec![DetailSegment::raw("unavailable")],
    }
}

pub(super) fn check_summary_segments(summary: &CheckSummary) -> Vec<DetailSegment> {
    let mut segments = Vec::new();
    push_check_part(
        &mut segments,
        format!("{} pass", summary.passed),
        active_theme().panel().fg(active_theme().success),
    );
    push_check_part(
        &mut segments,
        format!("{} fail", summary.failed),
        if summary.failed > 0 {
            active_theme()
                .panel()
                .fg(active_theme().error)
                .add_modifier(Modifier::BOLD)
        } else {
            active_theme().muted()
        },
    );
    if summary.pending > 0 {
        push_check_part(
            &mut segments,
            format!("{} pending", summary.pending),
            active_theme().panel().fg(active_theme().warning),
        );
    }
    if summary.skipped > 0 {
        push_check_part(
            &mut segments,
            format!("{} skipped", summary.skipped),
            active_theme().subtle(),
        );
    }
    if summary.incomplete {
        push_check_part(
            &mut segments,
            format!("{} total", summary.total),
            active_theme().muted(),
        );
    }
    segments
}

pub(super) fn failed_check_runs_summary(runs: &[FailedCheckRunSummary]) -> String {
    runs.iter()
        .map(|run| {
            let label = run
                .workflow
                .as_deref()
                .filter(|workflow| !workflow.trim().is_empty())
                .unwrap_or("Actions run");
            format!("{label} #{} ({})", run.run_id, run.checks.join(", "))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

pub(super) fn push_check_part(segments: &mut Vec<DetailSegment>, text: String, style: Style) {
    if !segments.is_empty() {
        segments.push(DetailSegment::raw(", "));
    }
    segments.push(DetailSegment::styled(text, style));
}

pub(super) fn quote_comment_for_reply(comment: &CommentPreview) -> String {
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

pub(super) fn markdown_blocks(text: &str) -> Vec<MarkdownBlock> {
    let mut blocks = Vec::new();
    let mut current = Vec::new();
    let mut link: Option<String> = None;
    let mut image: Option<MarkdownImage> = None;
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
                current.push(DetailSegment::styled("- ", active_theme().muted()));
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
            MarkdownEvent::Start(Tag::Image {
                dest_url, title, ..
            }) => {
                image = Some(MarkdownImage {
                    url: dest_url.to_string(),
                    alt: None,
                    title: non_empty_inline_text(&title),
                    width: None,
                    height: None,
                });
            }
            MarkdownEvent::End(TagEnd::Image) => {
                if let Some(image) = image.take() {
                    current.push(image_segment(&image));
                }
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
                } else if let Some(image) = image.as_mut() {
                    push_image_alt(image, &text);
                } else {
                    append_text_segments(
                        &mut current,
                        &text,
                        inline_style(strong_depth, emphasis_depth, link.is_some()),
                        link.clone(),
                    );
                }
            }
            MarkdownEvent::Code(text) => {
                if let Some(image) = image.as_mut() {
                    push_image_alt(image, &text);
                } else {
                    current.push(DetailSegment::styled(
                        text.to_string(),
                        active_theme().panel().fg(active_theme().success),
                    ));
                }
            }
            MarkdownEvent::SoftBreak | MarkdownEvent::HardBreak => {
                if in_code_block {
                    code_block.push('\n');
                } else if let Some(image) = image.as_mut() {
                    push_image_alt(image, " ");
                } else {
                    current.push(DetailSegment::raw("\n"));
                }
            }
            MarkdownEvent::InlineHtml(html) => {
                if let Some(image) = image.as_mut() {
                    push_image_alt(image, &html);
                    continue;
                }
                current.extend(html_image_segments(&html));
            }
            MarkdownEvent::Html(html) => {
                if in_code_block {
                    code_block.push_str(&html);
                    continue;
                }

                let segments = html_image_segments(&html);
                if !segments.is_empty() {
                    flush_markdown_block(
                        &mut blocks,
                        &mut current,
                        quote_depth,
                        MarkdownBlockKind::Text,
                    );
                    push_markdown_block(
                        &mut blocks,
                        quote_depth,
                        MarkdownBlockKind::Text,
                        segments,
                    );
                }
            }
            MarkdownEvent::Rule => push_markdown_block(
                &mut blocks,
                quote_depth,
                MarkdownBlockKind::Text,
                vec![DetailSegment::styled(
                    "─".repeat(24),
                    active_theme().subtle(),
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

pub(super) fn push_image_alt(image: &mut MarkdownImage, text: &str) {
    if let Some(alt) = image.alt.as_mut() {
        alt.push_str(text);
    } else {
        image.alt = Some(text.to_string());
    }
}

pub(super) fn html_image_segments(html: &str) -> Vec<DetailSegment> {
    let mut segments = Vec::new();
    for image in html_images(html) {
        if !segments.is_empty() {
            segments.push(DetailSegment::raw(" "));
        }
        segments.push(image_segment(&image));
    }
    segments
}

pub(super) fn html_images(html: &str) -> Vec<MarkdownImage> {
    let mut images = Vec::new();
    let lower = html.to_ascii_lowercase();
    let mut search_start = 0;
    while let Some(relative_start) = lower[search_start..].find("<img") {
        let tag_start = search_start + relative_start;
        let after_name = tag_start + "<img".len();
        if !html_tag_name_boundary(html, after_name) {
            search_start = after_name;
            continue;
        }

        let Some(tag_end) = html_tag_end(html, after_name) else {
            break;
        };
        let tag = &html[tag_start..=tag_end];
        if let Some(url) = html_attr_value(tag, "src") {
            images.push(MarkdownImage {
                url,
                alt: html_attr_value(tag, "alt").and_then(|alt| non_empty_inline_text(&alt)),
                title: html_attr_value(tag, "title")
                    .and_then(|title| non_empty_inline_text(&title)),
                width: html_attr_value(tag, "width")
                    .and_then(|width| non_empty_inline_text(&width)),
                height: html_attr_value(tag, "height")
                    .and_then(|height| non_empty_inline_text(&height)),
            });
        }
        search_start = tag_end + 1;
    }
    images
}

pub(super) fn html_tag_name_boundary(html: &str, index: usize) -> bool {
    html[index..]
        .chars()
        .next()
        .is_none_or(|ch| ch.is_ascii_whitespace() || matches!(ch, '/' | '>'))
}

pub(super) fn html_tag_end(html: &str, start: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, ch) in html[start..].char_indices() {
        match (quote, ch) {
            (Some(active), current) if current == active => quote = None,
            (None, '"' | '\'') => quote = Some(ch),
            (None, '>') => return Some(start + offset),
            _ => {}
        }
    }
    None
}

pub(super) fn html_attr_value(tag: &str, name: &str) -> Option<String> {
    let bytes = tag.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        while bytes
            .get(index)
            .is_some_and(|byte| byte.is_ascii_whitespace() || matches!(byte, b'<' | b'/' | b'>'))
        {
            index += 1;
        }

        let name_start = index;
        while bytes
            .get(index)
            .is_some_and(|byte| is_html_attr_name_byte(*byte))
        {
            index += 1;
        }
        if name_start == index {
            index += 1;
            continue;
        }
        let attr_name = &tag[name_start..index];
        while bytes
            .get(index)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            index += 1;
        }
        if bytes.get(index) != Some(&b'=') {
            continue;
        }
        index += 1;
        while bytes
            .get(index)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            index += 1;
        }

        let Some(first) = bytes.get(index).copied() else {
            break;
        };
        let value_start;
        let value_end;
        if matches!(first, b'"' | b'\'') {
            let quote = first;
            index += 1;
            value_start = index;
            while bytes.get(index).is_some_and(|byte| *byte != quote) {
                index += 1;
            }
            value_end = index;
            if bytes.get(index) == Some(&quote) {
                index += 1;
            }
        } else {
            value_start = index;
            while bytes
                .get(index)
                .is_some_and(|byte| !byte.is_ascii_whitespace() && *byte != b'>')
            {
                index += 1;
            }
            value_end = index;
        }

        if attr_name.eq_ignore_ascii_case(name) {
            return non_empty_inline_text(&decode_basic_html_entities(
                &tag[value_start..value_end],
            ));
        }
    }
    None
}

pub(super) fn is_html_attr_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':')
}

pub(super) fn image_segment(image: &MarkdownImage) -> DetailSegment {
    DetailSegment::link(image_label(image), image.url.clone())
}

pub(super) fn image_label(image: &MarkdownImage) -> String {
    if let Some(alt) = useful_image_label(image.alt.as_deref().or(image.title.as_deref())) {
        return format!("[image: {}]", truncate_inline(&alt, 72));
    }
    match (
        image.width.as_deref().and_then(non_empty_dimension),
        image.height.as_deref().and_then(non_empty_dimension),
    ) {
        (Some(width), Some(height)) => format!("[image {width}x{height}]"),
        (Some(width), None) => format!("[image width {width}]"),
        (None, Some(height)) => format!("[image height {height}]"),
        (None, None) => "[image]".to_string(),
    }
}

pub(super) fn useful_image_label(value: Option<&str>) -> Option<String> {
    let value = non_empty_inline_text(value?)?;
    if value.is_empty() || value.eq_ignore_ascii_case("image") {
        None
    } else {
        Some(value)
    }
}

pub(super) fn non_empty_dimension(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

pub(super) fn non_empty_inline_text(value: &str) -> Option<String> {
    let text = value.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}

pub(super) fn decode_basic_html_entities(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&#x22;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

pub(super) fn is_rust_code_info(info: &str) -> bool {
    matches!(
        info.split_whitespace().next().map(str::to_ascii_lowercase),
        Some(language) if matches!(language.as_str(), "rust" | "rs")
    )
}

pub(super) fn is_plain_code_info(info: &str) -> bool {
    matches!(
        info.split_whitespace().next().map(str::to_ascii_lowercase),
        Some(language) if matches!(
            language.as_str(),
            "plain" | "text" | "txt" | "log" | "console" | "output"
        )
    )
}

pub(super) fn highlight_code_line(line: &str, language: CodeLanguage) -> Vec<DetailSegment> {
    match language {
        CodeLanguage::Rust => highlight_rust_code_line(line),
        CodeLanguage::Plain => highlight_plain_code_line(line),
        CodeLanguage::Other => vec![DetailSegment::styled(line.to_string(), code_plain_style())],
    }
}

pub(super) fn highlight_plain_code_line(line: &str) -> Vec<DetailSegment> {
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

pub(super) fn plain_code_content_style(content: &str) -> Style {
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

pub(super) fn highlight_rust_code_line(line: &str) -> Vec<DetailSegment> {
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

pub(super) fn quoted_end(chars: &[char], start: usize, quote: char) -> usize {
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

pub(super) fn rust_char_literal_end(chars: &[char], start: usize) -> Option<usize> {
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

pub(super) fn is_rust_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

pub(super) fn is_rust_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

pub(super) fn is_rust_keyword(text: &str) -> bool {
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

pub(super) fn is_rust_primitive_type(text: &str) -> bool {
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

pub(super) fn push_highlighted_text(segments: &mut Vec<DetailSegment>, text: &str, style: Style) {
    let template = DetailSegment::styled("", style);
    push_text_segment(segments, &template, text);
}

pub(super) fn flush_table_cell(
    row: &mut Option<MarkdownTableRow>,
    current: &mut Vec<DetailSegment>,
) {
    if let Some(row) = row.as_mut() {
        row.cells.push(trim_table_cell(std::mem::take(current)));
        return;
    }
    current.clear();
}

pub(super) fn flush_table_row(
    table: &mut Option<MarkdownTable>,
    row: &mut Option<MarkdownTableRow>,
) {
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

pub(super) fn push_markdown_table(
    blocks: &mut Vec<MarkdownBlock>,
    quote_depth: u8,
    table: MarkdownTable,
) {
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

pub(super) fn table_row_segments(row: &MarkdownTableRow, widths: &[usize]) -> Vec<DetailSegment> {
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

pub(super) fn table_separator_segments(widths: &[usize]) -> Vec<DetailSegment> {
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

pub(super) fn trim_table_cell(mut segments: Vec<DetailSegment>) -> Vec<DetailSegment> {
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

pub(super) fn segments_width(segments: &[DetailSegment]) -> usize {
    segments
        .iter()
        .map(|segment| display_width(&segment.text))
        .sum()
}

pub(super) fn flush_markdown_block(
    blocks: &mut Vec<MarkdownBlock>,
    current: &mut Vec<DetailSegment>,
    quote_depth: u8,
    kind: MarkdownBlockKind,
) {
    if current.iter().any(|segment| !segment.text.is_empty()) {
        push_markdown_block(blocks, quote_depth, kind, std::mem::take(current));
    }
}

pub(super) fn push_markdown_block(
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

pub(super) fn markdown_gap_before(
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

pub(super) fn muted_quote_segments(mut segments: Vec<DetailSegment>) -> Vec<DetailSegment> {
    for segment in &mut segments {
        segment.style = segment.style.fg(active_theme().muted);
    }
    segments
}

pub(super) fn quote_prefix(depth: u8) -> Vec<DetailSegment> {
    if depth == 0 {
        return Vec::new();
    }

    vec![DetailSegment::styled(
        "│ ".repeat(depth.min(3) as usize),
        quote_style(),
    )]
}

pub(super) fn append_text_segments(
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
            copyable: true,
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

pub(super) fn append_mention_segments(current: &mut Vec<DetailSegment>, text: &str, style: Style) {
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

pub(super) fn find_github_mention_start(text: &str) -> Option<usize> {
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

pub(super) fn is_github_username_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '-'
}

pub(super) fn split_raw_urls(text: &str) -> Vec<(String, Option<String>)> {
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

pub(super) fn find_url_start(text: &str) -> Option<usize> {
    match (text.find("https://"), text.find("http://")) {
        (Some(https), Some(http)) => Some(https.min(http)),
        (Some(https), None) => Some(https),
        (None, Some(http)) => Some(http),
        (None, None) => None,
    }
}

pub(super) fn trim_url_trailing_punctuation(url: &mut String) -> String {
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

pub(super) fn push_char_segment(
    current: &mut Vec<DetailSegment>,
    template: &DetailSegment,
    ch: char,
) {
    if let Some(last) = current.last_mut()
        && last.style == template.style
        && last.link == template.link
        && last.action == template.action
        && last.copyable == template.copyable
    {
        last.text.push(ch);
        return;
    }

    current.push(DetailSegment {
        text: ch.to_string(),
        style: template.style,
        link: template.link.clone(),
        action: template.action.clone(),
        copyable: template.copyable,
    });
}

pub(super) fn inline_style(strong_depth: u8, emphasis_depth: u8, is_link: bool) -> Style {
    let mut style = if is_link {
        link_style()
    } else {
        active_theme().panel()
    };
    if strong_depth > 0 {
        style = style.add_modifier(Modifier::BOLD);
    }
    if emphasis_depth > 0 {
        style = style.add_modifier(Modifier::ITALIC);
    }
    style
}

pub(super) fn heading_style() -> Style {
    active_theme()
        .panel()
        .fg(active_theme().warning)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn link_style() -> Style {
    active_theme().link()
}

pub(super) fn table_separator_style() -> Style {
    active_theme().subtle()
}

pub(super) fn action_style() -> Style {
    active_theme().action()
}

pub(super) fn label_style() -> Style {
    active_theme()
        .panel()
        .fg(active_theme().label)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn reaction_style() -> Style {
    active_theme().panel().fg(active_theme().reaction)
}

pub(super) fn new_since_last_read_style() -> Style {
    active_theme()
        .panel()
        .fg(active_theme().success)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn quote_style() -> Style {
    active_theme().panel().fg(active_theme().quote)
}

pub(super) fn code_plain_style() -> Style {
    active_theme().panel().fg(active_theme().code)
}

pub(super) fn rust_keyword_style() -> Style {
    active_theme()
        .panel()
        .fg(active_theme().action)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn rust_type_style() -> Style {
    active_theme().panel().fg(active_theme().info)
}

pub(super) fn rust_string_style() -> Style {
    active_theme().panel().fg(active_theme().warning)
}

pub(super) fn rust_comment_style() -> Style {
    active_theme().panel().fg(active_theme().quote)
}

pub(super) fn rust_macro_style() -> Style {
    active_theme().panel().fg(active_theme().link)
}

pub(super) fn rust_number_style() -> Style {
    active_theme().panel().fg(active_theme().warning)
}

pub(super) fn log_error_style() -> Style {
    active_theme()
        .panel()
        .fg(active_theme().error)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn log_warning_style() -> Style {
    active_theme().panel().fg(active_theme().warning)
}

pub(super) fn log_info_style() -> Style {
    active_theme().panel().fg(active_theme().info)
}

pub(super) fn log_meta_style() -> Style {
    active_theme().subtle()
}

pub(super) fn diff_file_style() -> Style {
    active_theme()
        .panel()
        .fg(active_theme().focus)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn diff_file_link_style() -> Style {
    diff_file_style().add_modifier(Modifier::UNDERLINED)
}

pub(super) fn diff_hunk_style() -> Style {
    active_theme().panel().fg(active_theme().link)
}

pub(super) fn diff_gutter_style() -> Style {
    active_theme().subtle()
}

pub(super) fn diff_added_style() -> Style {
    active_theme().panel().fg(active_theme().added)
}

pub(super) fn diff_removed_style() -> Style {
    active_theme().panel().fg(active_theme().removed)
}

pub(super) fn diff_context_style() -> Style {
    active_theme().panel().fg(active_theme().code)
}

pub(super) fn diff_metadata_style() -> Style {
    active_theme().subtle()
}

pub(super) fn diff_inline_comment_marker_style() -> Style {
    active_theme()
        .panel()
        .fg(active_theme().action)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn review_resolved_style() -> Style {
    active_theme()
        .panel()
        .fg(active_theme().success)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn review_outdated_style() -> Style {
    active_theme().subtle()
}

pub(super) fn comment_author_style(selected: bool) -> Style {
    let style = if selected {
        active_theme().panel().fg(active_theme().warning)
    } else {
        active_theme().panel().fg(active_theme().focus)
    };
    style.add_modifier(Modifier::BOLD)
}

pub(super) fn comment_marker_style(selected: bool) -> Style {
    if selected {
        active_theme()
            .panel()
            .fg(active_theme().warning)
            .add_modifier(Modifier::BOLD)
    } else {
        active_theme().subtle()
    }
}

pub(super) fn comment_separator_style(selected: bool) -> Style {
    if selected {
        active_theme()
            .panel()
            .fg(active_theme().reaction)
            .add_modifier(Modifier::BOLD)
    } else {
        active_theme().subtle()
    }
}

pub(super) fn comment_search_match_style() -> Style {
    active_theme()
        .panel()
        .fg(active_theme().action)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn description_selected_heading_style() -> Style {
    active_theme()
        .panel()
        .fg(active_theme().focus)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn description_selected_separator_style() -> Style {
    active_theme()
        .panel()
        .fg(active_theme().focus)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn description_selected_rail_style() -> Style {
    active_theme()
        .panel()
        .fg(active_theme().focus)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn comment_selected_rail_style() -> Style {
    active_theme()
        .panel()
        .fg(active_theme().warning)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn comment_thread_style() -> Style {
    active_theme().subtle()
}
