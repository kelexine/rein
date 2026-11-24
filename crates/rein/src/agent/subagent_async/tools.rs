// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Model-callable async subagent tools (invoke, define, manage, send_message)

use rein_core::ModeId;
use rein_policy::Capability;
use rein_tools::{Tool, ToolError, ToolSchema};
use serde_json::{Value, json};

use crate::agent::AgentTool;
use crate::agent::workspace_iso::WorkspaceIsolation;

use super::manager::{InvokeSubagentRequest, SubagentDefinition, SubagentManager};

/// Model-callable tool spawning an async subagent (`invoke_subagent`).
#[derive(Clone)]
pub struct InvokeSubagentTool {
    manager: SubagentManager,
    parent_id: Option<String>,
    depth: u8,
}

impl InvokeSubagentTool {
    /// Creates the top-level [`InvokeSubagentTool`] for the orchestrator session.
    #[must_use]
    pub fn new(manager: SubagentManager) -> Self {
        Self {
            manager,
            parent_id: None,
            depth: 0,
        }
    }

    pub(super) fn nested(manager: SubagentManager, parent_id: String, depth: u8) -> Self {
        Self {
            manager,
            parent_id: Some(parent_id),
            depth,
        }
    }
}

impl Tool for InvokeSubagentTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "invoke_subagent".to_owned(),
            description: "Launch an asynchronous background subagent that sends its results back to your inbox upon completing its turn. Supports workspace modes: 'inherit' (default), 'share' (pure-Rust copy without .git), or 'branch' (isolated copy with .git).".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "type_name": { "type": "string", "description": "Built-in ('research', 'plan', 'coder', 'personal', 'general') or custom defined type name." },
                    "role": { "type": "string", "description": "Short 2-5 word role title." },
                    "prompt": { "type": "string", "description": "Self-contained task instructions for the subagent." },
                    "model": { "type": "string", "description": "Optional model override." },
                    "workspace": {
                        "type": "string",
                        "enum": ["inherit", "share", "branch"],
                        "description": "Workspace isolation mode (default: 'inherit')."
                    }
                },
                "required": ["type_name", "role", "prompt"]
            }),
        }
    }

    fn required_capability(&self, arguments: &Value) -> Result<Capability, ToolError> {
        let type_name = require_str(arguments, &["type_name", "type"], "invoke_subagent")?;
        let role = require_str(arguments, &["role"], "invoke_subagent")?;
        let _ = require_str(arguments, &["prompt"], "invoke_subagent")?;
        let parent_mode = self
            .manager
            .env
            .session_state
            .read()
            .map_or(ModeId::Coding, |s| s.active_mode);
        let custom = self
            .manager
            .inner
            .lock()
            .ok()
            .and_then(|g| g.definitions.get(type_name).cloned());
        let (agent_type, target_mode) = if let Some(def) = custom {
            let m = if def.enable_write_tools {
                ModeId::Coding
            } else {
                ModeId::Plan
            };
            (def.name, m)
        } else {
            let synthetic = json!({ "type": type_name, "role": role, "prompt": "ok" });
            let (t, _, m) = AgentTool::resolve_target(&synthetic, parent_mode)?;
            (t, m)
        };
        Ok(Capability::Subagent {
            agent_type,
            role: role.to_owned(),
            target_mode,
        })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let type_name = require_str(arguments, &["type_name", "type"], "invoke_subagent")?;
        let role = require_str(arguments, &["role"], "invoke_subagent")?;
        let prompt = require_str(arguments, &["prompt"], "invoke_subagent")?;
        let model = arguments.get("model").and_then(Value::as_str);
        let ws_mode =
            WorkspaceIsolation::parse(arguments.get("workspace").and_then(Value::as_str))?;

        let (conversation_id, ws_path) = self.manager.invoke(InvokeSubagentRequest {
            type_name,
            role,
            prompt,
            model_override: model,
            workspace_mode: ws_mode,
            parent_id: self.parent_id.as_deref(),
            depth: self.depth,
        })?;
        Ok(serde_json::to_string(&json!({
            "conversation_id": conversation_id,
            "status": "running",
            "workspace_mode": ws_mode.as_str(),
            "workspace_path": ws_path,
            "message": format!(
                "Subagent '{conversation_id}' is running in the background. Do not poll; you will be woken up automatically when it completes or sends a message."
            ),
        }))?)
    }
}

/// Registers a custom reusable subagent type for the current session (`define_subagent`).
#[derive(Clone)]
pub struct DefineSubagentTool {
    manager: SubagentManager,
}

impl DefineSubagentTool {
    /// Creates a [`DefineSubagentTool`] bound to `manager`.
    #[must_use]
    pub fn new(manager: SubagentManager) -> Self {
        Self { manager }
    }
}

