// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: JSON-RPC 2.0 wire types for Model Context Protocol (MCP) stdio communication

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Standard JSON-RPC 2.0 version string.
pub const JSONRPC_VERSION: &str = "2.0";

/// JSON-RPC 2.0 request envelope sent to an MCP server over stdio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// Protocol version (`"2.0"`).
    pub jsonrpc: String,
    /// Monotonic request identifier.
    pub id: u64,
    /// Method name (e.g. `"tools/list"`, `"tools/call"`).
    pub method: String,
    /// Optional structured parameters.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

impl JsonRpcRequest {
    /// Creates a new JSON-RPC 2.0 request envelope.
    #[must_use]
    pub fn new(id: u64, method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_owned(),
            id,
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC 2.0 response envelope returned by an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    /// Protocol version (`"2.0"`).
    pub jsonrpc: String,
    /// Correlated request identifier.
    pub id: u64,
    /// Successful result payload, if any.
    #[serde(default)]
    pub result: Option<Value>,
    /// Error payload, if any.
    #[serde(default)]
    pub error: Option<Value>,
}
