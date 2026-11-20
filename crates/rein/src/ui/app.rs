// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Minimalist borderless TUI state driving Core, Policy, Tools, Modes, Slash commands, and Model picker

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use rein_core::{AvailableModel, ToolCall, ToolOutput, UsageStats};
use rein_tools::{
    McpServerConfig, QuestionAnswer, QuestionSpec, TaskKind, TaskNotification, TaskRegistry,
    TaskStatus, TodoItem, TodoListState, TodoStatus, TodoStore,
};
use serde_json::Value;

use crate::agent::{
    ApprovalAction, ApprovalDecision, ApprovalRequest, HydratedEvent, LiveSubagentStatus,
    SubagentManager, SubagentRunState, SubagentTracker, SwarmManager, TurnCheckpoint,
    now_unix_secs,
};
use crate::commands::{SlashCommand, SlashRegistry};
use crate::config::{ProviderKind, SessionSummary, UiSettings};
use crate::modes::Profile;
use crate::plugins::PluginManager;
use crate::ui::entries::{ToolRunState, TranscriptEntry};
use crate::ui::mentions::{
    MentionCandidate, active_mention_query, collect_mention_candidates, filter_mention_candidates,
};

/// Active interactive 4-option permission prompt with inline `Tab` feedback editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingApproval {
    /// Policy prompt, concise command/action excerpt, and suggested CWD/Global pattern.
    pub request: ApprovalRequest,
    /// Highlighted option (`0`: Allow once, `1`: Allow CWD, `2`: Allow Global, `3`: Deny).
    pub selected: usize,
    /// Whether `Tab` opened inline feedback typing for the highlighted option.
    pub editing_feedback: bool,
    /// Optional user feedback sent back with the tool result.
    pub feedback: String,
}

/// Active interactive model selection popup opened by `/model`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingModelPicker {
    /// Models fetched dynamically from the provider backend.
    pub items: Vec<AvailableModel>,
    /// Currently highlighted model index.
    pub selected: usize,
    /// Target provider owning these server models.
    pub provider_kind: ProviderKind,
}

/// Active interactive session selection popup opened by `/resume`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSessionPicker {
    /// Saved sessions belonging exclusively to the active workspace directory.
    pub items: Vec<SessionSummary>,
    /// Currently highlighted session index.
    pub selected: usize,
}

/// Active step inside the interactive `/rewind` picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewindStep {
    /// Selecting which user turn (or `(current)`) to rewind to.
    SelectTurn,
    /// Choosing whether to rewind messages only or messages and code.
    SelectAction {
        /// Target turn index chosen in [`RewindStep::SelectTurn`].
        turn_index: usize,
    },
}

/// Source kind of an entry displayed in the bottom live strip, `/tasks` manager, or Task Details view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskTargetKind {
    /// Policy-confined child sub-agent.
    Subagent,
    /// Sandboxed background shell command.
    Shell,
    /// One-shot scheduled timer.
    Timer,
    /// Recurring 5-field cron schedule.
    Cron,
}

impl TaskTargetKind {
    /// Human-readable label shown in Task Details (`Agent`, `Shell`, `Timer`, `Cron`).
    #[must_use]
    pub fn display_type(self) -> &'static str {
        match self {
            Self::Subagent => "Agent",
            Self::Shell => "Shell",
            Self::Timer => "Timer",
            Self::Cron => "Cron",
        }
    }
}

/// Unified task/subagent row used by the bottom live strip and `/tasks` manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskListEntry {
    /// Task or subagent identifier.
    pub id: String,
    /// Kind (`Subagent`, `Shell`, `Timer`, `Cron`).
    pub kind: TaskTargetKind,
    /// Primary title (`Agent(type: role)` or command/schedule label).
    pub title: String,
    /// Short trailing status/model/elapsed string.
    pub detail: String,
    /// Local `HH:MM:SS` start timestamp.
    pub started_hms: String,
    /// Local `YYYY-MM-DD HH:MM:SS` start timestamp.
    pub started_full: String,
    /// Status word (`running`, `completed`, `failed`, `killed`, `fired`, `cancelled`).
    pub status_label: String,
    /// `true` while actively running.
    pub is_running: bool,
    /// `true` when terminal failure/kill.
    pub is_error: bool,
}

/// Live inspection screen opened via `Enter` from the bottom strip or `/tasks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDetailsView {
    /// Inspected task or subagent ID.
    pub target_id: String,
    /// Source kind.
    pub kind: TaskTargetKind,
    /// Scroll offset from top when `follow_bottom` is `false`.
    pub scroll: usize,
    /// Whether output follows the bottom automatically (`ctrl+end` enables, `↑`/`pgup` disables).
    pub follow_bottom: bool,
    /// Whether pressing `Esc` returns to `/tasks` manager instead of chat.
    pub return_to_manager: bool,
}

/// Live data snapshot rendered inside the **Task Details** screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDetailsSnapshot {
    /// Command or subagent label.
    pub command: String,
    /// Status label (`running`, `completed`, etc.).
    pub status_label: String,
    /// Type string (`Agent`, `Shell`, `Timer`, `Cron`).
    pub type_label: String,
    /// Local `YYYY-MM-DD HH:MM:SS` start timestamp.
    pub started_full: String,
    /// `true` if currently running.
    pub is_running: bool,
    /// `true` if failed/killed.
    pub is_error: bool,
    /// Output or step lines.
    pub output_lines: Vec<String>,
}

/// Active interactive `/tasks` manager overlay (`k` kills running, `x` deletes done).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingTaskManager {
    /// Currently highlighted row index.
    pub selected: usize,
}

/// Channel payload emitted by `ask_question` to prompt the user via the TUI.
pub type QuestionPromptRequest = (Vec<QuestionSpec>, mpsc::Sender<Option<Vec<QuestionAnswer>>>);
/// Shared active-turn sender bridging [`rein_tools::AskQuestionTool`] to the live Ratatui loop.
pub type SharedQuestionRelay = Arc<Mutex<Option<mpsc::Sender<QuestionPromptRequest>>>>;

/// Active interactive multiple-choice question dialog triggered by `ask_question`.
#[derive(Debug)]
pub struct PendingQuestion {
    /// Ordered list of questions to collect from the user.
    pub questions: Vec<QuestionSpec>,
    /// Index of the question currently shown.
    pub current: usize,
    /// Highlighted row (`0..options.len()` or `options.len()` = `"Other (custom)..."`).
    pub cursor: usize,
    /// Multi-select toggled option indices per question.
    pub selections: Vec<BTreeSet<usize>>,
    /// Optional custom `"Other"` text per question.
    pub custom_inputs: Vec<String>,
    /// Whether currently typing inside the `"Other"` row.
    pub editing_other: bool,
    /// Reply channel returning `Some(answers)` or `None` when cancelled.
    pub reply_tx: mpsc::Sender<Option<Vec<QuestionAnswer>>>,
}

/// Tabs inside the `/config` interactive modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigTab {
    /// Interactive toggleable settings.
    Settings,
    /// Session, provider, sandbox, and config-path status.
    Status,
    /// Effective configuration & repo override summary.
    Config,
    /// Token usage & quota summary.
    Usage,
}

impl ConfigTab {
    /// Ordered tabs for `←/→` switching.
    pub const ALL: [Self; 4] = [Self::Settings, Self::Status, Self::Config, Self::Usage];

    /// Display label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Settings => "Settings",
            Self::Status => "Status",
            Self::Config => "Config",
            Self::Usage => "Usage",
        }
    }
}

/// Interactive `/config` modal state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingConfigModal {
    /// Active tab.
    pub tab: ConfigTab,
    /// Search filter query on `Settings` / `Config`.
    pub search: String,
    /// Whether typing in the search bar.
    pub searching: bool,
    /// Highlighted row index.
    pub selected: usize,
}

/// Tabs inside the `/plugins` interactive modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginsTab {
    /// Browse plugins from registered git marketplaces.
    Discover,
    /// Installed plugins (`enabled` / `disabled`).
    Installed,
    /// Configured git marketplaces (`add` / `refresh` / `remove`).
    Marketplaces,
    /// Plugin load or parse diagnostics.
    Errors,
}

impl PluginsTab {
    /// Ordered tabs for `←/→` switching.
    pub const ALL: [Self; 4] = [
        Self::Discover,
        Self::Installed,
        Self::Marketplaces,
        Self::Errors,
    ];

    /// Display label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Discover => "Discover",
            Self::Installed => "Installed",
            Self::Marketplaces => "Marketplaces",
            Self::Errors => "Errors",
        }
    }
}

/// Interactive `/plugins` modal state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPluginsModal {
    /// Active tab.
    pub tab: PluginsTab,
    /// Live search query.
    pub search: String,
    /// Whether typing inside `Search plugins...`.
    pub searching: bool,
    /// Whether typing inside `Add marketplace (owner/repo or git URL)...`.
    pub adding_marketplace: bool,
    /// Marketplace add input buffer.
    pub marketplace_input: String,
    /// Highlighted row index.
    pub selected: usize,
    /// Optional marketplace filter (`Ctrl+S` on Discover).
    pub marketplace_filter: Option<String>,
    /// Transient status or error banner shown inside the modal.
    pub status_notice: Option<String>,
}

/// Active interactive `/rewind` popup opened by `/rewind`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRewindPicker {
    /// Recorded turn checkpoints in chronological order.
    pub items: Vec<TurnCheckpoint>,
    /// Highlighted row index (`items.len()` represents `(current)` in `SelectTurn`; `0 | 1` in `SelectAction`).
    pub selected: usize,
    /// Current picker step.
    pub step: RewindStep,
}

/// State owned by the in-process minimalist Ratatui frontend.
#[derive(Debug)]
pub struct AppState {
    active_profile: Profile,
    sandbox_summary: String,
    model_name: String,
    model_quota: Option<u8>,
    usage_totals: UsageStats,
    workspace_label: String,
    session_id: String,
    startup_notices: Vec<String>,
    slash_registry: SlashRegistry,
    slash_selected: usize,
    configured_commands: Vec<String>,
    mcp_servers: Vec<McpServerConfig>,
    entries: Vec<TranscriptEntry>,
    active_todo_title: Option<String>,
    active_todos: Vec<TodoItem>,
    todo_store: Option<TodoStore>,
    input: String,
    input_cursor: usize,
    ctrl_c_armed_at: Option<Instant>,
    pending_approval: Option<PendingApproval>,
    pending_model_picker: Option<PendingModelPicker>,
    pending_session_picker: Option<PendingSessionPicker>,
    pending_rewind_picker: Option<PendingRewindPicker>,
    expand_outputs: bool,
    scroll_offset: u16,
    auto_scroll: bool,
    busy: bool,
    busy_status: String,
    spinner_tick: usize,
    streaming_active: bool,
    thinking_active: bool,
    thinking_started: Option<Instant>,
    prompt_history: Vec<String>,
    history_cursor: Option<usize>,
    draft_input: String,
    bypass_permissions: bool,
    subagent_tracker: SubagentTracker,
    subagent_manager: Option<SubagentManager>,
    swarm_manager: Option<SwarmManager>,
    task_registry: TaskRegistry,
    task_notifications_rx: Option<mpsc::Receiver<TaskNotification>>,
    strip_selected: Option<usize>,
    pending_task_manager: Option<PendingTaskManager>,
    pending_task_details: Option<TaskDetailsView>,
    pending_question: Option<PendingQuestion>,
    question_relay: Option<SharedQuestionRelay>,
    mention_candidates: Vec<MentionCandidate>,
    mention_selected: usize,
    mention_dismissed_at: Option<usize>,
    ui_settings: UiSettings,
    repo_override_keys: Vec<String>,
    pending_config_modal: Option<PendingConfigModal>,
    plugin_manager: Option<PluginManager>,
    pending_plugins_modal: Option<PendingPluginsModal>,
    queued_messages: Vec<(String, String)>,
    steering_pending: Vec<String>,
}

