// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Ratatui view renderer module exports

mod layout;
mod modals;
mod prompt;

pub use layout::{compute_max_scroll, header_height, render_frame, transcript_viewport};
