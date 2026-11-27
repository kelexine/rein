// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-27
// Purpose: Material 3 Expressive container styles (floating bars, tonal surfaces, asymmetric bubbles, sheets)

use iced::border::Radius;
use iced::widget::container;
use iced::{Background, Border, Color, Theme};

use crate::theme::tokens::{
    LEVEL_1, LEVEL_2, LEVEL_3, LEVEL_4, ON_PRIMARY, ON_PRIMARY_CONTAINER, ON_SURFACE,
    ON_SURFACE_VARIANT, OUTLINE, OUTLINE_VARIANT, PRIMARY, PRIMARY_CONTAINER, RADIUS_FULL,
    RADIUS_LG, RADIUS_MD, RADIUS_XL, SECONDARY_CONTAINER, SURFACE, SURFACE_CONTAINER,
    SURFACE_CONTAINER_HIGH, SURFACE_CONTAINER_HIGHEST, SURFACE_CONTAINER_LOW,
    SURFACE_FLOATING_GLASS, nav_drawer_radius, side_sheet_radius, user_bubble_radius,
};

/// Main M3 `surface` canvas container style.
#[must_use]
pub fn canvas_container(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE)),
        text_color: Some(ON_SURFACE),
        ..Default::default()
    }
}

/// M3 Floating Top App Bar (translucent tonal glass pill with Level 2 shadow).
#[must_use]
pub fn floating_app_bar(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE_FLOATING_GLASS)),
        border: Border {
            color: OUTLINE_VARIANT,
            width: 1.0,
            radius: RADIUS_FULL.into(),
        },
        text_color: Some(ON_SURFACE),
        shadow: LEVEL_2,
        ..Default::default()
    }
}

/// M3 Navigation Rail container (`68px` vertical strip on leading edge).
#[must_use]
pub fn nav_rail_container(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE_CONTAINER_LOW)),
        border: Border {
            color: OUTLINE_VARIANT,
            width: 1.0,
            radius: Radius::default(),
        },
        text_color: Some(ON_SURFACE),
        ..Default::default()
    }
}

/// M3 Navigation Drawer container (expanded sidebar with `24px` trailing corners and Level 2 shadow).
#[must_use]
pub fn sidebar_container(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE_CONTAINER_LOW)),
        border: Border {
            color: OUTLINE_VARIANT,
            width: 1.0,
            radius: nav_drawer_radius(),
        },
        text_color: Some(ON_SURFACE),
        shadow: LEVEL_2,
        ..Default::default()
    }
}

/// M3 Side Sheet inspector container (`24px` leading corners and Level 2 shadow).
#[must_use]
pub fn inspector_container(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE_CONTAINER_LOW)),
        border: Border {
            color: OUTLINE_VARIANT,
            width: 1.0,
            radius: side_sheet_radius(),
        },
        text_color: Some(ON_SURFACE),
        shadow: LEVEL_2,
        ..Default::default()
    }
}

/// M3 Expressive floating composer pill (`28px` radius, `surface_container_high`, Level 3 shadow).
#[must_use]
pub fn composer_card(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE_CONTAINER_HIGH)),
        border: Border {
            color: OUTLINE,
            width: 1.0,
            radius: RADIUS_XL.into(),
        },
        text_color: Some(ON_SURFACE),
        shadow: LEVEL_3,
        ..Default::default()
    }
}

/// M3 Expressive user chat bubble (`primary_container` tonal surface, asymmetric tail, Level 1 shadow).
#[must_use]
pub fn user_bubble(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(PRIMARY_CONTAINER)),
        border: Border {
            color: Color::from_rgba(0.898, 0.663, 0.235, 0.28),
            width: 1.0,
            radius: user_bubble_radius(),
        },
        text_color: Some(ON_PRIMARY_CONTAINER),
        shadow: LEVEL_1,
        ..Default::default()
    }
}

/// Standard M3 tonal card container (`surface_container_high`, `20px` radius, Level 1 shadow).
#[must_use]
pub fn surface_card(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE_CONTAINER_HIGH)),
        border: Border {
            color: OUTLINE_VARIANT,
            width: 1.0,
            radius: RADIUS_LG.into(),
        },
        text_color: Some(ON_SURFACE),
        shadow: LEVEL_1,
        ..Default::default()
    }
}

/// Compact `PASTED` attachment card (`secondary_container` tint, `16px` radius, Level 1 shadow).
#[must_use]
pub fn pasted_badge_card(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SECONDARY_CONTAINER)),
        border: Border {
            color: OUTLINE,
            width: 1.0,
            radius: RADIUS_LG.into(),
        },
        text_color: Some(ON_SURFACE),
        shadow: LEVEL_1,
        ..Default::default()
    }
}

/// M3 Expressive Dialog & Popover container (`surface_container_highest`, `28px` radius, Level 4 shadow).
#[must_use]
pub fn elevated_card(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE_CONTAINER_HIGHEST)),
        border: Border {
            color: OUTLINE,
            width: 1.0,
            radius: RADIUS_XL.into(),
        },
        text_color: Some(ON_SURFACE),
        shadow: LEVEL_4,
        ..Default::default()
    }
}

/// M3 Tonal code / tool disclosure container (`surface_container`, `14px` radius).
#[must_use]
pub fn disclosure_card(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE_CONTAINER)),
        border: Border {
            color: OUTLINE_VARIANT,
            width: 1.0,
            radius: RADIUS_MD.into(),
        },
        text_color: Some(ON_SURFACE_VARIANT),
        ..Default::default()
    }
}

/// Expressive squircle badge container for the Citadel Owl hero emblem on the Home screen.
#[must_use]
pub fn hero_emblem_badge(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(PRIMARY_CONTAINER)),
        border: Border {
            color: Color::from_rgba(0.898, 0.663, 0.235, 0.35),
            width: 1.0,
            radius: RADIUS_LG.into(),
        },
        text_color: Some(PRIMARY),
        shadow: LEVEL_2,
        ..Default::default()
    }
}

/// Circular M3 primary user monogram avatar badge.
#[must_use]
pub fn avatar_badge(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(PRIMARY)),
        border: Border {
            radius: RADIUS_FULL.into(),
            ..Default::default()
        },
        text_color: Some(ON_PRIMARY),
        shadow: LEVEL_1,
        ..Default::default()
    }
}
