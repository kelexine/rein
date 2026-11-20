// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Public exports for sandboxed coding mode tools (read, write, edit, search, and bash)

/// Classifier-driven, policy-gated bash execution tool.
pub mod bash;
/// Exact string-replacement file editing tool.
pub mod edit;
/// Line-bounded file reading tool.
pub mod read;
/// Workspace glob file discovery and line-level grep search tools.
pub mod search;
/// Protected-path-checked file writing tool.
pub mod write;

pub use bash::BashTool;
pub use edit::EditTool;
pub use read::{DEFAULT_MAX_READ_LINES, ReadTool};
pub use search::{GlobTool, GrepTool, MAX_SEARCH_RESULTS};
pub use write::WriteTool;
