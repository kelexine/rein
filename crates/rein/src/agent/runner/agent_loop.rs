// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: In-process typestate agent loop wiring ContextBuilder, Provider, PolicyEngine, and ToolRegistry

use super::batch::*;
use super::format::*;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};
use rein_core::{
    CoreError, EventLog, Message, ModeId, Provider, ProviderStreamEvent, Role, ToolCall,
    ToolOutput, ToolSpec, Turn, UsageStats,
};
use rein_policy::PolicyEngine;
use rein_tools::{JsonlMemoryStore, ToolRegistry};

use crate::agent::snapshot::{
    TurnCheckpoint, append_checkpoint, capture_workspace_snapshot, checkpoints_path_for,
    count_jsonl_lines, now_unix_secs,
};
use crate::agent::subagent::SubagentSessionState;
use crate::modes::{ContextBuilder, ModeManager, Profile, RuntimeContext};

/// Maximum consecutive tool-execution rounds permitted in a single user turn.
pub const MAX_TOOL_ROUNDS: usize = 16;

/// Structured 4-option approval request presented to the user on [`PolicyDecision::Ask`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    /// Policy classification summary and prompt description.
    pub title: String,
    /// Concise action/command excerpt shown on `1. Allow: <excerpt>`.
    pub excerpt: String,
    /// Allowlist pattern shown on `2. Allow in CWD` and `3. Allow Global`.
    pub pattern: String,
}

/// Numbered selection chosen by the user for an [`ApprovalRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalAction {
    /// `1. Allow: <excerpt>` (one-shot execution for this call only).
    AllowOnce,
    /// `2. Allow in CWD: <pattern>` (persists to `<workspace>/.rein/commands.json`).
    AllowCwd(String),
    /// `3. Allow Global: <pattern>` (persists to `~/.rein/commands.json`).
    AllowGlobal(String),
    /// `4. Deny`.
    Deny,
}

/// Complete decision returned by [`ApprovalPrompter::confirm`], with optional `Tab` feedback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalDecision {
    /// Chosen option among Allow Once, Allow in CWD, Allow Global, or Deny.
    pub action: ApprovalAction,
    /// Optional freeform feedback written via `Tab` on any highlighted option, returned with the tool result.
    pub feedback: Option<String>,
}

/// Structured transcript event yielded when hydrating `<uid>.jsonl` on `--resume` or `/resume`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HydratedEvent {
    /// Submitted user prompt.
    User(String),
    /// Completed assistant text message.
    Assistant(String),
    /// Muted system note (for example compaction boundaries).
    Note(String),
    /// Completed tool call paired with its output payload.
    Tool {
        /// Tool call arguments and identifier.
        call: ToolCall,
        /// Tool output or denial text.
        output: ToolOutput,
    },
}

/// Interactive callbacks used by [`AgentLoop`] for numbered policy confirmation, streaming deltas, and tool status.
pub trait ApprovalPrompter {
    /// Displays `request` options (`1..=4`) to the user and returns the chosen [`ApprovalDecision`].
    fn confirm(&mut self, request: &ApprovalRequest) -> Result<ApprovalDecision>;

    /// Receives incremental streamed text chunks during a model turn.
    fn on_delta(&mut self, _delta: &str) {}

    /// Receives incremental model reasoning / thought chunks during a turn.
    fn on_thinking_delta(&mut self, _delta: &str) {}

    /// Receives compact 1-line tool activity summaries (`· read Cargo.toml`).
    fn on_tool_status(&mut self, _status: &str) {}

    /// Notified right when a tool call begins policy check / execution.
    fn on_tool_start(&mut self, _call: &ToolCall) {}

    /// Notified once a tool finishes (or is denied) with its resulting output.
    fn on_tool_finish(&mut self, _call: &ToolCall, _output: &ToolOutput) {}

    /// Notified once per provider turn with reported token usage.
    fn on_usage(&mut self, _usage: UsageStats) {}

