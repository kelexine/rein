// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Public exports for append-only JSONL audit logging

/// Audit log entry and JSONL writer.
pub mod log;

pub use log::{AuditEntry, AuditLog};
