use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::layout::{centered_rect_width, centered_rect_with_size};
use super::text::{display_width, truncate_inline};
use super::{InboxThreadAction, ReactionContent};
use crate::theme::active_theme;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CommandPalette {
    pub(super) query: String,
    pub(super) selected: usize,
}

#[derive(Debug, Clone)]
pub(super) struct PaletteCommand {
    pub(super) title: &'static str,
    pub(super) keys: String,
    pub(super) scope: &'static str,
    pub(super) detail: &'static str,
    pub(super) action: PaletteAction,
}

#[derive(Debug, Clone)]
pub(super) enum PaletteAction {
    Key(KeyEvent),
    Quit,
    ShowInfo,
    ShowHelp,
    ShowCommandPalette,
    Refresh,
    RecentItems,
    ToggleTheme,
    TopMenuSwitch,
    SearchCurrentRepo,
    SavedSearchFilter,
    SwitchProject,
    ProjectAdd,
    ProjectRemove,
    CopyGithubLink,
    CopyContent,
    ToggleMouseCapture,
    OpenSelected,
    ShowDiff,
    ClearIgnoredItems,
    ClearCache,
    InboxMarkDone,
    InboxMarkAllRead,
    InboxThreadAction(InboxThreadAction),
}

pub(super) fn command_palette_area(area: Rect) -> Rect {
    let width = centered_rect_width(76, area).max(24).min(area.width);
    let max_height = area.height.saturating_sub(2).max(3);
    let height = 18.min(max_height).max(3);
    centered_rect_with_size(width, height, area)
}

pub(super) fn command_palette_visible_start(selected: usize, len: usize, height: usize) -> usize {
    if height == 0 || len <= height {
        return 0;
    }
    selected.saturating_add(1).saturating_sub(height)
}

pub(super) fn command_palette_input_line(query: &str, width: usize) -> Line<'static> {
    let theme = active_theme();
    if query.is_empty() {
        return Line::from(vec![
            Span::styled("> ", theme.panel().fg(theme.focus)),
            Span::styled("Type to search commands", theme.subtle()),
        ]);
    }

    Line::from(vec![
        Span::styled("> ", theme.panel().fg(theme.focus)),
        Span::styled(
            truncate_inline(query, width.saturating_sub(2)),
            theme.panel(),
        ),
    ])
}

pub(super) fn command_palette_result_line(
    command: &PaletteCommand,
    selected: bool,
    width: usize,
) -> Line<'static> {
    let theme = active_theme();
    let marker = if selected { "> " } else { "  " };
    let text = format!(
        "{marker}{:<34} {:<16} {}",
        command.title, command.keys, command.scope
    );
    let style = if selected {
        theme.active()
    } else {
        theme.panel()
    };
    let detail_style = if selected {
        theme.active()
    } else {
        theme.subtle()
    };

    Line::from(vec![
        Span::styled(truncate_inline(&text, width), style),
        Span::styled(
            truncate_inline(
                &format!("  {}", command.detail),
                width.saturating_sub(display_width(&text)),
            ),
            detail_style,
        ),
    ])
}

