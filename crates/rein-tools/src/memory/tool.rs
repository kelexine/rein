// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Scoped MemoryTool allowing coding sessions to read personal memory behind policy

use rein_core::MemoryScope;
use rein_policy::{AccessMode, Capability};
use serde_json::{Value, json};

use crate::memory::{JsonlMemoryStore, MemoryRecord};
use crate::registry::{Tool, ToolError, ToolSchema};

/// Tool exposing scoped read/write access to [`JsonlMemoryStore`].
#[derive(Debug, Clone)]
pub struct MemoryTool {
    store: JsonlMemoryStore,
}

impl MemoryTool {
    /// Creates a [`MemoryTool`] backed by `store`.
    #[must_use]
    pub fn new(store: JsonlMemoryStore) -> Self {
        Self { store }
    }
}

impl Tool for MemoryTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "memory".to_owned(),
            description: "Read or write scoped JSONL memory (personal or project)".to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["action", "scope", "key"],
                "properties": {
                    "action": { "type": "string", "enum": ["read", "write"] },
                    "scope": { "type": "string", "enum": ["personal", "project"] },
                    "key": { "type": "string" },
                    "value": { "type": "string" }
                }
            }),
        }
    }

    fn required_capability(&self, arguments: &Value) -> Result<Capability, ToolError> {
        let scope = parse_scope(arguments)?;
        let mode = match arguments.get("action").and_then(Value::as_str) {
            Some("read") => AccessMode::Read,
            Some("write") => AccessMode::Write,
            _ => {
                return Err(ToolError::InvalidArguments {
                    tool: "memory".to_owned(),
                    reason: "expected 'action' to be 'read' or 'write'".to_owned(),
                });
            }
        };
        Ok(Capability::Memory { scope, mode })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let scope = parse_scope(arguments)?;
        let key = arguments
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments {
                tool: "memory".to_owned(),
                reason: "missing string field 'key'".to_owned(),
            })?;
        match arguments.get("action").and_then(Value::as_str) {
            Some("read") => {
                let records = self.store.query(scope, key)?;
                Ok(serde_json::to_string(&records)?)
            }
            Some("write") => {
                let value = arguments
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.store.append(&MemoryRecord {
                    scope,
                    key: key.to_owned(),
                    value: value.to_owned(),
                })?;
                Ok("memory record saved".to_owned())
            }
            _ => Err(ToolError::InvalidArguments {
                tool: "memory".to_owned(),
                reason: "invalid action".to_owned(),
            }),
        }
    }
}

fn parse_scope(arguments: &Value) -> Result<MemoryScope, ToolError> {
    match arguments.get("scope").and_then(Value::as_str) {
        Some("personal") => Ok(MemoryScope::Personal),
        Some("project") => Ok(MemoryScope::Project),
        _ => Err(ToolError::InvalidArguments {
            tool: "memory".to_owned(),
            reason: "expected 'scope' to be 'personal' or 'project'".to_owned(),
        }),
    }
}
