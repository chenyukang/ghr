use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(super) fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

pub(super) fn display_width_char(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(0)
}

pub(super) fn normalize_text(text: &str) -> String {
    text.replace('\r', "")
        .replace('\t', "    ")
        .trim()
        .to_string()
}

pub(super) fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    let mut truncated = text.chars().take(max_chars).collect::<String>();
    truncated.push_str("\n...");
    truncated
}

pub(super) fn truncate_inline(text: &str, max_width: usize) -> String {
    if display_width(text) <= max_width {
        return text.to_string();
    }

    if max_width <= 3 {
        return ".".repeat(max_width);
    }

    let mut truncated = String::new();
    let mut width = 0_usize;
    let limit = max_width.saturating_sub(3);
    for ch in text.chars() {
        let char_width = display_width_char(ch);
        if width.saturating_add(char_width) > limit {
            break;
        }
        truncated.push(ch);
        width = width.saturating_add(char_width);
    }
    truncated.push_str("...");
    truncated
}