/// Returns the vendor group label and sort rank (`0`: Google Gemini, `1`: Anthropic Claude, `2`: OpenAI GPT, `3`: Other).
#[must_use]
pub fn model_vendor_group(model_id: &str) -> (&'static str, u8) {
    let lower = model_id.to_ascii_lowercase();
    if lower.starts_with("gemini-") {
        ("Google Gemini", 0)
    } else if lower.starts_with("claude-") {
        ("Anthropic Claude", 1)
    } else if lower.starts_with("gpt-") || lower.starts_with('o') {
        ("OpenAI GPT", 2)
    } else {
        ("Other Models", 3)
    }
}

/// Returns `true` when `item_id` matches the active `provider/model` label,
/// mapping legacy `*-tiered` configs onto the default `-high` effort entry.
#[must_use]
pub fn is_model_active(current_model: &str, item_id: &str) -> bool {
    let bare = current_model
        .trim()
        .rsplit('/')
        .next()
        .unwrap_or(current_model);
    if bare == item_id {
        return true;
    }
    bare.strip_suffix("-tiered")
        .is_some_and(|stem| item_id == format!("{stem}-high"))
}

fn is_todo_tool(name: &str) -> bool {
    matches!(name, "todo_write" | "todo_read")
}

impl AppState {
    /// Creates a new [`AppState`] starting in `initial` profile.
    #[must_use]
    pub fn new(
        initial: Profile,
        sandbox_summary: impl Into<String>,
        model_name: impl Into<String>,
    ) -> Self {
        Self {
            active_profile: initial,
            sandbox_summary: sandbox_summary.into(),
            model_name: model_name.into(),
            model_quota: None,
            usage_totals: UsageStats::default(),
            workspace_label: ".".to_owned(),
            session_id: String::new(),
            startup_notices: Vec::new(),
            slash_registry: SlashRegistry::with_builtins(),
            slash_selected: 0,
            configured_commands: Vec::new(),
            mcp_servers: Vec::new(),
            entries: Vec::new(),
            active_todo_title: None,
            active_todos: Vec::new(),
            todo_store: None,
            input: String::new(),
            input_cursor: 0,
            ctrl_c_armed_at: None,
            pending_approval: None,
            pending_model_picker: None,
            pending_session_picker: None,
            pending_rewind_picker: None,
            expand_outputs: false,
            scroll_offset: 0,
            auto_scroll: true,
            busy: false,
            busy_status: "Thinking…".to_owned(),
            spinner_tick: 0,
            streaming_active: false,
            thinking_active: false,
            thinking_started: None,
            prompt_history: Vec::new(),
            history_cursor: None,
            draft_input: String::new(),
            bypass_permissions: false,
            subagent_tracker: SubagentTracker::new(),
            subagent_manager: None,
            swarm_manager: None,
            task_registry: TaskRegistry::new(),
            task_notifications_rx: None,
            strip_selected: None,
            pending_task_manager: None,
            pending_task_details: None,
            pending_question: None,
            question_relay: None,
            mention_candidates: Vec::new(),
            mention_selected: 0,
            mention_dismissed_at: None,
            ui_settings: UiSettings::default(),
            repo_override_keys: Vec::new(),
            pending_config_modal: None,
            plugin_manager: None,
            pending_plugins_modal: None,
            queued_messages: Vec::new(),
            steering_pending: Vec::new(),
        }
    }

    /// Attaches the async [`SubagentManager`] for cascading `/tasks` kills and shutdown cleanup.
    #[must_use]
    pub fn with_subagent_manager(mut self, manager: SubagentManager) -> Self {
        self.subagent_manager = Some(manager);
        self
    }

    /// Attaches the [`SwarmManager`] for `/tasks` worker kills and shutdown cleanup.
    #[must_use]
    pub fn with_swarm_manager(mut self, manager: SwarmManager) -> Self {
        self.swarm_manager = Some(manager);
        self
    }

    /// Attaches the shared [`SharedQuestionRelay`] bridging `ask_question` tool calls into the TUI.
    #[must_use]
    pub fn with_question_relay(mut self, relay: SharedQuestionRelay) -> Self {
        self.question_relay = Some(relay);
        self
    }

    /// Attaches initial [`UiSettings`] and repo override keys for `/config`.
    #[must_use]
    pub fn with_ui_settings(
        mut self,
        settings: UiSettings,
        repo_override_keys: Vec<String>,
    ) -> Self {
        self.expand_outputs = settings.verbose_output;
        self.auto_scroll = settings.auto_scroll;
        self.ui_settings = settings;
        self.repo_override_keys = repo_override_keys;
        self
    }

    /// Attaches the [`PluginManager`] for `/plugins` and `@` mention discovery.
    #[must_use]
    pub fn with_plugin_manager(mut self, manager: PluginManager) -> Self {
        self.plugin_manager = Some(manager);
        self
    }

    /// Returns a reference to the attached [`PluginManager`], if configured.
    #[must_use]
    pub fn plugin_manager(&self) -> Option<&PluginManager> {
        self.plugin_manager.as_ref()
    }

    /// Returns current [`UiSettings`].
    #[must_use]
    pub fn ui_settings(&self) -> &UiSettings {
        &self.ui_settings
    }

    /// Returns repo config keys overriding global settings.
    #[must_use]
    pub fn repo_override_keys(&self) -> &[String] {
        &self.repo_override_keys
    }

    /// Returns a reference to the attached [`SubagentManager`], if configured.
    #[must_use]
    pub fn subagent_manager(&self) -> Option<&SubagentManager> {
        self.subagent_manager.as_ref()
    }

    /// Returns a reference to the attached [`SwarmManager`], if configured.
    #[must_use]
    pub fn swarm_manager(&self) -> Option<&SwarmManager> {
        self.swarm_manager.as_ref()
    }

    /// Returns a reference to the shared [`SharedQuestionRelay`], if configured.
    #[must_use]
    pub fn question_relay(&self) -> Option<&SharedQuestionRelay> {
        self.question_relay.as_ref()
    }

    /// Attaches the session [`TaskRegistry`] tracking background bash processes, timers, and crons.
    #[must_use]
    pub fn with_task_registry(mut self, registry: TaskRegistry) -> Self {
        self.task_registry = registry;
        self
    }

    /// Attaches the inbox receiver delivering asynchronous [`TaskNotification`] wakeups.
    #[must_use]
    pub fn with_task_notifications(mut self, rx: mpsc::Receiver<TaskNotification>) -> Self {
        self.task_notifications_rx = Some(rx);
        self
    }

    /// Returns a reference to the active [`TaskRegistry`].
    #[must_use]
    pub fn task_registry(&self) -> &TaskRegistry {
        &self.task_registry
    }

    /// Returns a reference to the shared [`SubagentTracker`].
    #[must_use]
    pub fn subagent_tracker(&self) -> &SubagentTracker {
        &self.subagent_tracker
    }

