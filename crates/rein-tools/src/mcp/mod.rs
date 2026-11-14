// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Public exports for MCP client, JSON-RPC 2.0 protocol types, and policy-gated adapter

/// Stdio MCP client and tool adapter implementation.
pub mod client;
/// JSON-RPC 2.0 request and response envelopes for MCP servers.
pub mod protocol;

pub use client::{McpClient, McpServerConfig, McpToolAdapter};
pub use protocol::{JsonRpcRequest, JsonRpcResponse};