pub(super) fn command_palette_commands(command_palette_key: &str) -> Vec<PaletteCommand> {
    let mut commands = vec![
        palette_command(
            "Show Command Palette",
            command_palette_key.to_string(),
            "General",
            "Open this searchable command list",
            PaletteAction::ShowCommandPalette,
        ),
        palette_command(
            "Show Help",
            "?",
            "General",
            "Open the full shortcut reference",
            PaletteAction::ShowHelp,
        ),
        palette_command(
            "Info",
            "",
            "General",
            "Show ghr version, paths, runtime state, and ghr process memory",
            PaletteAction::ShowInfo,
        ),
        palette_command(
            "Close or Cancel",
            "Esc",
            "General",
            "Close the active dialog, search, or details focus",
            palette_key(KeyCode::Esc),
        ),
        palette_command(
            "Confirm or Open",
            "Enter",
            "General",
            "Run the active confirmation or open the selected item area",
            palette_key(KeyCode::Enter),
        ),
        palette_command(
            "Quit ghr",
            "q / Ctrl+C",
            "General",
            "Save UI state and quit",
            PaletteAction::Quit,
        ),
        palette_command(
            "Refresh",
            "r",
            "General",
            "Refresh dashboard data from GitHub",
            PaletteAction::Refresh,
        ),
        palette_command(
            "Recent Items",
            "",
            "General",
            "Find recently viewed pull requests and issues",
            PaletteAction::RecentItems,
        ),
        palette_command(
            "Toggle Theme",
            "",
            "General",
            "Cycle auto, dark, and light themes and save config.toml",
            PaletteAction::ToggleTheme,
        ),
        palette_command(
            "Top Menu Switch",
            "",
            "General",
            "Switch Inbox, PR, issue, search, or repo tabs and focus the top menu",
            PaletteAction::TopMenuSwitch,
        ),
        palette_command(
            "Mark Done",
            "",
            "Inbox",
            "Move the selected GitHub notification out of inbox lists",
            PaletteAction::InboxMarkDone,
        ),
        palette_command(
            "Mark All Read",
            "",
            "Inbox",
            "Mark every GitHub notification as read",
            PaletteAction::InboxMarkAllRead,
        ),
        palette_command(
            "Mute Thread",
            "",
            "Inbox",
            "Ignore future notifications for the selected inbox thread",
            PaletteAction::InboxThreadAction(InboxThreadAction::Mute),
        ),
        palette_command(
            "Subscribe Thread",
            "",
            "Inbox",
            "Subscribe to the selected inbox thread",
            PaletteAction::InboxThreadAction(InboxThreadAction::Subscribe),
        ),
        palette_command(
            "Unsubscribe Thread",
            "",
            "Inbox",
            "Unsubscribe from the selected inbox thread",
            PaletteAction::InboxThreadAction(InboxThreadAction::Unsubscribe),
        ),
        palette_command(
            "Search Current Repo",
            "S",
            "General",
            "Search the current PR or issue list with structured fields",
            PaletteAction::SearchCurrentRepo,
        ),
        palette_command(
            "Saved Search Filter",
            "",
            "General",
            "Run a named repo issue or pull request filter",
            PaletteAction::SavedSearchFilter,
        ),
        palette_command(
            "Project Switch",
            "",
            "General",
            "Filter configured repo projects and switch to one",
            PaletteAction::SwitchProject,
        ),
        palette_command(
            "Project Add",
            "",
            "General",
            "Add a repo project to the menu and config.toml",
            PaletteAction::ProjectAdd,
        ),
        palette_command(
            "Project Remove",
            "",
            "General",
            "Remove a configured repo project from config.toml",
            PaletteAction::ProjectRemove,
        ),
        palette_command(
            "Copy GitHub Link",
            "",
            "General",
            "Copy the selected comment or issue/PR link to the clipboard",
            PaletteAction::CopyGithubLink,
        ),
        palette_command(
            "Copy Content",
            "",
            "General",
            "Copy the selected comment body or issue/PR description to the clipboard",
            PaletteAction::CopyContent,
        ),
        palette_command(
            "Toggle Mouse Text Selection",
            "m",
            "General",
            "Switch between TUI mouse controls and terminal text selection",
            PaletteAction::ToggleMouseCapture,
        ),
        palette_command(
            "Open Selected in Browser",
            "o",
            "General",
            "Open the selected item or PR changes page",
            PaletteAction::OpenSelected,
        ),
        palette_command(
            "Show Pull Request Diff",
            "v",
            "General",
            "Open PR diff mode",
            PaletteAction::ShowDiff,
        ),
        palette_command(
            "Ignore Selected Item",
            "i",
            "General",
            "Hide the selected pull request or issue from lists",
            palette_key(KeyCode::Char('i')),
        ),
        palette_command(
            "Clear Ignored Items",
            "",
            "General",
            "Show all previously ignored pull requests and issues again",
            PaletteAction::ClearIgnoredItems,
        ),
        palette_command(
            "Clear Cache",
            "",
            "General",
            "Choose which local cache layer to clear",
            PaletteAction::ClearCache,
        ),
        palette_command(
            "Leave Diff Mode",
            "q / Esc",
            "Diff",
            "Return to the state before opening diff",
            palette_key(KeyCode::Char('q')),
        ),
        palette_command(
            "Toggle Diff Focus",
            "Tab / Shift+Tab",
            "Diff",
            "Switch focus between changed files and diff details",
            palette_key(KeyCode::Tab),
        ),
        palette_command(
            "Toggle List Details Focus",
            "Tab / Shift+Tab",
            "Focus",
            "Switch focus between the list and details panes",
            palette_key(KeyCode::Tab),
        ),
        palette_command(
            "Focus ghr Tabs",
            "1",
            "Focus",
            "Focus the top ghr tab group",
            palette_key(KeyCode::Char('1')),
        ),
        palette_command(
            "Focus Sections",
            "2",
            "Focus",
            "Focus the section tab group",
            palette_key(KeyCode::Char('2')),
        ),
        palette_command(
            "Focus List",
            "3",
            "Focus",
            "Focus the item list or changed-file list",
            palette_key(KeyCode::Char('3')),
        ),
        palette_command(
            "Focus Details",
            "4 / Enter",
            "Focus",
            "Focus Details from the list",
            palette_key(KeyCode::Char('4')),
        ),
        palette_command(
            "Next Focused Tab Group",
            "Tab",
            "Tabs",
            "Move within focused ghr or Sections tabs",
            palette_key(KeyCode::Tab),
        ),
        palette_command(
            "Previous Focused Tab Group",
            "Shift+Tab",
            "Tabs",
            "Move backward within focused ghr or Sections tabs",
            palette_key(KeyCode::BackTab),
        ),
        palette_command(
            "Move Tab Right",
            "l / Right / ]",
            "Tabs",
            "Move right in ghr or Sections",
            palette_key(KeyCode::Char('l')),
        ),
        palette_command(
            "Move Tab Left",
            "h / Left / [",
            "Tabs",
            "Move left in ghr or Sections",
            palette_key(KeyCode::Char('h')),
        ),
        palette_command(
            "Move Focus Down",
            "j / n / Down",
            "Focus",
            "Move focus from ghr to Sections, or from Sections to List",
            palette_key(KeyCode::Char('j')),
        ),
        palette_command(
            "Move Focus Up",
            "k / p / Up",
            "Focus",
            "Move focus from Sections to ghr",
            palette_key(KeyCode::Char('k')),
        ),
        palette_command(
            "Search Current List",
            "/",
            "List",
            "Open repo search in PR/issue lists, or fuzzy filter other lists",
            palette_key(KeyCode::Char('/')),
        ),
        palette_command(
            "Move Selection Down",
            "j / n / Down",
            "List",
            "Move list selection down",
            palette_key(KeyCode::Char('j')),
        ),
        palette_command(
            "Move Selection Up",
            "k / p / Up",
            "List",
            "Move list selection up, or focus Sections from the first item",
            palette_key(KeyCode::Char('k')),
        ),
        palette_command(
            "Page List Down",
            "PgDown / d",
            "List",
            "Move the list selection down by a visible page",
            palette_key(KeyCode::PageDown),
        ),
        palette_command(
            "Page List Up",
            "PgUp / u",
            "List",
            "Move the list selection up by a visible page",
            palette_key(KeyCode::PageUp),
        ),
        palette_command(
            "Page Details Down",
            "PgDown / d",
            "Details",
            "Scroll details down by a visible page",
            palette_key(KeyCode::PageDown),
        ),
        palette_command(
            "Page Details Up",
            "PgUp / u",
            "Details",
            "Scroll details up by a visible page",
            palette_key(KeyCode::PageUp),
        ),
        palette_command(
            "Jump to First",
            "g",
            "List/Details",
            "Jump list to first item, or details to top and clear comment focus",
            palette_key(KeyCode::Char('g')),
        ),
        palette_command(
            "Jump to Last",
            "G",
            "List/Details",
            "Jump list to last item, or details to bottom and focus the last comment",
            palette_key(KeyCode::Char('G')),
        ),
        palette_command(
            "Load Previous Result Page",
            "[",
            "List",
            "Load previous GitHub search result page",
            palette_alt_key(KeyCode::Char('[')),
        ),
        palette_command(
            "Load Next Result Page",
            "]",
            "List",
            "Load next GitHub search result page",
            palette_alt_key(KeyCode::Char(']')),
        ),
        palette_command(
            "Open PR Merge Confirmation",
            "M",
            "Pull Request",
            "Confirm merging the selected PR",
            palette_key(KeyCode::Char('M')),
        ),
        palette_command(
            "Open PR Close Confirmation",
            "C",
            "Pull Request",
            "Confirm closing the selected PR",
            palette_key(KeyCode::Char('C')),
        ),
        palette_command(
            "Open PR Approve Confirmation",
            "A",
            "Pull Request",
            "Confirm approving the selected PR",
            palette_key(KeyCode::Char('A')),
        ),
        palette_command(
            "Add Comment",
            "a / c",
            "Issue/PR",
            "Add a normal issue or PR comment",
            palette_key(KeyCode::Char('a')),
        ),
        palette_command(
            "Add Label",
            "L",
            "Issue/PR",
            "Add a label to the selected issue or PR",
            palette_key(KeyCode::Char('L')),
        ),
        palette_command(
            "Change Milestone",
            "t",
            "Issue/PR",
            "Change or clear the selected issue or PR milestone",
            palette_key(KeyCode::Char('t')),
        ),
        palette_command(
            "Create Issue / PR",
            "N",
            "Issue/PR",
            "Create an issue, or create a PR from local_dir in PR lists",
            palette_key(KeyCode::Char('N')),
        ),
        palette_command(
            "Add Reaction",
            "+",
            "Issue/PR",
            "Add a reaction to the selected item or focused comment",
            palette_key(KeyCode::Char('+')),
        ),
        palette_command(
            "Assign Assignee",
            "@",
            "Issue/PR",
            "Assign assignee logins to the selected issue or PR",
            palette_key(KeyCode::Char('@')),
        ),
        palette_command(
            "Unassign Assignee",
            "-",
            "Issue/PR",
            "Remove assignee logins from the selected issue or PR",
            palette_key(KeyCode::Char('-')),
        ),
        palette_command(
            "Search Details Comments",
            "/",
            "Details",
            "Search loaded comments by keyword",
            palette_key(KeyCode::Char('/')),
        ),
        palette_command(
            "Next Comment",
            "n",
            "Details",
            "Focus next visible comment",
            palette_key(KeyCode::Char('n')),
        ),
        palette_command(
            "Previous Comment",
            "p",
            "Details",
            "Focus previous visible comment, or the PR/issue details body before the first comment",
            palette_key(KeyCode::Char('p')),
        ),
        palette_command(
            "Reply to Focused Comment",
            "R",
            "Details",
            "Quote reply to the focused comment",
            palette_key(KeyCode::Char('R')),
        ),
        palette_command(
            "Edit Focused Comment",
            "e",
            "Details",
            "Edit the focused comment when it is yours",
            palette_key(KeyCode::Char('e')),
        ),
        palette_command(
            "Previous Diff File",
            "[",
            "Diff",
            "Jump to previous changed file",
            palette_key(KeyCode::Char('[')),
        ),
        palette_command(
            "Next Diff File",
            "]",
            "Diff",
            "Jump to next changed file",
            palette_key(KeyCode::Char(']')),
        ),
        palette_command(
            "Page Diff Details Down",
            "h",
            "Diff",
            "Page down across diff files from files or details focus",
            palette_key(KeyCode::Char('h')),
        ),
        palette_command(
            "Page Diff Details Up",
            "l",
            "Diff",
            "Page up across diff files from files or details focus",
            palette_key(KeyCode::Char('l')),
        ),
        palette_command(
            "Begin Review Range",
            "m",
            "Diff",
            "Begin a review range at the selected diff line",
            palette_key(KeyCode::Char('m')),
        ),
        palette_command(
            "End Review Range",
            "e",
            "Diff",
            "End the current review range",
            palette_key(KeyCode::Char('e')),
        ),
        palette_command(
            "Add Inline Review Comment",
            "c",
            "Diff",
            "Add a review comment on the selected diff line or range",
            palette_key(KeyCode::Char('c')),
        ),
        palette_command(
            "Confirm PR Action",
            "y / Enter",
            "Dialog",
            "Run the active PR confirmation",
            palette_key(KeyCode::Char('y')),
        ),
        palette_command(
            "Submit Comment",
            "Ctrl+Enter",
            "Comment Editor",
            "Send or update the current comment",
            palette_ctrl_key(KeyCode::Enter),
        ),
        palette_command(
            "Insert Comment Newline",
            "Enter",
            "Comment Editor",
            "Insert a newline in the comment editor",
            palette_key(KeyCode::Enter),
        ),
        palette_command(
            "Save Editor Draft",
            "Ctrl+S / Cmd+S",
            "Editor",
            "Save the active comment, issue, or pull request draft",
            palette_ctrl_key(KeyCode::Char('s')),
        ),
        palette_command(
            "Delete Previous Character",
            "Backspace",
            "Editor",
            "Delete the previous character while editing or filtering",
            palette_key(KeyCode::Backspace),
        ),
        palette_command(
            "Move Editor Cursor",
            "Arrows",
            "Editor",
            "Move inside editor text; Up and Down follow rendered lines",
            palette_key(KeyCode::Left),
        ),
        palette_command(
            "Jump To Line Edge",
            "Home / End",
            "Editor",
            "Move the editor cursor to the start or end of the current line",
            palette_key(KeyCode::Home),
        ),
        palette_command(
            "Delete Previous Word",
            "Ctrl+W",
            "Editor",
            "Delete the word before the editor cursor",
            palette_ctrl_key(KeyCode::Char('w')),
        ),
        palette_command(
            "Delete To Line Edge",
            "Ctrl+U / Ctrl+K",
            "Editor",
            "Delete editor text to the start or end of the current line",
            palette_ctrl_key(KeyCode::Char('u')),
        ),
        palette_command(
            "Delete Current Line",
            "Ctrl+X",
            "Editor",
            "Delete the whole line under the editor cursor",
            palette_ctrl_key(KeyCode::Char('x')),
        ),
        palette_command(
            "Undo Editor Edit",
            "Ctrl+Z / Cmd+Z",
            "Editor",
            "Undo the most recent editor text change",
            palette_ctrl_key(KeyCode::Char('z')),
        ),
        palette_command(
            "Redo Editor Edit",
            "Ctrl+R / Cmd+Shift+Z",
            "Editor",
            "Redo an editor text change",
            palette_ctrl_key(KeyCode::Char('r')),
        ),
        palette_command(
            "Choose Next Suggestion",
            "Down / Tab",
            "Label/Reaction",
            "Move to the next label or reaction suggestion",
            palette_key(KeyCode::Down),
        ),
        palette_command(
            "Choose Previous Suggestion",
            "Up / Shift+Tab",
            "Label/Reaction",
            "Move to the previous label or reaction suggestion",
            palette_key(KeyCode::Up),
        ),
        palette_command(
            "Confirm Label or Reaction",
            "Enter",
            "Label/Reaction",
            "Confirm the selected label or reaction",
            palette_key(KeyCode::Enter),
        ),
        palette_command(
            "Next Issue Field",
            "Tab",
            "Issue Dialog",
            "Move to the next issue field",
            palette_key(KeyCode::Tab),
        ),
        palette_command(
            "Previous Issue Field",
            "Shift+Tab",
            "Issue Dialog",
            "Move to the previous issue field",
            palette_key(KeyCode::BackTab),
        ),
        palette_command(
            "Create Issue from Dialog",
            "Ctrl+Enter",
            "Issue Dialog",
            "Create the issue from the issue dialog",
            palette_ctrl_key(KeyCode::Enter),
        ),
    ];

    for (index, reaction) in ReactionContent::ALL.iter().copied().enumerate() {
        let key = char::from_digit((index + 1) as u32, 10).unwrap_or('1');
        let keys = match index {
            0 => "1",
            1 => "2",
            2 => "3",
            3 => "4",
            4 => "5",
            5 => "6",
            6 => "7",
            _ => "8",
        };
        commands.push(palette_command(
            reaction.label(),
            keys,
            "Reaction Dialog",
            "Choose and add this reaction",
            palette_key(KeyCode::Char(key)),
        ));
    }

    commands
}

