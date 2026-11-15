// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Public exports for JSONL-backed scoped memory store and tool

/// Append-only JSONL memory store.
pub mod store;
/// Policy-gated memory tool implementation.
pub mod tool;

pub use store::{JsonlMemoryStore, MemoryKind, MemoryRecord, now_unix_secs};
pub use tool::MemoryTool;
