// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: In-process typestate agent loop wiring ContextBuilder, Provider, PolicyEngine, and ToolRegistry

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rein_core::{
    EventLog, MemoryScope, Message, ModeId, Provider, ProviderStreamEvent, Role, SessionEvent,
    ToolCall, ToolSpec, Turn,
};
use rein_policy::{PolicyDecision, PolicyEngine, WorkspaceSandbox, suggest_command_allow_pattern};
use rein_tools::{JsonlMemoryStore, MemoryRecord, ToolRegistry, ToolSchema};

use crate::config::append_allowed_command;
use crate::modes::{ContextBuilder, ModeManager, Profile};

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

/// Interactive callbacks used by [`AgentLoop`] for numbered policy confirmation, streaming deltas, and tool status.
pub trait ApprovalPrompter {
    /// Displays `request` options (`1..=4`) to the user and returns the chosen [`ApprovalDecision`].
    fn confirm(&mut self, request: &ApprovalRequest) -> Result<ApprovalDecision>;

    /// Receives incremental streamed text chunks during a model turn.
    fn on_delta(&mut self, _delta: &str) {}

    /// Receives compact 1-line tool activity summaries (`· read Cargo.toml`).
    fn on_tool_status(&mut self, _status: &str) {}
}

/// Coordinates the single-process agent turn loop across Core, Policy, Tools, and Modes.
pub struct AgentLoop {
    event_log: EventLog,
    full_transcript_path: Option<PathBuf>,
    repo_commands_path: Option<PathBuf>,
    global_commands_path: Option<PathBuf>,
    policy: PolicyEngine,
    registry: ToolRegistry,
    memory: JsonlMemoryStore,
    modes: ModeManager,
    history: Vec<Message>,
    allowed_commands: Vec<String>,
    extra_tools: Vec<String>,
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
            allowed_commands,
            extra_tools: Vec::new(),
        }
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

    /// Toggles between `Coding` and `Personal` profiles via [`ModeManager`].
    pub fn toggle_mode(&mut self) -> Result<ModeId> {
        let next_mode = match self.modes.active().mode {
            ModeId::Coding => ModeId::Personal,
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
        Ok(switched)
    }

    /// Clears active in-memory conversation history (`full-transcript.jsonl` stays preserved).
    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    /// Queries the dual-scope memory store for `scope` and substring `query`.
    pub fn query_memory(&self, scope: MemoryScope, query: &str) -> Result<Vec<MemoryRecord>> {
        self.memory
            .query(scope, query)
            .context("failed to query memory store")
    }

    /// Summarizes active conversation via `provider` with zero trimming, referencing `full-transcript.jsonl`.
    pub fn compact_with_provider(
        &mut self,
        focus: Option<&str>,
        provider: &mut dyn Provider,
    ) -> Result<String> {
        if self.history.is_empty() {
            return Ok("Nothing to compact yet.".to_owned());
        }
        let prior_count = self.history.len();
        let digest = format_history_for_summary(&self.history, focus);
        let events = provider.stream_turn(
            COMPACT_SYSTEM_PROMPT,
            &[Message::new(Role::User, digest)],
            &[],
        )?;
        let summary: String = events
            .into_iter()
            .filter_map(|ev| match ev {
                ProviderStreamEvent::TextDelta(delta) => Some(delta),
                _ => None,
            })
            .collect();
        let summary = summary.trim();
        if summary.is_empty() {
            bail!("provider returned empty compaction summary; history left unchanged");
        }

        let archive_note = self
            .full_transcript_path
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| ".rein/full-transcript.jsonl".to_owned());
        let compacted = Message::new(
            Role::User,
            format!(
                "[Compacted summary of {prior_count} prior messages; complete verbatim turns are archived in `{archive_note}` and can be inspected anytime with `read` or `grep`]\n\n{summary}"
            ),
        );
        self.history = vec![compacted];
        Ok(format!(
            "compacted {prior_count} messages via provider (archive: {archive_note})"
        ))
    }

    /// Hydrates conversation history, active mode, and clean display transcript lines from `session.jsonl`.
    pub fn hydrate_from_log(&mut self) -> Result<Vec<String>> {
        let events = self.event_log.read_all()?;
        let mut display_lines = Vec::new();
        let mut pending_calls: Vec<ToolCall> = Vec::new();
        self.history.clear();

        for ev in events {
            match ev {
                SessionEvent::ModeSwitched { mode } => {
                    let profile = self.build_profile(mode);
                    self.policy.set_rules(profile.policy_rules.clone());
                    self.modes = ModeManager::new(profile);
                }
                SessionEvent::ToolRequested { call } => {
                    display_lines.push(format_tool_status(&call));
                    pending_calls.push(call);
                }
                SessionEvent::ToolCompleted { output } => {
                    let mut tool_msg = Message::new(Role::Tool, output.content);
                    if let Some(matched) = pending_calls
                        .iter()
                        .find(|c| c.id == output.call_id)
                        .cloned()
                    {
                        tool_msg.tool_calls.push(matched);
                    }
                    self.history.push(tool_msg);
                }
                SessionEvent::MessageRecorded { message } => {
                    Self::hydrate_message(
                        &mut self.history,
                        &mut display_lines,
                        &mut pending_calls,
                        message,
                    );
                }
                SessionEvent::StreamDelta { .. }
                | SessionEvent::PolicyEvaluated { .. }
                | SessionEvent::MemoryAccessed { .. } => {}
            }
        }
        Ok(display_lines)
    }

    /// Executes a user prompt through the typestate loop until the model finishes or reaches max rounds.
    pub fn run_user_prompt(
        &mut self,
        prompt: &str,
        provider: &mut dyn Provider,
        prompter: &mut dyn ApprovalPrompter,
    ) -> Result<String> {
        let user_msg = Message::new(Role::User, prompt);
        record_full_message(self.full_transcript_path.as_deref(), &user_msg)?;
        self.history.push(user_msg);
        let mut final_text = String::new();

        for round in 0..MAX_TOOL_ROUNDS {
            let assembled = ContextBuilder::new(self.modes.active(), &self.memory)
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
                    ProviderStreamEvent::ToolUse(call) => requested_calls.push(call),
                    ProviderStreamEvent::Done => {}
                }
            }

            if requested_calls.is_empty() {
                let (_idle, msg) = streaming.finish_response()?;
                final_text = msg.content.clone();
                record_full_message(self.full_transcript_path.as_deref(), &msg)?;
                self.history.push(msg);
                return Ok(final_text);
            }
            execute_tool_batch(
                streaming,
                self.full_transcript_path.as_deref(),
                self.repo_commands_path.as_deref(),
                self.global_commands_path.as_deref(),
                &mut self.policy,
                &mut self.allowed_commands,
                &self.registry,
                &self.modes,
                &mut self.history,
                &streamed_text,
                requested_calls,
                prompter,
            )?;
        }
        Ok(final_text)
    }

    fn build_profile(&self, mode: ModeId) -> Profile {
        Profile::for_mode(mode)
            .with_allowed_commands(self.allowed_commands.clone())
            .with_extra_tools(&self.extra_tools)
    }

    fn hydrate_message(
        history: &mut Vec<Message>,
        display_lines: &mut Vec<String>,
        pending_calls: &mut Vec<ToolCall>,
        mut message: Message,
    ) {
        if message.role == Role::User && message.content == "[tool_result_continuation]" {
            return;
        }
        match message.role {
            Role::User => {
                pending_calls.clear();
                display_lines.push(format!("❯ {}", message.content));
            }
            Role::Assistant => {
                if !pending_calls.is_empty() && message.tool_calls.is_empty() {
                    message.tool_calls = std::mem::take(pending_calls);
                }
                if !message.content.trim().is_empty() {
                    display_lines.push(message.content.clone());
                }
            }
            Role::Tool | Role::System => {}
        }
        history.push(message);
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_tool_batch(
    streaming: Turn<'_, rein_core::Streaming>,
    full_transcript_path: Option<&Path>,
    repo_commands_path: Option<&Path>,
    global_commands_path: Option<&Path>,
    policy: &mut PolicyEngine,
    allowed_commands: &mut Vec<String>,
    registry: &ToolRegistry,
    modes: &ModeManager,
    history: &mut Vec<Message>,
    streamed_text: &str,
    calls: Vec<ToolCall>,
    prompter: &mut dyn ApprovalPrompter,
) -> Result<()> {
    let mut assistant_msg = Message::new(Role::Assistant, streamed_text);
    assistant_msg.tool_calls = calls.clone();
    record_full_message(full_transcript_path, &assistant_msg)?;
    history.push(assistant_msg);

    let mut iter = calls.into_iter();
    let Some(first) = iter.next() else {
        return Ok(());
    };
    let mut idle = execute_awaiting(
        streaming.request_tool(first)?,
        full_transcript_path,
        repo_commands_path,
        global_commands_path,
        policy,
        allowed_commands,
        registry,
        modes,
        history,
        prompter,
    )?;
    for call in iter {
        idle = execute_awaiting(
            idle.request_tool(call)?,
            full_transcript_path,
            repo_commands_path,
            global_commands_path,
            policy,
            allowed_commands,
            registry,
            modes,
            history,
            prompter,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn execute_awaiting<'a>(
    awaiting: Turn<'a, rein_core::AwaitingPolicy>,
    full_transcript_path: Option<&Path>,
    repo_commands_path: Option<&Path>,
    global_commands_path: Option<&Path>,
    policy: &mut PolicyEngine,
    allowed_commands: &mut Vec<String>,
    registry: &ToolRegistry,
    modes: &ModeManager,
    history: &mut Vec<Message>,
    prompter: &mut dyn ApprovalPrompter,
) -> Result<Turn<'a, rein_core::Idle>> {
    let call = awaiting.pending_call().clone();
    prompter.on_tool_status(&format_tool_status(&call));

    let decision =
        registry.evaluate_turn_for_profile(&awaiting, policy, &modes.active().allowed_tools)?;
    let (idle, mut output) = match decision {
        PolicyDecision::Allow { reason } => {
            let executing = awaiting.approve(reason)?;
            registry.execute_approved(executing)?
        }
        PolicyDecision::Ask { prompt: ask_prompt } => {
            let req = build_approval_request(&call, &ask_prompt, policy.sandbox());
            let choice = prompter.confirm(&req)?;
            let feedback = choice
                .feedback
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            match choice.action {
                ApprovalAction::AllowOnce => {
                    let executing = awaiting.approve("user approved once in TUI")?;
                    let (done, mut res) = registry.execute_approved(executing)?;
                    if let Some(fb) = feedback {
                        append_user_feedback(&mut res.content, &fb);
                    }
                    (done, res)
                }
                ApprovalAction::AllowCwd(pattern) => {
                    persist_allow_pattern(repo_commands_path, policy, allowed_commands, &pattern)?;
                    let executing = awaiting.approve(format!("user allowed '{pattern}' in CWD"))?;
                    let (done, mut res) = registry.execute_approved(executing)?;
                    if let Some(fb) = feedback {
                        append_user_feedback(&mut res.content, &fb);
                    }
                    (done, res)
                }
                ApprovalAction::AllowGlobal(pattern) => {
                    persist_allow_pattern(
                        global_commands_path,
                        policy,
                        allowed_commands,
                        &pattern,
                    )?;
                    let executing =
                        awaiting.approve(format!("user allowed '{pattern}' globally"))?;
                    let (done, mut res) = registry.execute_approved(executing)?;
                    if let Some(fb) = feedback {
                        append_user_feedback(&mut res.content, &fb);
                    }
                    (done, res)
                }
                ApprovalAction::Deny => {
                    let reason = match feedback {
                        Some(fb) => format!("user denied in TUI — User feedback: {fb}"),
                        None => "user denied in TUI".to_owned(),
                    };
                    awaiting.deny(reason)?
                }
            }
        }
        PolicyDecision::Deny { reason } => awaiting.deny(reason)?,
    };

    if output.content.is_empty() {
        output.content = "(no output)".to_owned();
    }
    let mut tool_msg = Message::new(Role::Tool, output.content);
    tool_msg.tool_calls.push(call);
    record_full_message(full_transcript_path, &tool_msg)?;
    history.push(tool_msg);
    Ok(idle)
}

fn persist_allow_pattern(
    persist_path: Option<&Path>,
    policy: &mut PolicyEngine,
    allowed_commands: &mut Vec<String>,
    pattern: &str,
) -> Result<()> {
    if let Some(path) = persist_path {
        append_allowed_command(path, pattern)?;
    }
    policy.allow_command_pattern(pattern);
    if !allowed_commands.iter().any(|p| p == pattern) {
        allowed_commands.push(pattern.to_owned());
    }
    Ok(())
}

fn append_user_feedback(content: &mut String, feedback: &str) {
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str("[User feedback: ");
    content.push_str(feedback);
    content.push(']');
}

/// Builds a numbered 4-option [`ApprovalRequest`] for any `PolicyDecision::Ask`.
#[must_use]
pub fn build_approval_request(
    call: &ToolCall,
    base_prompt: &str,
    sandbox: &WorkspaceSandbox,
) -> ApprovalRequest {
    if call.name == "bash" {
        let (program, args) = if let Some(cmd) = call
            .arguments
            .get("command")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            ("bash".to_owned(), vec!["-c".to_owned(), cmd.to_owned()])
        } else {
            let prog = call
                .arguments
                .get("program")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("bash")
                .to_owned();
            let argv = call
                .arguments
                .get("args")
                .and_then(serde_json::Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            (prog, argv)
        };
        let (excerpt, pattern) = suggest_command_allow_pattern(&program, &args, sandbox);
        return ApprovalRequest {
            title: base_prompt.to_owned(),
            excerpt,
            pattern,
        };
    }

    ApprovalRequest {
        title: base_prompt.to_owned(),
        excerpt: format_confirmation_prompt(call, base_prompt),
        pattern: format!("tool:{}", call.name),
    }
}

const COMPACT_SYSTEM_PROMPT: &str = "Summarize the conversation faithfully for context continuity. Preserve file paths, symbols, decisions, constraints, tool outcomes, and open tasks without inventing facts.";

fn record_full_message(path: Option<&Path>, message: &Message) -> Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    EventLog::new(path)
        .append(&SessionEvent::MessageRecorded {
            message: message.clone(),
        })
        .context("failed to append to full-transcript.jsonl")
}

fn format_history_for_summary(history: &[Message], focus: Option<&str>) -> String {
    let mut out = String::from("Summarize every important detail from this conversation:\n\n");
    if let Some(focus_text) = focus.filter(|f| !f.trim().is_empty()) {
        out.push_str("Focus especially on: ");
        out.push_str(focus_text.trim());
        out.push_str("\n\n");
    }
    for msg in history {
        let role_label = match msg.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::Tool => "Tool",
            Role::System => "System",
        };
        out.push_str(role_label);
        if !msg.tool_calls.is_empty() {
            let names: Vec<_> = msg.tool_calls.iter().map(|c| c.name.as_str()).collect();
            out.push_str(" [tools: ");
            out.push_str(&names.join(", "));
            out.push(']');
        }
        out.push_str(": ");
        out.push_str(&msg.content);
        out.push('\n');
    }
    out
}