    /// Non-blockingly drains all queued background task / schedule wakeup notifications.
    pub fn drain_task_notifications(&mut self) -> Vec<TaskNotification> {
        let Some(rx) = &self.task_notifications_rx else {
            return Vec::new();
        };
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(ev);
        }
        out
    }

    /// Returns `true` when currently browsing prompt history.
    #[must_use]
    pub fn history_cursor_active(&self) -> bool {
        self.history_cursor.is_some()
    }

    /// Attaches the shared [`SubagentTracker`] so running sub-agents render live above the prompt box.
    #[must_use]
    pub fn with_subagent_tracker(mut self, tracker: SubagentTracker) -> Self {
        self.subagent_tracker = tracker;
        self
    }

    /// Returns a snapshot of currently running sub-agents.
    #[must_use]
    pub fn active_subagents(&self) -> Vec<LiveSubagentStatus> {
        self.subagent_tracker.snapshot()
    }

    /// Marks whether `--dangerously-bypass-permission` / `--yolo` is active for TUI display.
    #[must_use]
    pub fn with_bypass_permissions(mut self, bypass: bool) -> Self {
        self.bypass_permissions = bypass;
        self
    }

    /// Returns `true` when explicit permission bypass (`--dangerously-bypass-permission` / `--yolo`) is active.
    #[must_use]
    pub fn bypass_permissions(&self) -> bool {
        self.bypass_permissions
    }

    /// Sets the workspace display path shown in the Citadel Owl header (`~/dev/rein`).
    #[must_use]
    pub fn with_workspace_label(mut self, label: impl Into<String>) -> Self {
        self.workspace_label = label.into();
        self
    }

    /// Sets the active session UID hash shown in the header for `--resume <uid>`.
    #[must_use]
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = session_id.into();
        self
    }

    /// Updates the active session UID hash (e.g., on `/clear` or `/resume <uid>`).
    pub fn set_session_id(&mut self, session_id: impl Into<String>) {
        self.session_id = session_id.into();
    }

    /// Returns the active session UID hash.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Attaches non-fatal startup notices rendered in amber beneath the header.
    #[must_use]
    pub fn with_startup_notices(mut self, notices: Vec<String>) -> Self {
        self.startup_notices = notices;
        self
    }

    /// Attaches the loaded [`SlashRegistry`] (built-ins plus `~/.rein/skills` and `.rein/skills`).
    #[must_use]
    pub fn with_slash_registry(mut self, registry: SlashRegistry) -> Self {
        self.slash_registry = registry;
        self
    }

    /// Stores merged `commands.json` entries for `/commands` display.
    #[must_use]
    pub fn with_configured_commands(mut self, commands: Vec<String>) -> Self {
        self.configured_commands = commands;
        self
    }

    /// Stores configured stdio MCP servers for `/mcp` display.
    #[must_use]
    pub fn with_mcp_servers(mut self, servers: Vec<McpServerConfig>) -> Self {
        self.mcp_servers = servers;
        self
    }

    /// Attaches the global session-scoped [`TodoStore`] and loads any active persisted tasks.
    #[must_use]
    pub fn with_todo_store(mut self, store: TodoStore) -> Self {
        if let Ok(state) = store.load_state() {
            self.set_active_todos(state.title, state.todos);
        }
        self.todo_store = Some(store);
        self
    }

    /// Switches the attached [`TodoStore`] to `session_uid` and reloads persisted tasks (or clears if absent/done).
    pub fn sync_todo_session(&mut self, session_uid: &str) {
        if let Some(store) = &self.todo_store {
            store.set_session_uid(session_uid);
            match store.load_state() {
                Ok(state) => self.set_active_todos(state.title, state.todos),
                Err(_) => self.clear_active_todos(),
            }
        } else {
            self.clear_active_todos();
        }
    }

    /// Updates the dynamic persistent bottom checklist, clearing it when empty or all completed.
    pub fn set_active_todos(&mut self, title: Option<String>, todos: Vec<TodoItem>) {
        let all_done = !todos.is_empty() && todos.iter().all(|t| t.status == TodoStatus::Completed);
        if todos.is_empty() || all_done {
            self.clear_active_todos();
            return;
        }
        if let Some(next_title) = title.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()) {
            self.active_todo_title = Some(next_title);
        }
        self.active_todos = todos;
    }

    /// Clears the dynamic persistent bottom checklist.
    pub fn clear_active_todos(&mut self) {
        self.active_todo_title = None;
        self.active_todos.clear();
    }

    /// Returns the optional checklist title displayed in the bottom todo bar.
    #[must_use]
    pub fn active_todo_title(&self) -> Option<&str> {
        self.active_todo_title.as_deref()
    }

    /// Returns the active session's todo checklist rendered persistently above the prompt.
    #[must_use]
    pub fn active_todos(&self) -> &[TodoItem] {
        &self.active_todos
    }

    /// Updates the displayed active [`Profile`].
    pub fn set_active_profile(&mut self, profile: Profile) {
        self.active_profile = profile;
    }

    /// Updates the displayed model identifier in the header and status bar.
    pub fn set_model_name(&mut self, model: impl Into<String>) {
        self.model_name = model.into();
    }

    /// Updates the remaining quota percentage (`0..=100`) shown in the status bar.
    pub fn set_model_quota(&mut self, quota: Option<u8>) {
        self.model_quota = quota;
    }

    /// Returns cached remaining quota percentage for the active model, if known.
    #[must_use]
    pub fn model_quota(&self) -> Option<u8> {
        self.model_quota
    }

    /// Accumulates per-turn token usage into session totals.
    pub fn record_usage(&mut self, turn_usage: UsageStats) {
        self.usage_totals.add_assign_turn(turn_usage);
    }

    /// Returns cumulative session token usage.
    #[must_use]
    pub fn usage_totals(&self) -> UsageStats {
        self.usage_totals
    }

    /// Returns the currently displayed [`Profile`].
    #[must_use]
    pub fn active_profile(&self) -> &Profile {
        &self.active_profile
    }

    /// Returns the concise sandbox label shown in the status bar.
    #[must_use]
    pub fn sandbox_summary(&self) -> &str {
        &self.sandbox_summary
    }

    /// Returns the active model name shown in the status bar.
    #[must_use]
    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    /// Returns the collapsed workspace path shown in the header.
    #[must_use]
    pub fn workspace_label(&self) -> &str {
        &self.workspace_label
    }

    /// Returns concise startup notices displayed beneath the header.
    #[must_use]
    pub fn startup_notices(&self) -> &[String] {
        &self.startup_notices
    }

    /// Returns the loaded [`SlashRegistry`].
    #[must_use]
    pub fn slash_registry(&self) -> &SlashRegistry {
        &self.slash_registry
    }

    /// Replaces the loaded [`SlashRegistry`] (for live plugin command reload).
    pub fn set_slash_registry(&mut self, registry: SlashRegistry) {
        self.slash_registry = registry;
        self.slash_selected = 0;
    }

    /// Returns pre-approved shell commands from global + repo `commands.json`.
    #[must_use]
    pub fn configured_commands(&self) -> &[String] {
        &self.configured_commands
    }

    /// Returns configured MCP servers.
    #[must_use]
    pub fn mcp_servers(&self) -> &[McpServerConfig] {
        &self.mcp_servers
    }

    /// Returns matching slash entries while typing a `/command` token (hidden while busy/asking).
    #[must_use]
    pub fn slash_matches(&self) -> Vec<&SlashCommand> {
        if self.busy || self.pending_approval.is_some() || self.pending_model_picker.is_some() {
            return Vec::new();
        }
        self.slash_registry.matching(&self.input)
    }

    /// Returns the active selected index inside the slash autocomplete popup.
    #[must_use]
    pub fn slash_selected(&self) -> usize {
        self.slash_selected
    }

    /// Returns the currently highlighted slash command, if the autocomplete popup is open.
    #[must_use]
    pub fn selected_slash(&self) -> Option<&SlashCommand> {
        let matches = self.slash_matches();
        matches.get(self.slash_selected).copied()
    }

    /// Moves the slash autocomplete selection up or down with wrap-around.
    pub fn move_slash_selection(&mut self, delta: isize) {
        let count = self.slash_matches().len();
        if count == 0 {
            self.slash_selected = 0;
            return;
        }
        let len = count as isize;
        let current = (self.slash_selected.min(count - 1)) as isize;
        self.slash_selected = (current + delta).rem_euclid(len) as usize;
    }

    /// Returns structured transcript entries.
    #[must_use]
    pub fn entries(&self) -> &[TranscriptEntry] {
        &self.entries
    }

    /// Returns whether truncated tool outputs are currently expanded (`Ctrl+O`).
    #[must_use]
    pub fn expand_outputs(&self) -> bool {
        self.expand_outputs
    }

    /// Toggles collapsed/expanded tool output rendering (`Ctrl+O`) and returns the new state.
    pub fn toggle_expand_outputs(&mut self) -> bool {
        self.expand_outputs = !self.expand_outputs;
        self.expand_outputs
    }

    /// Clears the display transcript and resets scroll position.
    pub fn clear_transcript(&mut self) {
        self.entries.clear();
        self.scroll_offset = 0;
        self.auto_scroll = true;
        self.streaming_active = false;
        self.thinking_active = false;
        self.thinking_started = None;
    }

    fn record_history(&mut self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        if self.prompt_history.last().is_some_and(|last| last == text) {
            return;
        }
        self.prompt_history.push(text.to_owned());
    }

    /// Appends hydrated structured events (used by `--resume` and `/resume`).
    pub fn extend_hydrated(&mut self, hydrated: Vec<HydratedEvent>) {
        self.finalize_thinking();
        self.streaming_active = false;
        for ev in hydrated {
            match ev {
                HydratedEvent::User(text) => {
                    self.record_history(&text);
                    self.entries.push(TranscriptEntry::UserTurn(text));
                }
                HydratedEvent::Assistant(text) => {
                    self.entries.push(TranscriptEntry::AssistantTurn(text));
                }
                HydratedEvent::Note(note) => {
                    self.entries.push(TranscriptEntry::SystemNote(note));
                }
                HydratedEvent::Tool { call, output } => {
                    if is_todo_tool(&call.name) {
                        if output.is_error {
                            self.entries.push(TranscriptEntry::ErrorNote(format!(
                                "{}: {}",
                                call.name, output.content
                            )));
                        } else {
                            self.apply_todo_output(&output.content);
                        }
                    } else {
                        self.entries
                            .push(TranscriptEntry::tool_completed(&call, &output));
                    }
                }
            }
        }
        self.auto_scroll = true;
    }

    /// Appends a user turn prompt to the structured transcript.
    pub fn push_user_turn(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.finalize_thinking();
        self.streaming_active = false;
        self.record_history(&text);
        self.history_cursor = None;
        self.draft_input.clear();
        self.entries.push(TranscriptEntry::UserTurn(text));
        self.auto_scroll = true;
    }

    /// Appends a muted system note (`· ...`).
    pub fn push_system_note(&mut self, note: impl Into<String>) {
        self.streaming_active = false;
        self.entries.push(TranscriptEntry::SystemNote(note.into()));
        self.auto_scroll = true;
    }

    /// Appends a highlighted error note (`! ...`).
    pub fn push_error_note(&mut self, err: impl Into<String>) {
        self.finalize_thinking();
        self.streaming_active = false;
        self.entries.push(TranscriptEntry::ErrorNote(err.into()));
        self.auto_scroll = true;
    }

    /// Records a newly started tool invocation in the transcript.
    pub fn push_tool_running(&mut self, call: &ToolCall) {
        self.finalize_thinking();
        self.streaming_active = false;
        if is_todo_tool(&call.name) {
            self.busy_status = if call.name == "todo_write" {
                self.apply_todo_call_args(&call.arguments);
                "Updating tasks…".to_owned()
            } else {
                "Reading tasks…".to_owned()
            };
            return;
        }
        self.busy_status = format!("Running {}…", call.name);
        self.entries.push(TranscriptEntry::tool_running(call));
        self.auto_scroll = true;
    }

    /// Updates a running tool entry with its output (or appends a completed entry if unmatched).
    pub fn finish_tool_entry(&mut self, call: &ToolCall, output: &ToolOutput) {
        self.streaming_active = false;
        self.busy_status = "Thinking…".to_owned();
        if is_todo_tool(&call.name) {
            if output.is_error {
                self.entries.push(TranscriptEntry::ErrorNote(format!(
                    "{}: {}",
                    call.name, output.content
                )));
                self.auto_scroll = true;
            } else {
                self.apply_todo_output(&output.content);
            }
            return;
        }
        if let Some(existing) = self.entries.iter_mut().rev().find_map(|entry| match entry {
            TranscriptEntry::Tool(tool) if tool.call_id == call.id => Some(tool),
            _ => None,
        }) {
            existing.state = if output.is_error {
                ToolRunState::Error
            } else {
                ToolRunState::Done
            };
            existing.output = output.content.clone();
        } else {
            self.entries
                .push(TranscriptEntry::tool_completed(call, output));
        }
        self.auto_scroll = true;
    }

    fn apply_todo_call_args(&mut self, args: &Value) {
        let title = args
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let Some(raw_items) = args.get("todos").and_then(Value::as_array) else {
            return;
        };
        let mut parsed = Vec::with_capacity(raw_items.len());
        for (idx, item) in raw_items.iter().enumerate() {
            let Some(content) = item.get("content").and_then(Value::as_str) else {
                continue;
            };
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .map_or_else(|| (idx + 1).to_string(), str::to_owned);
            let status = match item.get("status").and_then(Value::as_str) {
                Some("in_progress") => TodoStatus::InProgress,
                Some("completed") => TodoStatus::Completed,
                _ => TodoStatus::Pending,
            };
            parsed.push(TodoItem {
                id,
                content: content.to_owned(),
                status,
            });
        }
        self.set_active_todos(title, parsed);
    }

    fn apply_todo_output(&mut self, content: &str) {
        let Ok(val) = serde_json::from_str::<Value>(content) else {
            return;
        };
        if val.get("cleared").and_then(Value::as_bool) == Some(true) {
            self.clear_active_todos();
            return;
        }
        if let Ok(state) = serde_json::from_value::<TodoListState>(val) {
            self.set_active_todos(state.title, state.todos);
        }
    }

    /// Appends streamed reasoning/thought tokens into a trailing [`TranscriptEntry::Thinking`].
    pub fn append_thinking_delta(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        self.streaming_active = false;
        let elapsed = self
            .thinking_started
            .map_or(0, |started| started.elapsed().as_secs());
        if self.thinking_active
            && let Some(TranscriptEntry::Thinking {
                content,
                elapsed_secs,
                active: true,
            }) = self.entries.last_mut()
        {
            content.push_str(delta);
            *elapsed_secs = elapsed;
        } else {
            self.thinking_active = true;
            self.thinking_started = Some(Instant::now());
            self.entries.push(TranscriptEntry::Thinking {
                content: delta.to_owned(),
                elapsed_secs: 0,
                active: true,
            });
        }
        self.auto_scroll = true;
    }

    /// Finalizes any currently streaming thinking trace and records elapsed seconds.
    pub fn finalize_thinking(&mut self) {
        if !self.thinking_active {
            return;
        }
        self.thinking_active = false;
        let elapsed = self
            .thinking_started
            .take()
            .map_or(1, |started| started.elapsed().as_secs().max(1));
        if let Some(TranscriptEntry::Thinking {
            elapsed_secs,
            active,
            ..
        }) = self.entries.last_mut()
        {
            *active = false;
            *elapsed_secs = (*elapsed_secs).max(elapsed);
        }
    }

    /// Appends streamed assistant tokens cleanly into the trailing [`TranscriptEntry::AssistantTurn`].
    pub fn append_stream_delta(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        self.finalize_thinking();
        if self.streaming_active
            && let Some(TranscriptEntry::AssistantTurn(buf)) = self.entries.last_mut()
        {
            buf.push_str(delta);
        } else {
            self.entries
                .push(TranscriptEntry::AssistantTurn(delta.to_owned()));
            self.streaming_active = true;
        }
        self.auto_scroll = true;
    }

    /// Returns the current input buffer.
    #[must_use]
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Returns the clamped UTF-8 byte offset of the input cursor.
    #[must_use]
    pub fn input_cursor_byte(&self) -> usize {
        self.clamped_cursor()
    }

    fn clamped_cursor(&self) -> usize {
        let mut idx = self.input_cursor.min(self.input.len());
        while idx > 0 && !self.input.is_char_boundary(idx) {
            idx -= 1;
        }
        idx
    }

    /// Replaces the input buffer cleanly (e.g. on `Tab` slash completion).
    pub fn set_input(&mut self, text: impl Into<String>) {
        self.input = normalize_newlines(&text.into());
        self.input_cursor = self.input.len();
        self.slash_selected = 0;
        self.history_cursor = None;
        self.draft_input.clear();
    }

    /// Inserts a typed character at the cursor inside the input buffer.
    pub fn push_input_char(&mut self, ch: char) {
        let normalized = if ch == '\r' { '\n' } else { ch };
        let idx = self.clamped_cursor();
        self.input.insert(idx, normalized);
        self.input_cursor = idx + normalized.len_utf8();
        self.slash_selected = 0;
        self.history_cursor = None;
    }

    /// Inserts pasted multi-character/multiline text at the cursor without submitting.
    pub fn push_input_str(&mut self, raw: &str) {
        let normalized = normalize_newlines(raw);
        if normalized.is_empty() {
            return;
        }
        let idx = self.clamped_cursor();
        self.input.insert_str(idx, &normalized);
        self.input_cursor = idx + normalized.len();
        self.slash_selected = 0;
        self.history_cursor = None;
    }

    /// Removes the character immediately preceding the cursor.
    pub fn pop_input_char(&mut self) {
        let idx = self.clamped_cursor();
        if idx == 0 {
            return;
        }
        if let Some((start, _)) = self.input[..idx].char_indices().next_back() {
            let _ = self.input.remove(start);
            self.input_cursor = start;
        }
        self.slash_selected = 0;
        self.history_cursor = None;
    }

    /// Removes the character at the cursor (`Delete` key).
    pub fn delete_input_char(&mut self) {
        let idx = self.clamped_cursor();
        if idx < self.input.len() {
            let _ = self.input.remove(idx);
        }
        self.slash_selected = 0;
        self.history_cursor = None;
    }

    /// Moves the input cursor one character to the left.
    pub fn move_cursor_left(&mut self) {
        let idx = self.clamped_cursor();
        if let Some((prev, _)) = self.input[..idx].char_indices().next_back() {
            self.input_cursor = prev;
        }
    }

    /// Moves the input cursor one character to the right.
    pub fn move_cursor_right(&mut self) {
        let idx = self.clamped_cursor();
        if let Some(ch) = self.input[idx..].chars().next() {
            self.input_cursor = idx + ch.len_utf8();
        }
    }

    /// Moves the input cursor to the start of its current line.
    pub fn move_cursor_home(&mut self) {
        let idx = self.clamped_cursor();
        let line_start = self.input[..idx].rfind('\n').map_or(0, |pos| pos + 1);
        self.input_cursor = line_start;
    }

    /// Moves the input cursor to the end of its current line.
    pub fn move_cursor_end(&mut self) {
        let idx = self.clamped_cursor();
        let line_end = self.input[idx..]
            .find('\n')
            .map_or(self.input.len(), |rel| idx + rel);
        self.input_cursor = line_end;
    }

    /// Moves the cursor up (`-1`) or down (`+1`) inside multiline input, returning `false` at edges.
    pub fn move_cursor_vertical(&mut self, delta: isize) -> bool {
        if delta == 0 || !self.input.contains('\n') {
            return false;
        }
        let idx = self.clamped_cursor();
        let ranges = logical_line_ranges(&self.input);
        let Some(cur_row) = ranges
            .iter()
            .position(|(start, end)| idx >= *start && idx <= *end)
        else {
            return false;
        };
        let target_row = if delta < 0 {
            if cur_row == 0 {
                return false;
            }
            cur_row - 1
        } else {
            if cur_row + 1 >= ranges.len() {
                return false;
            }
            cur_row + 1
        };
        let col_chars = self.input[ranges[cur_row].0..idx].chars().count();
        let (t_start, t_end) = ranges[target_row];
        let offset = self.input[t_start..t_end]
            .char_indices()
            .nth(col_chars)
            .map_or(t_end - t_start, |(b, _)| b);
        self.input_cursor = t_start + offset;
        true
    }

    /// Takes and clears the current input buffer.
    pub fn take_input(&mut self) -> String {
        self.slash_selected = 0;
        self.history_cursor = None;
        self.input_cursor = 0;
        self.draft_input.clear();
        std::mem::take(&mut self.input)
    }

    /// Records a consecutive `Ctrl+C`; returns `true` when pressed twice within 2 seconds.
    pub fn arm_or_confirm_ctrl_c(&mut self, now: Instant) -> bool {
        let confirmed = self
            .ctrl_c_armed_at
            .is_some_and(|prev| now.duration_since(prev) <= Duration::from_secs(2));
        if confirmed {
            self.ctrl_c_armed_at = None;
            true
        } else {
            self.ctrl_c_armed_at = Some(now);
            false
        }
    }

    /// Clears the pending double-`Ctrl+C` confirmation window when any other key is pressed.
    pub fn clear_ctrl_c_arm(&mut self) {
        self.ctrl_c_armed_at = None;
    }

    /// Returns `true` when one `Ctrl+C` was pressed within the last 2 seconds.
    #[must_use]
    pub fn ctrl_c_exit_armed(&self) -> bool {
        self.ctrl_c_armed_at
            .is_some_and(|prev| prev.elapsed() <= Duration::from_secs(2))
    }

    /// Navigates previously submitted prompts in the input box (`-1` older, `+1` newer), restoring the live draft at the end.
    pub fn navigate_history(&mut self, delta: isize) {
        if self.prompt_history.is_empty() || delta == 0 {
            return;
        }
        let next_idx = if delta < 0 {
            match self.history_cursor {
                None => {
                    self.draft_input = self.input.clone();
                    Some(self.prompt_history.len().saturating_sub(1))
                }
                Some(idx) => Some(idx.saturating_sub(1)),
            }
        } else {
            match self.history_cursor {
                None => return,
                Some(idx) if idx + 1 < self.prompt_history.len() => Some(idx + 1),
                Some(_) => None,
            }
        };
        self.history_cursor = next_idx;
        self.input = match next_idx {
            Some(idx) => self.prompt_history.get(idx).cloned().unwrap_or_default(),
            None => std::mem::take(&mut self.draft_input),
        };
        self.input_cursor = self.input.len();
        self.slash_selected = 0;
    }

    /// Opens the interactive `/model` selector populated from server models, grouped by vendor.
    pub fn open_model_picker(
        &mut self,
        mut items: Vec<AvailableModel>,
        provider_kind: ProviderKind,
        current_model: &str,
    ) {
        items.sort_by_key(|m| model_vendor_group(&m.id).1);
        let selected = items
            .iter()
            .position(|m| is_model_active(current_model, &m.id))
            .unwrap_or(0);
        self.pending_model_picker = Some(PendingModelPicker {
            items,
            selected,
            provider_kind,
        });
    }

    /// Returns any active interactive `/model` picker state.
    #[must_use]
    pub fn pending_model_picker(&self) -> Option<&PendingModelPicker> {
        self.pending_model_picker.as_ref()
    }

    /// Moves selection up/down inside the interactive `/model` picker with wrap-around.
    pub fn move_model_selection(&mut self, delta: isize) {
        let Some(picker) = &mut self.pending_model_picker else {
            return;
        };
        let count = picker.items.len();
        if count == 0 {
            picker.selected = 0;
            return;
        }
        let len = isize::try_from(count).unwrap_or(1);
        let current = isize::try_from(picker.selected.min(count - 1)).unwrap_or(0);
        picker.selected = usize::try_from((current + delta).rem_euclid(len)).unwrap_or(0);
    }

    /// Closes the interactive `/model` picker and returns the chosen `(ProviderKind, AvailableModel)`.
    pub fn take_selected_model(&mut self) -> Option<(ProviderKind, AvailableModel)> {
        let picker = self.pending_model_picker.take()?;
        let chosen = picker.items.into_iter().nth(picker.selected)?;
        Some((picker.provider_kind, chosen))
    }

    /// Dismisses the interactive `/model` picker without making changes.
    pub fn clear_model_picker(&mut self) {
        self.pending_model_picker = None;
    }

    /// Opens the interactive `/resume` selector populated from sessions in the current directory.
    pub fn open_session_picker(&mut self, items: Vec<SessionSummary>) {
        let selected = items
            .iter()
            .position(|s| s.uid == self.session_id)
            .unwrap_or(0);
        self.pending_session_picker = Some(PendingSessionPicker { items, selected });
    }

    /// Returns any active interactive `/resume` session picker state.
    #[must_use]
    pub fn pending_session_picker(&self) -> Option<&PendingSessionPicker> {
        self.pending_session_picker.as_ref()
    }

    /// Moves selection up/down inside the interactive `/resume` picker with wrap-around.
    pub fn move_session_selection(&mut self, delta: isize) {
        let Some(picker) = &mut self.pending_session_picker else {
            return;
        };
        let count = picker.items.len();
        if count == 0 {
            picker.selected = 0;
            return;
        }
        let len = isize::try_from(count).unwrap_or(1);
        let current = isize::try_from(picker.selected.min(count - 1)).unwrap_or(0);
        picker.selected = usize::try_from((current + delta).rem_euclid(len)).unwrap_or(0);
    }

    /// Closes the interactive `/resume` picker and returns the chosen [`SessionSummary`].
    pub fn take_selected_session(&mut self) -> Option<SessionSummary> {
        let picker = self.pending_session_picker.take()?;
        picker.items.into_iter().nth(picker.selected)
    }

    /// Dismisses the interactive `/resume` picker without making changes.
    pub fn clear_session_picker(&mut self) {
        self.pending_session_picker = None;
    }

    /// Opens the interactive `/rewind` selector populated from recorded turn checkpoints.
    pub fn open_rewind_picker(&mut self, items: Vec<TurnCheckpoint>) {
        let selected = items.len();
        self.pending_rewind_picker = Some(PendingRewindPicker {
            items,
            selected,
            step: RewindStep::SelectTurn,
        });
    }

    /// Returns any active interactive `/rewind` picker state.
    #[must_use]
    pub fn pending_rewind_picker(&self) -> Option<&PendingRewindPicker> {
        self.pending_rewind_picker.as_ref()
    }

    /// Moves selection up/down inside the interactive `/rewind` picker with wrap-around.
    pub fn move_rewind_selection(&mut self, delta: isize) {
        let Some(picker) = &mut self.pending_rewind_picker else {
            return;
        };
        let count = match picker.step {
            RewindStep::SelectTurn => picker.items.len() + 1,
            RewindStep::SelectAction { turn_index } => {
                let has_snapshot = picker
                    .items
                    .get(turn_index)
                    .is_some_and(|c| c.snapshot.is_some());
                if has_snapshot { 2 } else { 1 }
            }
        };
        if count == 0 {
            picker.selected = 0;
            return;
        }
        let len = isize::try_from(count).unwrap_or(1);
        let current = isize::try_from(picker.selected.min(count - 1)).unwrap_or(0);
        picker.selected = usize::try_from((current + delta).rem_euclid(len)).unwrap_or(0);
    }

    /// Advances `/rewind` from turn selection to action selection, or returns `(turn_index, restore_code)` on confirm.
    pub fn confirm_rewind_step(&mut self) -> Option<(usize, bool)> {
        let picker = self.pending_rewind_picker.as_mut()?;
        match picker.step {
            RewindStep::SelectTurn => {
                if picker.selected >= picker.items.len() {
                    self.pending_rewind_picker = None;
                    return None;
                }
                let turn_index = picker.selected;
                let has_snapshot = picker
                    .items
                    .get(turn_index)
                    .is_some_and(|c| c.snapshot.is_some());
                picker.step = RewindStep::SelectAction { turn_index };
                picker.selected = usize::from(has_snapshot);
                None
            }
            RewindStep::SelectAction { turn_index } => {
                let has_snapshot = picker
                    .items
                    .get(turn_index)
                    .is_some_and(|c| c.snapshot.is_some());
                let restore_code = has_snapshot && picker.selected == 1;
                self.pending_rewind_picker = None;
                Some((turn_index, restore_code))
            }
        }
    }

    /// Cancels `/rewind` step.
    pub fn cancel_rewind_step(&mut self) {
        let Some(picker) = &mut self.pending_rewind_picker else {
            return;
        };
        match picker.step {
            RewindStep::SelectTurn => {
                self.pending_rewind_picker = None;
            }
            RewindStep::SelectAction { turn_index } => {
                picker.step = RewindStep::SelectTurn;
                picker.selected = turn_index;
            }
        }
    }

    /// Returns currently running subagents and background tasks rendered in the bottom strip below the input box.
    #[must_use]
    pub fn running_strip_items(&self) -> Vec<TaskListEntry> {
        let now = now_unix_secs();
        let mut items: Vec<TaskListEntry> = self
            .subagent_tracker
            .snapshot()
            .into_iter()
            .map(|s| subagent_to_entry(&s, now))
            .collect();
        for t in self.task_registry.list_tasks() {
            if !t.status.is_done() {
                items.push(task_to_entry(&t));
            }
        }
        items
    }

    /// Returns all tracked subagents and background tasks (running + done) for `/tasks` management.
    #[must_use]
    pub fn all_task_entries(&self) -> Vec<TaskListEntry> {
        let now = now_unix_secs();
        let mut items: Vec<TaskListEntry> = self
            .subagent_tracker
            .all_records()
            .into_iter()
            .map(|s| subagent_to_entry(&s, now))
            .collect();
        for t in self.task_registry.list_tasks() {
            items.push(task_to_entry(&t));
        }
        items
    }

    /// Returns the currently highlighted index inside the bottom live task strip, if focused.
    #[must_use]
    pub fn strip_selected(&self) -> Option<usize> {
        let items = self.running_strip_items();
        if items.is_empty() {
            return None;
        }
        self.strip_selected.map(|idx| idx.min(items.len() - 1))
    }

    /// Moves selection down/up inside the bottom live strip (`Up` from index `0` returns focus to input).
    pub fn move_strip_selection(&mut self, delta: isize) -> bool {
        let count = self.running_strip_items().len();
        if count == 0 {
            self.strip_selected = None;
            return false;
        }
        match self.strip_selected {
            None => {
                if delta > 0 {
                    self.strip_selected = Some(0);
                    true
                } else {
                    false
                }
            }
            Some(cur) => {
                if delta < 0 && cur == 0 {
                    self.strip_selected = None;
                    return true;
                }
                let max_idx = count.saturating_sub(1);
                let next = if delta < 0 {
                    cur.saturating_sub(delta.unsigned_abs())
                } else {
                    cur.saturating_add(delta.unsigned_abs()).min(max_idx)
                };
                self.strip_selected = Some(next);
                true
            }
        }
    }

    /// Returns focus from the bottom live strip back to the input box.
    pub fn clear_strip_selection(&mut self) {
        self.strip_selected = None;
    }

    /// Opens the **Task Details** screen for the currently highlighted item in the bottom strip.
    pub fn open_selected_strip_details(&mut self) -> bool {
        let items = self.running_strip_items();
        let Some(idx) = self.strip_selected() else {
            return false;
        };
        let Some(entry) = items.get(idx) else {
            return false;
        };
        self.strip_selected = None;
        self.pending_task_details = Some(TaskDetailsView {
            target_id: entry.id.clone(),
            kind: entry.kind,
            scroll: 0,
            follow_bottom: true,
            return_to_manager: false,
        });
        true
    }

    /// Opens the interactive `/tasks` manager overlay (`k` kills running, `x` deletes done).
    pub fn open_task_manager(&mut self) {
        self.strip_selected = None;
        self.pending_task_details = None;
        self.pending_task_manager = Some(PendingTaskManager { selected: 0 });
    }

    /// Returns any active `/tasks` manager overlay state.
    #[must_use]
    pub fn pending_task_manager(&self) -> Option<&PendingTaskManager> {
        self.pending_task_manager.as_ref()
    }

    /// Moves selection up/down inside `/tasks` manager with wrap-around.
    pub fn move_task_manager_selection(&mut self, delta: isize) {
        let count = self.all_task_entries().len();
        let Some(mgr) = &mut self.pending_task_manager else {
            return;
        };
        if count == 0 {
            mgr.selected = 0;
            return;
        }
        let len = isize::try_from(count).unwrap_or(1);
        let current = isize::try_from(mgr.selected.min(count - 1)).unwrap_or(0);
        mgr.selected = usize::try_from((current + delta).rem_euclid(len)).unwrap_or(0);
    }

    /// Closes the `/tasks` manager overlay.
    pub fn close_task_manager(&mut self) {
        self.pending_task_manager = None;
    }

    /// Opens the **Task Details** view for the selected row in `/tasks` manager.
    pub fn open_selected_manager_details(&mut self) -> bool {
        let items = self.all_task_entries();
        let Some(mgr) = &self.pending_task_manager else {
            return false;
        };
        if items.is_empty() {
            return false;
        }
        let entry = &items[mgr.selected.min(items.len() - 1)];
        self.pending_task_details = Some(TaskDetailsView {
            target_id: entry.id.clone(),
            kind: entry.kind,
            scroll: 0,
            follow_bottom: true,
            return_to_manager: true,
        });
        true
    }

    /// Kills the currently highlighted running task or subagent in `/tasks` (`k` key).
    pub fn kill_selected_managed_task(&mut self) -> Option<Result<String, String>> {
        let items = self.all_task_entries();
        let mgr = self.pending_task_manager.as_ref()?;
        let entry = items.get(mgr.selected.min(items.len().saturating_sub(1)))?;
        let res = match entry.kind {
            TaskTargetKind::Subagent => {
                if let Some(worker_name) = entry
                    .title
                    .strip_prefix("Agent(swarm: ")
                    .and_then(|rest| rest.strip_suffix(')'))
                    && let Some(sw_mgr) = &self.swarm_manager
                    && sw_mgr.kill_worker("coordinator", worker_name).is_ok()
                {
                    Ok(format!("killed swarm worker {worker_name}"))
                } else if let Some(sub_mgr) = &self.subagent_manager
                    && sub_mgr.kill_tree(&entry.id).is_ok()
                {
                    Ok(format!("killed subagent tree {}", entry.title))
                } else {
                    self.subagent_tracker
                        .kill(&entry.id)
                        .map(|s| format!("killed subagent {}", s.role))
                }
            }
            TaskTargetKind::Shell | TaskTargetKind::Timer | TaskTargetKind::Cron => self
                .task_registry
                .kill_task(&entry.id)
                .map(|t| format!("killed {} ({})", t.id, t.label)),
        };
        Some(res)
    }

    /// Deletes the currently highlighted finished task or subagent from `/tasks` (`x` key).
    pub fn delete_selected_done_task(&mut self) -> Option<Result<String, String>> {
        let items = self.all_task_entries();
        let selected = self.pending_task_manager.as_ref()?.selected;
        if items.is_empty() {
            return None;
        }
        let idx = selected.min(items.len() - 1);
        let entry = &items[idx];
        let res = match entry.kind {
            TaskTargetKind::Subagent => self
                .subagent_tracker
                .delete_done(&entry.id)
                .map(|()| format!("deleted subagent {}", entry.title)),
            TaskTargetKind::Shell | TaskTargetKind::Timer | TaskTargetKind::Cron => self
                .task_registry
                .delete_done_task(&entry.id)
                .map(|()| format!("deleted {}", entry.id)),
        };
        let remaining = self.all_task_entries().len();
        if let Some(mgr) = &mut self.pending_task_manager {
            if remaining == 0 {
                mgr.selected = 0;
            } else {
                mgr.selected = mgr.selected.min(remaining - 1);
            }
        }
        Some(res)
    }

    /// Returns any active **Task Details** screen state.
    #[must_use]
    pub fn pending_task_details(&self) -> Option<&TaskDetailsView> {
        self.pending_task_details.as_ref()
    }

    /// Builds a live [`TaskDetailsSnapshot`] for the currently open Task Details screen.
    #[must_use]
    pub fn task_details_snapshot(&self) -> Option<TaskDetailsSnapshot> {
        let details = self.pending_task_details.as_ref()?;
        match details.kind {
            TaskTargetKind::Subagent => {
                let s = self.subagent_tracker.get(&details.target_id)?;
                Some(TaskDetailsSnapshot {
                    command: format!("Agent({}: {})", s.agent_type, s.role),
                    status_label: s.run_state.label().to_owned(),
                    type_label: details.kind.display_type().to_owned(),
                    started_full: s.started_full,
                    is_running: !s.run_state.is_done(),
                    is_error: matches!(
                        s.run_state,
                        SubagentRunState::Failed | SubagentRunState::Killed
                    ),
                    output_lines: s.output_lines,
                })
            }
            TaskTargetKind::Shell | TaskTargetKind::Timer | TaskTargetKind::Cron => {
                let t = self
                    .task_registry
                    .task_status(&details.target_id, 500)
                    .ok()?;
                let lines = self.task_registry.task_output_lines(&details.target_id);
                Some(TaskDetailsSnapshot {
                    command: t.label,
                    status_label: t.status.label().to_owned(),
                    type_label: details.kind.display_type().to_owned(),
                    started_full: t.started_full,
                    is_running: !t.status.is_done(),
                    is_error: matches!(
                        t.status,
                        TaskStatus::Failed | TaskStatus::Killed | TaskStatus::Cancelled
                    ),
                    output_lines: lines,
                })
            }
        }
    }

    /// Scrolls the active **Task Details** output pane vertically by `delta` lines.
    pub fn scroll_task_details(&mut self, delta: isize, total_lines: usize, view_height: usize) {
        let Some(details) = &mut self.pending_task_details else {
            return;
        };
        let max_scroll = total_lines.saturating_sub(view_height.max(1));
        let base = if details.follow_bottom {
            max_scroll
        } else {
            details.scroll.min(max_scroll)
        };
        let next = if delta < 0 {
            base.saturating_sub(delta.unsigned_abs())
        } else {
            base.saturating_add(delta.unsigned_abs()).min(max_scroll)
        };
        details.scroll = next;
        details.follow_bottom = next >= max_scroll;
    }

    /// Jumps to the top of **Task Details** output (`Ctrl+Home`).
    pub fn task_details_home(&mut self) {
        if let Some(details) = &mut self.pending_task_details {
            details.scroll = 0;
            details.follow_bottom = false;
        }
    }

    /// Jumps to the bottom of **Task Details** output and re-enables live follow (`Ctrl+End`).
    pub fn task_details_end(&mut self) {
        if let Some(details) = &mut self.pending_task_details {
            details.follow_bottom = true;
        }
    }

    /// Closes the **Task Details** view back to `/tasks` manager or chat.
    pub fn close_task_details(&mut self) {
        if let Some(details) = self.pending_task_details.take()
            && details.return_to_manager
        {
            self.pending_task_manager = Some(PendingTaskManager { selected: 0 });
        }
    }

    /// Opens an interactive `ask_question` picker dialog.
    pub fn open_question_picker(
        &mut self,
        questions: Vec<QuestionSpec>,
        reply_tx: mpsc::Sender<Option<Vec<QuestionAnswer>>>,
    ) {
        let len = questions.len();
        self.pending_question = Some(PendingQuestion {
            questions,
            current: 0,
            cursor: 0,
            selections: vec![BTreeSet::new(); len],
            custom_inputs: vec![String::new(); len],
            editing_other: false,
            reply_tx,
        });
    }

    /// Returns any active `ask_question` dialog state.
    #[must_use]
    pub fn pending_question(&self) -> Option<&PendingQuestion> {
        self.pending_question.as_ref()
    }

    /// Moves option cursor up/down in the active `ask_question` dialog.
    pub fn move_question_cursor(&mut self, delta: isize) {
        let Some(pq) = &mut self.pending_question else {
            return;
        };
        let Some(q) = pq.questions.get(pq.current) else {
            return;
        };
        // Options + 1 final "Other (custom)..." row.
        let count = q.options.len() + 1;
        let len = isize::try_from(count).unwrap_or(1);
        let cur = isize::try_from(pq.cursor.min(count - 1)).unwrap_or(0);
        pq.cursor = usize::try_from((cur + delta).rem_euclid(len)).unwrap_or(0);
        pq.editing_other = pq.cursor == q.options.len();
    }

    /// Toggles the highlighted option (Space in multi-select) or enters `"Other"` typing mode.
    pub fn toggle_question_option(&mut self) {
        let Some(pq) = &mut self.pending_question else {
            return;
        };
        let Some(q) = pq.questions.get(pq.current) else {
            return;
        };
        if pq.cursor >= q.options.len() {
            pq.editing_other = true;
            return;
        }
        let set = &mut pq.selections[pq.current];
        if q.is_multi_select {
            if !set.remove(&pq.cursor) {
                set.insert(pq.cursor);
            }
        } else {
            set.clear();
            set.insert(pq.cursor);
        }
    }

    /// Appends one character to the custom `"Other"` response for the current question.
    pub fn push_question_other_char(&mut self, ch: char) {
        let Some(pq) = &mut self.pending_question else {
            return;
        };
        if let Some(q) = pq.questions.get(pq.current)
            && pq.cursor == q.options.len()
        {
            pq.editing_other = true;
            pq.custom_inputs[pq.current].push(ch);
        }
    }

    /// Deletes one character from the custom `"Other"` response for the current question.
    pub fn pop_question_other_char(&mut self) {
        let Some(pq) = &mut self.pending_question else {
            return;
        };
        if pq.editing_other {
            pq.custom_inputs[pq.current].pop();
        }
    }

    /// Confirms the current question and either advances or delivers all answers back to `ask_question`.
    pub fn confirm_question_step(&mut self) {
        let Some(pq) = &mut self.pending_question else {
            return;
        };
        let Some(q) = pq.questions.get(pq.current) else {
            return;
        };
        if !q.is_multi_select && pq.cursor < q.options.len() && pq.selections[pq.current].is_empty()
        {
            pq.selections[pq.current].insert(pq.cursor);
        }
        if pq.current + 1 < pq.questions.len() {
            pq.current += 1;
            pq.cursor = 0;
            pq.editing_other = false;
            return;
        }

        let Some(done) = self.pending_question.take() else {
            return;
        };
        let answers: Vec<QuestionAnswer> = done
            .questions
            .iter()
            .enumerate()
            .map(|(idx, q)| {
                let selected: Vec<String> = done.selections[idx]
                    .iter()
                    .filter_map(|&opt_idx| q.options.get(opt_idx).cloned())
                    .collect();
                let custom = done.custom_inputs[idx].trim();
                QuestionAnswer {
                    question: q.question.clone(),
                    selected,
                    custom_text: (!custom.is_empty()).then(|| custom.to_owned()),
                }
            })
            .collect();
        let _ = done.reply_tx.send(Some(answers));
    }

    /// Cancels any open `ask_question` dialog cleanly.
    pub fn cancel_question_picker(&mut self) {
        if let Some(done) = self.pending_question.take() {
            let _ = done.reply_tx.send(None);
        }
    }

    /// Truncates the rendered transcript entries back to the `turn_index`-th user prompt.
    pub fn truncate_entries_to_user_turn(&mut self, turn_index: usize) {
        let mut seen = 0usize;
        let mut cut_idx = self.entries.len();
        for (idx, entry) in self.entries.iter().enumerate() {
            if matches!(entry, TranscriptEntry::UserTurn(_)) {
                if seen == turn_index {
                    cut_idx = idx;
                    break;
                }
                seen += 1;
            }
        }
        self.entries.truncate(cut_idx);
        self.auto_scroll = true;
    }

    /// Sets or clears an interactive 4-option approval prompt from the policy layer.
    pub fn set_pending_approval(&mut self, request: Option<ApprovalRequest>) {
        self.pending_approval = request.map(|req| PendingApproval {
            request: req,
            selected: 0,
            editing_feedback: false,
            feedback: String::new(),
        });
    }

    /// Returns any active interactive permission dialog state.
    #[must_use]
    pub fn pending_approval(&self) -> Option<&PendingApproval> {
        self.pending_approval.as_ref()
    }

    /// Moves the highlighted option inside the active approval dialog with wrap-around (`0..4`).
    pub fn move_approval_selection(&mut self, delta: isize) {
        let Some(pending) = &mut self.pending_approval else {
            return;
        };
        let count = 4isize;
        let current = isize::try_from(pending.selected).unwrap_or(0);
        let next = (current + delta).rem_euclid(count);
        pending.selected = usize::try_from(next).unwrap_or(0);
    }

    /// Toggles inline feedback editing (`Tab`) for whichever approval option is highlighted.
    pub fn toggle_approval_feedback(&mut self) {
        if let Some(pending) = &mut self.pending_approval {
            pending.editing_feedback = !pending.editing_feedback;
        }
    }

    /// Appends a character to the approval feedback buffer when editing via `Tab`.
    pub fn push_approval_feedback_char(&mut self, ch: char) {
        if let Some(pending) = &mut self.pending_approval {
            pending.feedback.push(ch);
        }
    }

    /// Pops the trailing character from the approval feedback buffer.
    pub fn pop_approval_feedback_char(&mut self) {
        if let Some(pending) = &mut self.pending_approval {
            let _ = pending.feedback.pop();
        }
    }

    /// Resolves and clears the active approval prompt, updating session status on CWD/Global persistence.
    pub fn take_approval_decision(
        &mut self,
        explicit_idx: Option<usize>,
    ) -> Option<ApprovalDecision> {
        let pending = self.pending_approval.take()?;
        let choice = explicit_idx.unwrap_or(pending.selected);
        let action = match choice {
            0 => ApprovalAction::AllowOnce,
            1 => {
                let pat = pending.request.pattern.clone();
                if !self.configured_commands.contains(&pat) {
                    self.configured_commands.push(pat.clone());
                }
                ApprovalAction::AllowCwd(pat)
            }
            2 => {
                let pat = pending.request.pattern.clone();
                if !self.configured_commands.contains(&pat) {
                    self.configured_commands.push(pat.clone());
                }
                ApprovalAction::AllowGlobal(pat)
            }
            _ => ApprovalAction::Deny,
        };
        let feedback = if pending.feedback.trim().is_empty() {
            None
        } else {
            Some(pending.feedback.trim().to_owned())
        };
        Some(ApprovalDecision { action, feedback })
    }

    /// Marks whether a background model turn, login, or compaction is running.
    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
        if !busy {
            self.finalize_thinking();
            self.streaming_active = false;
            self.busy_status = "Thinking…".to_owned();
        }
    }

    /// Updates the concise status label next to the animated busy spinner.
    pub fn set_busy_status(&mut self, status: impl Into<String>) {
        self.busy_status = status.into();
    }

    /// Returns the active busy status message (`Thinking…`, `Running bash…`).
    #[must_use]
    pub fn busy_status(&self) -> &str {
        &self.busy_status
    }

    /// Advances the animated spinner frame counter while waiting on background work.
    pub fn advance_spinner(&mut self) {
        self.spinner_tick = self.spinner_tick.wrapping_add(1);
    }

    /// Returns the current spinner animation frame index.
    #[must_use]
    pub fn spinner_tick(&self) -> usize {
        self.spinner_tick
    }

    /// Returns `true` while a turn, login, or compaction is running in the background.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// Returns the effective vertical scroll offset against `max_scroll`.
    #[must_use]
    pub fn effective_scroll(&self, max_scroll: u16) -> u16 {
        if self.auto_scroll {
            max_scroll
        } else {
            self.scroll_offset.min(max_scroll)
        }
    }

    /// Re-enables sticky scroll-to-bottom tracking.
    pub fn scroll_to_bottom(&mut self) {
        self.auto_scroll = true;
    }

    /// Adjusts transcript scroll offset safely against the current viewport max scroll.
    pub fn scroll_by(&mut self, delta: i16, max_scroll: u16) {
        let base = if self.auto_scroll {
            max_scroll
        } else {
            self.scroll_offset.min(max_scroll)
        };
        if delta < 0 {
            self.scroll_offset = base.saturating_sub(delta.unsigned_abs());
            self.auto_scroll = false;
        } else {
            self.scroll_offset = base.saturating_add(delta.unsigned_abs()).min(max_scroll);
            if self.scroll_offset >= max_scroll {
                self.auto_scroll = true;
            }
        }
    }
}

