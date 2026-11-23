// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-19
// Purpose: Native Rust workspace glob file discovery and regex/context grep search tools

/// Workspace glob file discovery tool with brace, character-class, exclusion, and mtime sorting support.
pub mod glob;
/// Advanced workspace grep tool with NFA regex, file filters, context lines, and output modes.
pub mod grep;
/// Pure-Rust linear-time Thompson NFA regex compiler and matcher.
pub mod regex;
/// `.gitignore`-aware workspace file walker.
pub mod walk;

pub use glob::GlobTool;
pub use grep::GrepTool;

mod paginate;

pub use paginate::{MAX_CALLER_LIMIT, MAX_SEARCH_RESULTS};
pub(crate) use paginate::{format_paginated_lines, parse_limit_and_offset};
