// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-19
// Purpose: Policy-confined sub-agent tool (`agent`), live TUI status tracker, and child AgentLoop runner

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::Result;
use rein_core::{EventLog, Message, ModeId, Provider, ToolCall, ToolOutput};
use rein_policy::{
    AuditLog, Capability, PolicyEngine, PolicyRuleSet, SandboxStatus, WorkspaceSandbox,
    local_time_parts,
};
use rein_tools::{
    BashTool, CalendarTool, EditTool, EmailTool, GlobTool, GrepTool, JsonlMemoryStore, MemoryTool,
    NotesTool, ReadTool, TodoReadTool, TodoStore, Tool, ToolError, ToolRegistry, ToolSchema,
    WriteTool,
};
use serde_json::{Value, json};

use crate::agent::runner::{
    AgentLoop, ApprovalAction, ApprovalDecision, ApprovalPrompter, ApprovalRequest,
};
use crate::agent::snapshot::now_unix_secs;
use crate::config::{ReinConfig, build_provider, resolve_model_target};
use crate::modes::{Profile, RuntimeContext};

static SUBAGENT_SEQ: AtomicU64 = AtomicU64::new(1);

/// Default maximum tool rounds allowed for a single sub-agent run.
pub const DEFAULT_SUBAGENT_MAX_ROUNDS: usize = 8;

/// Factory closure producing a `(Provider, model_label)` for an optional model override.
pub type ProviderFactory = Arc<dyn Fn(Option<&str>) -> (Box<dyn Provider>, String) + Send + Sync>;

/// Lifecycle state of a tracked sub-agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentRunState {
    /// Currently executing rounds.
    Running,
    /// Completed current turn and asleep waiting for `send_message` follow-ups.
    Waiting,
    /// Finished successfully.
    Completed,
    /// Exited with an error.
    Failed,
    /// Cancelled / killed before finishing.
    Killed,
}

impl SubagentRunState {
    /// Returns `true` when this sub-agent has reached a terminal state.
    #[must_use]
    pub fn is_done(self) -> bool {
        !matches!(self, Self::Running | Self::Waiting)
    }

    /// Lowercase status label shown in the TUI.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Killed => "killed",
        }
    }
}

/// Live execution state of one sub-agent rendered below the TUI prompt box and in `/tasks`.
#[derive(Debug, Clone)]
pub struct LiveSubagentStatus {
    /// Unique runtime sub-agent identifier.
    pub id: String,
    /// Sub-agent type (`research`, `plan`, `coder`, `personal`).
    pub agent_type: String,
    /// Short 2–5 word role title (`Vendor ELF & Shim Auditor`).
    pub role: String,
    /// Display model name (`gemini-4-argon-medium`).
    pub model_label: String,
    /// Current human-readable activity (`Thinking…`, `Reading file`, `Running command`, `Searching…`).
    pub status: String,
    /// Current lifecycle state (`Running`, `Completed`, `Failed`, `Killed`).
    pub run_state: SubagentRunState,
    /// Unix timestamp (seconds) when this sub-agent started.
    pub started_unix_secs: u64,
    /// Local wall-clock `HH:MM:SS` start time.
    pub started_hms: String,
    /// Local wall-clock `YYYY-MM-DD HH:MM:SS` start timestamp.
    pub started_full: String,
    /// Completion Unix timestamp (seconds), if terminal.
    pub finished_unix_secs: Option<u64>,
    /// Accumulated step/tool log and final report lines for Task Details inspection.
    pub output_lines: Vec<String>,
    /// Per-subagent cancellation handle triggered via `k` in `/tasks` or parent cancellation.
    pub cancel_flag: Arc<AtomicBool>,
}

/// Thread-safe tracker of running and recently finished sub-agents shared with `AppState`.
#[derive(Debug, Clone, Default)]
pub struct SubagentTracker {
    records: Arc<Mutex<Vec<LiveSubagentStatus>>>,
}

impl SubagentTracker {
    /// Creates an empty [`SubagentTracker`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a newly started sub-agent.
    pub fn start(&self, mut status: LiveSubagentStatus) {
        let ts = i64::try_from(status.started_unix_secs).unwrap_or(0);
        let p = local_time_parts(ts);
        if status.started_hms.is_empty() {
            status.started_hms = format!("{:02}:{:02}:{:02}", p.hour, p.minute, p.second);
        }
        if status.started_full.is_empty() {
            status.started_full = format!(
                "{:04}-{:02}-{:02} {}",
                p.year, p.month, p.day, status.started_hms
            );
        }
        if let Ok(mut guard) = self.records.lock() {
            guard.push(status);
        }
    }

