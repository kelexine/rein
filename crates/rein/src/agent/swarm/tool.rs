// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Model-callable SwarmTool and SwarmPostTool adapters

use rein_core::ModeId;
use rein_policy::{AccessMode, Capability};
use rein_tools::{Tool, ToolError, ToolSchema};
use serde_json::{Value, json};

use crate::agent::WorkspaceIsolation;

use super::board::{SwarmAddRequest, SwarmManager};

/// Shared message-board & roster management tool (`swarm`).
#[derive(Clone)]
pub struct SwarmTool {
    manager: SwarmManager,
    caller: String,
}

impl SwarmTool {
    /// Creates the coordinator-privileged [`SwarmTool`].
    #[must_use]
    pub fn new(manager: SwarmManager) -> Self {
        Self {
            manager,
            caller: "coordinator".to_owned(),
        }
    }

    pub(super) fn for_worker(manager: SwarmManager, worker_name: String) -> Self {
        Self {
            manager,
            caller: worker_name,
        }
    }
}

impl Tool for SwarmTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "swarm".to_owned(),
            description: "Manage a multi-agent swarm collaborating via a shared message board. Actions: 'status', 'models', 'add' (with optional n, tasks, fork, model, workspace='inherit'|'share'|'branch'), 'model' (hot-swap waiting worker), 'kill', 'title', 'post' (post & @mention workers).".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "models", "add", "model", "kill", "title", "post"]
                    },
                    "name": { "type": "string" },
                    "brief": { "type": "string" },
                    "model": { "type": "string" },
                    "fork": { "type": "string" },
                    "workspace": { "type": "string", "enum": ["inherit", "share", "branch"] },
                    "n": { "type": "integer" },
                    "tasks": { "type": "array", "items": { "type": "string" } },
                    "all": { "type": "boolean" },
                    "title": { "type": "string" },
                    "message": { "type": "string" },
                    "why": { "type": "string" }
                },
                "required": ["action"]
            }),
        }
    }

    fn required_capability(&self, arguments: &Value) -> Result<Capability, ToolError> {
        let action = require_action(arguments)?;
        match action {
            "status" | "models" => Ok(Capability::Task {
                action: format!("swarm:{action}"),
                mode: AccessMode::Read,
            }),
            "add" => Ok(Capability::Subagent {
                agent_type: "swarm".to_owned(),
                role: arguments
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("worker")
                    .to_owned(),
                target_mode: ModeId::Coding,
            }),
            _ => Ok(Capability::Task {
                action: format!("swarm:{action}"),
                mode: AccessMode::Write,
            }),
        }
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let action = require_action(arguments)?;
        match action {
            "status" => Ok(serde_json::to_string(&self.manager.status_json())?),
            "models" => {
                let all = arguments
                    .get("all")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                Ok(serde_json::to_string(&self.manager.models_json(all))?)
            }
            "title" => {
                if self.caller != "coordinator" {
                    return Err(ToolError::InvalidArguments {
                        tool: "swarm".to_owned(),
                        reason: "only the coordinator may set swarm title".to_owned(),
                    });
                }
                let raw = arguments
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let set =
                    self.manager
                        .set_title(raw)
                        .map_err(|reason| ToolError::InvalidArguments {
                            tool: "swarm".to_owned(),
                            reason,
                        })?;
                Ok(serde_json::to_string(&json!({ "title": set }))?)
            }
            "add" => {
                let name = arguments
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| ToolError::InvalidArguments {
                        tool: "swarm".to_owned(),
                        reason: "expected non-empty string 'name' for swarm add".to_owned(),
                    })?;
                let brief = arguments
                    .get("brief")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| ToolError::InvalidArguments {
                        tool: "swarm".to_owned(),
                        reason: "expected non-empty string 'brief' for swarm add".to_owned(),
                    })?;
                let ws_mode =
                    WorkspaceIsolation::parse(arguments.get("workspace").and_then(Value::as_str))?;
                let n = arguments
                    .get("n")
                    .and_then(Value::as_u64)
                    .and_then(|v| usize::try_from(v).ok());
                let tasks: Vec<String> = arguments
                    .get("tasks")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                let res = self.manager.add_workers(
                    &self.caller,
                    SwarmAddRequest {
                        name,
                        brief,
                        model: arguments.get("model").and_then(Value::as_str),
                        fork: arguments.get("fork").and_then(Value::as_str),
                        workspace: ws_mode,
                        n,
                        tasks,
                    },
                )?;
                Ok(serde_json::to_string(&res)?)
            }
            "model" => {
                let target = arguments
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(&self.caller);
                let model = arguments
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| ToolError::InvalidArguments {
                        tool: "swarm".to_owned(),
                        reason: "expected non-empty string 'model'".to_owned(),
                    })?;
                let why = arguments
                    .get("why")
                    .and_then(Value::as_str)
                    .unwrap_or("requested model switch");
                let res = self
                    .manager
                    .switch_worker_model(&self.caller, target, model, why)
                    .map_err(|reason| ToolError::InvalidArguments {
                        tool: "swarm".to_owned(),
                        reason,
                    })?;
                Ok(serde_json::to_string(&res)?)
            }
            "kill" => {
                let name = arguments
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| ToolError::InvalidArguments {
                        tool: "swarm".to_owned(),
                        reason: "expected string 'name' to kill".to_owned(),
                    })?;
                let res = self
                    .manager
                    .kill_worker(&self.caller, name)
                    .map_err(|reason| ToolError::InvalidArguments {
                        tool: "swarm".to_owned(),
                        reason,
                    })?;
                Ok(serde_json::to_string(&res)?)
            }
            "post" => {
                let msg = arguments
                    .get("message")
                    .or_else(|| arguments.get("brief"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| ToolError::InvalidArguments {
                        tool: "swarm".to_owned(),
                        reason: "expected non-empty string 'message' for swarm post".to_owned(),
                    })?;
                let post =
                    self.manager
                        .post_board_message(&self.caller, msg)
                        .map_err(|reason| ToolError::InvalidArguments {
                            tool: "swarm".to_owned(),
                            reason,
                        })?;
                Ok(serde_json::to_string(&post)?)
            }
            other => Err(ToolError::InvalidArguments {
                tool: "swarm".to_owned(),
                reason: format!("unknown swarm action '{other}'"),
            }),
        }
    }
}

