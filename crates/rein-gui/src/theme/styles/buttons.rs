// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-27
// Purpose: Material 3 Expressive button, FAB, tonal pill, and chip styles with 8%/12% state layers

use iced::widget::button;
use iced::{Background, Border, Color, Theme};

use crate::theme::tokens::{
    ERROR, LEVEL_1, LEVEL_2, LEVEL_3, ON_PRIMARY, ON_PRIMARY_CONTAINER, ON_SECONDARY_CONTAINER,
    ON_SURFACE, ON_SURFACE_VARIANT, OUTLINE, OUTLINE_VARIANT, PRIMARY, PRIMARY_CONTAINER,
    RADIUS_FULL, RADIUS_LG, RADIUS_MD, SECONDARY_CONTAINER, SURFACE_CONTAINER_HIGH,
    SURFACE_CONTAINER_HIGHEST, state_layer,
};

/// M3 Primary FAB & Filled Primary button (`primary` fill, `on_primary` text, pill/squircle radius, Level 3 shadow).
#[must_use]
pub fn primary_button(_theme: &Theme, status: button::Status) -> button::Style {
    let (bg, shadow) = match status {
        button::Status::Hovered => (state_layer(PRIMARY, Color::WHITE, 0.10), LEVEL_3),
        button::Status::Pressed => (state_layer(PRIMARY, Color::WHITE, 0.16), LEVEL_1),
        button::Status::Disabled => (OUTLINE_VARIANT, LEVEL_1),
        button::Status::Active => (PRIMARY, LEVEL_2),
    };
    let fg = if matches!(status, button::Status::Disabled) {
        ON_SURFACE_VARIANT
    } else {
        ON_PRIMARY
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: fg,
        border: Border {
            radius: RADIUS_FULL.into(),
            ..Default::default()
        },
        shadow,
        ..Default::default()
    }
}

/// M3 Extended FAB style (`primary_container` tonal surface, `18px` squircle radius, Level 2 shadow).
#[must_use]
pub fn extended_fab(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => state_layer(PRIMARY_CONTAINER, ON_PRIMARY_CONTAINER, 0.08),
        button::Status::Pressed => state_layer(PRIMARY_CONTAINER, ON_PRIMARY_CONTAINER, 0.14),
        _ => PRIMARY_CONTAINER,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: ON_PRIMARY_CONTAINER,
        border: Border {
            color: Color::from_rgba(0.898, 0.663, 0.235, 0.32),
            width: 1.0,
            radius: RADIUS_LG.into(),
        },
        shadow: LEVEL_2,
        ..Default::default()
    }
}

/// M3 Filled Tonal Pill button (`secondary_container` fill, `on_secondary_container` text, full stadium radius).
#[must_use]
pub fn tonal_pill_button(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => state_layer(SECONDARY_CONTAINER, ON_SECONDARY_CONTAINER, 0.08),
        button::Status::Pressed => state_layer(SECONDARY_CONTAINER, ON_SECONDARY_CONTAINER, 0.14),
        _ => SECONDARY_CONTAINER,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: ON_SECONDARY_CONTAINER,
        border: Border {
            color: OUTLINE,
            width: 1.0,
            radius: RADIUS_FULL.into(),
        },
        shadow: LEVEL_1,
        ..Default::default()
    }
}

/// M3 Outlined / Surface Pill button (mode selector, secondary dialog actions).
#[must_use]
pub fn pill_button(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => state_layer(SURFACE_CONTAINER_HIGH, ON_SURFACE, 0.08),
        button::Status::Pressed => state_layer(SURFACE_CONTAINER_HIGH, ON_SURFACE, 0.14),
        _ => SURFACE_CONTAINER_HIGH,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: ON_SURFACE,
        border: Border {
            color: OUTLINE,
            width: 1.0,
            radius: RADIUS_FULL.into(),
        },
        shadow: LEVEL_1,
        ..Default::default()
    }
}

/// M3 Suggestion Chip button (`surface_container_high`, `14px` radius, subtle outline, Level 1 lift).
#[must_use]
pub fn suggestion_chip(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => state_layer(SURFACE_CONTAINER_HIGH, PRIMARY, 0.10),
        button::Status::Pressed => state_layer(SURFACE_CONTAINER_HIGH, PRIMARY, 0.16),
        _ => SURFACE_CONTAINER_HIGH,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: ON_SURFACE,
        border: Border {
            color: OUTLINE,
            width: 1.0,
            radius: RADIUS_MD.into(),
        },
        shadow: LEVEL_1,
        ..Default::default()
    }
}

/// M3 Standard Text / Icon button (transparent at rest, 8%/12% state layer pill on hover/press).
#[must_use]
pub fn ghost_button(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => Some(Background::Color(Color::from_rgba(
            0.925, 0.937, 0.957, 0.08,
        ))),
        button::Status::Pressed => Some(Background::Color(Color::from_rgba(
            0.925, 0.937, 0.957, 0.14,
        ))),
        _ => None,
    };
    button::Style {
        background: bg,
        text_color: ON_SURFACE,
        border: Border {
            radius: RADIUS_FULL.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// M3 Active Navigation Drawer / Rail indicator pill (`secondary_container` stadium pill).
#[must_use]
pub fn active_sidebar_button(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => state_layer(SECONDARY_CONTAINER, ON_SECONDARY_CONTAINER, 0.10),
        button::Status::Pressed => state_layer(SECONDARY_CONTAINER, ON_SECONDARY_CONTAINER, 0.16),
        _ => SECONDARY_CONTAINER,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: ON_SECONDARY_CONTAINER,
        border: Border {
            color: OUTLINE,
            width: 1.0,
            radius: RADIUS_FULL.into(),
        },
        ..Default::default()
    }
}

/// M3 Filled Error / Danger button (Stop FAB, Deny, Kill task).
#[must_use]
pub fn danger_button(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => state_layer(ERROR, Color::WHITE, 0.10),
        button::Status::Pressed => state_layer(ERROR, Color::WHITE, 0.16),
        _ => ERROR,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: Color::WHITE,
        border: Border {
            radius: RADIUS_FULL.into(),
            ..Default::default()
        },
        shadow: LEVEL_2,
        ..Default::default()
    }
}

/// M3 Tonal Icon Button (circular `+` trigger inside the floating composer).
#[must_use]
pub fn tonal_icon_button(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => state_layer(SURFACE_CONTAINER_HIGHEST, ON_SURFACE, 0.10),
        button::Status::Pressed => state_layer(SURFACE_CONTAINER_HIGHEST, ON_SURFACE, 0.16),
        _ => SURFACE_CONTAINER_HIGHEST,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: ON_SURFACE,
        border: Border {
            radius: RADIUS_FULL.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}
