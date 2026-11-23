// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Claude Desktop-inspired dark color palette and Iced Theme construction

use std::sync::LazyLock;

use iced::{Color, Theme, theme};

/// Main canvas background (`#1e1e1c`).
pub const BG_CANVAS: Color = Color::from_rgb(0.118, 0.118, 0.110);
/// Collapsible left sidebar background (`#181816`).
pub const BG_SIDEBAR: Color = Color::from_rgb(0.094, 0.094, 0.086);
/// Composer, user bubble, and card surface (`#262624`).
pub const BG_SURFACE: Color = Color::from_rgb(0.149, 0.149, 0.141);
/// Elevated popover / hover surface (`#2f2f2c`).
pub const BG_ELEVATED: Color = Color::from_rgb(0.184, 0.184, 0.173);
/// Subtle 1px divider & card border (`#3a3a37`).
pub const BORDER_SUBTLE: Color = Color::from_rgb(0.227, 0.227, 0.216);
/// Warm coral accent (`#d97757` — starburst & active highlights).
pub const ACCENT_CORAL: Color = Color::from_rgb(0.851, 0.467, 0.341);
/// Secondary cyan/blue accent for links and active badges.
pub const ACCENT_BLUE: Color = Color::from_rgb(0.451, 0.690, 0.961);
/// Primary warm off-white text (`#ecece8`).
pub const TEXT_PRIMARY: Color = Color::from_rgb(0.925, 0.925, 0.910);
/// Muted secondary text (`#9a9a94`).
pub const TEXT_MUTED: Color = Color::from_rgb(0.604, 0.604, 0.580);
/// Dim hint / timestamp text (`#6b6b66`).
pub const TEXT_DIM: Color = Color::from_rgb(0.420, 0.420, 0.400);
/// Success dot green.
pub const STATUS_GREEN: Color = Color::from_rgb(0.451, 0.824, 0.490);
/// Running / warning yellow.
pub const STATUS_YELLOW: Color = Color::from_rgb(0.918, 0.761, 0.357);
/// Error red.
pub const STATUS_RED: Color = Color::from_rgb(0.902, 0.420, 0.420);

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

/// Returns the cached Rein dark [`Theme`].
#[must_use]
pub fn rein_theme() -> Theme {
    REIN_THEME.clone()
}