fn normalize_newlines(raw: &str) -> String {
    raw.replace("\r\n", "\n").replace('\r', "\n")
}

fn subagent_to_entry(sub: &LiveSubagentStatus, now_secs: u64) -> TaskListEntry {
    let elapsed = sub
        .finished_unix_secs
        .unwrap_or(now_secs)
        .saturating_sub(sub.started_unix_secs);
    let is_running = !sub.run_state.is_done();
    let is_error = matches!(
        sub.run_state,
        SubagentRunState::Failed | SubagentRunState::Killed
    );
    let detail = if is_running {
        format!("{} · {} · {elapsed}s", sub.model_label, sub.status)
    } else {
        format!(
            "{} · {} · {elapsed}s",
            sub.model_label,
            sub.run_state.label()
        )
    };
    TaskListEntry {
        id: sub.id.clone(),
        kind: TaskTargetKind::Subagent,
        title: format!("Agent({}: {})", sub.agent_type, sub.role),
        detail,
        started_hms: sub.started_hms.clone(),
        started_full: sub.started_full.clone(),
        status_label: sub.run_state.label().to_owned(),
        is_running,
        is_error,
    }
}

fn task_to_entry(task: &rein_tools::TaskSummary) -> TaskListEntry {
    let kind = match task.kind {
        TaskKind::Shell => TaskTargetKind::Shell,
        TaskKind::Timer => TaskTargetKind::Timer,
        TaskKind::Cron => TaskTargetKind::Cron,
    };
    let is_running = !task.status.is_done();
    let is_error = matches!(
        task.status,
        TaskStatus::Failed | TaskStatus::Killed | TaskStatus::Cancelled
    );
    TaskListEntry {
        id: task.id.clone(),
        kind,
        title: task.label.clone(),
        detail: task.status.label().to_owned(),
        started_hms: task.started_hms.clone(),
        started_full: task.started_full.clone(),
        status_label: task.status.label().to_owned(),
        is_running,
        is_error,
    }
}

