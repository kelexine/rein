// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-27
// Purpose: Material 3 Expressive dark tonal palette seeded from Rein Obsidian & Citadel Amber-Gold

use std::sync::LazyLock;

use iced::widget::markdown;
use iced::{Color, Theme, theme};

/// M3 `surface` / main canvas background (`#111317`).
pub const SURFACE: Color = Color::from_rgb(0.067, 0.075, 0.090);
/// M3 `surface_container_lowest` (`#0B0D10`).
pub const SURFACE_CONTAINER_LOWEST: Color = Color::from_rgb(0.043, 0.051, 0.063);
/// M3 `surface_container_low` — nav rail, drawer & side sheet (`#14171D`).
pub const SURFACE_CONTAINER_LOW: Color = Color::from_rgb(0.078, 0.090, 0.114);
/// M3 `surface_container` — code blocks & tonal cards (`#181C24`).
pub const SURFACE_CONTAINER: Color = Color::from_rgb(0.094, 0.110, 0.141);
/// M3 `surface_container_high` — floating composer, dialogs & elevated cards (`#202530`).
pub const SURFACE_CONTAINER_HIGH: Color = Color::from_rgb(0.125, 0.145, 0.188);
/// M3 `surface_container_highest` — popovers, active pills & hover containers (`#29303E`).
pub const SURFACE_CONTAINER_HIGHEST: Color = Color::from_rgb(0.161, 0.188, 0.243);
/// Translucent floating glass surface (`rgba(32, 37, 48, 0.90)`).
pub const SURFACE_FLOATING_GLASS: Color = Color::from_rgba(0.125, 0.145, 0.188, 0.90);

/// M3 `primary` — Rein Citadel Amber-Gold (`#E5A93C`).
pub const PRIMARY: Color = Color::from_rgb(0.898, 0.663, 0.235);
/// M3 `on_primary` — deep contrast ink on amber-gold (`#1E1400`).
pub const ON_PRIMARY: Color = Color::from_rgb(0.118, 0.078, 0.000);
/// M3 `primary_container` — warm amber-tinted container (`#352810`).
pub const PRIMARY_CONTAINER: Color = Color::from_rgb(0.208, 0.157, 0.063);
/// M3 `on_primary_container` — luminous warm cream text (`#FFE0AC`).
pub const ON_PRIMARY_CONTAINER: Color = Color::from_rgb(1.000, 0.878, 0.675);

/// M3 `secondary` — electric slate-blue (`#7C9EFF`).
pub const SECONDARY: Color = Color::from_rgb(0.486, 0.620, 1.000);
/// M3 `secondary_container` — deep indigo-slate tonal fill (`#1D2944`).
pub const SECONDARY_CONTAINER: Color = Color::from_rgb(0.114, 0.161, 0.267);
/// M3 `on_secondary_container` — crisp ice-blue text (`#D8E2FF`).
pub const ON_SECONDARY_CONTAINER: Color = Color::from_rgb(0.847, 0.886, 1.000);

/// M3 `tertiary` — emerald green (`#73D18A`).
pub const TERTIARY: Color = Color::from_rgb(0.451, 0.820, 0.541);
/// M3 `error` — coral red (`#E86870`).
pub const ERROR: Color = Color::from_rgb(0.910, 0.408, 0.439);
/// M3 `error_container` (`#411519`).
pub const ERROR_CONTAINER: Color = Color::from_rgb(0.255, 0.082, 0.098);
/// M3 `on_error_container` (`#FFDAD6`).
pub const ON_ERROR_CONTAINER: Color = Color::from_rgb(1.000, 0.855, 0.839);

/// M3 `on_surface` — primary crisp off-white text (`#ECEFF4`).
pub const ON_SURFACE: Color = Color::from_rgb(0.925, 0.937, 0.957);
/// M3 `on_surface_variant` — muted slate-gray secondary text (`#9AA4B8`).
pub const ON_SURFACE_VARIANT: Color = Color::from_rgb(0.604, 0.643, 0.722);
/// M3 `outline` — subtle control border (`#3A4254`).
pub const OUTLINE: Color = Color::from_rgb(0.227, 0.259, 0.329);
/// M3 `outline_variant` — soft 1px divider & tonal card border (`#262C38`).
pub const OUTLINE_VARIANT: Color = Color::from_rgb(0.149, 0.173, 0.220);
/// M3 `scrim` — modal backdrop tint.
pub const SCRIM: Color = Color::from_rgba(0.024, 0.031, 0.047, 0.64);

/// Blends an M3 state-layer `overlay` onto `base` at `alpha` (`0.08` hover, `0.12` press).
#[must_use]
pub fn state_layer(base: Color, overlay: Color, alpha: f32) -> Color {
    let a = alpha.clamp(0.0, 1.0);
    let inv = 1.0 - a;
    Color {
        r: base.r * inv + overlay.r * a,
        g: base.g * inv + overlay.g * a,
        b: base.b * inv + overlay.b * a,
        a: base.a,
    }
}

/// Computes WCAG relative luminance of an sRGB [`Color`].
#[must_use]
pub fn relative_luminance(c: Color) -> f32 {
    fn channel(v: f32) -> f32 {
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
}

/// Computes WCAG contrast ratio (`1.0..=21.0`) between two colors.
#[must_use]
pub fn contrast_ratio(a: Color, b: Color) -> f32 {
    let la = relative_luminance(a);
    let lb = relative_luminance(b);
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

static REIN_THEME: LazyLock<Theme> = LazyLock::new(|| {
    Theme::custom(
        "Rein M3 Expressive Dark",
        theme::Palette {
            background: SURFACE,
            text: ON_SURFACE,
            primary: PRIMARY,
            success: TERTIARY,
            warning: PRIMARY,
            danger: ERROR,
        },
    )
});

static REIN_MD_SETTINGS: LazyLock<markdown::Settings> =
    LazyLock::new(|| markdown::Settings::with_text_size(14, REIN_THEME.clone()));

/// Returns the cached Rein M3 dark [`Theme`].
#[must_use]
pub fn rein_theme() -> Theme {
    REIN_THEME.clone()
}

/// Returns cached [`markdown::Settings`] avoiding per-frame [`Theme`] cloning.
#[must_use]
pub fn rein_markdown_settings() -> markdown::Settings {
    *REIN_MD_SETTINGS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn m3_roles_meet_wcag_contrast_and_state_layer_blends() {
        assert!(contrast_ratio(ON_SURFACE, SURFACE) >= 12.0);
        assert!(contrast_ratio(PRIMARY, ON_PRIMARY) >= 7.0);
        assert!(contrast_ratio(ON_PRIMARY_CONTAINER, PRIMARY_CONTAINER) >= 7.0);
        assert!(contrast_ratio(ON_SECONDARY_CONTAINER, SECONDARY_CONTAINER) >= 7.0);

        let hovered = state_layer(SURFACE_CONTAINER_HIGH, ON_SURFACE, 0.08);
        assert!(hovered.r > SURFACE_CONTAINER_HIGH.r);
    }
}
