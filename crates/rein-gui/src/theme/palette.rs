// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Palette aliases backed by Material 3 Expressive dark color tokens

use iced::Color;

pub use super::tokens::color::{
    ERROR as STATUS_RED, ON_PRIMARY, ON_PRIMARY_CONTAINER, ON_SECONDARY_CONTAINER,
    ON_SURFACE as TEXT_PRIMARY, ON_SURFACE_VARIANT as TEXT_MUTED, OUTLINE as BORDER_SUBTLE,
    OUTLINE_VARIANT as BORDER_SOFT, PRIMARY as ACCENT_CORAL, PRIMARY as STATUS_YELLOW,
    PRIMARY_CONTAINER, SCRIM, SECONDARY as ACCENT_BLUE, SECONDARY_CONTAINER, SURFACE as BG_CANVAS,
    SURFACE_CONTAINER as BG_CODE, SURFACE_CONTAINER_HIGH as BG_SURFACE,
    SURFACE_CONTAINER_HIGHEST as BG_ELEVATED, SURFACE_CONTAINER_LOW as BG_INSPECTOR,
    SURFACE_CONTAINER_LOW as BG_SIDEBAR, SURFACE_FLOATING_GLASS, TERTIARY as STATUS_GREEN,
    contrast_ratio, rein_markdown_settings, rein_theme, relative_luminance, state_layer,
};

/// Dim hint / timestamp / line-number gutter text (`#616B7F`).
pub const TEXT_DIM: Color = Color::from_rgb(0.380, 0.420, 0.498);

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
