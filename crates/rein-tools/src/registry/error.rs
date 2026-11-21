// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Error types for tool registration, capability extraction, and execution

use std::path::PathBuf;

use rein_policy::PolicyError;
use thiserror::Error;

/// Errors returned by tool lookup, policy verification, or execution.
#[derive(Debug, Error)]
pub enum ToolError {
    /// Requested tool name is not registered in the active profile.
    #[error("unknown tool '{0}'")]
    NotFound(String),
    /// Invalid or missing JSON arguments passed to a tool.
    #[error("invalid arguments for tool '{tool}': {reason}")]
    InvalidArguments {
        /// Tool name.
        tool: String,
        /// Explanation of argument error.
        reason: String,
    },
    /// Underlying policy evaluation error.
    #[error("policy check failed: {0}")]
    Policy(#[from] PolicyError),
    /// Filesystem or network I/O error during tool execution.
    #[error("tool I/O failed at '{path}': {source}")]
    Io {
        /// Path associated with the I/O failure.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },
    /// JSONL serialization or parsing failure inside a tool backend.
    #[error("tool serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    /// Runtime execution failure inside a tool (e.g., HTTP/API/conversion error).
    #[error("tool '{tool}' failed: {reason}")]
    Execution {
        /// Tool name.
        tool: String,
        /// Human-readable failure reason.
        reason: String,
    },
}
