// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Policy-enforced ToolRegistry managing registered tools, profile allowlists, and schema export

use std::collections::BTreeMap;

use rein_core::{AwaitingPolicy, Executing, Idle, ToolOutput, Turn};
use rein_policy::{PolicyDecision, PolicyEngine};

use crate::registry::{Tool, ToolError, ToolSchema};

/// Registry of tools holding per-tool schemas and enforcing policy evaluation prior to dispatch.
#[derive(Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    /// Creates an empty [`ToolRegistry`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a [`Tool`] implementation keyed by its schema name.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.schema().name;
        self.tools.insert(name, tool);
    }

    /// Returns schemas for the subset of tool names enabled in the active profile.
    #[must_use]
    pub fn schemas_for(&self, allowed_names: &[String]) -> Vec<ToolSchema> {
        allowed_names
            .iter()
            .filter_map(|name| self.tools.get(name).map(|t| t.schema()))
            .collect()
    }

    /// Evaluates policy for a turn in [`AwaitingPolicy`] state across all registered tools.
    pub fn evaluate_turn(
        &self,
        turn: &Turn<'_, AwaitingPolicy>,
        policy: &PolicyEngine,
    ) -> Result<PolicyDecision, ToolError> {
        let call = turn.pending_call();
        let tool = self
            .tools
            .get(&call.name)
            .ok_or_else(|| ToolError::NotFound(call.name.clone()))?;
        let capability = tool.required_capability(&call.arguments)?;
        Ok(policy.evaluate(&call.name, &capability)?)
    }

    /// Evaluates policy while strictly enforcing the active profile's `allowed_tools` list.
    pub fn evaluate_turn_for_profile(
        &self,
        turn: &Turn<'_, AwaitingPolicy>,
        policy: &PolicyEngine,
        allowed_tools: &[String],
    ) -> Result<PolicyDecision, ToolError> {
        let call = turn.pending_call();
        if !allowed_tools.iter().any(|allowed| allowed == &call.name) {
            return Ok(PolicyDecision::Deny {
                reason: format!("tool '{}' is not enabled in the active profile", call.name),
            });
        }
        self.evaluate_turn(turn, policy)
    }

    /// Executes a policy-approved [`Turn<Executing>`] and transitions the turn back to [`Idle`].
    pub fn execute_approved<'a>(
        &self,
        turn: Turn<'a, Executing>,
    ) -> Result<(Turn<'a, Idle>, ToolOutput), ToolError> {
        let call = turn.approved_call().clone();
        let tool = self
            .tools
            .get(&call.name)
            .ok_or_else(|| ToolError::NotFound(call.name.clone()))?;
        let output = match tool.execute(&call.arguments) {
            Ok(content) => ToolOutput {
                call_id: call.id,
                content,
                is_error: false,
            },
            Err(err) => ToolOutput {
                call_id: call.id,
                content: err.to_string(),
                is_error: true,
            },
        };
        let idle = turn
            .complete(output.clone())
            .map_err(|err| ToolError::InvalidArguments {
                tool: call.name,
                reason: err.to_string(),
            })?;
        Ok((idle, output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding::{EditTool, ReadTool, WriteTool};
    use crate::memory::{JsonlMemoryStore, MemoryRecord, MemoryTool};
    use crate::personal::NotesTool;
    use rein_core::{EventLog, MemoryScope, ToolCall, Turn};
    use rein_policy::{AuditLog, PolicyDecision, PolicyEngine, PolicyRuleSet, WorkspaceSandbox};

    #[test]
    fn coding_mode_reads_personal_memory_and_blocks_personal_tools()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join("rein_tools_profile_gate_test");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir)?;

        let store = JsonlMemoryStore::new(temp_dir.join("mem.jsonl"));
        store.append(&MemoryRecord {
            scope: MemoryScope::Personal,
            key: "editor".to_owned(),
            value: "helix".to_owned(),
        })?;

        let sandbox = WorkspaceSandbox::new(&temp_dir);
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ReadTool::new(sandbox.clone())));
        registry.register(Box::new(WriteTool::new(sandbox.clone())));
        registry.register(Box::new(EditTool::new(sandbox.clone())));
        registry.register(Box::new(MemoryTool::new(store)));
        registry.register(Box::new(NotesTool::new(temp_dir.join("notes.jsonl"))));

        let policy = PolicyEngine::new(
            PolicyRuleSet::coding_default(),
            sandbox,
            AuditLog::new(temp_dir.join("audit.jsonl")),
        );
        let log = EventLog::new(temp_dir.join("events.jsonl"));
        let coding_allowed = vec![
            "read".to_owned(),
            "write".to_owned(),
            "edit".to_owned(),
            "memory".to_owned(),
        ];

        let awaiting_mem = Turn::new(&log)
            .begin_user_turn("read personal preference")?
            .request_tool(ToolCall {
                id: "1".to_owned(),
                name: "memory".to_owned(),
                arguments: serde_json::json!({"action": "read", "scope": "personal", "key": "editor"}),
            })?;
        assert!(matches!(
            registry.evaluate_turn_for_profile(&awaiting_mem, &policy, &coding_allowed)?,
            PolicyDecision::Allow { .. }
        ));
        let (_idle, out) = registry.execute_approved(awaiting_mem.approve("ok")?)?;
        assert!(out.content.contains("helix"));

        let awaiting_notes = Turn::new(&log)
            .begin_user_turn("try calling notes from coding profile")?
            .request_tool(ToolCall {
                id: "2".to_owned(),
                name: "notes".to_owned(),
                arguments: serde_json::json!({"action": "list"}),
            })?;
        assert!(matches!(
            registry.evaluate_turn_for_profile(&awaiting_notes, &policy, &coding_allowed)?,
            PolicyDecision::Deny { .. }
        ));

        let _ = std::fs::remove_dir_all(&temp_dir);
        Ok(())
    }
}
