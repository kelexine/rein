// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Per-profile context strategy that assembles system prompt, scoped memory, history, and tools

use rein_core::{MemoryScope, Message, Role};
use rein_tools::{JsonlMemoryStore, ToolError, ToolRegistry, ToolSchema};
use serde::{Deserialize, Serialize};

use crate::modes::Profile;

/// Marker appended when a tool output is truncated to fit the context budget.
pub const TRUNCATION_MARKER: &str = "\n…[truncated by rein context strategy]";

/// Context window strategy bundled inside a [`Profile`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextStrategy {
    /// Maximum number of most-recent transcript messages kept per turn.
    pub max_history_messages: usize,
    /// Maximum characters kept from any single tool output message.
    pub max_tool_output_chars: usize,
    /// Maximum memory records injected per readable scope.
    pub max_memory_records: usize,
    /// Whether scoped memory is injected into the system prompt.
    pub inject_scoped_memory: bool,
}

impl ContextStrategy {
    /// Coding strategy: long history, large tool outputs (file reads, grep), compact memory.
    #[must_use]
    pub fn coding() -> Self {
        Self {
            max_history_messages: 60,
            max_tool_output_chars: 24_000,
            max_memory_records: 20,
            inject_scoped_memory: true,
        }
    }

    /// Plan strategy: long history and large read-only inspection outputs matching Coding mode.
    #[must_use]
    pub fn plan() -> Self {
        Self::coding()
    }

    /// Personal strategy: shorter history, small tool outputs, richer memory recall.
    #[must_use]
    pub fn personal() -> Self {
        Self {
            max_history_messages: 30,
            max_tool_output_chars: 6_000,
            max_memory_records: 50,
            inject_scoped_memory: true,
        }
    }
}

/// Fully assembled, provider-ready context for a single turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledContext {
    /// System prompt including injected scoped memory.
    pub system_prompt: String,
    /// Windowed and truncated transcript messages.
    pub messages: Vec<Message>,
    /// Tool schemas enabled by the active profile only.
    pub tools: Vec<ToolSchema>,
}

/// Builds an [`AssembledContext`] from a [`Profile`], memory store, and tool registry.
#[derive(Debug)]
pub struct ContextBuilder<'a> {
    profile: &'a Profile,
    memory: &'a JsonlMemoryStore,
}

impl<'a> ContextBuilder<'a> {
    /// Creates a [`ContextBuilder`] for `profile` reading memory from `memory`.
    #[must_use]
    pub fn new(profile: &'a Profile, memory: &'a JsonlMemoryStore) -> Self {
        Self { profile, memory }
    }

    /// Assembles the system prompt, windowed history, and profile-scoped tool schemas.
    pub fn build(
        &self,
        history: &[Message],
        registry: &ToolRegistry,
    ) -> Result<AssembledContext, ToolError> {
        let strategy = &self.profile.context_strategy;
        let system_prompt = if strategy.inject_scoped_memory {
            self.system_prompt_with_memory()?
        } else {
            self.profile.system_prompt.clone()
        };
        Ok(AssembledContext {
            system_prompt,
            messages: window_history(history, strategy),
            tools: registry.schemas_for(&self.profile.allowed_tools),
        })
    }

    fn system_prompt_with_memory(&self) -> Result<String, ToolError> {
        let mut prompt = self.profile.system_prompt.clone();
        for scope in &self.profile.readable_memory_scopes {
            let records = self.memory.query(*scope, "")?;
            if records.is_empty() {
                continue;
            }
            prompt.push_str(&format!("\n\n## {} memory\n", scope_label(*scope)));
            let skip = records
                .len()
                .saturating_sub(self.profile.context_strategy.max_memory_records);
            for record in records.into_iter().skip(skip) {
                prompt.push_str(&format!("- {}: {}\n", record.key, record.value));
            }
        }
        Ok(prompt)
    }
}

fn scope_label(scope: MemoryScope) -> &'static str {
    match scope {
        MemoryScope::Project => "Project",
        MemoryScope::Personal => "Personal",
    }
}

fn window_history(history: &[Message], strategy: &ContextStrategy) -> Vec<Message> {
    let mut skip = history.len().saturating_sub(strategy.max_history_messages);
    if skip > 0 {
        // Advancing past tool results and assistant function-call turns prevents slicing a tool_call/tool_result pair.
        while history.get(skip).is_some_and(|m| {
            m.role == Role::Tool || (m.role == Role::Assistant && !m.tool_calls.is_empty())
        }) {
            skip += 1;
        }
        if skip >= history.len()
            && let Some(prev_user) = history.iter().rposition(|m| m.role == Role::User)
        {
            skip = prev_user;
        }
    }
    history
        .iter()
        .skip(skip)
        .map(|msg| truncate_tool_output(msg, strategy.max_tool_output_chars))
        .collect()
}

