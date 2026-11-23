// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: GuiApp state and structured transcript items for rein-gui

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};
use std::time::Instant;

use iced::widget::markdown;
use rein::agent::HydratedEvent;
use rein::config::{ReinPaths, SessionSummary, list_sessions};
use rein::modes::Profile;
use rein_core::{AvailableModel, ModeId, ToolCall, ToolOutput, UsageStats};
use rein_tools::{QuestionAnswer, TaskNotification, TaskRegistry, TaskSummary, TodoItem};

use crate::worker::{PendingApprovalPrompt, PendingQuestionPrompt, WorkerHandle};

/// Execution state of a tool card in the GUI transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolItemState {
    /// Actively executing or running in the background.
    Running,
    /// Completed successfully.
    Done,
    /// Completed with error or denied.
    Error,
}

/// Structured transcript item rendered in the conversation column.
#[derive(Debug)]
pub enum TranscriptItem {
    /// User prompt bubble.
    User(String),
    /// Collapsible reasoning/thought trace.
    Thinking {
        /// Reasoning text.
        content: String,
        /// Elapsed seconds.
        elapsed_secs: u64,
        /// Whether tokens are still streaming.
        active: bool,
        /// Whether expanded in the UI.
        expanded: bool,
    },
    /// Assistant Markdown response parsed for `iced::widget::markdown`.
    Assistant {
        /// Raw Markdown text.
        raw: String,
        /// Parsed Iced Markdown items.
        parsed: markdown::Content,
    },
    /// Collapsible tool invocation card.
    Tool {
        /// Provider tool call ID.
        call_id: String,
        /// Tool name.
        name: String,
        /// Tool arguments JSON.
        arguments: serde_json::Value,
        /// Current run state.
        state: ToolItemState,
        /// Optional background task/subagent ID.
        background_task_id: Option<String>,
        /// Captured tool output.
        output: String,
        /// Whether expanded in the UI.
        expanded: bool,
    },
    /// Muted system/wakeup note.
    System(String),
    /// Error banner/note.
    Error(String),
}

/// Root Iced application state for `rein-gui`.
pub struct GuiApp {
    /// Background worker handle.
    pub worker: WorkerHandle,
    /// Filesystem paths for session listing & config.
    pub paths: ReinPaths,
    /// Task registry shared with tools.
    pub task_registry: TaskRegistry,
    /// Receiver for async background task/subagent wakeups.
    pub task_rx: mpsc::Receiver<TaskNotification>,
    /// Active mode profile.
    pub active_profile: Profile,
    /// Active provider/model display label.
    pub model_label: String,
    /// Active workspace label.
    pub workspace_label: String,
    /// Active sandbox status label.
    pub sandbox_label: String,
    /// Active session UID.
    pub session_uid: String,
    /// Accumulated session token usage.
    pub usage: UsageStats,
    /// Whether the collapsible left sidebar is open.
    pub sidebar_open: bool,
    /// Saved sessions shown in the sidebar.
    pub sessions: Vec<SessionSummary>,
    /// Current prompt composer text.
    pub input: String,
    /// Transcript items in the active session.
    pub items: Vec<TranscriptItem>,
    /// Whether a turn is currently running.
    pub busy: bool,
    /// Shared flag read by the subscription ticker to throttle idle redraws.
    pub busy_flag: Arc<AtomicBool>,
    /// Current busy status label (`Thinking…`, `Reasoning…`, `Responding…`, `Running …`).
    pub busy_status: String,
    /// Timestamp when the current busy operation began.
    pub busy_started_at: Option<Instant>,
    /// Timestamp of the last Markdown re-parse while streaming.
    pub last_md_parse: Instant,
    /// Animation tick counter.
    pub spinner_tick: usize,
    /// Active thinking start timestamp.
    pub thinking_started_at: Option<Instant>,
    /// Whether the model picker popover is open.
    pub model_picker_open: bool,
    /// Models returned by `provider.list_models()`.
    pub available_models: Vec<AvailableModel>,
    /// Pending policy approval modal.
    pub pending_approval: Option<PendingApprovalPrompt>,
    /// Pending `ask_question` modal.
    pub pending_question: Option<PendingQuestionPrompt>,
    /// Collected answers for `ask_question`.
    pub question_answers: Vec<QuestionAnswer>,
    /// Current question step index.
    pub question_step: usize,
    /// Selected option index for the current question step.
    pub question_selected: Option<usize>,
    /// Active todo checklist items.
    pub todos: Vec<TodoItem>,
    /// Active todo list title.
    pub todo_title: Option<String>,
    /// Whether the background task drawer is visible.
    pub task_drawer_open: bool,
    /// Snapshot of background tasks.
    pub tasks_snapshot: Vec<TaskSummary>,
    /// Greeting user name (`$USER` fallback `kelexine`).
    pub user_name: String,
}

