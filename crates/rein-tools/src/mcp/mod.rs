// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Public exports for MCP client, JSON-RPC 2.0 protocol types, and policy-gated adapter

/// Stdio MCP client and tool adapter implementation.
pub mod client;
/// Lazy MCP schema disk catalog and `call_mcp_tool` dispatcher.
pub mod lazy;
/// JSON-RPC 2.0 request and response envelopes for MCP servers.
pub mod protocol;

pub use client::{MCP_EXCHANGE_TIMEOUT_SECS, McpClient, McpServerConfig, McpToolAdapter};
pub use lazy::{CallMcpTool, McpCatalog, McpServerSummary};
pub use protocol::{JsonRpcRequest, JsonRpcResponse};
