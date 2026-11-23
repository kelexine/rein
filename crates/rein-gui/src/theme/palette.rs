// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Claude Desktop-inspired warm dark charcoal palette and cached Iced Theme/Markdown settings

use std::sync::LazyLock;

use iced::widget::markdown;
use iced::{Color, Theme, theme};

/// Main canvas background (`#1F1E1C`).
pub const BG_CANVAS: Color = Color::from_rgb(0.122, 0.118, 0.110);
/// Collapsible left sidebar background (`#181715`).
pub const BG_SIDEBAR: Color = Color::from_rgb(0.094, 0.090, 0.082);
/// Right-hand inspector drawer background (`#1C1B19`).
pub const BG_INSPECTOR: Color = Color::from_rgb(0.110, 0.106, 0.098);
/// Code block & expanded tool body background (`#262522`).
pub const BG_CODE: Color = Color::from_rgb(0.149, 0.145, 0.133);
/// Composer, user bubble, and card surface (`#2B2A27`).
pub const BG_SURFACE: Color = Color::from_rgb(0.169, 0.165, 0.153);
/// Elevated popover / hover surface (`#34322E`).
pub const BG_ELEVATED: Color = Color::from_rgb(0.204, 0.196, 0.180);
/// Subtle 1px divider & card border (`#3D3B37`).
pub const BORDER_SUBTLE: Color = Color::from_rgb(0.239, 0.231, 0.216);
/// Warm terracotta/coral accent (`#D97757`).
pub const ACCENT_CORAL: Color = Color::from_rgb(0.851, 0.467, 0.341);
/// Secondary muted slate-blue accent for sandbox badges.
pub const ACCENT_BLUE: Color = Color::from_rgb(0.478, 0.686, 0.922);
/// Primary warm off-white text (`#ECEBE6`).
pub const TEXT_PRIMARY: Color = Color::from_rgb(0.925, 0.922, 0.902);
/// Muted warm gray secondary text (`#9B9890`).
pub const TEXT_MUTED: Color = Color::from_rgb(0.608, 0.596, 0.565);
/// Dim hint / timestamp text (`#6F6C65`).
pub const TEXT_DIM: Color = Color::from_rgb(0.435, 0.424, 0.396);
/// Success dot green.
pub const STATUS_GREEN: Color = Color::from_rgb(0.451, 0.804, 0.490);
/// Running / warning amber.
pub const STATUS_YELLOW: Color = Color::from_rgb(0.918, 0.749, 0.365);
/// Error red.
pub const STATUS_RED: Color = Color::from_rgb(0.894, 0.427, 0.427);

static REIN_THEME: LazyLock<Theme> = LazyLock::new(|| {
    Theme::custom(
        "Rein Dark",
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
