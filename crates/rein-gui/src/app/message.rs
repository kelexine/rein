// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Iced Message enum for rein-gui user interactions and background events

use rein::agent::ApprovalAction;
use rein_core::ModeId;

/// All UI and background events processed by [`crate::app::update`].
#[derive(Debug, Clone)]
pub enum Message {
    /// Periodic tick draining worker & task-notification channels and advancing spinner.
    Tick,
    /// Prompt text input changed.
    InputChanged(String),
    /// Submit the current prompt.
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
    /// Toggle the model picker popover.
    ToggleModelPicker,
    /// Select a model ID from the picker.
    SelectModel(String),
    /// Compact conversation history.
    CompactHistory,
    /// Toggle expand/collapse for a thinking or tool transcript card by index.
    ToggleExpand(usize),
    /// Update approval modal feedback text.
    ApprovalFeedbackChanged(String),
    /// Resolve the pending policy approval request.
    ResolveApproval(ApprovalAction),
    /// Select an option index for the active `ask_question` step.
    SelectQuestionOption(usize),
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
}
