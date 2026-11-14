// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Public exports for sandboxed coding mode tools (read, write, edit, search, and shell)

/// Exact string-replacement file editing tool.
pub mod edit;
/// Line-bounded file reading tool.
pub mod read;
/// Workspace glob file discovery and line-level grep search tools.
pub mod search;
/// Classifier-driven, Landlock-sandboxed shell execution tool.
pub mod shell;
/// Protected-path-checked file writing tool.
pub mod write;

pub use edit::EditTool;
pub use read::{DEFAULT_MAX_READ_LINES, ReadTool};
pub use search::{GlobTool, GrepTool, MAX_SEARCH_RESULTS};
pub use shell::ShellTool;
pub use write::WriteTool;
