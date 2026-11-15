// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-15
// Purpose: Session/workspace Todo & Task management tools (todo_write and todo_read)

use std::fs;
use std::path::{Path, PathBuf};

use rein_core::MemoryScope;
use rein_policy::{AccessMode, Capability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::registry::{Tool, ToolError, ToolSchema};

/// Execution status of a tracked task item.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    /// Task has not started yet.
    #[default]
    Pending,
    /// Task is actively being worked on.
    InProgress,
    /// Task has finished.
    Completed,
}

/// Single task entry managed by [`TodoWriteTool`] and [`TodoReadTool`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    /// Stable short identifier (auto-numbered if omitted by callers).
    pub id: String,
    /// Clear, actionable task description.
    pub content: String,
    /// Current execution state (`pending`, `in_progress`, or `completed`).
    #[serde(default)]
    pub status: TodoStatus,
}

/// Persistent JSON todo list helper backed by `<workspace>/.rein/todos.json`.
#[derive(Debug, Clone)]
pub struct TodoStore {
    path: PathBuf,
}

impl TodoStore {
    /// Creates a [`TodoStore`] reading and writing `path`.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Returns the backing file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads all stored [`TodoItem`] entries (empty when the file does not exist yet).
    pub fn load(&self) -> Result<Vec<TodoItem>, ToolError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let raw = fs::read_to_string(&self.path).map_err(|source| ToolError::Io {
            path: self.path.clone(),
            source,
        })?;
        if raw.trim().is_empty() {
            return Ok(Vec::new());
        }
        Ok(serde_json::from_str(&raw)?)
    }

    /// Replaces stored todos atomically on disk.
    pub fn save(&self, items: &[TodoItem]) -> Result<(), ToolError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|source| ToolError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let encoded = serde_json::to_string_pretty(items)?;
        fs::write(&self.path, format!("{encoded}\n")).map_err(|source| ToolError::Io {
            path: self.path.clone(),
            source,
        })
    }
}

/// Tool replacing/updating the structured workspace todo checklist (`todo_write`).
#[derive(Debug, Clone)]
pub struct TodoWriteTool {
    store: TodoStore,
}

impl TodoWriteTool {
    /// Creates a [`TodoWriteTool`] backed by `store_path`.
    #[must_use]
    pub fn new(store_path: impl Into<PathBuf>) -> Self {
        Self {
            store: TodoStore::new(store_path),
        }
    }
}

impl Tool for TodoWriteTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "todo_write".to_owned(),
            description: "Create or update the structured task checklist for multi-step work"
                .to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "description": "Ordered list of todos reflecting the full current plan",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "content": { "type": "string" },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed"]
                                }
                            },
                            "required": ["content", "status"]
                        }
                    }
                },
                "required": ["todos"]
            }),
        }
    }

    fn required_capability(&self, _args: &Value) -> Result<Capability, ToolError> {
        Ok(Capability::Memory {
            scope: MemoryScope::Project,
            mode: AccessMode::Write,
        })
    }

    fn execute(&self, args: &Value) -> Result<String, ToolError> {
        let raw_items = args.get("todos").and_then(Value::as_array).ok_or_else(|| {
            ToolError::InvalidArguments {
                tool: "todo_write".to_owned(),
                reason: "missing array 'todos'".to_owned(),
            }
        })?;

        let mut parsed = Vec::with_capacity(raw_items.len());
        for (idx, item) in raw_items.iter().enumerate() {
            let content = item
                .get("content")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| ToolError::InvalidArguments {
                    tool: "todo_write".to_owned(),
                    reason: format!("todos[{idx}] missing non-empty 'content'"),
                })?;
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| (idx + 1).to_string());
            let status = match item
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending")
            {
                "pending" => TodoStatus::Pending,
                "in_progress" => TodoStatus::InProgress,
                "completed" | "done" => TodoStatus::Completed,
                other => {
                    return Err(ToolError::InvalidArguments {
                        tool: "todo_write".to_owned(),
                        reason: format!("todos[{idx}] has invalid status '{other}'"),
                    });
                }
            };
            parsed.push(TodoItem {
                id,
                content: content.to_owned(),
                status,
            });
        }

        self.store.save(&parsed)?;
        let done = parsed
            .iter()
            .filter(|t| t.status == TodoStatus::Completed)
            .count();
        let active = parsed
            .iter()
            .filter(|t| t.status == TodoStatus::InProgress)
            .count();
        let pending = parsed
            .iter()
            .filter(|t| t.status == TodoStatus::Pending)
            .count();
        let payload = json!({
            "todos": parsed,
            "summary": format!("{} tasks ({done} completed, {active} in progress, {pending} pending)", parsed.len()),
        });
        Ok(serde_json::to_string_pretty(&payload)?)
    }
}

/// Tool reading the current structured workspace todo checklist (`todo_read`).
#[derive(Debug, Clone)]
pub struct TodoReadTool {
    store: TodoStore,
}

impl TodoReadTool {
    /// Creates a [`TodoReadTool`] backed by `store_path`.
    #[must_use]
    pub fn new(store_path: impl Into<PathBuf>) -> Self {
        Self {
            store: TodoStore::new(store_path),
        }
    }
}

impl Tool for TodoReadTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "todo_read".to_owned(),
            description: "Read the current workspace task checklist".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    fn required_capability(&self, _args: &Value) -> Result<Capability, ToolError> {
        Ok(Capability::Memory {
            scope: MemoryScope::Project,
            mode: AccessMode::Read,
        })
    }

    fn execute(&self, _args: &Value) -> Result<String, ToolError> {
        let items = self.store.load()?;
        let payload = json!({ "todos": items });
        Ok(serde_json::to_string_pretty(&payload)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todo_write_and_read_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join("rein_tools_todo_roundtrip");
        let _ = fs::remove_dir_all(&temp_dir);
        let todo_path = temp_dir.join("todos.json");

        let writer = TodoWriteTool::new(&todo_path);
        let reader = TodoReadTool::new(&todo_path);

        let initial: Value = serde_json::from_str(&reader.execute(&json!({}))?)?;
        assert_eq!(initial["todos"], json!([]));

        let out_str = writer.execute(&json!({
            "todos": [
                { "content": "Inspect codebase", "status": "completed" },
                { "content": "Render markdown", "status": "in_progress" },
                { "id": "custom-3", "content": "Run tests", "status": "pending" }
            ]
        }))?;
        let out: Value = serde_json::from_str(&out_str)?;
        assert_eq!(out["todos"][0]["id"], json!("1"));
        assert_eq!(out["todos"][1]["status"], json!("in_progress"));
        assert_eq!(out["todos"][2]["id"], json!("custom-3"));

        let loaded: Value = serde_json::from_str(&reader.execute(&json!({}))?)?;
        assert_eq!(loaded["todos"].as_array().map(Vec::len), Some(3));
        Ok(())
    }
}
