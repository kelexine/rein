// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Rein Obsidian Slate & Amber-Gold dark palette, syntax colors, and cached Iced Theme

use std::sync::LazyLock;

use iced::widget::markdown;
use iced::{Color, Theme, theme};

/// Main canvas background (`#111317`).
pub const BG_CANVAS: Color = Color::from_rgb(0.067, 0.075, 0.090);
/// Collapsible left sidebar background (`#0C0E12`).
pub const BG_SIDEBAR: Color = Color::from_rgb(0.047, 0.055, 0.071);
/// Right-hand inspector drawer background (`#0F1115`).
pub const BG_INSPECTOR: Color = Color::from_rgb(0.059, 0.067, 0.082);
/// Code block & expanded tool body background (`#161920`).
pub const BG_CODE: Color = Color::from_rgb(0.086, 0.098, 0.125);
/// Composer, user bubble, and card surface (`#1B1F27`).
pub const BG_SURFACE: Color = Color::from_rgb(0.106, 0.122, 0.153);
/// Elevated popover / hover surface (`#232833`).
pub const BG_ELEVATED: Color = Color::from_rgb(0.137, 0.157, 0.200);
/// Subtle 1px divider & card border (`#313847`).
pub const BORDER_SUBTLE: Color = Color::from_rgb(0.192, 0.220, 0.278);
/// Softer border for composer and user bubbles (`#282E3A`).
pub const BORDER_SOFT: Color = Color::from_rgb(0.157, 0.180, 0.227);
/// Rein Citadel Amber-Gold primary accent (`#E5A93C`).
pub const ACCENT_CORAL: Color = Color::from_rgb(0.898, 0.663, 0.235);
/// Secondary electric slate-blue accent (`#7C9EFF`).
pub const ACCENT_BLUE: Color = Color::from_rgb(0.486, 0.620, 1.000);
/// Primary crisp off-white text (`#ECEFF4`).
pub const TEXT_PRIMARY: Color = Color::from_rgb(0.925, 0.937, 0.957);
/// Muted slate-gray secondary text (`#9AA4B8`).
pub const TEXT_MUTED: Color = Color::from_rgb(0.604, 0.643, 0.722);
/// Dim hint / timestamp / line-number gutter text (`#616B7F`).
pub const TEXT_DIM: Color = Color::from_rgb(0.380, 0.420, 0.498);
/// Success dot green (`#73D18A`).
pub const STATUS_GREEN: Color = Color::from_rgb(0.451, 0.820, 0.541);
/// Running / warning amber (`#E5A93C`).
pub const STATUS_YELLOW: Color = Color::from_rgb(0.898, 0.663, 0.235);
/// Error coral-red (`#E86870`).
pub const STATUS_RED: Color = Color::from_rgb(0.910, 0.408, 0.439);

/// Syntax highlight: keywords (`#C792EA`).
pub const SYN_KEYWORD: Color = Color::from_rgb(0.780, 0.573, 0.918);
/// Syntax highlight: string literals (`#A3BE8C`).
pub const SYN_STRING: Color = Color::from_rgb(0.639, 0.745, 0.549);
/// Syntax highlight: comments (`#677084`).
pub const SYN_COMMENT: Color = Color::from_rgb(0.404, 0.439, 0.518);
/// Syntax highlight: numbers & constants (`#E5A93C`).
pub const SYN_NUMBER: Color = Color::from_rgb(0.898, 0.663, 0.235);
/// Syntax highlight: types, flags & identifiers (`#7C9EFF`).
pub const SYN_ACCENT: Color = Color::from_rgb(0.486, 0.620, 1.000);

static REIN_THEME: LazyLock<Theme> = LazyLock::new(|| {
    Theme::custom(
        "Rein Obsidian",
        theme::Palette {
            background: BG_CANVAS,
            text: TEXT_PRIMARY,
            primary: ACCENT_CORAL,
            success: STATUS_GREEN,
            warning: STATUS_YELLOW,
            danger: STATUS_RED,
        },
    )
});

static REIN_MD_SETTINGS: LazyLock<markdown::Settings> =
    LazyLock::new(|| markdown::Settings::with_text_size(14, REIN_THEME.clone()));

/// Returns the cached Rein dark [`Theme`].
#[must_use]
pub fn rein_theme() -> Theme {
    REIN_THEME.clone()
}

/// Returns cached [`markdown::Settings`] avoiding per-frame [`Theme`] cloning.
#[must_use]
pub fn rein_markdown_settings() -> markdown::Settings {
    *REIN_MD_SETTINGS
}
