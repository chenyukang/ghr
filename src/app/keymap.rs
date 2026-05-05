use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::DEFAULT_COMMAND_PALETTE_KEY;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct KeyBinding {
    label: String,
    code: KeyCode,
    modifiers: KeyModifiers,
}

impl KeyBinding {
    pub(super) fn matches(&self, key: KeyEvent) -> bool {
        let expected_modifiers = command_palette_modifier_bits(self.modifiers);
        let actual_modifiers = command_palette_modifier_bits(key.modifiers);
        if expected_modifiers != actual_modifiers {
            return false;
        }
        if self.modifiers.contains(KeyModifiers::SHIFT)
            && !key.modifiers.contains(KeyModifiers::SHIFT)
        {
            return false;
        }

        match (&self.code, key.code) {
            (KeyCode::Char(expected), KeyCode::Char(actual))
                if expected_modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                expected.eq_ignore_ascii_case(&actual)
            }
            (KeyCode::Char(expected), KeyCode::Char(actual)) => *expected == actual,
            (expected, actual) => *expected == actual,
        }
    }

    pub(super) fn is_plain_text_char(&self) -> bool {
        matches!(self.code, KeyCode::Char(value) if !value.is_control())
            && !self
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    }
}

pub(super) fn normalized_command_palette_key(value: &str) -> String {
    command_palette_key_binding(value).label
}

pub(super) fn command_palette_key_binding(value: &str) -> KeyBinding {
    parse_key_binding(value).unwrap_or_else(|| {
        parse_key_binding(DEFAULT_COMMAND_PALETTE_KEY).expect("default command palette key parses")
    })
}

fn parse_key_binding(value: &str) -> Option<KeyBinding> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if value.chars().count() == 1 {
        let ch = value.chars().next()?;
        return Some(KeyBinding {
            label: value.to_string(),
            code: KeyCode::Char(ch),
            modifiers: KeyModifiers::NONE,
        });
    }

    let mut modifiers = KeyModifiers::NONE;
    let mut key = None;
    for raw_part in value.split('+') {
        let part = raw_part.trim();
        if part.is_empty() {
            return None;
        }
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers.insert(KeyModifiers::CONTROL),
            "alt" | "option" => modifiers.insert(KeyModifiers::ALT),
            "shift" => modifiers.insert(KeyModifiers::SHIFT),
            _ if key.is_none() => key = Some(part.to_string()),
            _ => return None,
        }
    }

    let key = key?;
    let code = parse_key_code(&key)?;
    Some(KeyBinding {
        label: key_binding_label(modifiers, &code),
        code,
        modifiers,
    })
}

fn parse_key_code(value: &str) -> Option<KeyCode> {
    let lower = value.to_ascii_lowercase();
    match lower.as_str() {
        "esc" | "escape" => Some(KeyCode::Esc),
        "enter" | "return" => Some(KeyCode::Enter),
        "tab" => Some(KeyCode::Tab),
        "backtab" => Some(KeyCode::BackTab),
        "backspace" => Some(KeyCode::Backspace),
        "space" => Some(KeyCode::Char(' ')),
        _ if value.chars().count() == 1 => Some(KeyCode::Char(value.chars().next()?)),
        _ => None,
    }
}

fn key_binding_label(modifiers: KeyModifiers, code: &KeyCode) -> String {
    let mut parts = Vec::new();
    if modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("Ctrl".to_string());
    }
    if modifiers.contains(KeyModifiers::ALT) {
        parts.push("Alt".to_string());
    }
    if modifiers.contains(KeyModifiers::SHIFT) {
        parts.push("Shift".to_string());
    }
    parts.push(key_code_label_with_modifiers(code, modifiers));
    parts.join("+")
}

fn key_code_label_with_modifiers(code: &KeyCode, modifiers: KeyModifiers) -> String {
    match code {
        KeyCode::Char(value)
            if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                && value.is_ascii_alphabetic() =>
        {
            value.to_ascii_uppercase().to_string()
        }
        _ => key_code_label(code),
    }
}

fn key_code_label(code: &KeyCode) -> String {
    match code {
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "Shift+Tab".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(value) => value.to_string(),
        other => format!("{other:?}"),
    }
}

fn command_palette_modifier_bits(modifiers: KeyModifiers) -> KeyModifiers {
    modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT)
}