/// Formats a compact 1-line tool indicator shown inline in the TUI transcript.
#[must_use]
pub fn format_tool_status(call: &ToolCall) -> String {
    let target = call
        .arguments
        .get("path")
        .or_else(|| call.arguments.get("pattern"))
        .or_else(|| call.arguments.get("command"))
        .or_else(|| call.arguments.get("program"))
        .or_else(|| call.arguments.get("action"))
        .or_else(|| call.arguments.get("key"))
        .and_then(serde_json::Value::as_str);
    match target {
        Some(arg) => format!("· {} {}", call.name, truncate_inline(arg, 60)),
        None => format!("· {}", call.name),
    }
}

/// Enhances `PolicyDecision::Ask` prompts with concise inline diff/summary for `write` and `edit`.
#[must_use]
pub fn format_confirmation_prompt(call: &ToolCall, base_prompt: &str) -> String {
    let args = &call.arguments;
    let path = args
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<unknown>");
    match call.name.as_str() {
        "edit" => {
            let old = args
                .get("old")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let new = args
                .get("new")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            format!(
                "edit {path} (-{} / +{})",
                truncate_inline(old, 32),
                truncate_inline(new, 32)
            )
        }
        "write" => {
            let content = args
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            format!(
                "write {path} ({} lines, {} bytes)",
                content.lines().count(),
                content.len()
            )
        }
        _ => base_prompt.to_owned(),
    }
}

