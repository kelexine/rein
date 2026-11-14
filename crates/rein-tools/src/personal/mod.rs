// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Public exports for native Rust personal mode tools (notes, calendar, email)

/// Native calendar event tool.
pub mod calendar;
/// Native email inbox/draft/send tool.
pub mod email;
/// Native JSONL notes tool.
pub mod notes;

pub use calendar::{CalendarEvent, CalendarTool};
pub use email::{EmailMessage, EmailStatus, EmailTool};
pub use notes::{NoteEntry, NotesTool};