    /// Notified immediately when `switch_mode` changes the active profile mid-turn.
    fn on_mode_switched(&mut self, _profile: &Profile) {}

    /// Notified when a mid-turn steering message from the user is injected into the conversation.
    fn on_steer_delivered(&mut self, _display: &str) {}
}

/// Coordinates the single-process agent turn loop across Core, Policy, Tools, and Modes.
pub struct AgentLoop {
    pub(super) event_log: EventLog,
    pub(super) full_transcript_path: Option<PathBuf>,
    repo_commands_path: Option<PathBuf>,
    global_commands_path: Option<PathBuf>,
    pub(super) policy: PolicyEngine,
    registry: ToolRegistry,
    pub(super) memory: JsonlMemoryStore,
    pub(super) modes: ModeManager,
    pub(super) history: Vec<Message>,
    pub(super) checkpoints: Vec<TurnCheckpoint>,
    allowed_commands: Vec<String>,
    extra_tools: Vec<String>,
    pub(super) runtime_context: RuntimeContext,
    subagent_state: Option<Arc<RwLock<SubagentSessionState>>>,
    subagent_cancel: Option<Arc<AtomicBool>>,
    steer_rx: Option<std::sync::mpsc::Receiver<(String, String)>>,
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
        let allowed_commands = initial_profile.policy_rules.allowed_commands.clone();
        Self {
            event_log,
            full_transcript_path: None,
            repo_commands_path: None,
            global_commands_path: None,
            policy,
            registry,
            memory,
            modes: ModeManager::new(initial_profile),
            history: Vec::new(),
            checkpoints: Vec::new(),
            allowed_commands,
            extra_tools: Vec::new(),
            runtime_context: RuntimeContext::new(),
            subagent_state: None,
            subagent_cancel: None,
            steer_rx: None,
        }
    }

    /// Attaches a mid-turn steering receiver `(display, resolved_prompt)` for the active turn.
    pub fn set_steer_receiver(&mut self, rx: std::sync::mpsc::Receiver<(String, String)>) {
        self.steer_rx = Some(rx);
    }

    /// Clears any attached mid-turn steering receiver.
    pub fn clear_steer_receiver(&mut self) {
        self.steer_rx = None;
    }

    fn drain_steer_messages(&mut self, prompter: &mut dyn ApprovalPrompter) -> Result<bool> {
        let Some(rx) = &self.steer_rx else {
            return Ok(false);
        };
        let mut drained = Vec::new();
        while let Ok(item) = rx.try_recv() {
            drained.push(item);
        }
        if drained.is_empty() {
            return Ok(false);
        }
        for (display, prompt_for_model) in drained {
            let msg = Message::new(Role::User, prompt_for_model);
            record_full_message(self.full_transcript_path.as_deref(), &msg)?;
            self.history.push(msg);
            prompter.on_steer_delivered(&display);
        }
        Ok(true)
    }

    /// Attaches shared policy and cancellation handles so spawned `agent` sub-agents stay confined to active session policy.
    #[must_use]
    pub fn with_subagent_handles(
        mut self,
        state: Arc<RwLock<SubagentSessionState>>,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        self.subagent_state = Some(state);
        self.subagent_cancel = Some(cancel);
        self.sync_subagent_state();
        self
    }

    pub(super) fn sync_subagent_state(&self) {
        if let Some(handle) = &self.subagent_state
            && let Ok(mut guard) = handle.write()
        {
            guard.active_mode = self.modes.active().mode;
            guard.policy_rules = self.policy.rules().clone();
            guard.bypass_permissions = self.policy.is_bypass_permissions();
            guard.allowed_commands = self.allowed_commands.clone();
            guard.sandbox_status = self.policy.sandbox_status().clone();
            guard.coordinator_history = self.history.clone();
        }
    }

    /// Seeds initial conversation history (used by `swarm` `fork` workers).
    #[must_use]
    pub fn with_seed_history(mut self, history: Vec<Message>) -> Self {
        self.history = history;
        self
    }

    /// Attaches dynamic runtime metadata injected into assembled turn system prompts.
    #[must_use]
    pub fn with_runtime_context(mut self, runtime_context: RuntimeContext) -> Self {
        self.runtime_context = runtime_context;
        self
    }

    /// Configures persistent CWD (`<workspace>/.rein/commands.json`) and Global (`~/.rein/commands.json`) paths.
    #[must_use]
    pub fn with_command_allowlist_paths(
        mut self,
        repo_path: impl Into<PathBuf>,
        global_path: impl Into<PathBuf>,
    ) -> Self {
        self.repo_commands_path = Some(repo_path.into());
        self.global_commands_path = Some(global_path.into());
        self
    }

    /// Configures persistent append-only `full-transcript.jsonl` archive preserved across `/compact`.
    #[must_use]
    pub fn with_full_transcript_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.full_transcript_path = Some(path.into());
        self
    }

    /// Returns the configured full uncompacted transcript path, if any.
    #[must_use]
    pub fn full_transcript_path(&self) -> Option<&Path> {
        self.full_transcript_path.as_deref()
    }

    /// Configures persistent allowlisted shell commands merged from global + repo `commands.json`.
    #[must_use]
    pub fn with_allowed_commands(mut self, commands: Vec<String>) -> Self {
        self.allowed_commands = commands;
        self
    }

    /// Configures extra registered tool names (e.g., MCP tools) enabled across profile switches.
    #[must_use]
    pub fn with_extra_tools(mut self, extra_tools: Vec<String>) -> Self {
        self.extra_tools = extra_tools;
        self
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

    /// Returns the current conversation history slice.
    #[must_use]
    pub fn history(&self) -> &[Message] {
        &self.history
    }

    /// Returns a reference to the dual-scope [`JsonlMemoryStore`].
    #[must_use]
    pub fn memory_store(&self) -> &JsonlMemoryStore {
        &self.memory
    }

    /// Cycles between `Coding`, `Plan`, and `Personal` profiles via [`ModeManager`].
    pub fn toggle_mode(&mut self) -> Result<ModeId> {
        let next_mode = match self.modes.active().mode {
            ModeId::Coding => ModeId::Plan,
            ModeId::Plan => ModeId::Personal,
            ModeId::Personal => ModeId::Coding,
        };
        self.set_mode(next_mode)
    }

    /// Switches explicitly to `target_mode` (no-op if already active).
    pub fn set_mode(&mut self, target_mode: ModeId) -> Result<ModeId> {
        if self.modes.active().mode == target_mode {
            return Ok(target_mode);
        }
        let next = self.build_profile(target_mode);
        let switched = self
            .modes
            .switch_to(next, &mut self.policy, &self.event_log)
            .context("failed to switch mode profile")?;
        self.sync_subagent_state();
        Ok(switched)
    }

    /// Returns recorded pre-turn checkpoints for `/rewind`.
    /// Executes a user prompt through the typestate loop until the model finishes or reaches max rounds.
    pub fn run_user_prompt(
        &mut self,
        prompt: &str,
        provider: &mut dyn Provider,
        prompter: &mut dyn ApprovalPrompter,
    ) -> Result<String> {
        let never = AtomicBool::new(false);
        self.run_user_prompt_cancellable(prompt, provider, prompter, &never)
    }

    /// Executes a user prompt through the typestate loop, aborting cleanly if `cancel` is set.
    pub fn run_user_prompt_cancellable(
        &mut self,
        prompt: &str,
        provider: &mut dyn Provider,
        prompter: &mut dyn ApprovalPrompter,
        cancel: &AtomicBool,
    ) -> Result<String> {
        let preview = prompt
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .chars()
            .take(80)
            .collect::<String>();
        let snapshot = capture_workspace_snapshot(self.policy.sandbox().root()).ok();
        let checkpoint = TurnCheckpoint {
            turn_index: self.checkpoints.len(),
            prompt_preview: preview,
            timestamp_secs: now_unix_secs(),
            history_len: self.history.len(),
            event_count: count_jsonl_lines(self.event_log.path()),
            full_transcript_count: self
                .full_transcript_path
                .as_deref()
                .map_or(0, count_jsonl_lines),
            snapshot,
        };
        let _ = append_checkpoint(&checkpoints_path_for(self.event_log.path()), &checkpoint);
        self.checkpoints.push(checkpoint);
        self.sync_subagent_state();
        if let Some(sub_cancel) = &self.subagent_cancel {
            sub_cancel.store(false, Ordering::Relaxed);
        }

        let user_msg = Message::new(Role::User, prompt);
        record_full_message(self.full_transcript_path.as_deref(), &user_msg)?;
        self.history.push(user_msg);
        let mut final_text = String::new();

        for round in 0..MAX_TOOL_ROUNDS {
            if cancel.load(Ordering::Relaxed) {
                return Err(CoreError::Cancelled.into());
            }
            if round > 0 {
                let _ = self.drain_steer_messages(prompter)?;
            }
            let assembled = ContextBuilder::new(self.modes.active(), &self.memory)
                .with_runtime_context(&self.runtime_context)
                .build(&self.history, &self.registry)
                .context("failed to assemble turn context")?;
            let specs: Vec<ToolSpec> = assembled.tools.iter().map(schema_to_spec).collect();
            let turn_prompt = if round == 0 {
                prompt
            } else {
                "[tool_result_continuation]"
            };
            let mut streaming = Turn::new(&self.event_log).begin_user_turn(turn_prompt)?;

            let mut streamed_text = String::new();
            let mut requested_calls: Vec<ToolCall> = Vec::new();
            let mut turn_usage: Option<UsageStats> = None;
            let events = match provider.stream_turn_cancellable(
                &assembled.system_prompt,
                &assembled.messages,
                &specs,
                cancel,
                &mut |ev| match ev {
                    ProviderStreamEvent::ThinkingDelta(delta) => prompter.on_thinking_delta(delta),
                    ProviderStreamEvent::TextDelta(delta) => prompter.on_delta(delta),
                    _ => {}
                },
            ) {
                Ok(evs) => evs,
                Err(CoreError::Cancelled) => {
                    if let Some(sub_cancel) = &self.subagent_cancel {
                        sub_cancel.store(true, Ordering::Relaxed);
                    }
                    finalize_cancelled_stream(
                        streaming,
                        self.full_transcript_path.as_deref(),
                        &mut self.history,
                    )?;
                    return Err(CoreError::Cancelled.into());
                }
                Err(err) => return Err(err.into()),
            };

            for ev in events {
                match ev {
                    ProviderStreamEvent::ThinkingDelta(_) => {}
                    ProviderStreamEvent::TextDelta(delta) => {
                        streamed_text.push_str(&delta);
                        streaming.push_delta(&delta)?;
                    }
                    ProviderStreamEvent::ToolUse(call) => requested_calls.push(call),
                    ProviderStreamEvent::Usage(usage) => turn_usage = Some(usage),
                    ProviderStreamEvent::Done => {}
                }
            }
            if let Some(usage) = turn_usage {
                prompter.on_usage(usage);
            }

            if cancel.load(Ordering::Relaxed) {
                finalize_cancelled_stream(
                    streaming,
                    self.full_transcript_path.as_deref(),
                    &mut self.history,
                )?;
                return Err(CoreError::Cancelled.into());
            }

            if requested_calls.is_empty() {
                let (_idle, msg) = streaming.finish_response()?;
                final_text = msg.content.clone();
                record_full_message(self.full_transcript_path.as_deref(), &msg)?;
                self.history.push(msg);
                self.sync_subagent_state();
                if self.drain_steer_messages(prompter)? {
                    continue;
                }
                return Ok(final_text);
            }
            execute_tool_batch(
                streaming,
                &self.event_log,
                self.full_transcript_path.as_deref(),
                self.repo_commands_path.as_deref(),
                self.global_commands_path.as_deref(),
                &mut self.policy,
                &mut self.allowed_commands,
                &self.extra_tools,
                &self.registry,
                &mut self.modes,
                &mut self.history,
                &streamed_text,
                requested_calls,
                prompter,
                cancel,
            )?;
            if cancel.load(Ordering::Relaxed) {
                return Err(CoreError::Cancelled.into());
            }
        }
        Ok(final_text)
    }

    pub(super) fn build_profile(&self, mode: ModeId) -> Profile {
        build_mode_profile(mode, &self.allowed_commands, &self.extra_tools)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rein_core::{ReplayProvider, SessionEvent};
    use rein_policy::{AuditLog, WorkspaceSandbox};
    use rein_tools::ReadTool;

    struct RecordingPrompter {
        deltas: String,
        statuses: Vec<String>,
        confirms: Vec<ApprovalRequest>,
        scripted: Vec<ApprovalDecision>,
    }

    impl RecordingPrompter {
        fn new() -> Self {
            Self {
                deltas: String::new(),
                statuses: Vec::new(),
                confirms: Vec::new(),
                scripted: Vec::new(),
            }
        }

        fn with_decisions(decisions: Vec<ApprovalDecision>) -> Self {
            Self {
                deltas: String::new(),
                statuses: Vec::new(),
                confirms: Vec::new(),
                scripted: decisions,
            }
        }
    }

    impl ApprovalPrompter for RecordingPrompter {
        fn confirm(&mut self, request: &ApprovalRequest) -> Result<ApprovalDecision> {
            self.confirms.push(request.clone());
            if !self.scripted.is_empty() {
                return Ok(self.scripted.remove(0));
            }
            Ok(ApprovalDecision {
                action: ApprovalAction::AllowOnce,
                feedback: None,
            })
        }

        fn on_delta(&mut self, delta: &str) {
            self.deltas.push_str(delta);
        }

        fn on_tool_status(&mut self, status: &str) {
            self.statuses.push(status.to_owned());
        }
    }

    #[test]
    fn executes_multi_tool_batch_and_hydrates_resume_log() -> Result<()> {
        let dir = std::env::temp_dir().join("rein_agent_multi_tool_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("a.txt"), "alpha")?;
        std::fs::write(dir.join("b.txt"), "beta")?;

        let recorded = vec![
            SessionEvent::StreamDelta {
                delta: "Reading both files. ".to_owned(),
            },
            SessionEvent::ToolRequested {
                call: ToolCall {
                    id: "call_1".to_owned(),
                    name: "read".to_owned(),
                    arguments: serde_json::json!({"path": "a.txt"}),
                },
            },
            SessionEvent::ToolRequested {
                call: ToolCall {
                    id: "call_2".to_owned(),
                    name: "read".to_owned(),
                    arguments: serde_json::json!({"path": "b.txt"}),
                },
            },
            SessionEvent::StreamDelta {
                delta: "Read alpha and beta.".to_owned(),
            },
            SessionEvent::MessageRecorded {
                message: Message::new(Role::Assistant, "Read alpha and beta."),
            },
        ];

        let mut replay = ReplayProvider::from_events(&recorded);
        let sandbox = WorkspaceSandbox::new(&dir);
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ReadTool::new(sandbox.clone())));

        let profile = Profile::coding();
        let policy = PolicyEngine::new(
            profile.policy_rules.clone(),
            sandbox.clone(),
            AuditLog::new(dir.join("audit.jsonl")),
        );
        let log_path = dir.join("session.jsonl");
        let mut agent = AgentLoop::new(
            EventLog::new(&log_path),
            policy,
            registry,
            JsonlMemoryStore::new(dir.join("memory.jsonl")),
            profile,
        );

        let mut prompter = RecordingPrompter::new();
        let reply = agent.run_user_prompt("read both files", &mut replay, &mut prompter)?;
        assert_eq!(reply, "Read alpha and beta.");
        assert_eq!(prompter.statuses, vec!["· read a.txt", "· read b.txt"]);
        assert!(prompter.deltas.contains("Reading both files."));

        let mut resumed_registry = ToolRegistry::new();
        resumed_registry.register(Box::new(ReadTool::new(sandbox.clone())));
        let resumed_profile = Profile::coding();
        let resumed_policy = PolicyEngine::new(
            resumed_profile.policy_rules.clone(),
            sandbox,
            AuditLog::new(dir.join("audit.jsonl")),
        );
        let mut resumed = AgentLoop::new(
            EventLog::new(&log_path),
            resumed_policy,
            resumed_registry,
            JsonlMemoryStore::new(dir.join("memory.jsonl")),
            resumed_profile,
        );
        let transcript = resumed.hydrate_from_log()?;
        assert!(transcript.iter().any(|l| l == "❯ read both files"));
        assert!(transcript.iter().any(|l| l == "· read a.txt"));
        assert!(transcript.iter().any(|l| l == "Read alpha and beta."));
        assert_eq!(resumed.history().len(), 5);
        assert_eq!(resumed.history()[0].role, Role::User);
        assert_eq!(resumed.history()[1].role, Role::Assistant);
        assert_eq!(resumed.history()[1].tool_calls.len(), 2);
        assert_eq!(resumed.history()[2].role, Role::Tool);
        assert_eq!(resumed.history()[3].role, Role::Tool);
        assert_eq!(resumed.history()[4].role, Role::Assistant);

        let confirm_msg = format_confirmation_prompt(
            &ToolCall {
                id: "c1".to_owned(),
                name: "edit".to_owned(),
                arguments: serde_json::json!({"path": "foo.rs", "old": "bar()", "new": "baz()"}),
            },
            "confirm?",
        );
        assert!(confirm_msg.contains("edit foo.rs (-bar() / +baz())"));

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn compacts_via_provider_while_keeping_full_transcript_archive() -> Result<()> {
        let dir = std::env::temp_dir().join("rein_compact_full_transcript_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;

        let events = vec![
            SessionEvent::MessageRecorded {
                message: Message::new(Role::Assistant, "first answer"),
            },
            SessionEvent::MessageRecorded {
                message: Message::new(Role::Assistant, "concise summary of first answer"),
            },
            SessionEvent::MessageRecorded {
                message: Message::new(Role::Assistant, "second answer after compaction"),
            },
        ];
        let mut replay = ReplayProvider::from_events(&events);
        let sandbox = WorkspaceSandbox::new(&dir);
        let profile = Profile::coding();
        let policy = PolicyEngine::new(
            profile.policy_rules.clone(),
            sandbox.clone(),
            AuditLog::new(dir.join("audit.jsonl")),
        );
        let session_path = dir.join("a1b2c3d4.jsonl");
        let full_path = dir.join("a1b2c3d4.full.jsonl");
        let mut agent = AgentLoop::new(
            EventLog::new(&session_path),
            policy,
            ToolRegistry::new(),
            JsonlMemoryStore::new(dir.join("memory.jsonl")),
            profile.clone(),
        )
        .with_full_transcript_path(&full_path);

        let mut prompter = RecordingPrompter::new();
        let _ = agent.run_user_prompt("first question", &mut replay, &mut prompter)?;
        let status = agent.compact_with_provider(None, &mut replay)?;
        assert!(status.contains("compacted 2 messages"));
        assert_eq!(agent.history().len(), 1);
        assert!(agent.history()[0].content.contains("concise summary"));
        assert!(agent.history()[0].content.contains("a1b2c3d4.full.jsonl"));

        let _ = agent.run_user_prompt("follow up question", &mut replay, &mut prompter)?;

        let archived = EventLog::new(&full_path).read_all()?;
        assert_eq!(archived.len(), 4);

        let resumed_policy = PolicyEngine::new(
            profile.policy_rules.clone(),
            sandbox,
            AuditLog::new(dir.join("audit.jsonl")),
        );
        let mut resumed = AgentLoop::new(
            EventLog::new(&session_path),
            resumed_policy,
            ToolRegistry::new(),
            JsonlMemoryStore::new(dir.join("memory.jsonl")),
            profile,
        )
        .with_full_transcript_path(&full_path);
        let hydrated = resumed.hydrate_structured()?;
        assert!(
            hydrated
                .iter()
                .any(|ev| matches!(ev, HydratedEvent::Note(n) if n.contains("compacted 2")))
        );
        assert_eq!(resumed.history().len(), 3);
        assert_eq!(resumed.history()[0].role, Role::User);
        assert!(resumed.history()[0].content.contains("concise summary"));
        assert_eq!(resumed.history()[1].content, "follow up question");
        assert_eq!(
            resumed.history()[2].content,
            "second answer after compaction"
        );

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn approval_cwd_persists_sandboxed_tool_and_attaches_feedback_on_allow_and_deny() -> Result<()>
    {
        use rein_tools::{EditTool, WriteTool};

        let dir = std::env::temp_dir().join("rein_agent_approval_feedback_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("target.txt"), "hello world")?;

        let events = vec![
            SessionEvent::ToolRequested {
                call: ToolCall {
                    id: "call_w1".to_owned(),
                    name: "write".to_owned(),
                    arguments: serde_json::json!({"path": "first.txt", "content": "one"}),
                },
            },
            SessionEvent::ToolRequested {
                call: ToolCall {
                    id: "call_w2".to_owned(),
                    name: "write".to_owned(),
                    arguments: serde_json::json!({"path": "second.txt", "content": "two"}),
                },
            },
            SessionEvent::StreamDelta {
                delta: "Wrote files.".to_owned(),
            },
            SessionEvent::MessageRecorded {
                message: Message::new(Role::Assistant, "Wrote files."),
            },
        ];
        let mut replay = ReplayProvider::from_events(&events);
        let sandbox = WorkspaceSandbox::new(&dir);
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(WriteTool::new(sandbox.clone())));
        registry.register(Box::new(EditTool::new(sandbox.clone())));

        let profile =
            Profile::personal().with_extra_tools(&["write".to_owned(), "edit".to_owned()]);
        let policy = PolicyEngine::new(
            profile.policy_rules.clone(),
            sandbox,
            AuditLog::new(dir.join("audit.jsonl")),
        );
        let repo_cmds = dir.join(".rein/commands.json");
        let global_cmds = dir.join(".rein/global_commands.json");
        let mut agent = AgentLoop::new(
            EventLog::new(dir.join("session.jsonl")),
            policy,
            registry,
            JsonlMemoryStore::new(dir.join("memory.jsonl")),
            profile,
        )
        .with_extra_tools(vec!["write".to_owned(), "edit".to_owned()])
        .with_command_allowlist_paths(&repo_cmds, &global_cmds);

        let mut prompter = RecordingPrompter::with_decisions(vec![ApprovalDecision {
            action: ApprovalAction::AllowCwd("tool:write".to_owned()),
            feedback: Some("keep files minimal".to_owned()),
        }]);
        let _ = agent.run_user_prompt("write two files", &mut replay, &mut prompter)?;
        assert_eq!(prompter.confirms.len(), 1);
        assert_eq!(prompter.confirms[0].pattern, "tool:write");
        let persisted: Vec<String> = serde_json::from_str(&std::fs::read_to_string(&repo_cmds)?)?;
        assert_eq!(persisted, vec!["tool:write"]);
        assert!(
            agent.history().iter().any(|m| m.role == Role::Tool
                && m.content.contains("[User feedback: keep files minimal]"))
        );

        let deny_events = vec![
            SessionEvent::ToolRequested {
                call: ToolCall {
                    id: "call_e1".to_owned(),
                    name: "edit".to_owned(),
                    arguments: serde_json::json!({"path": "target.txt", "old": "hello", "new": "hi"}),
                },
            },
            SessionEvent::StreamDelta {
                delta: "Understood.".to_owned(),
            },
            SessionEvent::MessageRecorded {
                message: Message::new(Role::Assistant, "Understood."),
            },
        ];
        let mut deny_replay = ReplayProvider::from_events(&deny_events);
        let mut deny_prompter = RecordingPrompter::with_decisions(vec![ApprovalDecision {
            action: ApprovalAction::Deny,
            feedback: Some("use cargo check instead".to_owned()),
        }]);
        let _ = agent.run_user_prompt("edit target", &mut deny_replay, &mut deny_prompter)?;
        assert_eq!(deny_prompter.confirms.len(), 1);
        assert!(
            agent
                .history()
                .iter()
                .rev()
                .find(|m| m.role == Role::Tool)
                .is_some_and(|m| m.content.contains("use cargo check instead"))
        );

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn model_initiated_switch_mode_updates_active_profile_and_toolset() -> Result<()> {
        let dir = std::env::temp_dir().join("rein_agent_switch_mode_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;

        let events = vec![
            SessionEvent::ToolRequested {
                call: ToolCall {
                    id: "call_sw_1".to_owned(),
                    name: "switch_mode".to_owned(),
                    arguments: serde_json::json!({ "mode": "plan", "reason": "design first" }),
                },
            },
            SessionEvent::ToolRequested {
                call: ToolCall {
                    id: "call_notes_denied".to_owned(),
                    name: "notes".to_owned(),
                    arguments: serde_json::json!({ "action": "list" }),
                },
            },
            SessionEvent::ToolRequested {
                call: ToolCall {
                    id: "call_sw_2".to_owned(),
                    name: "switch_mode".to_owned(),
                    arguments: serde_json::json!({ "mode": "personal", "reason": "check notes" }),
                },
            },
            SessionEvent::StreamDelta {
                delta: "Now in personal mode.".to_owned(),
            },
            SessionEvent::MessageRecorded {
                message: Message::new(Role::Assistant, "Now in personal mode."),
            },
        ];
        let mut replay = ReplayProvider::from_events(&events);
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ReadTool::new(WorkspaceSandbox::new(&dir))));
        registry.register(Box::new(rein_tools::SwitchModeTool::new()));
        registry.register(Box::new(rein_tools::NotesTool::new(
            dir.join("notes.jsonl"),
        )));
        let profile = Profile::coding();
        let policy = PolicyEngine::new(
            profile.policy_rules.clone(),
            WorkspaceSandbox::new(&dir),
            AuditLog::new(dir.join("audit.jsonl")),
        );
        let mut agent = AgentLoop::new(
            EventLog::new(dir.join("session.jsonl")),
            policy,
            registry,
            JsonlMemoryStore::new(dir.join("memory.jsonl")),
            profile,
        );
        let mut prompter = RecordingPrompter::new();
        let out =
            agent.run_user_prompt("switch to plan then personal", &mut replay, &mut prompter)?;
        assert_eq!(out, "Now in personal mode.");
        assert_eq!(agent.active_profile().mode, ModeId::Personal);
        assert_eq!(agent.toggle_mode()?, ModeId::Coding);
        assert_eq!(agent.toggle_mode()?, ModeId::Plan);
        assert_eq!(agent.toggle_mode()?, ModeId::Personal);

        let denied_notes = agent.history().iter().find(|m| {
            m.role == Role::Tool
                && m.tool_calls
                    .first()
                    .is_some_and(|c| c.id == "call_notes_denied")
        });
        assert!(denied_notes.is_some_and(|m| m.content.contains("not enabled")));

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
