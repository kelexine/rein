// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-19
// Purpose: Model-facing `manage_task` tool for listing, inspecting, sending stdin to, and killing background tasks

use rein_policy::{AccessMode, Capability};
use serde_json::{Value, json};

use crate::registry::{Tool, ToolError, ToolSchema};
use crate::tasks::TaskRegistry;

/// Inspects, interacts with (`send_input`), and kills background tasks in a [`TaskRegistry`].
#[derive(Debug, Clone)]
pub struct ManageTaskTool {
    registry: TaskRegistry,
}

impl ManageTaskTool {
    /// Creates a [`ManageTaskTool`] bound to the active session's `registry`.
    #[must_use]
    pub fn new(registry: TaskRegistry) -> Self {
        Self { registry }
    }
}

impl Tool for ManageTaskTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "manage_task".to_owned(),
            description: "Inspect or control background tasks spawned by bash or schedule. Actions: 'list', 'status' (tail output), 'send_input' (write stdin to a running shell task), 'kill' (stop task/schedule).".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "status", "send_input", "kill"]
                    },
                    "task_id": {
                        "type": "string",
                        "description": "Target task ID for 'status', 'send_input', and 'kill'."
                    },
                    "input": {
                        "type": "string",
                        "description": "Stdin text written to the running shell process for 'send_input'."
                    },
                    "tail_lines": {
                        "type": "integer",
                        "description": "Number of recent output lines to return for 'status' (default 40)."
                    }
                },
                "required": ["action"]
            }),
        }
    }

    fn required_capability(&self, arguments: &Value) -> Result<Capability, ToolError> {
        let action = require_action(arguments)?;
        let mode = match action {
            "list" | "status" => AccessMode::Read,
            _ => AccessMode::Write,
        };
        Ok(Capability::Task {
            action: action.to_owned(),
            mode,
        })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let action = require_action(arguments)?;
        match action {
            "list" => {
                let tasks = self.registry.list_tasks();
                Ok(serde_json::to_string(&json!({ "tasks": tasks }))?)
            }
            "status" => {
                let task_id = require_task_id(arguments)?;
                let tail_lines = arguments
                    .get("tail_lines")
                    .and_then(Value::as_u64)
                    .map_or(40, |v| usize::try_from(v).unwrap_or(40).clamp(1, 500));
                let summary = self
                    .registry
                    .task_status(task_id, tail_lines)
                    .map_err(|reason| ToolError::InvalidArguments {
                        tool: "manage_task".to_owned(),
                        reason,
                    })?;
                Ok(serde_json::to_string(&summary)?)
            }
            "send_input" => {
                let task_id = require_task_id(arguments)?;
                let input = arguments
                    .get("input")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError::InvalidArguments {
                        tool: "manage_task".to_owned(),
                        reason: "'input' string is required for action 'send_input'".to_owned(),
                    })?;
                self.registry.send_input(task_id, input).map_err(|reason| {
                    ToolError::InvalidArguments {
                        tool: "manage_task".to_owned(),
                        reason,
                    }
                })?;
                Ok(serde_json::to_string(&json!({
                    "sent": true,
                    "task_id": task_id,
                    "bytes": input.len()
                }))?)
            }
            "kill" => {
                let task_id = require_task_id(arguments)?;
                let summary = self.registry.kill_task(task_id).map_err(|reason| {
                    ToolError::InvalidArguments {
                        tool: "manage_task".to_owned(),
                        reason,
                    }
                })?;
                Ok(serde_json::to_string(&json!({
                    "killed": true,
                    "task": summary
                }))?)
            }
            other => Err(ToolError::InvalidArguments {
                tool: "manage_task".to_owned(),
                reason: format!("unsupported manage_task action '{other}'"),
            }),
        }
    }
}

fn require_action(arguments: &Value) -> Result<&str, ToolError> {
    let action = arguments
        .get("action")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ToolError::InvalidArguments {
            tool: "manage_task".to_owned(),
            reason: "expected string 'action' ('list', 'status', 'send_input', or 'kill')"
                .to_owned(),
        })?;
    if !matches!(action, "list" | "status" | "send_input" | "kill") {
        return Err(ToolError::InvalidArguments {
            tool: "manage_task".to_owned(),
            reason: format!("unknown action '{action}'"),
        });
    }
    Ok(action)
}

fn require_task_id(arguments: &Value) -> Result<&str, ToolError> {
    arguments
        .get("task_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ToolError::InvalidArguments {
            tool: "manage_task".to_owned(),
            reason: "expected non-empty string 'task_id'".to_owned(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::TimerCondition;

    #[test]
    fn list_status_kill_and_send_input_rejection_work_via_tool()
    -> Result<(), Box<dyn std::error::Error>> {
        let reg = TaskRegistry::new();
        let sched = reg.schedule_timer(60, "ping".to_owned(), TimerCondition::Never, None)?;
        let tool = ManageTaskTool::new(reg.clone());

        let list_raw = tool.execute(&json!({ "action": "list" }))?;
        assert!(list_raw.contains(&sched.id));

        let status_raw = tool.execute(&json!({ "action": "status", "task_id": sched.id }))?;
        assert!(status_raw.contains("running"));

        // Non-shell task rejects send_input cleanly.
        assert!(
            tool.execute(&json!({
                "action": "send_input",
                "task_id": sched.id,
                "input": "hello\n"
            }))
            .is_err()
        );

        let kill_raw = tool.execute(&json!({ "action": "kill", "task_id": sched.id }))?;
        assert!(kill_raw.contains("\"killed\":true"));
        reg.delete_done_task(&sched.id)?;
        Ok(())
    }
}
