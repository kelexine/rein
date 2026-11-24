// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Core AppState struct, transcript/todo/history management, and state unit tests

use std::sync::mpsc;
use std::time::Instant;

use rein_core::{AvailableModel, ToolCall, ToolOutput, UsageStats};
use rein_tools::{
    McpServerConfig, TaskNotification, TaskRegistry, TodoItem, TodoListState, TodoStatus, TodoStore,
};
use serde_json::Value;

use crate::agent::{
    HydratedEvent, LiveSubagentStatus, SubagentManager, SubagentTracker, SwarmManager,
};
use crate::commands::{SlashCommand, SlashRegistry};
use crate::config::UiSettings;
use crate::modes::Profile;
use crate::plugins::PluginManager;
use crate::ui::entries::{ToolRunState, TranscriptEntry};
use crate::ui::mentions::MentionCandidate;

/// Active interactive 4-option permission prompt with inline `Tab` feedback editor.
use super::modals::extract_background_task_id;
use super::types::*;

/// State owned by the in-process minimalist Ratatui frontend.
#[derive(Debug)]
pub struct AppState {
    pub(super) active_profile: Profile,
    pub(super) sandbox_summary: String,
    pub(super) model_name: String,
    pub(super) model_quota: Option<u8>,
    pub(super) usage_totals: UsageStats,
    pub(super) workspace_label: String,
    pub(super) session_id: String,
    pub(super) startup_notices: Vec<String>,
    pub(super) slash_registry: SlashRegistry,
    pub(super) slash_selected: usize,
    pub(super) configured_commands: Vec<String>,
    pub(super) mcp_servers: Vec<McpServerConfig>,
    pub(super) entries: Vec<TranscriptEntry>,
    pub(super) active_todo_title: Option<String>,
    pub(super) active_todos: Vec<TodoItem>,
    pub(super) todo_store: Option<TodoStore>,
    pub(super) input: String,
    pub(super) input_cursor: usize,
    pub(super) ctrl_c_armed_at: Option<Instant>,
    pub(super) pending_approval: Option<PendingApproval>,
    pub(super) pending_model_picker: Option<PendingModelPicker>,
    pub(super) pending_session_picker: Option<PendingSessionPicker>,
    pub(super) pending_rewind_picker: Option<PendingRewindPicker>,
    pub(super) expand_outputs: bool,
    pub(super) scroll_offset: u16,
    pub(super) auto_scroll: bool,
    pub(super) busy: bool,
    pub(super) busy_status: String,
    pub(super) busy_started_at: Option<Instant>,
    pub(super) spinner_tick: usize,
    pub(super) streaming_active: bool,
    pub(super) thinking_active: bool,
    pub(super) thinking_started: Option<Instant>,
    pub(super) prompt_history: Vec<String>,
    pub(super) history_cursor: Option<usize>,
    pub(super) draft_input: String,
    pub(super) bypass_permissions: bool,
    pub(super) subagent_tracker: SubagentTracker,
    pub(super) subagent_manager: Option<SubagentManager>,
    pub(super) swarm_manager: Option<SwarmManager>,
    pub(super) task_registry: TaskRegistry,
    pub(super) task_notifications_rx: Option<mpsc::Receiver<TaskNotification>>,
    pub(super) strip_selected: Option<usize>,
    pub(super) pending_task_manager: Option<PendingTaskManager>,
    pub(super) pending_task_details: Option<TaskDetailsView>,
    pub(super) pending_question: Option<PendingQuestion>,
    pub(super) question_relay: Option<SharedQuestionRelay>,
    pub(super) mention_candidates: Vec<MentionCandidate>,
    pub(super) mention_selected: usize,
    pub(super) mention_dismissed_at: Option<usize>,
    pub(super) ui_settings: UiSettings,
    pub(super) repo_override_keys: Vec<String>,
    pub(super) pending_config_modal: Option<PendingConfigModal>,
    pub(super) plugin_manager: Option<PluginManager>,
    pub(super) pending_plugins_modal: Option<PendingPluginsModal>,
    pub(super) queued_messages: Vec<(String, String)>,
    pub(super) steering_pending: Vec<String>,
    pub(super) cached_models: Vec<AvailableModel>,
    pub(super) model_refresh_rx: Option<mpsc::Receiver<Vec<AvailableModel>>>,
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

pub(super) fn is_todo_tool(name: &str) -> bool {
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
            busy_started_at: None,
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
            cached_models: Vec::new(),
            model_refresh_rx: None,
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

    /// Pre-seeds the in-memory model catalog from the startup disk cache.
    #[must_use]
    pub fn with_cached_models(mut self, models: Vec<AvailableModel>) -> Self {
        self.cached_models = models;
        self
    }

    /// Attaches a background model-refresh receiver populated on TUI launch.
    #[must_use]
    pub fn with_model_refresh_rx(mut self, rx: mpsc::Receiver<Vec<AvailableModel>>) -> Self {
        self.model_refresh_rx = Some(rx);
        self
    }

    /// Returns the cached model list for the active provider.
    #[must_use]
    pub fn cached_models(&self) -> &[AvailableModel] {
        &self.cached_models
    }

    /// Replaces the cached model list for the active provider.
    pub fn set_cached_models(&mut self, models: Vec<AvailableModel>) {
        self.cached_models = models;
    }

    /// Drains any completed startup model refresh without blocking.
    pub fn poll_model_refresh(&mut self) {
        if let Some(rx) = &self.model_refresh_rx
            && let Ok(models) = rx.try_recv()
        {
            if !models.is_empty() {
                self.cached_models = models;
            }
            self.model_refresh_rx = None;
        }
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
        let background_id = (!output.is_error)
            .then(|| extract_background_task_id(&output.content))
            .flatten();
        if let Some(existing) = self.entries.iter_mut().rev().find_map(|entry| match entry {
            TranscriptEntry::Tool(tool) if tool.call_id == call.id => Some(tool),
            _ => None,
        }) {
            if let Some(task_id) = background_id {
                existing.state = ToolRunState::Running;
                existing.background_task_id = Some(task_id);
            } else {
                existing.state = if output.is_error {
                    ToolRunState::Error
                } else {
                    ToolRunState::Done
                };
            }
            existing.output = output.content.clone();
        } else {
            let mut entry = TranscriptEntry::tool_completed(call, output);
            if let (Some(task_id), TranscriptEntry::Tool(tool)) = (background_id, &mut entry) {
                tool.state = ToolRunState::Running;
                tool.background_task_id = Some(task_id);
            }
            self.entries.push(entry);
        }
        self.auto_scroll = true;
    }

    /// Transitions a backgrounded tool transcript entry from `Running` to `Done` or `Error` on wakeup.
    pub fn complete_background_tool(&mut self, task_id: &str, is_error: bool, output: String) {
        if let Some(existing) = self.entries.iter_mut().rev().find_map(|entry| match entry {
            TranscriptEntry::Tool(tool) if tool.background_task_id.as_deref() == Some(task_id) => {
                Some(tool)
            }
            _ => None,
        }) {
            existing.state = if is_error {
                ToolRunState::Error
            } else {
                ToolRunState::Done
            };
            existing.output = output;
            self.auto_scroll = true;
        }
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
        self.busy_status = "Reasoning…".to_owned();
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
        self.busy_status = "Responding…".to_owned();
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
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::config::ProviderKind;

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

    #[test]
    fn backgrounded_tool_stays_running_until_completed_on_wakeup() {
        let mut state = AppState::new(Profile::coding(), "sandbox", "model");
        let call = ToolCall {
            id: "call_bg".to_owned(),
            name: "bash".to_owned(),
            arguments: serde_json::json!({"command": "sleep 5"}),
        };
        state.push_tool_running(&call);
        state.finish_tool_entry(
            &call,
            &ToolOutput {
                call_id: "call_bg".to_owned(),
                content: serde_json::json!({
                    "status": "backgrounded",
                    "task_id": "task-1",
                })
                .to_string(),
                is_error: false,
            },
        );
        match &state.entries()[0] {
            TranscriptEntry::Tool(tool) => {
                assert_eq!(tool.state, ToolRunState::Running);
                assert_eq!(tool.background_task_id.as_deref(), Some("task-1"));
            }
            other => panic!("expected tool entry, got {other:?}"),
        }

        state.complete_background_tool("task-1", false, "done".to_owned());
        match &state.entries()[0] {
            TranscriptEntry::Tool(tool) => {
                assert_eq!(tool.state, ToolRunState::Done);
                assert_eq!(tool.output, "done");
            }
            other => panic!("expected tool entry, got {other:?}"),
        }
    }
}
