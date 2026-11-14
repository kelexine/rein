// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: In-process typestate agent loop wiring ContextBuilder, Provider, PolicyEngine, and ToolRegistry

use anyhow::{Context, Result};
use rein_core::{
    EventLog, Message, ModeId, Provider, ProviderStreamEvent, Role, ToolCall, ToolSpec, Turn,
};
use rein_policy::{PolicyDecision, PolicyEngine};
use rein_tools::{JsonlMemoryStore, ToolRegistry, ToolSchema};

use crate::modes::{ContextBuilder, ModeManager, Profile};

/// Maximum consecutive tool-execution rounds permitted in a single user turn.
pub const MAX_TOOL_ROUNDS: usize = 16;

/// Interactive callbacks used by [`AgentLoop`] for policy `[y/n]` confirmation and live streaming.
pub trait ApprovalPrompter {
    /// Displays `prompt` to the user and returns `true` if approved.
    fn confirm(&mut self, prompt: &str) -> Result<bool>;

    /// Receives incremental streamed text chunks during a model turn.
    fn on_delta(&mut self, _delta: &str) {}
}

/// Coordinates the single-process agent turn loop across Core, Policy, Tools, and Modes.
pub struct AgentLoop {
    event_log: EventLog,
    policy: PolicyEngine,
    registry: ToolRegistry,
    memory: JsonlMemoryStore,
    modes: ModeManager,
    history: Vec<Message>,
}

impl AgentLoop {
    /// Creates a new [`AgentLoop`] with all layers wired in-process.
    #[must_use]
    pub fn new(
        event_log: EventLog,
        policy: PolicyEngine,
        registry: ToolRegistry,
        memory: JsonlMemoryStore,
        initial_profile: Profile,
    ) -> Self {
        Self {
            event_log,
            policy,
            registry,
            memory,
            modes: ModeManager::new(initial_profile),
            history: Vec::new(),
        }
    }

    /// Returns the active [`Profile`].
    #[must_use]
    pub fn active_profile(&self) -> &Profile {
        self.modes.active()
    }

    /// Returns the [`PolicyEngine`] reference.
    #[must_use]
    pub fn policy(&self) -> &PolicyEngine {
        &self.policy
    }

    /// Toggles between `Coding` and `Personal` profiles via [`ModeManager`].
    pub fn toggle_mode(&mut self) -> Result<ModeId> {
        let next = match self.modes.active().mode {
            ModeId::Coding => Profile::personal(),
            ModeId::Personal => Profile::coding(),
        };
        let switched = self
            .modes
            .switch_to(next, &mut self.policy, &self.event_log)
            .context("failed to switch mode profile")?;
        Ok(switched)
    }