fn logical_line_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    for (idx, ch) in text.char_indices() {
        if ch == '\n' {
            ranges.push((start, idx));
            start = idx + 1;
        }
    }
    ranges.push((start, text.len()));
    ranges
}

/// Single row displayed in `/config` Settings/Config tabs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigSettingRow {
    /// Stable setting identifier.
    pub key: &'static str,
    /// Human-readable label.
    pub label: &'static str,
    /// Formatted current value.
    pub value: String,
    /// Whether this key is overridden in `<workspace>/.rein/config.json`.
    pub repo_overridden: bool,
}

impl AppState {
    // ── @ Mentions ───────────────────────────────────────────────────────────

    /// Refreshes cached `@` autocomplete candidates from `cwd`, agents, plugins, and MCP servers.
    pub fn refresh_mention_candidates(&mut self, cwd: &std::path::Path) {
        let mut extra_agents = Vec::new();
        if let Some(sub_mgr) = &self.subagent_manager {
            for def in sub_mgr.list_definitions() {
                extra_agents.push((def.name, def.description));
            }
        }
        let mut plugins_list = Vec::new();
        if let Some(pm) = &self.plugin_manager {
            let bundle = pm.collect_enabled_components();
            for ag in bundle.agents {
                extra_agents.push((ag.name, ag.description));
            }
            for rec in pm.installed_plugins() {
                if rec.enabled {
                    plugins_list.push((rec.name, rec.description));
                }
            }
        }
        let mcp_names: Vec<String> = self.mcp_servers.iter().map(|s| s.name.clone()).collect();
        self.mention_candidates = collect_mention_candidates(
            cwd,
            self.ui_settings.respect_gitignore_picker,
            &extra_agents,
            &plugins_list,
            &mcp_names,
        );
        self.mention_selected = 0;
    }