    /// Updates the current human-readable activity string for sub-agent `id`.
    pub fn update_status(&self, id: &str, new_status: impl Into<String>) {
        if let Ok(mut guard) = self.records.lock()
            && let Some(entry) = guard.iter_mut().find(|s| s.id == id)
        {
            entry.status = new_status.into();
        }
    }

    /// Appends one progress or output line for sub-agent `id` (bounded to last 500 lines).
    pub fn append_log(&self, id: &str, line: impl Into<String>) {
        if let Ok(mut guard) = self.records.lock()
            && let Some(entry) = guard.iter_mut().find(|s| s.id == id)
        {
            entry.output_lines.push(line.into());
            if entry.output_lines.len() > 500 {
                let excess = entry.output_lines.len() - 500;
                entry.output_lines.drain(..excess);
            }
        }
    }

    /// Marks sub-agent `id` as sleeping (`Waiting`) after finishing a turn while awaiting `send_message`.
    pub fn set_waiting(&self, id: &str, report_text: &str) {
        if let Ok(mut guard) = self.records.lock()
            && let Some(entry) = guard.iter_mut().find(|s| s.id == id)
        {
            if entry.cancel_flag.load(Ordering::Acquire) {
                entry.run_state = SubagentRunState::Killed;
                entry.status = "killed".to_owned();
                entry.finished_unix_secs = Some(now_unix_secs());
                return;
            }
            entry.run_state = SubagentRunState::Waiting;
            entry.status = "waiting".to_owned();
            if !report_text.trim().is_empty() {
                entry
                    .output_lines
                    .extend(report_text.lines().map(str::to_owned));
            }
        }
    }

    /// Marks sub-agent `id` as transitioning back into `Running` on a new message.
    pub fn set_running(&self, id: &str, activity: impl Into<String>) {
        if let Ok(mut guard) = self.records.lock()
            && let Some(entry) = guard.iter_mut().find(|s| s.id == id)
        {
            entry.run_state = SubagentRunState::Running;
            entry.status = activity.into();
        }
    }

    /// Marks sub-agent `id` as finished while retaining its record for inspection/deletion (`x`).
    pub fn finish(&self, id: &str, final_state: SubagentRunState, output_text: &str) {
        if let Ok(mut guard) = self.records.lock()
            && let Some(entry) = guard.iter_mut().find(|s| s.id == id)
        {
            let resolved = if entry.cancel_flag.load(Ordering::Acquire) {
                SubagentRunState::Killed
            } else {
                final_state
            };
            entry.run_state = resolved;
            entry.status = resolved.label().to_owned();
            entry.finished_unix_secs = Some(now_unix_secs());
            if !output_text.trim().is_empty() {
                if !entry.output_lines.is_empty() {
                    entry.output_lines.push(String::new());
                }
                entry
                    .output_lines
                    .extend(output_text.lines().map(str::to_owned));
            }
        }
    }

    /// Requests cancellation of running sub-agent `id`.
    pub fn kill(&self, id: &str) -> Result<LiveSubagentStatus, String> {
        let mut guard = self
            .records
            .lock()
            .map_err(|_| "subagent tracker lock poisoned".to_owned())?;
        let Some(entry) = guard.iter_mut().find(|s| s.id == id) else {
            return Err(format!("unknown subagent '{id}'"));
        };
        if entry.run_state.is_done() {
            return Err(format!(
                "subagent '{id}' is already {}",
                entry.run_state.label()
            ));
        }
        entry.cancel_flag.store(true, Ordering::Release);
        entry.run_state = SubagentRunState::Killed;
        entry.status = "killed".to_owned();
        entry.finished_unix_secs = Some(now_unix_secs());
        Ok(entry.clone())
    }

    /// Triggers cancellation on all currently running sub-agents (e.g., on `Esc` or shutdown).
    pub fn cancel_all(&self) {
        if let Ok(mut guard) = self.records.lock() {
            for entry in guard.iter_mut() {
                if entry.run_state == SubagentRunState::Running {
                    entry.cancel_flag.store(true, Ordering::Release);
                    entry.run_state = SubagentRunState::Killed;
                    entry.status = "killed".to_owned();
                    entry.finished_unix_secs = Some(now_unix_secs());
                }
            }
        }
    }