impl GuiApp {
    /// Constructs a [`GuiApp`] from a bootstrapped [`rein::runtime::ReinSession`].
    #[must_use]
    pub fn from_session(mut session: rein::runtime::ReinSession) -> Self {
        let initial_events = if session.should_hydrate {
            session.agent.hydrate_structured().unwrap_or_default()
        } else {
            Vec::new()
        };
        let active_profile = session.agent.active_profile().clone();
        let paths = session.loaded.paths.clone();
        let model_label = session.model_label.clone();
        let workspace_label = session.workspace_label.clone();
        let sandbox_label = session.sandbox_label.clone();
        let session_uid = session.session_uid.clone();
        let notices = session.notices.clone();
        let task_registry = session.task_registry.clone();
        let sessions = list_sessions(&paths).unwrap_or_default();

        // Take ownership of `task_rx` before moving `session` into `WorkerHandle`.
        let (_dummy_tx, dummy_rx) = mpsc::channel();
        let task_rx = std::mem::replace(&mut session.task_rx, dummy_rx);

        let (worker, _rx) = WorkerHandle::spawn(session);
        let user_name = std::env::var("USER")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "kelexine".to_owned());

        let mut app = Self {
            worker,
            paths,
            task_registry,
            task_rx,
            active_profile,
            model_label,
            workspace_label,
            sandbox_label,
            session_uid,
            usage: UsageStats::default(),
            sidebar_open: false,
            sessions,
            input: String::new(),
            items: Vec::new(),
            busy: false,
            busy_flag: Arc::new(AtomicBool::new(false)),
            busy_status: "Thinking…".to_owned(),
            busy_started_at: None,
            last_md_parse: Instant::now(),
            spinner_tick: 0,
            thinking_started_at: None,
            model_picker_open: false,
            available_models: Vec::new(),
            pending_approval: None,
            pending_question: None,
            question_answers: Vec::new(),
            question_step: 0,
            question_selected: None,
            todos: Vec::new(),
            todo_title: None,
            task_drawer_open: false,
            tasks_snapshot: Vec::new(),
            user_name,
        };
        for notice in notices {
            app.items.push(TranscriptItem::System(notice));
        }
        app.apply_hydrated(initial_events);
        app
    }

    /// Fallback constructor when boot closure is invoked more than once.
    #[must_use]
    pub fn fallback(workspace_root: &std::path::Path, args: &[String]) -> Self {
        match rein::runtime::ReinSession::bootstrap(workspace_root, args) {
            Ok(session) => Self::from_session(session),
            Err(_) => {
                let cwd = std::env::temp_dir();
                let loaded = rein::config::LoadedReinState {
                    config: rein::config::ReinConfig::default(),
                    paths: rein::config::ReinPaths::from_dirs(cwd.clone(), cwd),
                    allowed_commands: Vec::new(),
                };
                if let Ok(session) = rein::runtime::ReinSession::from_loaded(
                    workspace_root,
                    loaded,
                    false,
                    rein::runtime::ResumeSpec::NewSession,
                ) {
                    return Self::from_session(session);
                }
                std::process::exit(1);
            }
        }
    }

    /// Returns the active operating mode.
    #[must_use]
    pub fn mode(&self) -> ModeId {
        self.active_profile.mode
    }

    /// Hydrates transcript items from replayed session events.
    pub fn apply_hydrated(&mut self, events: Vec<HydratedEvent>) {
        for ev in events {
            match ev {
                HydratedEvent::User(text) => {
                    self.items.push(TranscriptItem::User(text));
                }
                HydratedEvent::Assistant(text) => {
                    self.items.push(TranscriptItem::Assistant {
                        parsed: markdown::Content::parse(&text),
                        raw: text,
                    });
                }
                HydratedEvent::Tool { call, output } => {
                    self.push_hydrated_tool(&call, &output);
                }
                HydratedEvent::Note(note) => {
                    self.items.push(TranscriptItem::System(note));
                }
            }
        }
    }

    fn push_hydrated_tool(&mut self, call: &ToolCall, output: &ToolOutput) {
        self.items.push(TranscriptItem::Tool {
            call_id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
            state: if output.is_error {
                ToolItemState::Error
            } else {
                ToolItemState::Done
            },
            background_task_id: None,
            output: output.content.clone(),
            expanded: false,
        });
    }

    /// Refreshes the sidebar session list from disk.
    pub fn refresh_sessions(&mut self) {
        self.sessions = list_sessions(&self.paths).unwrap_or_default();
    }
}
