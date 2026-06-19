use unicode_width::UnicodeWidthChar;

pub(super) fn display_width(text: &str) -> usize {
    let mut width = 0;
    let mut last_char_width = None;
    for ch in text.chars() {
        if ch == '\u{fe0f}' {
            if let Some(previous_width) = last_char_width
                && previous_width < 2
            {
                width += 2 - previous_width;
                last_char_width = Some(2);
            }
            continue;
        }

        let char_width = display_width_char(ch);
        width += char_width;
        last_char_width = Some(char_width);
    }
    width
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

pub(super) fn expand_tabs(text: &str, tab_width: usize) -> String {
    let tab_width = tab_width.max(1);
    let mut expanded = String::new();
    let mut column = 0_usize;
    let mut last_char_width = None;

    for ch in text.chars() {
        match ch {
            '\t' => {
                let spaces = tab_width - (column % tab_width);
                for _ in 0..spaces {
                    expanded.push(' ');
                }
                column += spaces;
                last_char_width = Some(spaces);
            }
            '\n' => {
                expanded.push(ch);
                column = 0;
                last_char_width = None;
            }
            '\u{fe0f}' => {
                expanded.push(ch);
                if let Some(previous_width) = last_char_width
                    && previous_width < 2
                {
                    column += 2 - previous_width;
                    last_char_width = Some(2);
                }
            }
            _ => {
                expanded.push(ch);
                let char_width = display_width_char(ch);
                column += char_width;
                last_char_width = Some(char_width);
            }
        }
    }

    expanded
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
