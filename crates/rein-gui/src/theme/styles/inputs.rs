// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-27
// Purpose: Material 3 text input styles for floating composer and dialog fields

use iced::widget::text_input;
use iced::{Background, Border, Color, Theme};

use crate::theme::tokens::{ON_SURFACE, ON_SURFACE_VARIANT};

/// Borderless composer text input style inside the M3 floating pill composer.
#[must_use]
pub fn composer_input(_theme: &Theme, _status: text_input::Status) -> text_input::Style {
    text_input::Style {
        background: Background::Color(Color::TRANSPARENT),
        border: Border::default(),
        icon: ON_SURFACE_VARIANT,
        placeholder: ON_SURFACE_VARIANT,
        value: ON_SURFACE,
        selection: Color::from_rgba(0.898, 0.663, 0.235, 0.30),
    }
}
