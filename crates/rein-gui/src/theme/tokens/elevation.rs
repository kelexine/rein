// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-27
// Purpose: Material 3 elevation shadow presets (Levels 0 through 4)

use iced::{Color, Shadow, Vector};

/// Level 0 — flat surface, no shadow.
pub const LEVEL_0: Shadow = Shadow {
    color: Color::TRANSPARENT,
    offset: Vector::new(0.0, 0.0),
    blur_radius: 0.0,
};

/// Level 1 — subtle lift for chips, user bubbles, and tonal cards.
pub const LEVEL_1: Shadow = Shadow {
    color: Color::from_rgba(0.0, 0.0, 0.0, 0.28),
    offset: Vector::new(0.0, 2.0),
    blur_radius: 8.0,
};

/// Level 2 — floating top app bar, navigation drawer, and side sheet.
pub const LEVEL_2: Shadow = Shadow {
    color: Color::from_rgba(0.0, 0.0, 0.0, 0.36),
    offset: Vector::new(0.0, 4.0),
    blur_radius: 16.0,
};

/// Level 3 — floating pill composer and primary/extended FABs.
pub const LEVEL_3: Shadow = Shadow {
    color: Color::from_rgba(0.0, 0.0, 0.0, 0.45),
    offset: Vector::new(0.0, 8.0),
    blur_radius: 24.0,
};

/// Level 4 — modal dialogs, floating `+` action menu, and model picker popover.
pub const LEVEL_4: Shadow = Shadow {
    color: Color::from_rgba(0.0, 0.0, 0.0, 0.56),
    offset: Vector::new(0.0, 12.0),
    blur_radius: 32.0,
};