    /// Executes a user prompt through the typestate loop until the model finishes or reaches max rounds.
    pub fn run_user_prompt(
        &mut self,
        prompt: &str,
        provider: &mut dyn Provider,
        prompter: &mut dyn ApprovalPrompter,
    ) -> Result<String> {
        self.history.push(Message::new(Role::User, prompt));
        let mut final_text = String::new();

        for round in 0..MAX_TOOL_ROUNDS {
            let assembled = ContextBuilder::new(self.modes.active(), &self.memory)
                .build(&self.history, &self.registry)
                .context("failed to assemble turn context")?;
            let specs: Vec<ToolSpec> = assembled.tools.iter().map(schema_to_spec).collect();

            let mut streaming = if round == 0 {
                Turn::new(&self.event_log).begin_user_turn(prompt)?
            } else {
                Turn::new(&self.event_log).begin_user_turn("[tool_result_continuation]")?
            };

            let mut streamed_text = String::new();
            let mut requested_call: Option<ToolCall> = None;
            let events = provider.stream_turn_with(
                &assembled.system_prompt,
                &assembled.messages,
                &specs,
                &mut |ev| {
                    if let ProviderStreamEvent::TextDelta(delta) = ev {
                        prompter.on_delta(delta);
                    }
                },
            )?;

            for ev in events {
                match ev {
                    ProviderStreamEvent::TextDelta(delta) => {
                        streamed_text.push_str(&delta);
                        streaming.push_delta(&delta)?;
                    }
                    ProviderStreamEvent::ToolUse(call) => {
                        requested_call = Some(call);
                    }
                    ProviderStreamEvent::Done => {}
                }
            }

            if let Some(call) = requested_call {
                let mut assistant_msg = Message::new(Role::Assistant, &streamed_text);
                assistant_msg.tool_calls.push(call.clone());
                self.history.push(assistant_msg);

                let awaiting = streaming.request_tool(call.clone())?;
                let decision = self.registry.evaluate_turn_for_profile(
                    &awaiting,
                    &self.policy,
                    &self.modes.active().allowed_tools,
                )?;
                let (_idle, output) = match decision {
                    PolicyDecision::Allow { reason } => {
                        let executing = awaiting.approve(reason)?;
                        self.registry.execute_approved(executing)?
                    }
                    PolicyDecision::Ask { prompt: ask_prompt } => {
                        if prompter.confirm(&ask_prompt)? {
                            let executing = awaiting.approve("user approved in TUI")?;
                            self.registry.execute_approved(executing)?
                        } else {
                            awaiting.deny("user denied in TUI")?
                        }
                    }
                    PolicyDecision::Deny { reason } => awaiting.deny(reason)?,
                };

                let mut tool_msg = Message::new(Role::Tool, output.content);
                tool_msg.tool_calls.push(call);
                self.history.push(tool_msg);
            } else {
                let (_idle, msg) = streaming.finish_response()?;
                final_text = msg.content.clone();
                self.history.push(msg);
                return Ok(final_text);
            }
        }
        Ok(final_text)
    }
}

fn schema_to_spec(schema: &ToolSchema) -> ToolSpec {
    ToolSpec {
        name: schema.name.clone(),
        description: schema.description.clone(),
        input_schema: schema.input_schema.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rein_core::{ReplayProvider, SessionEvent};
    use rein_policy::{AuditLog, WorkspaceSandbox};
    use rein_tools::ReadTool;

    struct AlwaysApprove(String);
    impl ApprovalPrompter for AlwaysApprove {
        fn confirm(&mut self, _prompt: &str) -> Result<bool> {
            Ok(true)
        }

        fn on_delta(&mut self, delta: &str) {
            self.0.push_str(delta);
        }
    }

    #[test]
    fn executes_multi_round_replay_loop_through_policy_gate() -> Result<()> {
        let dir = std::env::temp_dir().join("rein_agent_loop_replay_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("note.txt"), "hello rein")?;

        let recorded = vec![
            SessionEvent::StreamDelta {
                delta: "Let me read note.txt. ".to_owned(),
            },
            SessionEvent::ToolRequested {
                call: ToolCall {
                    id: "call_1".to_owned(),
                    name: "read".to_owned(),
                    arguments: serde_json::json!({"path": "note.txt"}),
                },
            },
            SessionEvent::StreamDelta {
                delta: "The file says hello rein.".to_owned(),
            },
            SessionEvent::MessageRecorded {
                message: Message::new(Role::Assistant, "The file says hello rein."),
            },
        ];

        let mut replay = ReplayProvider::from_events(&recorded);
        let sandbox = WorkspaceSandbox::new(&dir);
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ReadTool::new(sandbox.clone())));

        let profile = Profile::coding();
        let policy = PolicyEngine::new(
            profile.policy_rules.clone(),
            sandbox,
            AuditLog::new(dir.join("audit.jsonl")),
        );
        let mut agent = AgentLoop::new(
            EventLog::new(dir.join("session.jsonl")),
            policy,
            registry,
            JsonlMemoryStore::new(dir.join("memory.jsonl")),
            profile,
        );

        let mut prompter = AlwaysApprove(String::new());
        let reply = agent.run_user_prompt("what is in note.txt?", &mut replay, &mut prompter)?;
        assert_eq!(reply, "The file says hello rein.");
        assert!(prompter.0.contains("Let me read note.txt."));

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
