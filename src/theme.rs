use std::cell::Cell;

use ratatui::style::{Color, Modifier, Style};
use serde::{Deserialize, Serialize};

thread_local! {
    static ACTIVE_THEME: Cell<u8> = const { Cell::new(ThemeName::Dark as u8) };
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeName {
    #[default]
    Dark = 0,
    Light = 1,
}

impl ThemeName {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Light,
            _ => Self::Dark,
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::Dark => Self::Light,
            Self::Light => Self::Dark,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub background: Color,
    pub surface: Color,
    pub text: Color,
    pub muted: Color,
    pub subtle: Color,
    pub border: Color,
    pub focus: Color,
    pub focus_alt: Color,
    pub highlight_fg: Color,
    pub highlight_bg: Color,
    pub selected_fg: Color,
    pub selected_bg: Color,
    pub link: Color,
    pub action: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub info: Color,
    pub added: Color,
    pub removed: Color,
    pub code: Color,
    pub quote: Color,
    pub reaction: Color,
    pub label: Color,
    pub search: Color,
}

impl Theme {
    pub fn from_name(name: ThemeName) -> Self {
        match name {
            ThemeName::Dark => Self {
                background: Color::Reset,
                surface: Color::Reset,
                text: Color::White,
                muted: Color::Gray,
                subtle: Color::DarkGray,
                border: Color::DarkGray,
                focus: Color::Cyan,
                focus_alt: Color::Yellow,
                highlight_fg: Color::Black,
                highlight_bg: Color::LightCyan,
                selected_fg: Color::White,
                selected_bg: Color::DarkGray,
                link: Color::LightBlue,
                action: Color::LightMagenta,
                success: Color::LightGreen,
                warning: Color::Yellow,
                error: Color::LightRed,
                info: Color::Cyan,
                added: Color::LightGreen,
                removed: Color::LightRed,
                code: Color::Gray,
                quote: Color::DarkGray,
                reaction: Color::LightYellow,
                label: Color::LightBlue,
                search: Color::Yellow,
            },
            ThemeName::Light => Self {
                background: Color::Rgb(248, 250, 252),
                surface: Color::Rgb(248, 250, 252),
                text: Color::Rgb(30, 41, 59),
                muted: Color::Rgb(100, 116, 139),
                subtle: Color::Rgb(148, 163, 184),
                border: Color::Rgb(203, 213, 225),
                focus: Color::Rgb(3, 105, 161),
                focus_alt: Color::Rgb(133, 77, 14),
                highlight_fg: Color::Rgb(248, 250, 252),
                highlight_bg: Color::Rgb(3, 105, 161),
                selected_fg: Color::Rgb(15, 23, 42),
                selected_bg: Color::Rgb(226, 232, 240),
                link: Color::Rgb(79, 70, 229),
                action: Color::Rgb(190, 24, 93),
                success: Color::Rgb(22, 101, 52),
                warning: Color::Rgb(161, 98, 7),
                error: Color::Rgb(185, 28, 28),
                info: Color::Rgb(3, 105, 161),
                added: Color::Rgb(22, 101, 52),
                removed: Color::Rgb(185, 28, 28),
                code: Color::Rgb(71, 85, 105),
                quote: Color::Rgb(100, 116, 139),
                reaction: Color::Rgb(161, 98, 7),
                label: Color::Rgb(79, 70, 229),
                search: Color::Rgb(194, 65, 12),
            },
        }
    }

    pub fn base(self) -> Style {
        self.with_bg(Style::default().fg(self.text), self.background)
    }

    pub fn panel(self) -> Style {
        self.with_bg(Style::default().fg(self.text), self.surface)
    }

    pub fn muted(self) -> Style {
        self.panel().fg(self.muted)
    }

    pub fn subtle(self) -> Style {
        self.panel().fg(self.subtle)
    }

    pub fn border(self) -> Style {
        self.panel().fg(self.border)
    }

    pub fn focus_border(self) -> Style {
        self.panel().fg(self.focus).add_modifier(Modifier::BOLD)
    }

    pub fn focus_alt_border(self) -> Style {
        self.panel().fg(self.focus_alt).add_modifier(Modifier::BOLD)
    }

    pub fn active(self) -> Style {
        self.panel()
            .fg(self.highlight_fg)
            .bg(self.highlight_bg)
            .add_modifier(Modifier::BOLD)
    }

    pub fn selected(self) -> Style {
        self.panel()
            .fg(self.selected_fg)
            .bg(self.selected_bg)
            .add_modifier(Modifier::BOLD)
    }

    pub fn link(self) -> Style {
        self.panel()
            .fg(self.link)
            .add_modifier(Modifier::UNDERLINED)
    }

    pub fn action(self) -> Style {
        self.panel()
            .fg(self.action)
            .add_modifier(Modifier::UNDERLINED)
    }

    fn with_bg(self, style: Style, background: Color) -> Style {
        if matches!(background, Color::Reset) {
            style
        } else {
            style.bg(background)
        }
    }
}

pub fn set_active_theme(name: ThemeName) {
    ACTIVE_THEME.with(|theme| theme.set(name as u8));
}

pub fn active_theme_name() -> ThemeName {
    ACTIVE_THEME.with(|theme| ThemeName::from_u8(theme.get()))
}

pub fn active_theme() -> Theme {
    Theme::from_name(active_theme_name())
}
