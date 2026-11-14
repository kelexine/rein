// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Error types for policy evaluation, sandbox verification, and audit logging

use std::path::PathBuf;
use thiserror::Error;

/// Errors emitted by `rein-policy` operations.
#[derive(Debug, Error)]
pub enum PolicyError {
    /// Underlying filesystem or audit log I/O failure.
    #[error("policy I/O failed at '{path}': {source}")]
    Io {
        /// File or directory path involved in the failure.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },
    /// JSONL serialization or deserialization failure in the audit log.
    #[error("audit serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    /// Path escaped the configured workspace sandbox root.
    #[error("sandbox violation: path '{attempted}' escapes root '{root}'")]
    SandboxEscape {
        /// Canonical workspace root path.
        root: PathBuf,
        /// Attempted target path.
        attempted: PathBuf,
    },
    /// Kernel sandboxing (Landlock) is unavailable on the host OS/kernel.
    #[error("kernel sandbox unavailable: {0}")]
    SandboxUnavailable(String),
    /// Failed to apply Landlock or `PR_SET_NO_NEW_PRIVS` in the child process.
    #[error("failed to apply kernel sandbox: {0}")]
    SandboxApply(String),
}