    /// Removes a finished sub-agent record (`x` action in `/tasks`). Refuses if still running.
    pub fn delete_done(&self, id: &str) -> Result<(), String> {
        let mut guard = self
            .records
            .lock()
            .map_err(|_| "subagent tracker lock poisoned".to_owned())?;
        let Some(pos) = guard.iter().position(|s| s.id == id) else {
            return Err(format!("unknown subagent '{id}'"));
        };
        if !guard[pos].run_state.is_done() {
            return Err(format!(
                "cannot delete running subagent '{id}' (kill first)"
            ));
        }
        guard.remove(pos);
        Ok(())
    }

    /// Returns a snapshot of currently running sub-agents.
    #[must_use]
    pub fn snapshot(&self) -> Vec<LiveSubagentStatus> {
        self.records
            .lock()
            .map(|g| {
                g.iter()
                    .filter(|s| s.run_state == SubagentRunState::Running)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns all tracked sub-agents (both running and finished) for `/tasks` management.
    #[must_use]
    pub fn all_records(&self) -> Vec<LiveSubagentStatus> {
        self.records.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// Looks up one sub-agent record by `id`.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<LiveSubagentStatus> {
        self.records
            .lock()
            .ok()
            .and_then(|g| g.iter().find(|s| s.id == id).cloned())
    }
}

/// Live parent session policy state inherited by spawned sub-agents.
#[derive(Debug, Clone)]
pub struct SubagentSessionState {
    /// Active parent operating mode (`Coding`, `Plan`, or `Personal`).
    pub active_mode: ModeId,
    /// Active parent policy rule set.
    pub policy_rules: PolicyRuleSet,
    /// Whether `--dangerously-bypass-permission` / `--yolo` is active on the parent session.
    pub bypass_permissions: bool,
    /// Merged `commands.json` allowlist patterns from the parent session.
    pub allowed_commands: Vec<String>,
    /// Detected sandbox execution status from the parent session.
    pub sandbox_status: SandboxStatus,
    /// Snapshot of the coordinator conversation history used for `swarm` `fork: "coordinator"`.
    pub coordinator_history: Vec<Message>,
}

impl Default for SubagentSessionState {
    fn default() -> Self {
        let rules = PolicyRuleSet::coding_default();
        Self {
            active_mode: ModeId::Coding,
            policy_rules: rules,
            bypass_permissions: false,
            allowed_commands: Vec::new(),
            sandbox_status: SandboxStatus::Unavailable {
                reason: "unprobed".to_owned(),
            },
            coordinator_history: Vec::new(),
        }
    }
}

/// Configuration and shared handles required by [`AgentTool`] to spawn policy-confined child agents.
#[derive(Clone)]
pub struct SubagentEnv {
    /// Workspace path confinement shared with the parent session.
    pub sandbox: WorkspaceSandbox,
    /// Shared append-only audit log path (`audit.jsonl`).
    pub audit_log_path: PathBuf,
    /// Directory where isolated `subagent-<id>.jsonl` session logs are written.
    pub sessions_dir: PathBuf,
    /// Directory where bash tool outputs are spilled.
    pub bash_outputs_dir: PathBuf,
    /// Personal notes JSONL path.
    pub notes_file: PathBuf,
    /// Personal calendar JSONL path.
    pub calendar_file: PathBuf,
    /// Personal `.ics` sync directory.
    pub calendar_ics_dir: PathBuf,
    /// Personal email JSONL path.
    pub email_file: PathBuf,
    /// Personal Maildir inbox path.
    pub maildir_path: PathBuf,
    /// Dual-scope memory store shared with the parent session.
    pub memory: JsonlMemoryStore,
    /// Session todo store (read-only inside sub-agents).
    pub todo_store: TodoStore,
    /// Parent runtime context (workspace label, sandbox label, date).
    pub runtime_context: RuntimeContext,
    /// Live parent policy state updated by `AgentLoop`.
    pub session_state: Arc<RwLock<SubagentSessionState>>,
    /// Live TUI status tracker.
    pub tracker: SubagentTracker,
    /// Cancellation flag mirrored from the active parent turn.
    pub cancel: Arc<AtomicBool>,
    /// Provider factory creating a fresh [`Provider`] for each sub-agent.
    pub provider_factory: ProviderFactory,
}

impl SubagentEnv {
    /// Builds a default [`ProviderFactory`] backed by [`ReinConfig`] and [`build_provider`].
    #[must_use]
    pub fn config_provider_factory(config: ReinConfig) -> ProviderFactory {
        Arc::new(move |model_override: Option<&str>| {
            let mut cfg = config.clone();
            let mut kind = cfg.resolved_provider();
            if let Some(raw) = model_override.map(str::trim).filter(|s| !s.is_empty()) {
                let (resolved_kind, model_id) = resolve_model_target(&cfg, raw);
                kind = resolved_kind;
                cfg.provider = Some(kind);
                cfg.model = Some(model_id);
            }
            build_provider(kind, &cfg)
        })
    }
}

/// Model-callable `agent` tool spawning a policy-confined sub-agent in its own isolated context.
#[derive(Clone)]
pub struct AgentTool {
    env: SubagentEnv,
}

impl AgentTool {
    /// Creates an [`AgentTool`] wired to `env`.
    #[must_use]
    pub fn new(env: SubagentEnv) -> Self {
        Self { env }
    }

    /// Resolves `(normalized_type, role, target_mode)` from raw tool arguments and active parent mode.
    pub fn resolve_target(
        arguments: &Value,
        parent_mode: ModeId,
    ) -> Result<(String, String, ModeId), ToolError> {
        let raw_type = arguments
            .get("type")
            .or_else(|| arguments.get("subagent_type"))
            .and_then(Value::as_str)
            .unwrap_or("research")
            .trim()
            .to_ascii_lowercase();
        let role = arguments
            .get("role")
            .or_else(|| arguments.get("description"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("Subagent Task")
            .to_owned();

        let (norm_type, target_mode) = match raw_type.as_str() {
            "research" | "explore" | "search" => ("research".to_owned(), ModeId::Plan),
            "plan" | "architect" => ("plan".to_owned(), ModeId::Plan),
            "coder" | "code" | "coding" => ("coder".to_owned(), ModeId::Coding),
            "personal" => ("personal".to_owned(), ModeId::Personal),
            "general" | "self" => {
                let t = match parent_mode {
                    ModeId::Coding => "coder",
                    ModeId::Plan => "research",
                    ModeId::Personal => "personal",
                };
                (t.to_owned(), parent_mode)
            }
            other => {
                return Err(ToolError::InvalidArguments {
                    tool: "agent".to_owned(),
                    reason: format!(
                        "unknown subagent type '{other}' (expected 'research', 'plan', 'coder', 'personal', or 'general')"
                    ),
                });
            }
        };
        Ok((norm_type, role, target_mode))
    }
}

impl Tool for AgentTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "agent".to_owned(),
            description: "Spawn an isolated sub-agent to research, audit, plan, or implement a focused task and return its findings. Emit multiple `agent` tool calls in a single turn to run sub-agents concurrently in parallel. In Plan mode, sub-agents are strictly confined to read-only `research`/`plan`; `coder` sub-agents require Coding mode.".to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["role", "prompt"],
                "properties": {
                    "type": {
                        "type": "string",
                        "enum": ["research", "plan", "coder", "personal", "general"],
                        "description": "Sub-agent capability profile: 'research' (read-only codebase/file analysis, default), 'plan' (read-only architecture planning), 'coder' (file edits & commands; requires active Coding mode), or 'personal'"
                    },
                    "role": {
                        "type": "string",
                        "description": "Short 2-5 word job title distinguishing this sub-agent (e.g. 'Vendor ELF & Shim Auditor', 'Policy Test Researcher')"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Self-contained task instructions and expected deliverable for the sub-agent"
                    },
                    "model": {
                        "type": "string",
                        "description": "Optional model override for this sub-agent"
                    }
                }
            }),
        }
    }

