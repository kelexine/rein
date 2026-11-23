// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: GuiApp state, precomputed transcript items, and inspector state for rein-gui

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};
use std::time::Instant;

use iced::widget::{Id, markdown};
use rein::agent::HydratedEvent;
use rein::config::{ReinPaths, SessionSummary, list_sessions};
use rein::modes::Profile;
use rein_core::{AvailableModel, ModeId, ToolCall, ToolOutput, UsageStats};
use rein_tools::{QuestionAnswer, TaskNotification, TaskRegistry, TaskSummary, TodoItem};

use crate::worker::{PendingApprovalPrompt, PendingQuestionPrompt, WorkerHandle};

const USER_COLLAPSE_LINES: usize = 6;
const USER_COLLAPSE_CHARS: usize = 420;
const USER_PASTED_LINES: usize = 16;
const USER_PASTED_BYTES: usize = 1200;

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

/// Right-hand split inspector drawer content (e.g. Pasted content or full Tool output).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectorPane {
    /// Header title shown in the inspector bar.
    pub title: String,
    /// Metadata subtitle (e.g. `15.7 kB · 142 lines`).
    pub subtitle: String,
    /// Full scrollable text content.
    pub content: String,
}

/// Pre-truncated sidebar session summary avoiding per-frame string allocations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarSession {
    /// Unique session identifier.
    pub uid: String,
    /// Pre-truncated preview label.
    pub label: String,
    /// Number of recorded turns.
    pub turn_count: usize,
}

impl SidebarSession {
    fn from_summary(summary: &SessionSummary) -> Self {
        let raw = summary.preview.as_deref().unwrap_or("Empty session");
        let label: String = raw.chars().take(32).collect();
        Self {
            uid: summary.uid.clone(),
            label,
            turn_count: summary.turn_count,
        }
    }
}

/// Structured transcript item rendered in the conversation column.
#[derive(Debug)]
pub enum TranscriptItem {
    /// User prompt bubble with precomputed collapse/pasted metadata.
    User {
        /// Full prompt text.
        raw: String,
        /// Precomputed short preview for collapsed long prompts.
        preview: String,
        /// Metadata subtitle (`15.7 kB · 142 lines`) for pasted cards.
        subtitle: String,
        /// Whether the prompt exceeds inline collapse threshold.
        is_long: bool,
        /// Whether the prompt is large enough to render as a `PASTED` attachment card.
        is_pasted: bool,
        /// Whether the inline long prompt is currently expanded.
        expanded: bool,
    },
    /// Collapsible reasoning/thought trace.
    Thinking {
        /// Full reasoning text.
        content: String,
        /// Precomputed single-line preview when collapsed.
        preview: String,
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

impl TranscriptItem {
    /// Constructs a [`TranscriptItem::User`] with precomputed preview and pasted-card flags.
    #[must_use]
    pub fn user(raw: String) -> Self {
        let line_count = raw.lines().count().max(1);
        let char_count = raw.chars().count();
        let byte_len = raw.len();
        let is_pasted = line_count >= USER_PASTED_LINES || byte_len >= USER_PASTED_BYTES;
        let is_long = !is_pasted && (line_count > USER_COLLAPSE_LINES || char_count > USER_COLLAPSE_CHARS);
        let preview = if is_pasted {
            raw.lines()
                .take(2)
                .collect::<Vec<_>>()
                .join("\n")
                .chars()
                .take(90)
                .collect()
        } else if is_long {
            raw.chars().take(280).collect()
        } else {
            String::new()
        };
        let subtitle = format_size_and_lines(byte_len, line_count);
        Self::User {
            raw,
            preview,
            subtitle,
            is_long,
            is_pasted,
            expanded: false,
        }
    }
}

/// Formats byte size and line count into a concise metadata subtitle.
#[must_use]
pub fn format_size_and_lines(bytes: usize, lines: usize) -> String {
    let size = if bytes >= 1024 {
        format!("{:.1} kB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    };
    format!("{size} · {lines} lines")
}

/// Precomputes a collapsed single-line preview for thinking blocks.
#[must_use]
pub fn collapse_preview(content: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(max_chars.min(content.len()));
    let mut prev_space = true;
    for ch in content.chars() {
        let is_ws = ch.is_whitespace();
        if is_ws {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
        if out.chars().count() >= max_chars {
            break;
        }
    }
    out.trim().to_owned()
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
    pub sessions: Vec<SidebarSession>,
    /// Current prompt composer text.
    pub input: String,
    /// Transcript items in the active session.
    pub items: Vec<TranscriptItem>,
    /// Scrollable widget ID for auto-scrolling the transcript.
    pub transcript_id: Id,
    /// Optional right-hand split inspector drawer.
    pub inspector: Option<InspectorPane>,
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
        let cached_models = session.cached_models.clone();
        let task_registry = session.task_registry.clone();
        let sessions = list_sessions(&paths)
            .unwrap_or_default()
            .iter()
            .map(SidebarSession::from_summary)
            .collect();

        // Take ownership of `task_rx` before moving `session` into `WorkerHandle`.
        let (_dummy_tx, dummy_rx) = mpsc::channel();
        let task_rx = std::mem::replace(&mut session.task_rx, dummy_rx);

        let (worker, _rx) = WorkerHandle::spawn(session);
        worker.send(crate::worker::WorkerCommand::FetchModels);
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
            transcript_id: Id::unique(),
            inspector: None,
            busy: false,
            busy_flag: Arc::new(AtomicBool::new(false)),
            busy_status: "Thinking…".to_owned(),
            busy_started_at: None,
            last_md_parse: Instant::now(),
            spinner_tick: 0,
            thinking_started_at: None,
            model_picker_open: false,
            available_models: cached_models,
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
                    self.items.push(TranscriptItem::user(text));
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
        self.sessions = list_sessions(&self.paths)
            .unwrap_or_default()
            .iter()
            .map(SidebarSession::from_summary)
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_user_classifies_short_long_and_pasted_prompts() {
        let short = TranscriptItem::user("hello rein".to_owned());
        assert!(matches!(
            short,
            TranscriptItem::User {
                is_long: false,
                is_pasted: false,
                ..
            }
        ));

        let long_lines = (0..8).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let long = TranscriptItem::user(long_lines);
        assert!(matches!(
            long,
            TranscriptItem::User {
                is_long: true,
                is_pasted: false,
                ..
            }
        ));

        let pasted_lines = (0..20)
            .map(|i| format!("log line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let pasted = TranscriptItem::user(pasted_lines);
        assert!(matches!(
            pasted,
            TranscriptItem::User {
                is_pasted: true,
                is_long: false,
                ..
            }
        ));
    }

    #[test]
    fn collapse_preview_normalizes_whitespace_and_truncates() {
        let collapsed = collapse_preview("hello   world\n\nfrom   rein", 15);
        assert_eq!(collapsed, "hello world fro");
    }
}
