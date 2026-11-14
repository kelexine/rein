// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Structured tool call request and execution output payloads

use serde::{Deserialize, Serialize};

/// A structured request from the model to invoke a registered tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique identifier for correlating tool output to the call.
    pub id: String,
    /// Registered tool name.
    pub name: String,
    /// Structured JSON arguments for the tool.
    pub arguments: serde_json::Value,
}

/// Result produced by executing a [`ToolCall`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOutput {
    /// Identifier of the originating [`ToolCall`].
    pub call_id: String,
    /// Serialized textual or JSON payload returned to the model.
    pub content: String,
    /// Whether the tool invocation failed.
    pub is_error: bool,
}
