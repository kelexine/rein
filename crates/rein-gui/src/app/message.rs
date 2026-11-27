// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Iced Message enum for rein-gui user interactions and background events

use iced::widget::scrollable;
use rein::agent::ApprovalAction;
use rein::config::ProviderKind;
use rein_core::ModeId;

use super::state::SettingsCategory;

/// Settings view user interactions organized by category.
#[derive(Debug, Clone)]
pub enum SettingsMessage {
    /// Switch active settings category tab.
    SelectCategory(SettingsCategory),
    /// Switch active provider (`Anthropic` or `Antigravity`).
    SelectProvider(ProviderKind),
    /// Trigger live model catalog refresh from active provider.
    RefreshModels,
    /// Start Antigravity Google OAuth browser sign-in.
    StartOAuthLogin,
    /// Edit `max_tokens` text input.
    MaxTokensChanged(String),
    /// Edit `endpoint` text input.
    EndpointChanged(String),
    /// Edit `api_key_env` text input.
    ApiKeyEnvChanged(String),
    /// Edit `antigravity_base_url` text input.
    AntigravityBaseUrlChanged(String),
    /// Edit `antigravity_project_id` text input.
    AntigravityProjectIdChanged(String),
    /// Save provider & endpoint fields to `~/.rein/config.json`.
    SaveProviderConfig,
    /// Toggle a boolean [`rein::config::UiSettings`] field by key.
    ToggleUiBool(&'static str),
    /// Cycle a multi-option [`rein::config::UiSettings`] field (`default_permission_mode`, `editor_mode`, `queued_messages`).
    CycleUiChoice(&'static str),
    /// Edit new workspace allowlist pattern draft.
    RepoCommandInputChanged(String),
    /// Add draft pattern to workspace allowlist.
    AddRepoCommand,
    /// Remove pattern from workspace allowlist.
    RemoveRepoCommand(String),
    /// Edit new global allowlist pattern draft.
    GlobalCommandInputChanged(String),
    /// Add draft pattern to global allowlist.
    AddGlobalCommand,
    /// Remove pattern from global allowlist.
    RemoveGlobalCommand(String),
    /// Edit new MCP server name draft.
    McpNameChanged(String),
    /// Edit new MCP server command draft.
    McpCommandChanged(String),
    /// Edit new MCP server args draft (space-separated).
    McpArgsChanged(String),
    /// Add draft stdio MCP server to `~/.rein/config.json`.
    AddMcpServer,
    /// Remove stdio MCP server by name.
    RemoveMcpServer(String),
    /// Edit new marketplace repo/URL draft.
    MarketplaceInputChanged(String),
    /// Register new plugin marketplace.
    AddMarketplace,
    /// Sync/refresh all registered plugin marketplaces via `git`.
    RefreshMarketplaces,
    /// Remove a registered plugin marketplace.
    RemoveMarketplace(String),
    /// Install a plugin by name from catalog.
    InstallPlugin(String),
    /// Toggle an installed plugin enabled/disabled.
    TogglePlugin(String),
    /// Uninstall an installed plugin.
    UninstallPlugin(String),
    /// Toggle AutoDream `dream.enabled`.
    ToggleDreamEnabled,
    /// Edit AutoDream `min_hours`.
    DreamMinHoursChanged(String),
    /// Edit AutoDream `min_sessions`.
    DreamMinSessionsChanged(String),
    /// Edit AutoDream `max_turns`.
    DreamMaxTurnsChanged(String),
    /// Edit Personal mode `ics_dir`.
    IcsDirChanged(String),
    /// Edit Personal mode `maildir`.
    MaildirChanged(String),
    /// Save Memory, AutoDream & Personal paths to `~/.rein/config.json`.
    SaveMemoryAndPersonalConfig,
}

/// All UI and background events processed by [`crate::app::update`].
#[derive(Debug, Clone)]
pub enum Message {
    /// Periodic tick draining worker & task-notification channels and advancing spinner.
    Tick,
    /// Prompt text input changed.
    InputChanged(String),
    /// Submit the current prompt (or steer active turn if busy).
    SubmitPrompt,
    /// Submit a quick-action prompt from the home chips.
    QuickPrompt(String),
    /// Cancel/stop the active turn (`Esc` or Stop button).
    CancelTurn,
    /// Toggle collapsible left sidebar (`Ctrl+B` or topbar button).
    ToggleSidebar,
    /// Start a new chat session.
    NewSession,
    /// Resume an existing session from the sidebar list.
    ResumeSession(String),
    /// Switch active operating mode (`Coding`, `Plan`, `Personal`).
    SwitchMode(ModeId),
    /// Toggle the `+` action popover menu above the composer.
    TogglePlusMenu,
    /// Toggle the model picker popover.
    ToggleModelPicker,
    /// Select a model ID from the picker.
    SelectModel(String),
    /// Compact conversation history.
    CompactHistory,
    /// Toggle expand/collapse for a thinking or tool transcript card by index.
    ToggleExpand(usize),
    /// Toggle expand/collapse for a grouped run of tool calls keyed by its starting index.
    ToggleToolGroup(usize),
    /// Toggle expand/collapse for a long user prompt bubble by index.
    ToggleUserExpand(usize),
    /// Transcript viewport scrolled by the user.
    TranscriptScrolled(scrollable::Viewport),
    /// Open the right-hand split inspector pane with full paste/code/output content.
    OpenInspector {
        /// Inspector header title.
        title: String,
        /// Inspector metadata subtitle (e.g. size and line count).
        subtitle: String,
        /// Full text content displayed in the inspector.
        content: String,
    },
    /// Close the right-hand split inspector pane.
    CloseInspector,
    /// Copy text to the system clipboard.
    CopyText(String),
    /// Update approval modal feedback text.
    ApprovalFeedbackChanged(String),
    /// Resolve the pending policy approval request.
    ResolveApproval(ApprovalAction),
    /// Select or toggle an option index for the active `ask_question` step.
    SelectQuestionOption(usize),
    /// Update custom free-text answer for the active `ask_question` step.
    QuestionCustomTextChanged(String),
    /// Confirm the current `ask_question` step.
    ConfirmQuestion,
    /// Cancel the `ask_question` modal.
    CancelQuestion,
    /// Markdown link clicked.
    LinkClicked(String),
    /// Toggle background task drawer visibility.
    ToggleTaskDrawer,
    /// Kill a background task by ID.
    KillBackgroundTask(String),
    /// Toggle the Material 3 Expressive Settings view (`Ctrl+,` or nav rail button).
    ToggleSettings,
    /// Handle an interaction inside the Settings view.
    Settings(SettingsMessage),
}