    /// Returns active `@query` matches when the `@` popup should be shown.
    #[must_use]
    pub fn active_mention_matches(&self) -> Option<(usize, String, Vec<&MentionCandidate>)> {
        if self.busy || self.input.starts_with('/') {
            return None;
        }
        let (at_idx, query) = active_mention_query(&self.input, self.input_cursor)?;
        if self.mention_dismissed_at == Some(at_idx) {
            return None;
        }
        let matches = filter_mention_candidates(&self.mention_candidates, &query);
        if matches.is_empty() {
            return None;
        }
        Some((at_idx, query, matches))
    }

    /// Returns the highlighted `@` candidate index.
    #[must_use]
    pub fn mention_selected(&self) -> usize {
        self.mention_selected
    }

    /// Moves the `@` popup selection up or down with wrap-around.
    pub fn move_mention_selection(&mut self, delta: isize) {
        let count = self.active_mention_matches().map_or(0, |(_, _, m)| m.len());
        if count == 0 {
            self.mention_selected = 0;
            return;
        }
        let len = count as isize;
        let current = (self.mention_selected.min(count - 1)) as isize;
        self.mention_selected = (current + delta).rem_euclid(len) as usize;
    }

    /// Inserts the highlighted `@` candidate into the input buffer and advances the cursor.
    pub fn accept_selected_mention(&mut self) -> bool {
        let Some((at_idx, _, matches)) = self.active_mention_matches() else {
            return false;
        };
        let Some(chosen) = matches.get(self.mention_selected).copied() else {
            return false;
        };
        let insert = format!("@{} ", chosen.insert_text);
        let tail = self.input[self.input_cursor..].to_owned();
        self.input.truncate(at_idx);
        self.input.push_str(&insert);
        self.input_cursor = self.input.len();
        self.input.push_str(&tail);
        self.mention_selected = 0;
        self.mention_dismissed_at = None;
        true
    }

