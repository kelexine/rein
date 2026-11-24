// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Iced Message enum for rein-gui user interactions and background events

use iced::widget::scrollable;
use rein::agent::ApprovalAction;
use rein_core::ModeId;

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
}
