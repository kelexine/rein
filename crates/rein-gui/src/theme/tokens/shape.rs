// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-27
// Purpose: Material 3 Expressive corner radius scale and asymmetric shape helpers

use iced::border::Radius;

/// Extra-small corner radius (`4px`).
pub const RADIUS_XS: f32 = 4.0;
/// Small corner radius (`8px`).
pub const RADIUS_SM: f32 = 8.0;
/// Medium corner radius (`14px` — chips, code blocks, menu items).
pub const RADIUS_MD: f32 = 14.0;
/// Large corner radius (`20px` — FABs, tool cards, navigation containers).
pub const RADIUS_LG: f32 = 20.0;
/// Extra-large expressive corner radius (`28px` — floating composer, M3 dialogs).
pub const RADIUS_XL: f32 = 28.0;
/// Full pill / stadium corner radius (`999px`).
pub const RADIUS_FULL: f32 = 999.0;

/// Asymmetric M3 Expressive user chat bubble radius (`22px` with a `6px` bottom-right tail).
#[must_use]
pub fn user_bubble_radius() -> Radius {
    Radius {
        top_left: 22.0,
        top_right: 22.0,
        bottom_right: 6.0,
        bottom_left: 22.0,
    }
}

/// M3 Navigation Drawer radius (rounded trailing `24px` corners).
#[must_use]
pub fn nav_drawer_radius() -> Radius {
    Radius {
        top_left: 0.0,
        top_right: 24.0,
        bottom_right: 24.0,
        bottom_left: 0.0,
    }
}

/// M3 Side Sheet inspector radius (rounded leading `24px` corners).
#[must_use]
pub fn side_sheet_radius() -> Radius {
    Radius {
        top_left: 24.0,
        top_right: 0.0,
        bottom_right: 0.0,
        bottom_left: 24.0,
    }
}