    /// Dismisses the `@` popup at the current `@` token position.
    pub fn dismiss_mention_popup(&mut self) {
        if let Some((at_idx, _)) = active_mention_query(&self.input, self.input_cursor) {
            self.mention_dismissed_at = Some(at_idx);
        }
    }

    /// Clears `@` dismissal if the cursor is no longer on that `@` token.
    pub fn sync_mention_dismissal(&mut self) {
        if let Some(dismissed) = self.mention_dismissed_at {
            match active_mention_query(&self.input, self.input_cursor) {
                Some((at_idx, _)) if at_idx == dismissed => {}
                _ => self.mention_dismissed_at = None,
            }
        }
    }

    // ── /config modal ────────────────────────────────────────────────────────

    /// Opens the interactive `/config` modal.
    pub fn open_config_modal(&mut self) {
        self.pending_config_modal = Some(PendingConfigModal {
            tab: ConfigTab::Settings,
            search: String::new(),
            searching: false,
            selected: 0,
        });
    }

    /// Returns the active `/config` modal state.
    #[must_use]
    pub fn pending_config_modal(&self) -> Option<&PendingConfigModal> {
        self.pending_config_modal.as_ref()
    }

    /// Mutable access to `/config` modal state.
    pub fn pending_config_modal_mut(&mut self) -> Option<&mut PendingConfigModal> {
        self.pending_config_modal.as_mut()
    }

    /// Closes `/config` modal.
    pub fn close_config_modal(&mut self) {
        self.pending_config_modal = None;
    }

    /// Returns filtered setting rows for `/config`.
    #[must_use]
    pub fn config_setting_rows(&self) -> Vec<ConfigSettingRow> {
        let on_off = |b: bool| if b { "on" } else { "off" }.to_owned();
        let has_override = |k: &str| self.repo_override_keys.iter().any(|r| r == k);
        let all = vec![
            ConfigSettingRow {
                key: "auto_compact",
                label: "Auto-compact",
                value: on_off(self.ui_settings.auto_compact),
                repo_overridden: false,
            },
            ConfigSettingRow {
                key: "thinking_mode",
                label: "Thinking mode",
                value: on_off(self.ui_settings.thinking_mode),
                repo_overridden: false,
            },
            ConfigSettingRow {
                key: "verbose_output",
                label: "Verbose output",
                value: on_off(self.ui_settings.verbose_output),
                repo_overridden: false,
            },
            ConfigSettingRow {
                key: "show_turn_duration",
                label: "Show turn duration",
                value: on_off(self.ui_settings.show_turn_duration),
                repo_overridden: false,
            },
            ConfigSettingRow {
                key: "auto_scroll",
                label: "Auto-scroll",
                value: on_off(self.ui_settings.auto_scroll),
                repo_overridden: false,
            },
            ConfigSettingRow {
                key: "respect_gitignore_picker",
                label: "Respect .gitignore in file picker",
                value: on_off(self.ui_settings.respect_gitignore_picker),
                repo_overridden: false,
            },
            ConfigSettingRow {
                key: "default_permission_mode",
                label: "Default permission mode",
                value: self.ui_settings.default_permission_mode.clone(),
                repo_overridden: false,
            },
            ConfigSettingRow {
                key: "editor_mode",
                label: "Editor mode",
                value: self.ui_settings.editor_mode.clone(),
                repo_overridden: false,
            },
            ConfigSettingRow {
                key: "queued_messages",
                label: "Queued Messages",
                value: if self.ui_settings.queued_messages == "queue" {
                    "Queue".to_owned()
                } else {
                    "Send Immediately".to_owned()
                },
                repo_overridden: false,
            },
            ConfigSettingRow {
                key: "show_tips",
                label: "Show tips",
                value: on_off(self.ui_settings.show_tips),
                repo_overridden: false,
            },
            ConfigSettingRow {
                key: "model",
                label: "Model",
                value: format!("{} >", self.model_name),
                repo_overridden: has_override("model"),
            },
        ];
        let query = self
            .pending_config_modal
            .as_ref()
            .map(|m| m.search.trim().to_ascii_lowercase())
            .unwrap_or_default();
        if query.is_empty() {
            all
        } else {
            all.into_iter()
                .filter(|r| {
                    r.label.to_ascii_lowercase().contains(&query)
                        || r.value.to_ascii_lowercase().contains(&query)
                })
                .collect()
        }
    }

