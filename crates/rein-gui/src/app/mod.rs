// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: App module exports for rein-gui

/// Elm message definitions.
pub mod message;
/// GUI application state and transcript item models.
pub mod state;
/// Elm update handler and worker event synchronization.
pub mod update;

pub use message::Message;
pub use state::{GuiApp, ToolItemState, TranscriptItem};
pub use update::update;