fn palette_command(
    title: &'static str,
    keys: impl Into<String>,
    scope: &'static str,
    detail: &'static str,
    action: PaletteAction,
) -> PaletteCommand {
    PaletteCommand {
        title,
        keys: keys.into(),
        scope,
        detail,
        action,
    }
}

fn palette_key(code: KeyCode) -> PaletteAction {
    PaletteAction::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn palette_ctrl_key(code: KeyCode) -> PaletteAction {
    PaletteAction::Key(KeyEvent::new(code, KeyModifiers::CONTROL))
}

fn palette_alt_key(code: KeyCode) -> PaletteAction {
    PaletteAction::Key(KeyEvent::new(code, KeyModifiers::ALT))
}

pub(super) fn command_palette_filtered_indices(
    commands: &[PaletteCommand],
    query: &str,
) -> Vec<usize> {
    let query = query.trim();
    if query.is_empty() {
        return (0..commands.len()).collect();
    }

    let mut scored = commands
        .iter()
        .enumerate()
        .filter_map(|(index, command)| {
            command_palette_score(command, query).map(|score| (index, score))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|(left_index, left_score), (right_index, right_score)| {
        right_score
            .cmp(left_score)
            .then_with(|| left_index.cmp(right_index))
    });
    scored.into_iter().map(|(index, _)| index).collect()
}

fn command_palette_score(command: &PaletteCommand, query: &str) -> Option<i64> {
    let raw_query = query.trim();
    let query = command_palette_normalized_text(raw_query);
    if query.is_empty() {
        return raw_query.is_empty().then_some(0);
    }

    let fields = [
        (command.title, 40_000),
        (command.keys.as_str(), 30_000),
        (command.scope, 20_000),
        (command.detail, 10_000),
    ];
    let mut best = None;
    for (field, base) in fields {
        if let Some(score) = command_palette_text_score(field, &query) {
            best = Some(best.unwrap_or(i64::MIN).max(base + score));
        }
    }

    let combined = format!(
        "{} {} {} {}",
        command.title, command.keys, command.scope, command.detail
    );
    if let Some(score) = command_palette_text_score(&combined, &query) {
        best = Some(best.unwrap_or(i64::MIN).max(score));
    }

    best
}

fn command_palette_text_score(text: &str, query: &str) -> Option<i64> {
    let text = command_palette_normalized_text(text);
    if text.is_empty() {
        return None;
    }
    if text == query {
        return Some(30_000);
    }
    if text.starts_with(query) {
        return Some(25_000);
    }
    if let Some(index) = text.find(query) {
        return Some(22_000 - index.min(500) as i64);
    }

    let words = text.split_whitespace().collect::<Vec<_>>();
    let initials = words
        .iter()
        .filter_map(|word| word.chars().next())
        .collect::<String>();
    let mut total = 0;
    for token in query.split_whitespace() {
        total += command_palette_token_score(token, &words, &initials)?;
    }
    Some(total)
}

fn command_palette_token_score(token: &str, words: &[&str], initials: &str) -> Option<i64> {
    let mut best = None;
    for (index, word) in words.iter().enumerate() {
        let score = if *word == token {
            Some(8_000)
        } else if word.starts_with(token) {
            Some(7_000)
        } else {
            word.find(token)
                .map(|offset| 5_000 - offset.min(300) as i64)
        };

        if let Some(score) = score {
            best = Some(best.unwrap_or(i64::MIN).max(score - index.min(200) as i64));
        }
    }

    if token.chars().count() <= 4 && initials.starts_with(token) {
        best = Some(best.unwrap_or(i64::MIN).max(6_500));
    }

    best
}

pub(super) fn command_palette_normalized_text(text: &str) -> String {
    let mut normalized = String::new();
    let mut last_was_space = true;
    for ch in text.chars() {
        for lower in ch.to_lowercase() {
            if lower.is_alphanumeric() {
                normalized.push(lower);
                last_was_space = false;
            } else if command_palette_search_symbol(lower) {
                if !last_was_space {
                    normalized.push(' ');
                }
                normalized.push(lower);
                normalized.push(' ');
                last_was_space = true;
            } else if !last_was_space {
                normalized.push(' ');
                last_was_space = true;
            }
        }
    }
    normalized.trim().to_string()
}

fn command_palette_search_symbol(ch: char) -> bool {
    matches!(ch, ':' | '/' | '?' | '+' | '-' | '@' | '[' | ']')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_COMMAND_PALETTE_KEY;

    #[test]
    fn command_palette_lists_and_fuzzy_filters_shortcuts() {
        let commands = command_palette_commands(DEFAULT_COMMAND_PALETTE_KEY);
        assert!(commands.iter().any(|command| command.keys == ":"));
        assert!(commands.iter().any(|command| command.keys == "Ctrl+Enter"));
        assert!(
            commands
                .iter()
                .any(|command| command.title == "Open PR Merge Confirmation")
        );
        assert!(
            commands
                .iter()
                .any(|command| command.title == "Create Issue / PR")
        );
        assert!(
            commands
                .iter()
                .any(|command| command.title == "Change Milestone")
        );
        assert!(commands.iter().any(|command| command.title == "Info"));
        assert!(
            commands
                .iter()
                .any(|command| command.title == "Clear Cache")
        );
        assert!(
            commands
                .iter()
                .any(|command| command.title == "Mark All Read")
        );
        assert!(commands.iter().any(|command| command.title == "Mark Done"));
        assert!(
            commands
                .iter()
                .any(|command| command.title == "Mute Thread")
        );
        assert!(
            commands
                .iter()
                .any(|command| command.title == "Subscribe Thread")
        );
        assert!(
            commands
                .iter()
                .any(|command| command.title == "Unsubscribe Thread")
        );
        assert!(
            commands
                .iter()
                .any(|command| command.title == "Ignore Selected Item" && command.keys == "i")
        );
        assert!(
            commands
                .iter()
                .any(|command| command.title == "Move Focus Down"
                    && command.keys == "j / n / Down")
        );
        assert!(
            commands
                .iter()
                .any(|command| command.title == "Move Focus Up" && command.keys == "k / p / Up")
        );
        assert!(commands.iter().any(
            |command| command.title == "Move Selection Down" && command.keys == "j / n / Down"
        ));
        assert!(
            commands
                .iter()
                .any(|command| command.title == "Move Selection Up" && command.keys == "k / p / Up")
        );
        assert!(
            commands
                .iter()
                .any(|command| command.title == "Load Next Result Page" && command.keys == "]")
        );
        assert!(
            commands
                .iter()
                .any(|command| command.scope == "Reaction Dialog")
        );

        let merge_matches = command_palette_filtered_indices(&commands, "merge");
        assert_eq!(
            commands[merge_matches[0]].title,
            "Open PR Merge Confirmation"
        );

        let issue_matches = command_palette_filtered_indices(&commands, "ctrl enter issue");
        assert!(
            issue_matches
                .iter()
                .any(|index| commands[*index].title == "Create Issue from Dialog")
        );

        let milestone_matches = command_palette_filtered_indices(&commands, "milestone");
        let milestone_titles = milestone_matches
            .iter()
            .map(|index| commands[*index].title)
            .collect::<Vec<_>>();
        assert!(milestone_titles.contains(&"Change Milestone"));
        assert!(!milestone_titles.contains(&"Open PR Merge Confirmation"));
        assert!(!milestone_titles.contains(&"Open PR Close Confirmation"));
        assert!(!milestone_titles.contains(&"Open PR Approve Confirmation"));
        assert!(!milestone_titles.contains(&"Toggle Mouse Text Selection"));

        let reaction_matches = command_palette_filtered_indices(&commands, "+");
        assert!(
            reaction_matches
                .iter()
                .any(|index| commands[*index].title == "Add Reaction")
        );
        assert!(command_palette_filtered_indices(&commands, ".").is_empty());
        assert!(
            commands
                .iter()
                .any(|command| command.title == "Project Switch")
        );
        assert!(
            commands
                .iter()
                .any(|command| command.title == "Project Add")
        );
        assert!(
            commands
                .iter()
                .any(|command| command.title == "Project Remove")
        );
        assert!(
            commands
                .iter()
                .any(|command| command.title == "Copy GitHub Link")
        );
        assert!(
            commands
                .iter()
                .any(|command| command.title == "Copy Content")
        );
    }
}