    /// Toggles or cycles the selected `/config` setting and returns `(key, open_model_picker)`.
    pub fn activate_selected_config_setting(&mut self) -> Option<(&'static str, bool)> {
        let rows = self.config_setting_rows();
        let sel = self.pending_config_modal.as_ref()?.selected;
        let row = rows.get(sel)?;
        let key = row.key;
        match key {
            "auto_compact" => self.ui_settings.auto_compact = !self.ui_settings.auto_compact,
            "thinking_mode" => self.ui_settings.thinking_mode = !self.ui_settings.thinking_mode,
            "verbose_output" => {
                self.ui_settings.verbose_output = !self.ui_settings.verbose_output;
                self.expand_outputs = self.ui_settings.verbose_output;
            }
            "show_turn_duration" => {
                self.ui_settings.show_turn_duration = !self.ui_settings.show_turn_duration;
            }
            "auto_scroll" => {
                self.ui_settings.auto_scroll = !self.ui_settings.auto_scroll;
                self.auto_scroll = self.ui_settings.auto_scroll;
            }
            "respect_gitignore_picker" => {
                self.ui_settings.respect_gitignore_picker =
                    !self.ui_settings.respect_gitignore_picker;
            }
            "default_permission_mode" => {
                self.ui_settings.default_permission_mode =
                    if self.ui_settings.default_permission_mode == "policy-gated" {
                        "auto-bypass".to_owned()
                    } else {
                        "policy-gated".to_owned()
                    };
            }
            "editor_mode" => {
                self.ui_settings.editor_mode = if self.ui_settings.editor_mode == "normal" {
                    "vi".to_owned()
                } else {
                    "normal".to_owned()
                };
            }
            "queued_messages" => {
                self.ui_settings.queued_messages = if self.ui_settings.queued_messages == "queue" {
                    "send_immediately".to_owned()
                } else {
                    "queue".to_owned()
                };
            }
            "show_tips" => self.ui_settings.show_tips = !self.ui_settings.show_tips,
            "model" => return Some((key, true)),
            _ => {}
        }
        Some((key, false))
    }

    // ── /plugins modal ───────────────────────────────────────────────────────

    /// Opens the interactive `/plugins` modal.
    pub fn open_plugins_modal(&mut self) {
        self.pending_plugins_modal = Some(PendingPluginsModal {
            tab: PluginsTab::Discover,
            search: String::new(),
            searching: false,
            adding_marketplace: false,
            marketplace_input: String::new(),
            selected: 0,
            marketplace_filter: None,
            status_notice: None,
        });
    }

    /// Returns the active `/plugins` modal state.
    #[must_use]
    pub fn pending_plugins_modal(&self) -> Option<&PendingPluginsModal> {
        self.pending_plugins_modal.as_ref()
    }

    /// Mutable access to `/plugins` modal state.
    pub fn pending_plugins_modal_mut(&mut self) -> Option<&mut PendingPluginsModal> {
        self.pending_plugins_modal.as_mut()
    }

    /// Closes `/plugins` modal.
    pub fn close_plugins_modal(&mut self) {
        self.pending_plugins_modal = None;
    }

    // ── Queued / Steering Messages ───────────────────────────────────────────

    /// Enqueues a `(display, resolved_prompt)` follow-up message while the model is busy.
    pub fn enqueue_queued_message(&mut self, display: String, resolved_prompt: String) {
        self.queued_messages.push((display, resolved_prompt));
    }

    /// Pops the oldest queued message for immediate steering (`Alt+M`) or post-turn execution.
    pub fn take_next_queued_message(&mut self) -> Option<(String, String)> {
        if self.queued_messages.is_empty() {
            None
        } else {
            Some(self.queued_messages.remove(0))
        }
    }

    /// Pops the most recently queued message back into the input box for editing (`Up`).
    pub fn pop_last_queued_for_edit(&mut self) -> Option<String> {
        self.queued_messages.pop().map(|(display, _)| display)
    }

    /// Returns queued follow-up messages `(display, resolved_prompt)`.
    #[must_use]
    pub fn queued_messages(&self) -> &[(String, String)] {
        &self.queued_messages
    }

    /// Records a steering message currently being delivered (`Sending to agent...`).
    pub fn push_steering_pending(&mut self, display: String) {
        self.steering_pending.push(display);
    }

    /// Marks the oldest pending steering message as delivered into the active turn.
    pub fn mark_steering_delivered(&mut self, display: &str) {
        if let Some(pos) = self.steering_pending.iter().position(|s| s == display) {
            self.steering_pending.remove(pos);
        } else if !self.steering_pending.is_empty() {
            self.steering_pending.remove(0);
        }
        self.push_user_turn(display);
    }

    /// Clears any undelivered steering-pending indicators (e.g. when a turn ends).
    pub fn clear_steering_pending(&mut self) {
        self.steering_pending.clear();
    }

    /// Returns steering messages currently waiting to be injected into the active turn.
    #[must_use]
    pub fn steering_pending(&self) -> &[String] {
        &self.steering_pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_navigation_restores_draft_and_hydrates() {
        let mut state = AppState::new(
            Profile::coding(),
            "off",
            "antigravity/claude-sonnet-5-5-medium",
        );
        state.extend_hydrated(vec![HydratedEvent::User("first prompt".to_owned())]);
        state.push_user_turn("second prompt");
        state.push_input_char('d');
        state.push_input_char('r');

        state.navigate_history(-1);
        assert_eq!(state.input(), "second prompt");
        state.navigate_history(-1);
        assert_eq!(state.input(), "first prompt");
        state.navigate_history(-1);
        assert_eq!(state.input(), "first prompt");
        state.navigate_history(1);
        assert_eq!(state.input(), "second prompt");
        state.navigate_history(1);
        assert_eq!(state.input(), "dr");
    }

    #[test]
    fn model_picker_groups_by_vendor_and_thinking_finalizes() {
        let mut state = AppState::new(
            Profile::coding(),
            "off",
            "antigravity/gemini-3.8-flash-tiered",
        );
        state.open_model_picker(
            vec![
                AvailableModel {
                    id: "gpt-oss-120b-medium".to_owned(),
                    display_name: "GPT-OSS 120B".to_owned(),
                    quota_percent: Some(90),
                    recommended: true,
                },
                AvailableModel {
                    id: "claude-sonnet-5-5-medium".to_owned(),
                    display_name: "Claude Sonnet 5.5".to_owned(),
                    quota_percent: Some(80),
                    recommended: true,
                },
                AvailableModel {
                    id: "gemini-3.8-flash-tiered".to_owned(),
                    display_name: "Gemini 3.8 Flash".to_owned(),
                    quota_percent: Some(100),
                    recommended: true,
                },
            ],
            ProviderKind::Antigravity,
            "antigravity/gemini-3.8-flash-tiered",
        );
        let picker = state.pending_model_picker().expect("picker open");
        assert_eq!(picker.items[0].id, "gemini-3.8-flash-tiered");
        assert_eq!(picker.items[1].id, "claude-sonnet-5-5-medium");
        assert_eq!(picker.items[2].id, "gpt-oss-120b-medium");
        assert_eq!(picker.selected, 0);

        state.append_thinking_delta("checking files");
        state.append_stream_delta("done");
        assert!(matches!(
            &state.entries()[0],
            TranscriptEntry::Thinking {
                active: false,
                elapsed_secs,
                ..
            } if *elapsed_secs >= 1
        ));
    }

    #[test]
    fn multiline_paste_cursor_movement_and_double_ctrl_c_work() {
        let mut state = AppState::new(Profile::coding(), "off", "antigravity/gemini-3.8-flash");
        state.push_input_str("line one\r\nline three");
        assert_eq!(state.input(), "line one\nline three");
        assert!(state.move_cursor_vertical(-1));
        assert!(state.move_cursor_vertical(1));
        assert!(!state.move_cursor_vertical(1));
        state.move_cursor_home();
        state.push_input_str("mid ");
        assert_eq!(state.input(), "line one\nmid line three");

        let t0 = Instant::now();
        assert!(!state.arm_or_confirm_ctrl_c(t0));
        assert!(state.ctrl_c_exit_armed());
        assert!(state.arm_or_confirm_ctrl_c(t0 + Duration::from_millis(500)));

        assert!(!state.arm_or_confirm_ctrl_c(t0));
        state.clear_ctrl_c_arm();
        assert!(!state.ctrl_c_exit_armed());
        assert!(!state.arm_or_confirm_ctrl_c(t0 + Duration::from_millis(200)));
    }

    #[test]
    fn todo_tools_update_bottom_bar_without_cluttering_transcript_and_clear_on_done() {
        use serde_json::json;

        let mut state = AppState::new(Profile::coding(), "off", "antigravity/gemini-3.8-flash");
        let write_call = ToolCall {
            id: "call_1".to_owned(),
            name: "todo_write".to_owned(),
            arguments: json!({
                "title": "Auth refactor",
                "todos": [
                    { "id": "1", "content": "Extract JWT parser", "status": "in_progress" },
                    { "id": "2", "content": "Add unit tests", "status": "pending" }
                ]
            }),
        };

        state.push_tool_running(&write_call);
        assert!(state.entries().is_empty());
        assert_eq!(state.active_todo_title(), Some("Auth refactor"));
        assert_eq!(state.active_todos().len(), 2);

        state.finish_tool_entry(
            &write_call,
            &ToolOutput {
                call_id: "call_1".to_owned(),
                content: json!({
                    "ok": true,
                    "title": "Auth refactor",
                    "cleared": false,
                    "todos": [
                        { "id": "1", "content": "Extract JWT parser", "status": "completed" },
                        { "id": "2", "content": "Add unit tests", "status": "in_progress" }
                    ]
                })
                .to_string(),
                is_error: false,
            },
        );
        assert!(state.entries().is_empty());
        assert_eq!(state.active_todos()[0].status, TodoStatus::Completed);
        assert_eq!(state.active_todos()[1].status, TodoStatus::InProgress);

        let complete_call = ToolCall {
            id: "call_2".to_owned(),
            name: "todo_write".to_owned(),
            arguments: json!({
                "todos": [
                    { "id": "1", "content": "Extract JWT parser", "status": "completed" },
                    { "id": "2", "content": "Add unit tests", "status": "completed" }
                ]
            }),
        };
        state.push_tool_running(&complete_call);
        state.finish_tool_entry(
            &complete_call,
            &ToolOutput {
                call_id: "call_2".to_owned(),
                content: json!({
                    "ok": true,
                    "cleared": true,
                    "todos": []
                })
                .to_string(),
                is_error: false,
            },
        );
        assert!(state.entries().is_empty());
        assert!(state.active_todo_title().is_none());
        assert!(state.active_todos().is_empty());
    }
}