/// Worker-facing tool to post updates on the shared swarm board and `@mention` peers/coordinator.
#[derive(Clone)]
pub(super) struct SwarmPostTool {
    manager: SwarmManager,
    author: String,
}

impl SwarmPostTool {
    pub(super) fn new(manager: SwarmManager, author: String) -> Self {
        Self { manager, author }
    }
}

impl Tool for SwarmPostTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "swarm_post".to_owned(),
            description: "Post a message to the shared swarm board; @mention '@coordinator' or '@worker' to wake them.".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                },
                "required": ["message"]
            }),
        }
    }

    fn required_capability(&self, _arguments: &Value) -> Result<Capability, ToolError> {
        Ok(Capability::Task {
            action: "swarm:post".to_owned(),
            mode: AccessMode::Write,
        })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let msg = arguments
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ToolError::InvalidArguments {
                tool: "swarm_post".to_owned(),
                reason: "expected non-empty string 'message'".to_owned(),
            })?;
        let post = self
            .manager
            .post_board_message(&self.author, msg)
            .map_err(|reason| ToolError::InvalidArguments {
                tool: "swarm_post".to_owned(),
                reason,
            })?;
        Ok(serde_json::to_string(&post)?)
    }
}

fn require_action(arguments: &Value) -> Result<&str, ToolError> {
    arguments
        .get("action")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ToolError::InvalidArguments {
            tool: "swarm".to_owned(),
            reason: "expected string 'action'".to_owned(),
        })
}