    fn required_capability(&self, arguments: &Value) -> Result<Capability, ToolError> {
        let _prompt = extract_non_empty_prompt(arguments)?;
        let parent_mode = self
            .env
            .session_state
            .read()
            .map_or(ModeId::Coding, |s| s.active_mode);
        let (agent_type, role, target_mode) = Self::resolve_target(arguments, parent_mode)?;
        Ok(Capability::Subagent {
            agent_type,
            role,
            target_mode,
        })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let prompt = extract_non_empty_prompt(arguments)?;
        let state = self
            .env
            .session_state
            .read()
            .map(|s| s.clone())
            .unwrap_or_default();
        let (agent_type, role, target_mode) = Self::resolve_target(arguments, state.active_mode)?;

        // Double-check mode confinement at execution boundary.
        if state.active_mode == ModeId::Plan && target_mode != ModeId::Plan {
            return Err(ToolError::InvalidArguments {
                tool: "agent".to_owned(),
                reason: format!(
                    "Plan mode confines sub-agents to read-only research (cannot run '{agent_type}')"
                ),
            });
        }
        if target_mode == ModeId::Coding && state.active_mode != ModeId::Coding {
            return Err(ToolError::InvalidArguments {
                tool: "agent".to_owned(),
                reason: "coding sub-agents require the active session to be in Coding mode"
                    .to_owned(),
            });
        }

        let model_override = arguments.get("model").and_then(Value::as_str);
        let (mut provider, model_label) = (self.env.provider_factory)(model_override);

        let sub_id = next_subagent_id();
        let _ = std::fs::create_dir_all(&self.env.sessions_dir);
        let log_path = self.env.sessions_dir.join(format!("{sub_id}.jsonl"));

        let mut profile =
            build_subagent_profile(target_mode, &agent_type, &role, &state.allowed_commands);
        // If running in the same mode as the parent, inherit any stricter parent rule flags.
        if target_mode == state.active_mode {
            profile.policy_rules = state.policy_rules.clone();
        }

        let policy = PolicyEngine::with_sandbox_status(
            profile.policy_rules.clone(),
            self.env.sandbox.clone(),
            state.sandbox_status.clone(),
            AuditLog::new(&self.env.audit_log_path),
        )
        .with_bypass_permissions(state.bypass_permissions);

        let registry = build_child_registry(&self.env);
        let mut child_agent = AgentLoop::new(
            EventLog::new(&log_path),
            policy,
            registry,
            self.env.memory.clone(),
            profile,
        )
        .with_allowed_commands(state.allowed_commands)
        .with_runtime_context(self.env.runtime_context.clone());

        let sub_cancel = Arc::new(AtomicBool::new(self.env.cancel.load(Ordering::Acquire)));
        self.env.tracker.start(LiveSubagentStatus {
            id: sub_id.clone(),
            agent_type: agent_type.clone(),
            role: role.clone(),
            model_label: short_model_display(&model_label),
            status: "Thinking…".to_owned(),
            run_state: SubagentRunState::Running,
            started_unix_secs: now_unix_secs(),
            started_hms: String::new(),
            started_full: String::new(),
            finished_unix_secs: None,
            output_lines: vec![format!("[prompt] {prompt}")],
            cancel_flag: Arc::clone(&sub_cancel),
        });

        let mut prompter = SubagentPrompter {
            id: sub_id.clone(),
            tracker: self.env.tracker.clone(),
            parent_cancel: Arc::clone(&self.env.cancel),
            sub_cancel: Arc::clone(&sub_cancel),
        };
        let sub_prompt = format!(
            "[Subagent Role: {role} ({agent_type})]\nTask:\n{prompt}\n\nProvide a concise, well-structured findings/completion report for the parent orchestrator."
        );
        let result = child_agent.run_user_prompt_cancellable(
            &sub_prompt,
            provider.as_mut(),
            &mut prompter,
            &sub_cancel,
        );
        match result {
            Ok(answer) => {
                self.env
                    .tracker
                    .finish(&sub_id, SubagentRunState::Completed, answer.trim());
                Ok(format!("[{agent_type}: {role}]\n{}", answer.trim()))
            }
            Err(err) => {
                self.env
                    .tracker
                    .finish(&sub_id, SubagentRunState::Failed, &err.to_string());
                Err(ToolError::InvalidArguments {
                    tool: "agent".to_owned(),
                    reason: format!("sub-agent '{role}' failed: {err}"),
                })
            }
        }
    }
}

