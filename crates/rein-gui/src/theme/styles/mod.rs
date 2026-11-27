// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-27
// Purpose: Material 3 Expressive component style exports (surfaces, buttons, inputs)

/// M3 button, FAB, tonal pill, and suggestion chip styles.
pub mod buttons;
/// M3 text input styles.
pub mod inputs;
/// M3 container, floating app bar, nav rail/drawer, bubble, and dialog surface styles.
pub mod surfaces;

pub use buttons::{
    active_sidebar_button, danger_button, extended_fab, ghost_button, pill_button, primary_button,
    suggestion_chip, tonal_icon_button, tonal_pill_button,
};
pub use inputs::composer_input;
pub use surfaces::{
    avatar_badge, canvas_container, composer_card, disclosure_card, elevated_card,
    floating_app_bar, hero_emblem_badge, inspector_container, nav_rail_container,
    pasted_badge_card, sidebar_container, surface_card, user_bubble,
};
