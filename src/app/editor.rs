use super::*;
use tui_textarea::{CursorMove, TextArea, WrapMode};

#[derive(Debug, Clone)]
pub(super) struct EditorText {
    textarea: TextArea<'static>,
    text: String,
}

impl PartialEq for EditorText {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text && self.textarea.cursor() == other.textarea.cursor()
    }
}

impl Eq for EditorText {}

impl PartialEq<&str> for EditorText {
    fn eq(&self, other: &&str) -> bool {
        self.text() == *other
    }
}

impl EditorText {
    #[cfg(test)]
    pub(super) fn empty() -> Self {
        Self::from_text("")
    }

    pub(super) fn from_text(text: impl AsRef<str>) -> Self {
        let lines = text.as_ref().split('\n').map(str::to_string).collect();
        let mut editor = Self::from_lines(lines);
        editor.move_to_end();
        editor
    }

    pub(super) fn from_lines(mut lines: Vec<String>) -> Self {
        if lines.is_empty() {
            lines.push(String::new());
        }
        let mut textarea = TextArea::new(lines);
        textarea.set_wrap_mode(WrapMode::Glyph);
        textarea.set_style(modal_text_style());
        textarea.set_cursor_line_style(active_theme().panel());
        textarea.set_cursor_style(active_theme().active());
        let text = textarea.lines().join("\n");
        Self { textarea, text }
    }

    pub(super) fn text(&self) -> &str {
        &self.text
    }

    #[cfg(test)]
    pub(super) fn as_str(&self) -> &str {
        self.text()
    }

    #[cfg(test)]
    pub(super) fn contains(&self, needle: &str) -> bool {
        self.text.contains(needle)
    }

    #[cfg(test)]
    pub(super) fn clear(&mut self) {
        *self = Self::empty();
    }

    pub(super) fn set_text(&mut self, text: impl AsRef<str>) {
        *self = Self::from_text(text);
    }

    pub(super) fn replace_range(&mut self, start: usize, end: usize, replacement: &str) {
        let start = clamp_text_cursor(self.text(), start);
        let end = clamp_text_cursor(self.text(), end).max(start);
        let mut next = String::with_capacity(
            self.text
                .len()
                .saturating_sub(end.saturating_sub(start))
                .saturating_add(replacement.len()),
        );
        next.push_str(&self.text[..start]);
        next.push_str(replacement);
        next.push_str(&self.text[end..]);
        *self = Self::from_text(next);
        self.set_cursor_byte(start.saturating_add(replacement.len()));
    }

    fn sync_text(&mut self) {
        self.text = self.textarea.lines().join("\n");
    }

    pub(super) fn cursor_byte(&self) -> usize {
        let (row, col) = self.textarea.cursor();
        let mut offset = 0_usize;
        for line in self.textarea.lines().iter().take(row) {
            offset = offset.saturating_add(line.len()).saturating_add(1);
        }
        let Some(line) = self.textarea.lines().get(row) else {
            return offset;
        };
        offset.saturating_add(byte_for_char_column(line, col))
    }

    pub(super) fn set_cursor_byte(&mut self, cursor: usize) {
        let cursor = clamp_text_cursor(self.text(), cursor);
        let mut remaining = cursor;
        for (row, line) in self.textarea.lines().iter().enumerate() {
            if remaining <= line.len() {
                let col = line[..remaining].chars().count();
                self.move_to(row, col);
                return;
            }
            remaining = remaining.saturating_sub(line.len().saturating_add(1));
        }
        self.move_to_end();
    }

    pub(super) fn move_to(&mut self, row: usize, col: usize) {
        self.textarea.move_cursor(CursorMove::Jump(
            row.min(usize::from(u16::MAX)) as u16,
            col.min(usize::from(u16::MAX)) as u16,
        ));
    }

    pub(super) fn move_to_end(&mut self) {
        self.textarea.move_cursor(CursorMove::Bottom);
        self.textarea.move_cursor(CursorMove::End);
    }

    pub(super) fn input_key(&mut self, key: KeyEvent, multiline: bool) -> bool {
        match key.code {
            KeyCode::Enter if !multiline => false,
            KeyCode::Enter => {
                self.textarea.input(key);
                self.sync_text();
                true
            }
            KeyCode::Char('m') if !multiline && key.modifiers.contains(KeyModifiers::CONTROL) => {
                false
            }
            KeyCode::Char('z' | 'Z') if editor_redo_key(key) => {
                self.textarea.redo();
                self.sync_text();
                true
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.textarea.delete_line_by_head();
                self.sync_text();
                true
            }
            KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.delete_current_line();
                true
            }
            KeyCode::Char('z' | 'Z') if editor_undo_key(key) => {
                self.textarea.undo();
                self.sync_text();
                true
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.textarea.redo();
                self.sync_text();
                true
            }
            KeyCode::PageDown | KeyCode::PageUp => false,
            KeyCode::Char(_)
            | KeyCode::Tab
            | KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Home
            | KeyCode::End => {
                self.textarea.input(key);
                self.sync_text();
                true
            }
            _ => false,
        }
    }

    pub(super) fn delete_current_line(&mut self) -> bool {
        let (row, _) = self.textarea.cursor();
        let old_lines = self.textarea.lines();
        if old_lines.len() == 1 {
            if old_lines.first().is_none_or(String::is_empty) {
                return false;
            }
            *self = Self::from_lines(vec![String::new()]);
            return true;
        }

        let mut lines = old_lines.to_vec();
        if row < lines.len() {
            lines.remove(row);
        }
        let next_row = row.min(lines.len().saturating_sub(1));
        *self = Self::from_lines(lines);
        self.move_to(next_row, 0);
        true
    }
}

pub(super) fn editor_undo_key(key: KeyEvent) -> bool {
    key.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
        && !key.modifiers.contains(KeyModifiers::SHIFT)
}

pub(super) fn editor_redo_key(key: KeyEvent) -> bool {
    key.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
        && key.modifiers.contains(KeyModifiers::SHIFT)
}

pub(super) fn editor_save_draft_key(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('s' | 'S'))
        && key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
        && !key.modifiers.contains(KeyModifiers::ALT)
}

pub(super) fn byte_for_char_column(text: &str, column: usize) -> usize {
    text.char_indices()
        .nth(column)
        .map(|(index, _)| index)
        .unwrap_or(text.len())
}
