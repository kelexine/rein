// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Worker command/event types and interactive approval prompter for rein-gui

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};

use anyhow::Result;
use rein::agent::{ApprovalDecision, ApprovalPrompter, ApprovalRequest};
use rein::modes::Profile;
use rein_core::{AvailableModel, ModeId, ToolCall, ToolOutput, UsageStats};
use rein_tools::{QuestionAnswer, QuestionSpec};

/// Commands sent from the Iced UI thread to the background agent worker.
#[derive(Debug)]
pub enum WorkerCommand {
    /// Execute a user or wakeup prompt on `AgentLoop`.
    RunPrompt {
        /// Full prompt sent to the model.
        prompt: String,
    },
    /// Switch active operating mode (`coding`, `plan`, `personal`).
    SwitchMode(ModeId),
    /// Fetch available models from the active provider.
    FetchModels,
    /// Select a different model ID on the active provider.
    SelectModel(String),
    /// Compact conversation history.
    Compact,
    /// Clear conversation state for a new session.
    NewSession,
    /// Hydrate an existing session by UID.
    ResumeSession(String),
}

/// Interactive policy approval request forwarded to the GUI modal.
#[derive(Debug, Clone)]
pub struct PendingApprovalPrompt {
    /// Underlying approval request metadata.
    pub request: ApprovalRequest,
    /// Feedback input edited by the user.
    pub feedback: String,
}

/// Interactive `ask_question` request forwarded to the GUI modal.
#[derive(Debug)]
pub struct PendingQuestionPrompt {
    /// Questions emitted by the model.
    pub questions: Vec<QuestionSpec>,
    /// Reply channel back to `AskQuestionTool`.
    pub reply_tx: Sender<Option<Vec<QuestionAnswer>>>,
}

/// Events emitted from the background worker to the Iced UI.
#[derive(Debug)]
pub enum WorkerEvent {
    /// Reasoning/thought delta streamed by the model.
    ThinkingDelta(String),
    /// Assistant response text delta streamed by the model.
    TextDelta(String),
    /// Tool execution started.
    ToolStart(ToolCall),
    /// Tool execution completed.
    ToolFinish(ToolCall, ToolOutput),
    /// Token usage update.
    Usage(UsageStats),
    /// Active profile switched.
    ModeSwitched(Box<Profile>),
    /// Policy approval required before executing a tool.
    ApprovalRequested(ApprovalRequest),
    /// `ask_question` tool waiting on user answers.
    QuestionRequested {
        /// Questions to answer.
        questions: Vec<QuestionSpec>,
        /// Reply channel back to the tool.
        reply_tx: Sender<Option<Vec<QuestionAnswer>>>,
    },
    /// Available models returned by provider.
    ModelsLoaded(Vec<AvailableModel>),
    /// Model switched successfully.
    ModelSelected(String),
    /// Session resumed with hydrated events and active profile.
    SessionHydrated {
        /// Resumed session UID.
        session_uid: String,
        /// Hydrated structured transcript events.
        events: Vec<rein::agent::HydratedEvent>,
        /// Active mode profile.
        profile: Box<Profile>,
    },
    /// Fresh session initialized.
    SessionReset {
        /// New session UID.
        session_uid: String,
        /// Active mode profile.
        profile: Box<Profile>,
    },
    /// Status note (compaction, errors, cancellations).
    Note(String),
    /// Mid-turn user steering message delivered to the active turn.
    SteerDelivered(String),
    /// Error note.
    Error(String),
    /// Current worker command finished.
    TurnFinished,
}

/// [`ApprovalPrompter`] bridging `AgentLoop` callbacks and approvals to the GUI `mpsc` channels.
pub struct GuiAgentBridge {
    pub(crate) ev_tx: Sender<WorkerEvent>,
    pub(crate) approval_rx: Receiver<ApprovalDecision>,
    pub(crate) wake_flag: Arc<AtomicBool>,
}

impl GuiAgentBridge {
    pub(crate) fn emit(&self, ev: WorkerEvent) {
        let _ = self.ev_tx.send(ev);
        self.wake_flag.store(true, Ordering::Release);
    }
}

impl ApprovalPrompter for GuiAgentBridge {
    fn confirm(&mut self, request: &ApprovalRequest) -> Result<ApprovalDecision> {
        self.emit(WorkerEvent::ApprovalRequested(request.clone()));
        Ok(self.approval_rx.recv()?)
    }

    fn on_thinking_delta(&mut self, delta: &str) {
        self.emit(WorkerEvent::ThinkingDelta(delta.to_owned()));
    }

    fn on_delta(&mut self, delta: &str) {
        self.emit(WorkerEvent::TextDelta(delta.to_owned()));
    }

    fn on_tool_start(&mut self, call: &ToolCall) {
        self.emit(WorkerEvent::ToolStart(call.clone()));
    }

    fn on_tool_finish(&mut self, call: &ToolCall, output: &ToolOutput) {
        self.emit(WorkerEvent::ToolFinish(call.clone(), output.clone()));
    }

    fn on_usage(&mut self, usage: UsageStats) {
        self.emit(WorkerEvent::Usage(usage));
    }

    fn on_mode_switched(&mut self, profile: &Profile) {
        self.emit(WorkerEvent::ModeSwitched(Box::new(profile.clone())));
    }

    fn on_steer_delivered(&mut self, display: &str) {
        self.emit(WorkerEvent::SteerDelivered(display.to_owned()));
    }
}
