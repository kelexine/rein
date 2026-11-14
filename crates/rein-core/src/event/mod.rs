// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Public exports for core event logging and error types

/// Core error definitions.
pub mod error;
/// Append-only JSONL event log implementation.
pub mod log;
/// Session event payload variants.
pub mod types;

pub use error::CoreError;
pub use log::EventLog;
pub use types::SessionEvent;
