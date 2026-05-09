use std::cell::Cell;
#[cfg(target_os = "macos")]
use std::process::Command;

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
    CatppuccinMocha = 2,
    CatppuccinLatte = 3,
    Nord = 4,
    GruvboxDark = 5,
    GruvboxLight = 6,
    TokyoNight = 7,
    Dracula = 8,
    SolarizedDark = 9,
    SolarizedLight = 10,
    OneDark = 11,
    Monokai = 12,
    #[serde(rename = "github_dark", alias = "git_hub_dark")]
    GitHubDark = 13,
    #[serde(rename = "github_light", alias = "git_hub_light")]
    GitHubLight = 14,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreference {
    #[default]
    Auto,
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeFamily {
    Dark,
    Light,
}

impl ThemeName {
    pub const ALL: &[ThemeName] = &[
        ThemeName::Dark,
        ThemeName::Light,
        ThemeName::CatppuccinMocha,
        ThemeName::CatppuccinLatte,
        ThemeName::Nord,
        ThemeName::GruvboxDark,
        ThemeName::GruvboxLight,
        ThemeName::TokyoNight,
        ThemeName::Dracula,
        ThemeName::SolarizedDark,
        ThemeName::SolarizedLight,
        ThemeName::OneDark,
        ThemeName::Monokai,
        ThemeName::GitHubDark,
        ThemeName::GitHubLight,
    ];

    fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Dark,
            1 => Self::Light,
            2 => Self::CatppuccinMocha,
            3 => Self::CatppuccinLatte,
            4 => Self::Nord,
            5 => Self::GruvboxDark,
            6 => Self::GruvboxLight,
            7 => Self::TokyoNight,
            8 => Self::Dracula,
            9 => Self::SolarizedDark,
            10 => Self::SolarizedLight,
            11 => Self::OneDark,
            12 => Self::Monokai,
            13 => Self::GitHubDark,
            14 => Self::GitHubLight,
            _ => Self::Dark,
        }
    }

    pub fn family(self) -> ThemeFamily {
        match self {
            Self::Dark => ThemeFamily::Dark,
            Self::Light => ThemeFamily::Light,
            Self::CatppuccinMocha => ThemeFamily::Dark,
            Self::CatppuccinLatte => ThemeFamily::Light,
            Self::Nord => ThemeFamily::Dark,
            Self::GruvboxDark => ThemeFamily::Dark,
            Self::GruvboxLight => ThemeFamily::Light,
            Self::TokyoNight => ThemeFamily::Dark,
            Self::Dracula => ThemeFamily::Dark,
            Self::SolarizedDark => ThemeFamily::Dark,
            Self::SolarizedLight => ThemeFamily::Light,
            Self::OneDark => ThemeFamily::Dark,
            Self::Monokai => ThemeFamily::Dark,
            Self::GitHubDark => ThemeFamily::Dark,
            Self::GitHubLight => ThemeFamily::Light,
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::Dark => Self::Light,
            Self::Light => Self::Dark,
            Self::CatppuccinMocha => Self::CatppuccinLatte,
            Self::CatppuccinLatte => Self::CatppuccinMocha,
            Self::Nord => Self::Light,
            Self::GruvboxDark => Self::GruvboxLight,
            Self::GruvboxLight => Self::GruvboxDark,
            Self::TokyoNight => Self::Light,
            Self::Dracula => Self::Light,
            Self::SolarizedDark => Self::SolarizedLight,
            Self::SolarizedLight => Self::SolarizedDark,
            Self::OneDark => Self::Light,
            Self::Monokai => Self::Light,
            Self::GitHubDark => Self::GitHubLight,
            Self::GitHubLight => Self::GitHubDark,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::CatppuccinMocha => "catppuccin_mocha",
            Self::CatppuccinLatte => "catppuccin_latte",
            Self::Nord => "nord",
            Self::GruvboxDark => "gruvbox_dark",
            Self::GruvboxLight => "gruvbox_light",
            Self::TokyoNight => "tokyo_night",
            Self::Dracula => "dracula",
            Self::SolarizedDark => "solarized_dark",
            Self::SolarizedLight => "solarized_light",
            Self::OneDark => "one_dark",
            Self::Monokai => "monokai",
            Self::GitHubDark => "github_dark",
            Self::GitHubLight => "github_light",
        }
    }
}

