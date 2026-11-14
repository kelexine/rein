// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Tool trait and JSON Schema metadata per tool

use rein_policy::Capability;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::registry::ToolError;

/// Declarative JSON Schema metadata exposed to the LLM provider for a single tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSchema {
    /// Unique tool identifier.
    pub name: String,
    /// Human-readable description of what the tool does.
    pub description: String,
    /// JSON Schema object describing input parameters.
    pub input_schema: Value,
}

/// Trait implemented by all coding, personal, and MCP tools.
pub trait Tool: Send + Sync {
    /// Returns the tool schema advertised to the model.
    #[must_use]
    fn schema(&self) -> ToolSchema;

    /// Computes the exact [`Capability`] required for the given invocation arguments.
    fn required_capability(&self, arguments: &Value) -> Result<Capability, ToolError>;

    /// Executes the tool after the policy layer has approved the required capability.
    fn execute(&self, arguments: &Value) -> Result<String, ToolError>;
}
