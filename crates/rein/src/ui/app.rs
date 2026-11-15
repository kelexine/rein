// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Minimalist borderless TUI state driving Core, Policy, Tools, Modes, Slash commands, and Model picker

use rein_core::{AvailableModel, ToolCall, ToolOutput, UsageStats};
use rein_tools::McpServerConfig;

use crate::agent::{ApprovalAction, ApprovalDecision, ApprovalRequest, HydratedEvent};
use crate::commands::{SlashCommand, SlashRegistry};
use crate::config::ProviderKind;
use crate::modes::Profile;
use crate::ui::entries::{ToolRunState, TranscriptEntry};

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

/// State owned by the in-process minimalist Ratatui frontend.
#[derive(Debug)]
pub struct AppState {
    active_profile: Profile,
    sandbox_summary: String,
    model_name: String,
    model_quota: Option<u8>,
    usage_totals: UsageStats,
    workspace_label: String,
    startup_notices: Vec<String>,
    slash_registry: SlashRegistry,
    slash_selected: usize,
    configured_commands: Vec<String>,
    mcp_servers: Vec<McpServerConfig>,
    entries: Vec<TranscriptEntry>,
    input: String,
    pending_approval: Option<PendingApproval>,
    pending_model_picker: Option<PendingModelPicker>,
    expand_outputs: bool,
    scroll_offset: u16,
    auto_scroll: bool,
    busy: bool,
    busy_status: String,
    spinner_tick: usize,
    streaming_active: bool,
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
            startup_notices: Vec::new(),
            slash_registry: SlashRegistry::with_builtins(),
            slash_selected: 0,
            configured_commands: Vec::new(),
            mcp_servers: Vec::new(),
            entries: Vec::new(),
            input: String::new(),
            pending_approval: None,
            pending_model_picker: None,
            expand_outputs: false,
            scroll_offset: 0,
            auto_scroll: true,
            busy: false,
            busy_status: "Thinking…".to_owned(),
            spinner_tick: 0,
            streaming_active: false,
        }
    }

    /// Sets the workspace display path shown in the Citadel Owl header (`~/dev/rein`).
    #[must_use]
    pub fn with_workspace_label(mut self, label: impl Into<String>) -> Self {
        self.workspace_label = label.into();
        self
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
    }

    /// Appends hydrated structured events (used by `--resume` and `/resume`).
    pub fn extend_hydrated(&mut self, hydrated: Vec<HydratedEvent>) {
        self.streaming_active = false;
        for ev in hydrated {
            match ev {
                HydratedEvent::User(text) => self.entries.push(TranscriptEntry::UserTurn(text)),
                HydratedEvent::Assistant(text) => {
                    self.entries.push(TranscriptEntry::AssistantTurn(text));
                }
                HydratedEvent::Tool { call, output } => {
                    self.entries
                        .push(TranscriptEntry::tool_completed(&call, &output));
                }
            }
        }
        self.auto_scroll = true;
    }

    /// Appends a user turn prompt to the structured transcript.
    pub fn push_user_turn(&mut self, text: impl Into<String>) {
        self.streaming_active = false;
        self.entries.push(TranscriptEntry::UserTurn(text.into()));
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
        self.streaming_active = false;
        self.entries.push(TranscriptEntry::ErrorNote(err.into()));
        self.auto_scroll = true;
    }

    /// Records a newly started tool invocation in the transcript.
    pub fn push_tool_running(&mut self, call: &ToolCall) {
        self.streaming_active = false;
        self.busy_status = format!("Running {}…", call.name);
        self.entries.push(TranscriptEntry::tool_running(call));
        self.auto_scroll = true;
    }

    /// Updates a running tool entry with its output (or appends a completed entry if unmatched).
    pub fn finish_tool_entry(&mut self, call: &ToolCall, output: &ToolOutput) {
        self.streaming_active = false;
        self.busy_status = "Thinking…".to_owned();
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

    /// Appends streamed assistant tokens cleanly into the trailing [`TranscriptEntry::AssistantTurn`].
    pub fn append_stream_delta(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
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

    /// Replaces the input buffer cleanly (e.g. on `Tab` slash completion).
    pub fn set_input(&mut self, text: impl Into<String>) {
        self.input = text.into();
        self.slash_selected = 0;
    }

    /// Pushes a typed character to the input buffer.
    pub fn push_input_char(&mut self, ch: char) {
        self.input.push(ch);
        self.slash_selected = 0;
    }

    /// Removes the last character from the input buffer.
    pub fn pop_input_char(&mut self) {
        let _ = self.input.pop();
        self.slash_selected = 0;
    }

    /// Takes and clears the current input buffer.
    pub fn take_input(&mut self) -> String {
        self.slash_selected = 0;
        std::mem::take(&mut self.input)
    }

    /// Opens the interactive `/model` selector populated from server models.
    pub fn open_model_picker(
        &mut self,
        items: Vec<AvailableModel>,
        provider_kind: ProviderKind,
        current_model: &str,
    ) {
        let selected = items
            .iter()
            .position(|m| current_model.ends_with(&m.id))
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
