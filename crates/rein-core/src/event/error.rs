// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Error types for rein-core event log, replay, and provider operations

use std::path::PathBuf;
use thiserror::Error;

/// Errors produced by core event log, state transitions, or provider operations.
#[derive(Debug, Error)]
pub enum CoreError {
    /// Underlying filesystem I/O failure with path context.
    #[error("event log I/O failed at '{path}': {source}")]
    Io {
        /// File path where I/O failed.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },
    /// JSONL serialization or deserialization failure.
    #[error("event serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    /// Replay stream exhausted before expected state transition completed.
    #[error("replay event stream exhausted at sequence {seq}")]
    ReplayExhausted {
        /// Sequence index where the stream ended.
        seq: u64,
    },
    /// Provider communication or configuration error.
    #[error("provider error: {0}")]
    Provider(String),
    /// Turn streaming or execution cancelled by the user (`Esc`).
    #[error("turn cancelled by user")]
    Cancelled,
}
