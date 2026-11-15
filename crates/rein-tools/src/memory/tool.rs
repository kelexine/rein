// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Scoped MemoryTool supporting read, upsert write, delete, and list behind policy

use rein_core::MemoryScope;
use rein_policy::{AccessMode, Capability};
use serde_json::{Value, json};

use crate::memory::{JsonlMemoryStore, MemoryKind, MemoryRecord};
use crate::registry::{Tool, ToolError, ToolSchema};

/// Tool exposing scoped read/write/delete/list access to [`JsonlMemoryStore`].
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
            description: "Read, list, upsert, or delete persistent scoped memory ('personal' across workspaces or 'project' for this repo). Store durable facts/conventions rather than transient checklists.".to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["action", "scope"],
                "properties": {
                    "action": { "type": "string", "enum": ["read", "write", "delete", "list"] },
                    "scope": { "type": "string", "enum": ["personal", "project"] },
                    "key": { "type": "string", "description": "Topic key or search query (required for write/delete/read)" },
                    "value": { "type": "string", "description": "Memory content when action='write'" },
                    "kind": { "type": "string", "enum": ["general", "user", "feedback", "project", "reference"] }
                }
            }),
        }
    }

    fn required_capability(&self, arguments: &Value) -> Result<Capability, ToolError> {
        let scope = parse_scope(arguments)?;
        let mode = match arguments.get("action").and_then(Value::as_str) {
            Some("read" | "list") => AccessMode::Read,
            Some("write" | "delete") => AccessMode::Write,
            _ => {
                return Err(ToolError::InvalidArguments {
                    tool: "memory".to_owned(),
                    reason: "expected 'action' to be 'read', 'write', 'delete', or 'list'"
                        .to_owned(),
                });
            }
        };
        Ok(Capability::Memory { scope, mode })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let scope = parse_scope(arguments)?;
        match arguments.get("action").and_then(Value::as_str) {
            Some("list") => Ok(serde_json::to_string(&self.store.query(scope, "")?)?),
            Some("read") => {
                let key = arguments.get("key").and_then(Value::as_str).unwrap_or("");
                Ok(serde_json::to_string(&self.store.query(scope, key)?)?)
            }
            Some("write") => {
                let key = require_key(arguments)?;
                let value = arguments
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let kind = arguments
                    .get("kind")
                    .and_then(Value::as_str)
                    .map_or(MemoryKind::General, MemoryKind::parse_lossy);
                self.store
                    .upsert(&MemoryRecord::new(scope, key, value).with_kind(kind))?;
                Ok("memory record saved".to_owned())
            }
            Some("delete") => {
                let key = require_key(arguments)?;
                let removed = self.store.delete(scope, key)?;
                Ok(if removed {
                    "memory record deleted".to_owned()
                } else {
                    "memory record not found".to_owned()
                })
            }
            _ => Err(ToolError::InvalidArguments {
                tool: "memory".to_owned(),
                reason: "invalid action".to_owned(),
            }),
        }
    }
}

fn require_key(arguments: &Value) -> Result<&str, ToolError> {
    arguments
        .get("key")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ToolError::InvalidArguments {
            tool: "memory".to_owned(),
            reason: "missing non-empty string field 'key'".to_owned(),
        })
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
