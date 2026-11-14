// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Public exports for tool registry, coding tools, JSONL memory, personal tools, and MCP client

//! Policy-enforced tool registry, coding tools, JSONL memory, native personal tools, and MCP client for Rein.
#![forbid(unsafe_code)]

/// Sandboxed coding tools (read, write, edit, glob, grep, shell).
pub mod coding;
/// Model Context Protocol (MCP) client and policy-gated tool adapter.
pub mod mcp;
/// Scoped JSONL memory store and tool.
pub mod memory;
/// Native Rust personal tools (notes, calendar, email).
pub mod personal;
/// Tool trait, schema, and policy-gated registry.
pub mod registry;

pub use coding::{EditTool, GlobTool, GrepTool, ReadTool, ShellTool, WriteTool};
pub use mcp::{JsonRpcRequest, JsonRpcResponse, McpClient, McpServerConfig, McpToolAdapter};
pub use memory::{JsonlMemoryStore, MemoryRecord, MemoryTool};
pub use personal::{
    CalendarEvent, CalendarTool, EmailMessage, EmailStatus, EmailTool, NoteEntry, NotesTool,
};
pub use registry::{Tool, ToolError, ToolRegistry, ToolSchema};