impl ThemePreference {
    pub fn effective(self) -> ThemeName {
        self.effective_with(detect_system_theme)
    }

    fn effective_with(self, detect: impl FnOnce() -> Option<ThemeName>) -> ThemeName {
        match self {
            Self::Auto => detect().unwrap_or(ThemeName::Dark),
            Self::Dark => ThemeName::Dark,
            Self::Light => ThemeName::Light,
        }
    }

    pub fn from_theme_name(theme: ThemeName) -> Self {
        match theme.family() {
            ThemeFamily::Dark => Self::Dark,
            ThemeFamily::Light => Self::Light,
        }
    }

    pub fn is_auto(self) -> bool {
        matches!(self, Self::Auto)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

pub fn detect_system_theme() -> Option<ThemeName> {
    detect_macos_system_theme()
}

#[cfg(target_os = "macos")]
fn detect_macos_system_theme() -> Option<ThemeName> {
    let output = Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
        .ok()?;
    macos_system_theme_from_defaults_output(output.status.success(), &output.stdout, &output.stderr)
}

#[cfg(target_os = "macos")]
fn macos_system_theme_from_defaults_output(
    success: bool,
    stdout: &[u8],
    stderr: &[u8],
) -> Option<ThemeName> {
    if !success {
        let error = String::from_utf8_lossy(stderr);
        if error.contains("does not exist") {
            return Some(ThemeName::Light);
        }
        return None;
    }
    let style = String::from_utf8_lossy(stdout).to_ascii_lowercase();
    if style.trim().contains("dark") {
        Some(ThemeName::Dark)
    } else {
        Some(ThemeName::Light)
    }
}

#[cfg(not(target_os = "macos"))]
fn detect_macos_system_theme() -> Option<ThemeName> {
    None
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
    pub code_bg: Color,
    pub quote_bg: Color,
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
                background: Color::Rgb(36, 39, 51),
                surface: Color::Rgb(36, 39, 51),
                text: Color::White,
                muted: Color::Gray,
                subtle: Color::Rgb(124, 132, 158),
                border: Color::Rgb(92, 104, 132),
                focus: Color::Cyan,
                focus_alt: Color::Yellow,
                highlight_fg: Color::Black,
                highlight_bg: Color::LightCyan,
                selected_fg: Color::White,
                selected_bg: Color::DarkGray,
                code_bg: Color::Rgb(27, 31, 42),
                quote_bg: Color::Rgb(43, 47, 61),
                link: Color::LightBlue,
                action: Color::LightMagenta,
                success: Color::LightGreen,
                warning: Color::Yellow,
                error: Color::LightRed,
                info: Color::Cyan,
                added: Color::LightGreen,
                removed: Color::LightRed,
                code: Color::Gray,
                quote: Color::Rgb(156, 163, 185),
                reaction: Color::LightYellow,
                label: Color::LightBlue,
                search: Color::Yellow,
            },
            ThemeName::Light => Self {
                background: Color::Rgb(241, 245, 249),
                surface: Color::Rgb(241, 245, 249),
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
                code_bg: Color::Rgb(226, 232, 240),
                quote_bg: Color::Rgb(226, 232, 240),
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
            // https://catppuccin.com/palette — Mocha
            ThemeName::CatppuccinMocha => Self {
                background: Color::Rgb(30, 30, 46),
                surface: Color::Rgb(24, 24, 37),
                text: Color::Rgb(205, 214, 244),
                muted: Color::Rgb(108, 112, 134),
                subtle: Color::Rgb(166, 173, 200),
                border: Color::Rgb(69, 71, 90),
                focus: Color::Rgb(137, 180, 250),
                focus_alt: Color::Rgb(250, 179, 135),
                highlight_fg: Color::Rgb(30, 30, 46),
                highlight_bg: Color::Rgb(137, 180, 250),
                selected_fg: Color::Rgb(205, 214, 244),
                selected_bg: Color::Rgb(69, 71, 90),
                code_bg: Color::Rgb(17, 17, 27),
                quote_bg: Color::Rgb(49, 50, 68),
                link: Color::Rgb(137, 180, 250),
                action: Color::Rgb(245, 194, 231),
                success: Color::Rgb(166, 227, 161),
                warning: Color::Rgb(249, 226, 175),
                error: Color::Rgb(243, 139, 168),
                info: Color::Rgb(137, 220, 235),
                added: Color::Rgb(166, 227, 161),
                removed: Color::Rgb(243, 139, 168),
                code: Color::Rgb(166, 173, 200),
                quote: Color::Rgb(186, 194, 222),
                reaction: Color::Rgb(249, 226, 175),
                label: Color::Rgb(137, 180, 250),
                search: Color::Rgb(249, 226, 175),
            },
            // https://catppuccin.com/palette — Latte
            ThemeName::CatppuccinLatte => Self {
                background: Color::Rgb(239, 241, 245),
                surface: Color::Rgb(230, 233, 239),
                text: Color::Rgb(76, 79, 105),
                muted: Color::Rgb(140, 143, 161),
                subtle: Color::Rgb(156, 160, 176),
                border: Color::Rgb(204, 208, 218),
                focus: Color::Rgb(30, 102, 245),
                focus_alt: Color::Rgb(254, 100, 11),
                highlight_fg: Color::Rgb(239, 241, 245),
                highlight_bg: Color::Rgb(30, 102, 245),
                selected_fg: Color::Rgb(76, 79, 105),
                selected_bg: Color::Rgb(204, 208, 218),
                code_bg: Color::Rgb(230, 233, 239),
                quote_bg: Color::Rgb(204, 208, 218),
                link: Color::Rgb(30, 102, 245),
                action: Color::Rgb(234, 118, 203),
                success: Color::Rgb(64, 160, 43),
                warning: Color::Rgb(223, 142, 29),
                error: Color::Rgb(210, 15, 57),
                info: Color::Rgb(4, 165, 229),
                added: Color::Rgb(64, 160, 43),
                removed: Color::Rgb(210, 15, 57),
                code: Color::Rgb(124, 127, 147),
                quote: Color::Rgb(92, 95, 119),
                reaction: Color::Rgb(223, 142, 29),
                label: Color::Rgb(30, 102, 245),
                search: Color::Rgb(223, 142, 29),
            },
            // https://www.nordtheme.com/docs/colors-and-fonts
            ThemeName::Nord => Self {
                background: Color::Rgb(46, 52, 64),
                surface: Color::Rgb(59, 66, 82),
                text: Color::Rgb(216, 222, 233),
                muted: Color::Rgb(97, 110, 136),
                subtle: Color::Rgb(129, 161, 193),
                border: Color::Rgb(76, 86, 106),
                focus: Color::Rgb(136, 192, 208),
                focus_alt: Color::Rgb(208, 135, 112),
                highlight_fg: Color::Rgb(46, 52, 64),
                highlight_bg: Color::Rgb(136, 192, 208),
                selected_fg: Color::Rgb(216, 222, 233),
                selected_bg: Color::Rgb(76, 86, 106),
                code_bg: Color::Rgb(46, 52, 64),
                quote_bg: Color::Rgb(59, 66, 82),
                link: Color::Rgb(136, 192, 208),
                action: Color::Rgb(180, 142, 173),
                success: Color::Rgb(163, 190, 140),
                warning: Color::Rgb(235, 203, 139),
                error: Color::Rgb(191, 97, 106),
                info: Color::Rgb(129, 161, 193),
                added: Color::Rgb(163, 190, 140),
                removed: Color::Rgb(191, 97, 106),
                code: Color::Rgb(129, 161, 193),
                quote: Color::Rgb(216, 222, 233),
                reaction: Color::Rgb(235, 203, 139),
                label: Color::Rgb(136, 192, 208),
                search: Color::Rgb(235, 203, 139),
            },
            // https://github.com/morhetz/gruvbox
            ThemeName::GruvboxDark => Self {
                background: Color::Rgb(40, 40, 40),
                surface: Color::Rgb(29, 32, 33),
                text: Color::Rgb(235, 219, 178),
                muted: Color::Rgb(146, 131, 116),
                subtle: Color::Rgb(168, 153, 132),
                border: Color::Rgb(80, 73, 69),
                focus: Color::Rgb(131, 165, 152),
                focus_alt: Color::Rgb(254, 128, 25),
                highlight_fg: Color::Rgb(40, 40, 40),
                highlight_bg: Color::Rgb(131, 165, 152),
                selected_fg: Color::Rgb(235, 219, 178),
                selected_bg: Color::Rgb(80, 73, 69),
                code_bg: Color::Rgb(29, 32, 33),
                quote_bg: Color::Rgb(60, 56, 54),
                link: Color::Rgb(131, 165, 152),
                action: Color::Rgb(211, 134, 155),
                success: Color::Rgb(184, 187, 38),
                warning: Color::Rgb(250, 189, 47),
                error: Color::Rgb(251, 73, 52),
                info: Color::Rgb(131, 165, 152),
                added: Color::Rgb(184, 187, 38),
                removed: Color::Rgb(251, 73, 52),
                code: Color::Rgb(168, 153, 132),
                quote: Color::Rgb(189, 174, 147),
                reaction: Color::Rgb(250, 189, 47),
                label: Color::Rgb(131, 165, 152),
                search: Color::Rgb(250, 189, 47),
            },
            ThemeName::GruvboxLight => Self {
                background: Color::Rgb(251, 241, 199),
                surface: Color::Rgb(242, 229, 188),
                text: Color::Rgb(60, 56, 54),
                muted: Color::Rgb(146, 131, 116),
                subtle: Color::Rgb(124, 111, 100),
                border: Color::Rgb(213, 196, 161),
                focus: Color::Rgb(7, 102, 120),
                focus_alt: Color::Rgb(175, 58, 3),
                highlight_fg: Color::Rgb(251, 241, 199),
                highlight_bg: Color::Rgb(7, 102, 120),
                selected_fg: Color::Rgb(60, 56, 54),
                selected_bg: Color::Rgb(213, 196, 161),
                code_bg: Color::Rgb(242, 229, 188),
                quote_bg: Color::Rgb(235, 219, 178),
                link: Color::Rgb(7, 102, 120),
                action: Color::Rgb(143, 63, 113),
                success: Color::Rgb(121, 116, 14),
                warning: Color::Rgb(181, 118, 20),
                error: Color::Rgb(157, 0, 6),
                info: Color::Rgb(7, 102, 120),
                added: Color::Rgb(121, 116, 14),
                removed: Color::Rgb(157, 0, 6),
                code: Color::Rgb(124, 111, 100),
                quote: Color::Rgb(80, 73, 69),
                reaction: Color::Rgb(181, 118, 20),
                label: Color::Rgb(7, 102, 120),
                search: Color::Rgb(181, 118, 20),
            },
            // https://github.com/folke/tokyonight.nvim
            ThemeName::TokyoNight => Self {
                background: Color::Rgb(26, 27, 38),
                surface: Color::Rgb(22, 22, 30),
                text: Color::Rgb(192, 202, 245),
                muted: Color::Rgb(86, 95, 137),
                subtle: Color::Rgb(154, 165, 206),
                border: Color::Rgb(59, 66, 97),
                focus: Color::Rgb(122, 162, 247),
                focus_alt: Color::Rgb(255, 158, 100),
                highlight_fg: Color::Rgb(26, 27, 38),
                highlight_bg: Color::Rgb(122, 162, 247),
                selected_fg: Color::Rgb(192, 202, 245),
                selected_bg: Color::Rgb(59, 66, 97),
                code_bg: Color::Rgb(22, 22, 30),
                quote_bg: Color::Rgb(41, 46, 66),
                link: Color::Rgb(122, 162, 247),
                action: Color::Rgb(187, 154, 247),
                success: Color::Rgb(158, 206, 106),
                warning: Color::Rgb(224, 175, 104),
                error: Color::Rgb(247, 118, 142),
                info: Color::Rgb(125, 207, 255),
                added: Color::Rgb(158, 206, 106),
                removed: Color::Rgb(247, 118, 142),
                code: Color::Rgb(154, 165, 206),
                quote: Color::Rgb(172, 176, 208),
                reaction: Color::Rgb(224, 175, 104),
                label: Color::Rgb(122, 162, 247),
                search: Color::Rgb(224, 175, 104),
            },
            // https://draculatheme.com
            ThemeName::Dracula => Self {
                background: Color::Rgb(40, 42, 54),
                surface: Color::Rgb(33, 34, 44),
                text: Color::Rgb(248, 248, 242),
                muted: Color::Rgb(98, 114, 164),
                subtle: Color::Rgb(189, 147, 249),
                border: Color::Rgb(68, 71, 90),
                focus: Color::Rgb(189, 147, 249),
                focus_alt: Color::Rgb(255, 184, 108),
                highlight_fg: Color::Rgb(40, 42, 54),
                highlight_bg: Color::Rgb(189, 147, 249),
                selected_fg: Color::Rgb(248, 248, 242),
                selected_bg: Color::Rgb(68, 71, 90),
                code_bg: Color::Rgb(33, 34, 44),
                quote_bg: Color::Rgb(68, 71, 90),
                link: Color::Rgb(139, 233, 253),
                action: Color::Rgb(255, 121, 198),
                success: Color::Rgb(80, 250, 123),
                warning: Color::Rgb(241, 250, 140),
                error: Color::Rgb(255, 85, 85),
                info: Color::Rgb(139, 233, 253),
                added: Color::Rgb(80, 250, 123),
                removed: Color::Rgb(255, 85, 85),
                code: Color::Rgb(139, 233, 253),
                quote: Color::Rgb(241, 250, 140),
                reaction: Color::Rgb(241, 250, 140),
                label: Color::Rgb(139, 233, 253),
                search: Color::Rgb(241, 250, 140),
            },
            // https://ethanschoonover.com/solarized
            ThemeName::SolarizedDark => Self {
                background: Color::Rgb(0, 43, 54),
                surface: Color::Rgb(7, 54, 66),
                text: Color::Rgb(147, 161, 161),
                muted: Color::Rgb(88, 110, 117),
                subtle: Color::Rgb(101, 123, 131),
                border: Color::Rgb(88, 110, 117),
                focus: Color::Rgb(38, 139, 210),
                focus_alt: Color::Rgb(203, 75, 22),
                highlight_fg: Color::Rgb(0, 43, 54),
                highlight_bg: Color::Rgb(38, 139, 210),
                selected_fg: Color::Rgb(147, 161, 161),
                selected_bg: Color::Rgb(88, 110, 117),
                code_bg: Color::Rgb(7, 54, 66),
                quote_bg: Color::Rgb(0, 43, 54),
                link: Color::Rgb(38, 139, 210),
                action: Color::Rgb(211, 54, 130),
                success: Color::Rgb(133, 153, 0),
                warning: Color::Rgb(181, 137, 0),
                error: Color::Rgb(220, 50, 47),
                info: Color::Rgb(42, 161, 152),
                added: Color::Rgb(133, 153, 0),
                removed: Color::Rgb(220, 50, 47),
                code: Color::Rgb(131, 148, 150),
                quote: Color::Rgb(147, 161, 161),
                reaction: Color::Rgb(181, 137, 0),
                label: Color::Rgb(38, 139, 210),
                search: Color::Rgb(181, 137, 0),
            },
            ThemeName::SolarizedLight => Self {
                background: Color::Rgb(253, 246, 227),
                surface: Color::Rgb(238, 232, 213),
                text: Color::Rgb(88, 110, 117),
                muted: Color::Rgb(147, 161, 161),
                subtle: Color::Rgb(131, 148, 150),
                border: Color::Rgb(147, 161, 161),
                focus: Color::Rgb(38, 139, 210),
                focus_alt: Color::Rgb(203, 75, 22),
                highlight_fg: Color::Rgb(253, 246, 227),
                highlight_bg: Color::Rgb(38, 139, 210),
                selected_fg: Color::Rgb(88, 110, 117),
                selected_bg: Color::Rgb(147, 161, 161),
                code_bg: Color::Rgb(238, 232, 213),
                quote_bg: Color::Rgb(253, 246, 227),
                link: Color::Rgb(38, 139, 210),
                action: Color::Rgb(211, 54, 130),
                success: Color::Rgb(133, 153, 0),
                warning: Color::Rgb(181, 137, 0),
                error: Color::Rgb(220, 50, 47),
                info: Color::Rgb(42, 161, 152),
                added: Color::Rgb(133, 153, 0),
                removed: Color::Rgb(220, 50, 47),
                code: Color::Rgb(131, 148, 150),
                quote: Color::Rgb(101, 123, 131),
                reaction: Color::Rgb(181, 137, 0),
                label: Color::Rgb(38, 139, 210),
                search: Color::Rgb(181, 137, 0),
            },
            // https://github.com/atom/atom/tree/master/packages/one-dark-ui
            ThemeName::OneDark => Self {
                background: Color::Rgb(40, 44, 52),
                surface: Color::Rgb(33, 37, 43),
                text: Color::Rgb(171, 178, 191),
                muted: Color::Rgb(92, 99, 112),
                subtle: Color::Rgb(130, 137, 151),
                border: Color::Rgb(62, 68, 81),
                focus: Color::Rgb(97, 175, 239),
                focus_alt: Color::Rgb(209, 154, 102),
                highlight_fg: Color::Rgb(40, 44, 52),
                highlight_bg: Color::Rgb(97, 175, 239),
                selected_fg: Color::Rgb(171, 178, 191),
                selected_bg: Color::Rgb(62, 68, 81),
                code_bg: Color::Rgb(33, 37, 43),
                quote_bg: Color::Rgb(44, 49, 60),
                link: Color::Rgb(97, 175, 239),
                action: Color::Rgb(198, 120, 221),
                success: Color::Rgb(152, 195, 121),
                warning: Color::Rgb(229, 192, 123),
                error: Color::Rgb(224, 108, 117),
                info: Color::Rgb(86, 182, 194),
                added: Color::Rgb(152, 195, 121),
                removed: Color::Rgb(224, 108, 117),
                code: Color::Rgb(130, 137, 151),
                quote: Color::Rgb(171, 178, 191),
                reaction: Color::Rgb(229, 192, 123),
                label: Color::Rgb(97, 175, 239),
                search: Color::Rgb(229, 192, 123),
            },
            // https://monokai.pro
            ThemeName::Monokai => Self {
                background: Color::Rgb(39, 40, 34),
                surface: Color::Rgb(30, 31, 28),
                text: Color::Rgb(248, 248, 242),
                muted: Color::Rgb(117, 113, 94),
                subtle: Color::Rgb(166, 166, 166),
                border: Color::Rgb(73, 72, 62),
                focus: Color::Rgb(166, 226, 46),
                focus_alt: Color::Rgb(253, 151, 31),
                highlight_fg: Color::Rgb(39, 40, 34),
                highlight_bg: Color::Rgb(166, 226, 46),
                selected_fg: Color::Rgb(248, 248, 242),
                selected_bg: Color::Rgb(73, 72, 62),
                code_bg: Color::Rgb(30, 31, 28),
                quote_bg: Color::Rgb(62, 61, 50),
                link: Color::Rgb(102, 217, 239),
                action: Color::Rgb(249, 38, 114),
                success: Color::Rgb(166, 226, 46),
                warning: Color::Rgb(230, 219, 116),
                error: Color::Rgb(249, 38, 114),
                info: Color::Rgb(102, 217, 239),
                added: Color::Rgb(166, 226, 46),
                removed: Color::Rgb(249, 38, 114),
                code: Color::Rgb(166, 166, 166),
                quote: Color::Rgb(230, 219, 116),
                reaction: Color::Rgb(230, 219, 116),
                label: Color::Rgb(102, 217, 239),
                search: Color::Rgb(230, 219, 116),
            },
            // https://primer.style — GitHub dark
            ThemeName::GitHubDark => Self {
                background: Color::Rgb(13, 17, 23),
                surface: Color::Rgb(22, 27, 34),
                text: Color::Rgb(201, 209, 217),
                muted: Color::Rgb(139, 148, 158),
                subtle: Color::Rgb(110, 118, 129),
                border: Color::Rgb(48, 54, 61),
                focus: Color::Rgb(88, 166, 255),
                focus_alt: Color::Rgb(240, 136, 62),
                highlight_fg: Color::Rgb(13, 17, 23),
                highlight_bg: Color::Rgb(88, 166, 255),
                selected_fg: Color::Rgb(201, 209, 217),
                selected_bg: Color::Rgb(48, 54, 61),
                code_bg: Color::Rgb(22, 27, 34),
                quote_bg: Color::Rgb(33, 38, 45),
                link: Color::Rgb(88, 166, 255),
                action: Color::Rgb(247, 120, 186),
                success: Color::Rgb(63, 185, 80),
                warning: Color::Rgb(210, 153, 34),
                error: Color::Rgb(248, 81, 73),
                info: Color::Rgb(88, 166, 255),
                added: Color::Rgb(63, 185, 80),
                removed: Color::Rgb(248, 81, 73),
                code: Color::Rgb(139, 148, 158),
                quote: Color::Rgb(201, 209, 217),
                reaction: Color::Rgb(210, 153, 34),
                label: Color::Rgb(88, 166, 255),
                search: Color::Rgb(210, 153, 34),
            },
            ThemeName::GitHubLight => Self {
                background: Color::Rgb(255, 255, 255),
                surface: Color::Rgb(246, 248, 250),
                text: Color::Rgb(36, 41, 47),
                muted: Color::Rgb(101, 109, 118),
                subtle: Color::Rgb(87, 96, 106),
                border: Color::Rgb(208, 215, 222),
                focus: Color::Rgb(9, 105, 218),
                focus_alt: Color::Rgb(207, 34, 46),
                highlight_fg: Color::Rgb(255, 255, 255),
                highlight_bg: Color::Rgb(9, 105, 218),
                selected_fg: Color::Rgb(36, 41, 47),
                selected_bg: Color::Rgb(208, 215, 222),
                code_bg: Color::Rgb(246, 248, 250),
                quote_bg: Color::Rgb(246, 248, 250),
                link: Color::Rgb(9, 105, 218),
                action: Color::Rgb(207, 34, 46),
                success: Color::Rgb(26, 127, 55),
                warning: Color::Rgb(154, 103, 0),
                error: Color::Rgb(207, 34, 46),
                info: Color::Rgb(9, 105, 218),
                added: Color::Rgb(26, 127, 55),
                removed: Color::Rgb(207, 34, 46),
                code: Color::Rgb(101, 109, 118),
                quote: Color::Rgb(87, 96, 106),
                reaction: Color::Rgb(154, 103, 0),
                label: Color::Rgb(9, 105, 218),
                search: Color::Rgb(154, 103, 0),
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

#[cfg(test)]
mod tests {
    use super::*;
    use toml;

    #[test]
    fn auto_theme_uses_detected_system_theme() {
        assert_eq!(
            ThemePreference::Auto.effective_with(|| Some(ThemeName::Light)),
            ThemeName::Light
        );
        assert_eq!(
            ThemePreference::Auto.effective_with(|| Some(ThemeName::Dark)),
            ThemeName::Dark
        );
    }

    #[test]
    fn auto_theme_falls_back_to_dark_when_detection_fails() {
        assert_eq!(
            ThemePreference::Auto.effective_with(|| None),
            ThemeName::Dark
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_theme_detection_failure_uses_auto_fallback() {
        assert_eq!(
            macos_system_theme_from_defaults_output(false, b"", b"permission denied"),
            None
        );
        assert_eq!(
            ThemePreference::Auto.effective_with(|| {
                macos_system_theme_from_defaults_output(false, b"", b"permission denied")
            }),
            ThemeName::Dark
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_missing_interface_style_means_light_theme() {
        assert_eq!(
            macos_system_theme_from_defaults_output(
                false,
                b"",
                b"The domain/default pair of (kCFPreferencesAnyApplication, AppleInterfaceStyle) does not exist",
            ),
            Some(ThemeName::Light)
        );
    }

    #[test]
    fn dark_theme_uses_explicit_background() {
        let theme = Theme::from_name(ThemeName::Dark);

        assert_ne!(theme.background, Color::Reset);
        assert_ne!(theme.surface, Color::Reset);
    }

    #[test]
    fn dark_theme_uses_visible_chrome_colors() {
        let theme = Theme::from_name(ThemeName::Dark);

        assert_ne!(theme.border, Color::DarkGray);
        assert_ne!(theme.subtle, Color::DarkGray);
        assert_ne!(theme.border, theme.background);
        assert_ne!(theme.subtle, theme.background);
        assert_ne!(theme.code_bg, theme.background);
        assert_ne!(theme.quote_bg, theme.background);
    }

    #[test]
    fn all_themes_use_explicit_colors() {
        for &name in ThemeName::ALL {
            let theme = Theme::from_name(name);
            assert_ne!(
                theme.background,
                Color::Reset,
                "{name:?} should define background"
            );
            assert_ne!(theme.surface, Color::Reset, "{name:?} should define surface");
            assert_ne!(theme.text, Color::Reset, "{name:?} should define text");
            assert_ne!(theme.border, theme.background, "{name:?} border must differ from background");
        }
    }

    #[test]
    fn theme_family_is_consistent() {
        for &name in ThemeName::ALL {
            let theme = Theme::from_name(name);
            assert!(
                !name.as_str().is_empty(),
                "{name:?} should have a non-empty as_str"
            );
            // Dark themes should have darker backgrounds than text
            if name.family() == ThemeFamily::Dark {
                let bg = match theme.background {
                    Color::Rgb(r, g, b) => (r as u32 + g as u32 + b as u32) / 3,
                    _ => 0,
                };
                let fg = match theme.text {
                    Color::Rgb(r, g, b) => (r as u32 + g as u32 + b as u32) / 3,
                    _ => 255,
                };
                assert!(
                    bg < fg,
                    "{name:?} dark theme: background brightness ({bg}) should be less than text brightness ({fg})"
                );
            }
        }
    }

    #[test]
    fn all_theme_names_have_unique_discriminants() {
        let mut seen = std::collections::HashSet::new();
        for &name in ThemeName::ALL {
            assert!(
                seen.insert(name as u8),
                "duplicate discriminant: {name:?} = {}",
                name as u8
            );
        }
    }

    #[test]
    fn from_u8_round_trips_all_variants() {
        for &name in ThemeName::ALL {
            assert_eq!(ThemeName::from_u8(name as u8), name);
        }
    }

    #[test]
    fn github_theme_variants_serialize_as_github_not_git_hub() {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Wrapper {
            theme: ThemeName,
        }
        for (name, expected) in [
            (ThemeName::GitHubDark, "github_dark"),
            (ThemeName::GitHubLight, "github_light"),
        ] {
            let wrapper = Wrapper { theme: name };
            let encoded = toml::to_string_pretty(&wrapper).unwrap();
            assert!(
                encoded.contains(&format!("theme = \"{expected}\"")),
                "expected theme = \"{expected}\" in:\n{encoded}"
            );
            let decoded: Wrapper =
                toml::from_str(&format!("theme = \"{expected}\"")).unwrap();
            assert_eq!(decoded.theme, name);
        }
    }

    #[test]
    fn toggle_flips_family() {
        for &name in ThemeName::ALL {
            let toggled = name.toggled();
            assert_ne!(name, toggled, "{name:?} toggle must change theme");
            assert_ne!(
                name.family(),
                toggled.family(),
                "{name:?} toggle must flip family"
            );
        }
    }
}