impl Tool for DefineSubagentTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "define_subagent".to_owned(),
            description: "Define a reusable custom subagent type for invoke_subagent.".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "description": { "type": "string" },
                    "system_prompt": { "type": "string" },
                    "enable_write_tools": { "type": "boolean" },
                    "enable_mcp_tools": { "type": "boolean" },
                    "enable_subagent_tools": { "type": "boolean" }
                },
                "required": ["name", "description", "system_prompt"]
            }),
        }
    }

    fn required_capability(&self, arguments: &Value) -> Result<Capability, ToolError> {
        let name = require_str(arguments, &["name"], "define_subagent")?;
        let enable_writes = arguments
            .get("enable_write_tools")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let target_mode = if enable_writes {
            ModeId::Coding
        } else {
            ModeId::Plan
        };
        Ok(Capability::Subagent {
            agent_type: name.to_owned(),
            role: "define_subagent".to_owned(),
            target_mode,
        })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let name = require_str(arguments, &["name"], "define_subagent")?.to_owned();
        let description = require_str(arguments, &["description"], "define_subagent")?.to_owned();
        let system_prompt =
            require_str(arguments, &["system_prompt"], "define_subagent")?.to_owned();
        let def = SubagentDefinition {
            name: name.clone(),
            description,
            system_prompt,
            enable_write_tools: arguments
                .get("enable_write_tools")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            enable_mcp_tools: arguments
                .get("enable_mcp_tools")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            enable_subagent_tools: arguments
                .get("enable_subagent_tools")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        };
        self.manager
            .define(def)
            .map_err(|reason| ToolError::InvalidArguments {
                tool: "define_subagent".to_owned(),
                reason,
            })?;
        Ok(serde_json::to_string(&json!({ "defined": name }))?)
    }
}

/// Lists and cascades termination across active async subagents (`manage_subagents`).
#[derive(Clone)]
pub struct ManageSubagentsTool {
    manager: SubagentManager,
}

impl ManageSubagentsTool {
    /// Creates a [`ManageSubagentsTool`] bound to `manager`.
    #[must_use]
    pub fn new(manager: SubagentManager) -> Self {
        Self { manager }
    }
}

impl Tool for ManageSubagentsTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "manage_subagents".to_owned(),
            description: "List or terminate spawned async subagents and their descendant/isolated workspaces ('list', 'kill', 'kill_all').".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["list", "kill", "kill_all"] },
                    "conversation_ids": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                },
                "required": ["action"]
            }),
        }
    }

    fn required_capability(&self, arguments: &Value) -> Result<Capability, ToolError> {
        let action = require_str(arguments, &["action"], "manage_subagents")?;
        Ok(Capability::Task {
            action: action.to_owned(),
            mode: if action == "list" {
                rein_policy::AccessMode::Read
            } else {
                rein_policy::AccessMode::Write
            },
        })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let action = require_str(arguments, &["action"], "manage_subagents")?;
        match action {
            "list" => Ok(serde_json::to_string(&json!({
                "subagents": self.manager.list()
            }))?),
            "kill" => {
                let ids: Vec<&str> = arguments
                    .get("conversation_ids")
                    .and_then(Value::as_array)
                    .map(|arr| arr.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                if ids.is_empty() {
                    return Err(ToolError::InvalidArguments {
                        tool: "manage_subagents".to_owned(),
                        reason: "expected non-empty 'conversation_ids' for action 'kill'"
                            .to_owned(),
                    });
                }
                let mut killed = Vec::new();
                for id in ids {
                    let mut tree = self.manager.kill_tree(id).map_err(|reason| {
                        ToolError::InvalidArguments {
                            tool: "manage_subagents".to_owned(),
                            reason,
                        }
                    })?;
                    killed.append(&mut tree);
                }
                Ok(serde_json::to_string(&json!({ "killed": killed }))?)
            }
            "kill_all" => {
                let killed = self.manager.kill_all();
                Ok(serde_json::to_string(&json!({ "killed": killed }))?)
            }
            other => Err(ToolError::InvalidArguments {
                tool: "manage_subagents".to_owned(),
                reason: format!("unknown manage_subagents action '{other}'"),
            }),
        }
    }
}

/// Sends a message to a spawned subagent mailbox or back to `"parent"` (`send_message`).
#[derive(Clone)]
pub struct SendMessageTool {
    manager: SubagentManager,
    sender_label: String,
}

impl SendMessageTool {
    /// Creates a [`SendMessageTool`] for the orchestrator parent session.
    #[must_use]
    pub fn new(manager: SubagentManager) -> Self {
        Self {
            manager,
            sender_label: "parent".to_owned(),
        }
    }

    pub(super) fn with_sender(manager: SubagentManager, sender_label: String) -> Self {
        Self {
            manager,
            sender_label,
        }
    }
}

impl Tool for SendMessageTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "send_message".to_owned(),
            description: "Send a message to an active/waiting subagent by conversation_id (waking it for a follow-up turn), or to 'parent' from inside a subagent.".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "recipient": { "type": "string" },
                    "message": { "type": "string" }
                },
                "required": ["recipient", "message"]
            }),
        }
    }

    fn required_capability(&self, arguments: &Value) -> Result<Capability, ToolError> {
        let _ = require_str(arguments, &["recipient"], "send_message")?;
        let _ = require_str(arguments, &["message"], "send_message")?;
        Ok(Capability::Task {
            action: "send_message".to_owned(),
            mode: rein_policy::AccessMode::Write,
        })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let recipient = require_str(arguments, &["recipient"], "send_message")?;
        let message = require_str(arguments, &["message"], "send_message")?;
        let note = self
            .manager
            .send_message(&self.sender_label, recipient, message)
            .map_err(|reason| ToolError::InvalidArguments {
                tool: "send_message".to_owned(),
                reason,
            })?;
        Ok(serde_json::to_string(&json!({ "status": note }))?)
    }
}

fn require_str<'a>(arguments: &'a Value, keys: &[&str], tool: &str) -> Result<&'a str, ToolError> {
    for key in keys {
        if let Some(s) = arguments
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Ok(s);
        }
    }
    Err(ToolError::InvalidArguments {
        tool: tool.to_owned(),
        reason: format!("expected non-empty string '{}'", keys[0]),
    })
}
