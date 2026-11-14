// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Public exports for the in-process Ratatui terminal frontend

/// Application state for the Ratatui frontend.
pub mod app;
/// Raw terminal RAII guard and interactive event loop.
pub mod terminal;
/// Ratatui layout and frame renderer.
pub mod view;

pub use app::AppState;
pub use terminal::run_tui;
pub use view::render_frame;
