// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Terminal UI loop module exports

mod commands;
mod idle_keys;
mod session;
mod streaming_keys;

pub use session::run_tui;
