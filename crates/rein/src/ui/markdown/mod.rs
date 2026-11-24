// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Terminal Markdown renderer module exports

mod blocks;
mod inline;

pub use blocks::{render_assistant_markdown, render_markdown_blocks};
pub use inline::parse_inline_spans;
