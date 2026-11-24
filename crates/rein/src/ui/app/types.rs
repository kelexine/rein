// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Minimalist borderless TUI state driving Core, Policy, Tools, Modes, Slash commands, and Model picker

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, mpsc};

use rein_core::AvailableModel;
use rein_tools::{QuestionAnswer, QuestionSpec};

use crate::agent::{ApprovalRequest, TurnCheckpoint};
use crate::config::{ProviderKind, SessionSummary};

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