fn extract_non_empty_prompt(arguments: &Value) -> Result<&str, ToolError> {
    arguments
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ToolError::InvalidArguments {
            tool: "agent".to_owned(),
            reason: "missing non-empty string field 'prompt'".to_owned(),
        })
}

pub(crate) fn next_subagent_id() -> String {
    let seq = SUBAGENT_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("subagent-{}-{seq}", std::process::id())
}

pub(crate) fn build_subagent_profile(
    target_mode: ModeId,
    agent_type: &str,
    role: &str,
    allowed_commands: &[String],
) -> Profile {
    let mut profile =
        Profile::for_mode(target_mode).with_allowed_commands(allowed_commands.to_vec());
    // Strip orchestrator/interactive tools from child sub-agents by default so they cannot
    // recurse unconditionally, escalate modes, or overwrite the parent orchestrator's todo checklist.
    profile.allowed_tools.retain(|t| {
        !matches!(
            t.as_str(),
            "agent"
                | "invoke_subagent"
                | "define_subagent"
                | "manage_subagents"
                | "switch_mode"
                | "todo_write"
                | "ask_question"
                | "manage_task"
                | "schedule"
        )
    });
    profile.system_prompt.push_str(&format!(
        "\n\n## Sub-Agent Confinement\n- You are running as an isolated `{agent_type}` sub-agent with role **{role}**.\n- You cannot switch modes or overwrite the parent todo list. Focus strictly on your assigned task and return a clear, evidence-backed report."
    ));
    profile
}

