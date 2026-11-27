// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Theme module exports for rein-gui (M3 Expressive tokens, palette, and component styles)

/// Legacy and semantic color aliases backed by M3 tokens.
pub mod palette;
/// M3 Expressive surface, button,FAB, chip, and input styles.
pub mod styles;
/// Material 3 Expressive design tokens (color, shape, elevation, type scale).
pub mod tokens;

pub use palette::rein_theme;