fn truncate_tool_output(msg: &Message, max_chars: usize) -> Message {
    if msg.role != Role::Tool || msg.content.chars().count() <= max_chars {
        return msg.clone();
    }
    let mut clipped: String = msg.content.chars().take(max_chars).collect();
    clipped.push_str(TRUNCATION_MARKER);
    Message {
        role: msg.role,
        content: clipped,
        tool_calls: msg.tool_calls.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rein_policy::WorkspaceSandbox;
    use rein_tools::{MemoryRecord, NotesTool, ReadTool};

    #[test]
    fn coding_context_injects_both_scopes_and_filters_tools()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("rein_context_builder_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;

        let memory = JsonlMemoryStore::new(dir.join("memory.jsonl"));
        memory.append(&MemoryRecord {
            scope: MemoryScope::Personal,
            key: "editor".to_owned(),
            value: "helix".to_owned(),
        })?;
        memory.append(&MemoryRecord {
            scope: MemoryScope::Project,
            key: "msrv".to_owned(),
            value: "1.99".to_owned(),
        })?;

        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ReadTool::new(WorkspaceSandbox::new(&dir))));
        registry.register(Box::new(NotesTool::new(dir.join("notes.jsonl"))));

        let coding = Profile::coding();
        let mut history = vec![Message::new(Role::Tool, "x".repeat(30_000))];
        history.push(Message::new(Role::User, "hi"));
        let ctx = ContextBuilder::new(&coding, &memory).build(&history, &registry)?;

        assert!(ctx.system_prompt.contains("editor: helix"));
        assert!(ctx.system_prompt.contains("msrv: 1.99"));
        assert!(ctx.tools.iter().any(|t| t.name == "read"));
        assert!(!ctx.tools.iter().any(|t| t.name == "notes"));
        assert!(ctx.messages[0].content.ends_with(TRUNCATION_MARKER));

        let personal = Profile::personal();
        let pctx = ContextBuilder::new(&personal, &memory).build(&[], &registry)?;
        assert!(pctx.system_prompt.contains("editor: helix"));
        assert!(!pctx.system_prompt.contains("msrv"));
        assert!(pctx.tools.iter().any(|t| t.name == "notes"));

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn window_history_skips_orphaned_leading_tool_results_after_slice() {
        let mut assistant = Message::new(Role::Assistant, "running bash");
        assistant.tool_calls.push(rein_core::message::ToolCall {
            id: "c1".to_owned(),
            name: "bash".to_owned(),
            arguments: serde_json::json!({"command": "ls"}),
        });
        let mut tool = Message::new(Role::Tool, "ok");
        tool.tool_calls = assistant.tool_calls.clone();

        let mut history = vec![
            Message::new(Role::User, "old"),
            assistant,
            tool, // index 2: raw cut point when max_history_messages = 3
        ];
        history.push(Message::new(Role::Assistant, "done"));
        history.push(Message::new(Role::User, "next"));

        let strategy = ContextStrategy {
            max_history_messages: 3,
            max_memory_records: 8,
            max_tool_output_chars: 1000,
            inject_scoped_memory: false,
        };
        let windowed = window_history(&history, &strategy);
        assert_eq!(windowed.len(), 2);
        assert_eq!(windowed[0].role, Role::Assistant);
        assert_eq!(windowed[1].role, Role::User);
    }

    #[test]
    fn window_history_skips_assistant_function_call_slice_and_scopes_plan_tools() {
        let mut assistant = Message::new(Role::Assistant, "running bash");
        assistant.tool_calls.push(rein_core::message::ToolCall {
            id: "c1".to_owned(),
            name: "bash".to_owned(),
            arguments: serde_json::json!({"command": "ls"}),
        });
        let mut tool = Message::new(Role::Tool, "ok");
        tool.tool_calls = assistant.tool_calls.clone();

        let history = vec![
            Message::new(Role::User, "old"),
            assistant, // index 1: cut point when max_history_messages = 3
            tool,
            Message::new(Role::User, "next"),
        ];
        let strategy = ContextStrategy {
            max_history_messages: 3,
            max_memory_records: 8,
            max_tool_output_chars: 1000,
            inject_scoped_memory: false,
        };
        let windowed = window_history(&history, &strategy);
        assert_eq!(windowed.len(), 1);
        assert_eq!(windowed[0].role, Role::User);
        assert_eq!(windowed[0].content, "next");

        let plan = Profile::plan();
        assert!(plan.allows_tool("read"));
        assert!(plan.allows_tool("switch_mode"));
        assert!(!plan.allows_tool("write"));
        assert!(!plan.allows_tool("edit"));
        assert!(!plan.allows_tool("notes"));
        assert!(!plan.allows_tool("calendar"));
        assert!(!plan.allows_tool("email"));
    }
}