fn truncate_inline(raw: &str, max_chars: usize) -> String {
    let single = raw.lines().next().unwrap_or_default().trim();
    if single.chars().count() <= max_chars {
        single.to_owned()
    } else {
        let prefix: String = single.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{prefix}…")
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
    use rein_core::ReplayProvider;
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
        ];
        let mut replay = ReplayProvider::from_events(&events);
        let sandbox = WorkspaceSandbox::new(&dir);
        let profile = Profile::coding();
        let policy = PolicyEngine::new(
            profile.policy_rules.clone(),
            sandbox,
            AuditLog::new(dir.join("audit.jsonl")),
        );
        let full_path = dir.join("full-transcript.jsonl");
        let mut agent = AgentLoop::new(
            EventLog::new(dir.join("session.jsonl")),
            policy,
            ToolRegistry::new(),
            JsonlMemoryStore::new(dir.join("memory.jsonl")),
            profile,
        )
        .with_full_transcript_path(&full_path);

        let mut prompter = RecordingPrompter::new();
        let _ = agent.run_user_prompt("first question", &mut replay, &mut prompter)?;
        let status = agent.compact_with_provider(None, &mut replay)?;
        assert!(status.contains("compacted 2 messages"));
        assert_eq!(agent.history().len(), 1);
        assert!(agent.history()[0].content.contains("concise summary"));
        assert!(agent.history()[0].content.contains("full-transcript.jsonl"));

        let archived = EventLog::new(&full_path).read_all()?;
        assert_eq!(archived.len(), 2);

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
}