pub(crate) fn build_child_registry(env: &SubagentEnv) -> ToolRegistry {
    build_child_registry_for_sandbox(env, &env.sandbox)
}

pub(crate) fn build_child_registry_for_sandbox(
    env: &SubagentEnv,
    sandbox: &WorkspaceSandbox,
) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadTool::new(sandbox.clone())));
    registry.register(Box::new(WriteTool::new(sandbox.clone())));
    registry.register(Box::new(EditTool::new(sandbox.clone())));
    registry.register(Box::new(GlobTool::new(sandbox.clone())));
    registry.register(Box::new(GrepTool::new(sandbox.clone())));
    registry.register(Box::new(
        BashTool::new(sandbox.clone()).with_output_dir(&env.bash_outputs_dir),
    ));
    registry.register(Box::new(MemoryTool::new(env.memory.clone())));
    registry.register(Box::new(NotesTool::new(&env.notes_file)));
    registry.register(Box::new(
        CalendarTool::new(&env.calendar_file).with_ics_dir(&env.calendar_ics_dir),
    ));
    registry.register(Box::new(
        EmailTool::new(&env.email_file).with_maildir(&env.maildir_path),
    ));
    registry.register(Box::new(TodoReadTool::with_store(env.todo_store.clone())));
    registry
}

pub(crate) fn short_model_display(label: &str) -> String {
    let bare = label.rsplit('/').next().unwrap_or(label);
    bare.to_owned()
}

pub(crate) struct SubagentPrompter {
    pub(crate) id: String,
    pub(crate) tracker: SubagentTracker,
    pub(crate) parent_cancel: Arc<AtomicBool>,
    pub(crate) sub_cancel: Arc<AtomicBool>,
}

impl SubagentPrompter {
    fn sync_parent_cancel(&self) {
        if self.parent_cancel.load(Ordering::Acquire) {
            self.sub_cancel.store(true, Ordering::Release);
        }
    }
}

impl ApprovalPrompter for SubagentPrompter {
    fn confirm(&mut self, _request: &ApprovalRequest) -> Result<ApprovalDecision> {
        self.sync_parent_cancel();
        Ok(ApprovalDecision {
            action: ApprovalAction::Deny,
            feedback: Some(
                "sub-agent runs non-interactively; only policy-allowed or allowlisted actions may run without --yolo".to_owned(),
            ),
        })
    }

    fn on_thinking_delta(&mut self, _delta: &str) {
        self.sync_parent_cancel();
        self.tracker.update_status(&self.id, "Thinking…");
    }

    fn on_delta(&mut self, _delta: &str) {
        self.sync_parent_cancel();
        self.tracker.update_status(&self.id, "Writing report…");
    }

    fn on_tool_start(&mut self, call: &ToolCall) {
        self.sync_parent_cancel();
        let activity = match call.name.as_str() {
            "read" => "Reading file",
            "glob" | "grep" => "Searching files",
            "bash" => "Running command",
            "edit" | "write" => "Editing file",
            "memory" => "Querying memory",
            _ => "Running tool",
        };
        self.tracker.update_status(&self.id, activity);
        self.tracker
            .append_log(&self.id, format!("[tool] {}", call.name));
    }

    fn on_tool_finish(&mut self, call: &ToolCall, output: &ToolOutput) {
        self.sync_parent_cancel();
        let state = if output.is_error { "error" } else { "ok" };
        self.tracker
            .append_log(&self.id, format!("[done] {} ({state})", call.name));
        self.tracker.update_status(&self.id, "Thinking…");
    }
}
