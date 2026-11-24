// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-19
// Purpose: Session-scoped TaskRegistry managing backgrounded bash process groups, timers, crons, and wakeup events

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::Duration;

use rein_policy::{SandboxSpec, SpawnedSandboxProcess, spawn_sandboxed_command};
use serde::{Deserialize, Serialize};

use super::io_helpers::*;
use crate::registry::ToolError;
use crate::tasks::cron::CronSchedule;

/// Bounded per-task output tail retained in memory (64 KB).
pub const TASK_OUTPUT_CAP_BYTES: usize = 64 * 1024;
/// Minimum allowed `wait_ms_before_async` threshold (ms).
pub const MIN_WAIT_MS_BEFORE_ASYNC: u64 = 500;
/// Maximum allowed `wait_ms_before_async` threshold (ms).
pub const MAX_WAIT_MS_BEFORE_ASYNC: u64 = 10_000;
/// Default `wait_ms_before_async` wait before auto-backgrounding long-running `bash` commands.
pub const DEFAULT_WAIT_MS_BEFORE_ASYNC: u64 = 10_000;
/// Maximum random jitter (seconds) added to non-exact cron triggers.
const CRON_DEFAULT_JITTER_SECS: u64 = 4;

/// Category of background task managed by [`TaskRegistry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// Backgrounded or daemon sandboxed shell process.
    Shell,
    /// One-shot timer (`schedule` with `duration_seconds`).
    Timer,
    /// Recurring 5-field cron schedule (`schedule` with `cron_expression`).
    Cron,
}

/// Execution lifecycle status of a registered task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Task is actively executing or waiting to fire.
    Running,
    /// Shell task exited with code `0`.
    Completed,
    /// Shell task exited with a non-zero status code.
    Failed,
    /// Task was terminated via `kill`.
    Killed,
    /// Timer or finite-iteration cron completed all scheduled triggers.
    Fired,
    /// Timer was cancelled early by an incoming `timer_condition` event.
    Cancelled,
}

impl TaskStatus {
    /// Returns `true` when the task has reached a terminal state and can be deleted (`x` in `/tasks`).
    #[must_use]
    pub fn is_done(self) -> bool {
        !matches!(self, Self::Running)
    }

    /// Human-readable status word rendered in UI and tool outputs.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Killed => "killed",
            Self::Fired => "fired",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Early cancellation rule for one-shot timers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TimerCondition {
    /// Never cancel early; wait full `duration_seconds` unless explicitly killed.
    #[default]
    Never,
    /// Cancel early if any inbox event / message arrives before timer expiry.
    Any,
    /// Cancel early if a message or exit notification from a specific sender/task ID arrives.
    Sender(String),
}

impl TimerCondition {
    /// Parses a string `"never"`, `"any"`, or explicit sender/task ID.
    #[must_use]
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(str::trim).filter(|s| !s.is_empty()) {
            None => Self::Never,
            Some(s) if s.eq_ignore_ascii_case("never") => Self::Never,
            Some(s) if s.eq_ignore_ascii_case("any") => Self::Any,
            Some(sender) => Self::Sender(sender.to_owned()),
        }
    }

    fn matches(&self, sender_id: &str) -> bool {
        match self {
            Self::Never => false,
            Self::Any => true,
            Self::Sender(target) => target == sender_id,
        }
    }
}

/// Public snapshot of one task rendered in the TUI strip, `/tasks` view, Task Details, and `manage_task`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSummary {
    /// Unique task identifier (e.g. `task-1`).
    pub id: String,
    /// Task type (`Shell`, `Timer`, or `Cron`).
    pub kind: TaskKind,
    /// Current lifecycle status.
    pub status: TaskStatus,
    /// Command string or human-readable timer/cron label.
    pub label: String,
    /// Prompt delivered when a timer or cron fires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Whether this shell command was started with `is_daemon: true`.
    pub is_daemon: bool,
    /// Start Unix timestamp (seconds).
    pub started_unix_secs: u64,
    /// Local wall-clock `HH:MM:SS` start time.
    pub started_hms: String,
    /// Local wall-clock `YYYY-MM-DD HH:MM:SS` start timestamp.
    pub started_full: String,
    /// Completion Unix timestamp (seconds), if terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_unix_secs: Option<u64>,
    /// Exit status code for shell processes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Disk log file path for streamed shell stdout/stderr.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_path: Option<PathBuf>,
    /// Tail excerpt of captured output.
    pub output_tail: String,
}

/// Asynchronous notification emitted by background tasks to wake up the agent loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskNotification {
    /// Backgrounded shell command finished or crashed.
    TaskExited {
        /// Task identifier (`task-N`).
        task_id: String,
        /// Command summary.
        command: String,
        /// Terminal status (`Completed` or `Failed`).
        status: TaskStatus,
        /// Process exit code.
        exit_code: Option<i32>,
        /// Tail excerpt of stdout/stderr.
        tail: String,
        /// Streamed log path on disk.
        log_path: Option<PathBuf>,
    },
    /// Timer or cron trigger fired with a wakeup prompt.
    ScheduleFired {
        /// Task identifier (`task-N`).
        task_id: String,
        /// Human-readable schedule label.
        label: String,
        /// Prompt to inject into the agent loop.
        prompt: String,
        /// 1-based iteration count.
        iteration: u64,
        /// `true` when this trigger completed the schedule (one-shot or `max_iterations` reached).
        finished: bool,
    },
    /// Background subagent finished a turn or sent a message to the parent inbox.
    SubagentMessage {
        /// Subagent conversation identifier (`subagent-...`).
        conversation_id: String,
        /// Subagent role title.
        role: String,
        /// Subagent type (`research`, `plan`, `coder`, `personal`, or custom).
        agent_type: String,
        /// Message or completion report delivered to the parent session.
        content: String,
        /// `true` when the subagent terminated or errored rather than waiting for follow-up.
        finished: bool,
    },
}

/// Result returned by [`TaskRegistry::spawn_bash_task`] after racing `wait_ms_before_async`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BashSpawnOutcome {
    /// Process completed within `wait_ms_before_async`.
    Completed {
        /// Process exit status code.
        exit_code: i32,
        /// Captured stdout excerpt.
        stdout: String,
        /// Captured stderr excerpt.
        stderr: String,
        /// Whether either stream exceeded inline caps.
        truncated: bool,
        /// Streamed log file path on disk.
        log_path: Option<PathBuf>,
    },
    /// Process exceeded `wait_ms_before_async` (or `is_daemon = true`) and continues running in background.
    Backgrounded {
        /// Assigned task ID (`task-N`).
        task_id: String,
        /// Whether started with `is_daemon = true`.
        is_daemon: bool,
        /// Partial combined stdout/stderr captured so far.
        partial_output: String,
        /// Streamed log file path on disk.
        log_path: Option<PathBuf>,
    },
}

pub(super) enum TaskRuntime {
    Shell {
        proc: Arc<SpawnedSandboxProcess>,
        ring: Arc<Mutex<Vec<u8>>>,
    },
    Timer {
        condition: TimerCondition,
        cancel: Arc<AtomicBool>,
        wake: Arc<(Mutex<bool>, Condvar)>,
    },
    Cron {
        cancel: Arc<AtomicBool>,
        wake: Arc<(Mutex<bool>, Condvar)>,
    },
}

pub(super) struct TaskEntry {
    pub(super) summary: TaskSummary,
    visible: bool,
    pub(super) runtime: TaskRuntime,
}

#[derive(Default)]
struct RegistryInner {
    tasks: BTreeMap<String, TaskEntry>,
    order: VecDeque<String>,
    inbox_tx: Option<mpsc::Sender<TaskNotification>>,
}

/// Thread-safe per-session registry of background shell tasks, one-shot timers, and cron schedules.
#[derive(Clone, Default)]
pub struct TaskRegistry {
    inner: Arc<Mutex<RegistryInner>>,
}

impl std::fmt::Debug for TaskRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskRegistry").finish_non_exhaustive()
    }
}

impl TaskRegistry {
    /// Creates an empty [`TaskRegistry`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Attaches an `mpsc::Sender<TaskNotification>` that wakes the idle agent loop when tasks exit or fire.
    pub fn set_inbox_tx(&self, tx: mpsc::Sender<TaskNotification>) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.inbox_tx = Some(tx);
        }
    }

    /// Broadcasts an arrival event to cancel any one-shot timer matching `TimerCondition`.
    pub fn notify_external_event(&self, sender_id: &str) {
        let mut to_wake = Vec::new();
        if let Ok(mut guard) = self.inner.lock() {
            for entry in guard.tasks.values_mut() {
                if entry.summary.status == TaskStatus::Running
                    && let TaskRuntime::Timer {
                        condition,
                        cancel,
                        wake,
                    } = &entry.runtime
                    && condition.matches(sender_id)
                {
                    cancel.store(true, Ordering::Release);
                    entry.summary.status = TaskStatus::Cancelled;
                    entry.summary.finished_unix_secs = Some(now_unix_secs());
                    to_wake.push(Arc::clone(wake));
                }
            }
        }
        for wake in to_wake {
            signal_condvar(&wake);
        }
    }

    /// Spawns a sandboxed command and races completion against `wait_ms_before_async` (no execution timeout once backgrounded).
    pub fn spawn_bash_task(
        &self,
        spec: SandboxSpec,
        command_label: String,
        wait_ms_before_async: Option<u64>,
        is_daemon: bool,
    ) -> Result<BashSpawnOutcome, ToolError> {
        let mut spawned = spawn_sandboxed_command(&spec)?;
        let (stdout_pipe, stderr_pipe) = spawned.take_stdout_stderr();
        let log_writer = spawned.log_writer();
        let log_path = spawned.log_path().to_path_buf();
        let proc = Arc::new(spawned);

        let task_id = next_task_id();
        let started = now_unix_secs();
        let (started_hms, started_full) = format_local_timestamps(started);
        let combined_ring = Arc::new(Mutex::new(Vec::new()));
        let stdout_buf = Arc::new(Mutex::new(Vec::new()));
        let stderr_buf = Arc::new(Mutex::new(Vec::new()));
        let visible_flag = Arc::new(AtomicBool::new(is_daemon));
        let done_signal = Arc::new((Mutex::new(false), Condvar::new()));

        let summary = TaskSummary {
            id: task_id.clone(),
            kind: TaskKind::Shell,
            status: TaskStatus::Running,
            label: command_label.clone(),
            prompt: None,
            is_daemon,
            started_unix_secs: started,
            started_hms,
            started_full,
            finished_unix_secs: None,
            exit_code: None,
            log_path: Some(log_path.clone()),
            output_tail: String::new(),
        };

        if let Ok(mut guard) = self.inner.lock() {
            guard.order.push_back(task_id.clone());
            guard.tasks.insert(
                task_id.clone(),
                TaskEntry {
                    summary,
                    visible: is_daemon,
                    runtime: TaskRuntime::Shell {
                        proc: Arc::clone(&proc),
                        ring: Arc::clone(&combined_ring),
                    },
                },
            );
        }

        let out_handle = {
            let log_w = Arc::clone(&log_writer);
            let s_buf = Arc::clone(&stdout_buf);
            let comb = Arc::clone(&combined_ring);
            std::thread::spawn(move || {
                drain_split(stdout_pipe, "[stdout]\n", &log_w, &s_buf, &comb)
            })
        };
        let err_handle = {
            let log_w = Arc::clone(&log_writer);
            let e_buf = Arc::clone(&stderr_buf);
            let comb = Arc::clone(&combined_ring);
            std::thread::spawn(move || {
                drain_split(stderr_pipe, "[stderr]\n", &log_w, &e_buf, &comb)
            })
        };

        {
            let reg = self.clone();
            let tid = task_id.clone();
            let proc_w = Arc::clone(&proc);
            let comb_w = Arc::clone(&combined_ring);
            let vis_w = Arc::clone(&visible_flag);
            let done_w = Arc::clone(&done_signal);
            let log_p = log_path.clone();
            let label_w = command_label;
            std::thread::spawn(move || {
                let _ = out_handle.join();
                let _ = err_handle.join();
                let exit_code = proc_w.wait().unwrap_or(-1);
                let tail = read_ring_tail(&comb_w, 40);
                let terminal = if proc_w.was_killed() {
                    TaskStatus::Killed
                } else if exit_code == 0 {
                    TaskStatus::Completed
                } else {
                    TaskStatus::Failed
                };
                let inbox_tx = reg.finish_shell_task(&tid, terminal, exit_code, tail.clone());
                signal_condvar(&done_w);

                if vis_w.load(Ordering::Acquire)
                    && !proc_w.was_killed()
                    && let Some(tx) = inbox_tx
                {
                    let _ = tx.send(TaskNotification::TaskExited {
                        task_id: tid,
                        command: label_w,
                        status: terminal,
                        exit_code: Some(exit_code),
                        tail,
                        log_path: Some(log_p),
                    });
                }
            });
        }

        if is_daemon {
            return Ok(BashSpawnOutcome::Backgrounded {
                task_id,
                is_daemon: true,
                partial_output: read_ring_tail(&combined_ring, 20),
                log_path: Some(log_path),
            });
        }

        let wait_ms = wait_ms_before_async
            .unwrap_or(DEFAULT_WAIT_MS_BEFORE_ASYNC)
            .clamp(MIN_WAIT_MS_BEFORE_ASYNC, MAX_WAIT_MS_BEFORE_ASYNC);
        let finished_in_time = wait_condvar_timeout(&done_signal, Duration::from_millis(wait_ms));
        if finished_in_time {
            let exit_code = self.take_completed_sync(&task_id).unwrap_or(-1);
            let (stdout, out_trunc) = read_stream_buffer(&stdout_buf);
            let (stderr, err_trunc) = read_stream_buffer(&stderr_buf);
            return Ok(BashSpawnOutcome::Completed {
                exit_code,
                stdout,
                stderr,
                truncated: out_trunc || err_trunc,
                log_path: Some(log_path),
            });
        }

        visible_flag.store(true, Ordering::Release);
        if let Ok(mut guard) = self.inner.lock()
            && let Some(entry) = guard.tasks.get_mut(&task_id)
        {
            entry.visible = true;
            entry.summary.output_tail = read_ring_tail(&combined_ring, 40);
        }
        Ok(BashSpawnOutcome::Backgrounded {
            task_id,
            is_daemon: false,
            partial_output: read_ring_tail(&combined_ring, 20),
            log_path: Some(log_path),
        })
    }

    /// Registers and starts a one-shot timer task in [`TaskRegistry`].
    pub fn schedule_timer(
        &self,
        duration_seconds: u64,
        prompt: String,
        condition: TimerCondition,
        label: Option<String>,
    ) -> Result<TaskSummary, ToolError> {
        if duration_seconds == 0 {
            return Err(ToolError::InvalidArguments {
                tool: "schedule".to_owned(),
                reason: "duration_seconds must be >= 1".to_owned(),
            });
        }
        let task_id = next_task_id();
        let started = now_unix_secs();
        let (started_hms, started_full) = format_local_timestamps(started);
        let display_label = label
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("{} timer", format_duration_short(duration_seconds)));
        let cancel = Arc::new(AtomicBool::new(false));
        let wake = Arc::new((Mutex::new(false), Condvar::new()));

        let summary = TaskSummary {
            id: task_id.clone(),
            kind: TaskKind::Timer,
            status: TaskStatus::Running,
            label: display_label.clone(),
            prompt: Some(prompt.clone()),
            is_daemon: false,
            started_unix_secs: started,
            started_hms,
            started_full,
            finished_unix_secs: None,
            exit_code: None,
            log_path: None,
            output_tail: String::new(),
        };

        if let Ok(mut guard) = self.inner.lock() {
            guard.order.push_back(task_id.clone());
            guard.tasks.insert(
                task_id.clone(),
                TaskEntry {
                    summary: summary.clone(),
                    visible: true,
                    runtime: TaskRuntime::Timer {
                        condition,
                        cancel: Arc::clone(&cancel),
                        wake: Arc::clone(&wake),
                    },
                },
            );
        }

        let reg = self.clone();
        let tid = task_id.clone();
        std::thread::spawn(move || {
            let _ = wait_condvar_timeout(&wake, Duration::from_secs(duration_seconds));
            if cancel.load(Ordering::Acquire) {
                return;
            }
            let inbox_tx = reg.mark_schedule_finished(&tid, TaskStatus::Fired, &prompt);
            if let Some(tx) = inbox_tx {
                let _ = tx.send(TaskNotification::ScheduleFired {
                    task_id: tid,
                    label: display_label,
                    prompt,
                    iteration: 1,
                    finished: true,
                });
            }
        });

        Ok(summary)
    }

    /// Registers and starts a recurring 5-field cron schedule in [`TaskRegistry`].
    pub fn schedule_cron(
        &self,
        cron_expression: &str,
        prompt: String,
        max_iterations: Option<u64>,
        exact_timing: bool,
        label: Option<String>,
    ) -> Result<TaskSummary, ToolError> {
        let sched =
            CronSchedule::parse(cron_expression).map_err(|reason| ToolError::InvalidArguments {
                tool: "schedule".to_owned(),
                reason,
            })?;
        let task_id = next_task_id();
        let started = now_unix_secs();
        let (started_hms, started_full) = format_local_timestamps(started);
        let display_label = label
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("cron: {}", sched.as_str()));
        let cancel = Arc::new(AtomicBool::new(false));
        let wake = Arc::new((Mutex::new(false), Condvar::new()));

        let summary = TaskSummary {
            id: task_id.clone(),
            kind: TaskKind::Cron,
            status: TaskStatus::Running,
            label: display_label.clone(),
            prompt: Some(prompt.clone()),
            is_daemon: false,
            started_unix_secs: started,
            started_hms,
            started_full,
            finished_unix_secs: None,
            exit_code: None,
            log_path: None,
            output_tail: String::new(),
        };

        if let Ok(mut guard) = self.inner.lock() {
            guard.order.push_back(task_id.clone());
            guard.tasks.insert(
                task_id.clone(),
                TaskEntry {
                    summary: summary.clone(),
                    visible: true,
                    runtime: TaskRuntime::Cron {
                        cancel: Arc::clone(&cancel),
                        wake: Arc::clone(&wake),
                    },
                },
            );
        }

        let reg = self.clone();
        let tid = task_id.clone();
        let jitter = if exact_timing {
            0
        } else {
            started % (CRON_DEFAULT_JITTER_SECS + 1)
        };
        std::thread::spawn(move || {
            let mut iteration = 0u64;
            while !cancel.load(Ordering::Acquire) {
                if let Some(max) = max_iterations
                    && iteration >= max
                {
                    break;
                }
                let now = now_unix_secs();
                let Some(next_ts) = sched.next_fire_after(now, jitter) else {
                    break;
                };
                let wait_secs = next_ts.saturating_sub(now).max(1);
                let _ = wait_condvar_timeout(&wake, Duration::from_secs(wait_secs));
                if cancel.load(Ordering::Acquire) {
                    return;
                }
                iteration += 1;
                let finished = max_iterations.is_some_and(|max| iteration >= max);
                let inbox_tx = reg.record_cron_trigger(&tid, iteration, &prompt, finished);
                if let Some(tx) = inbox_tx {
                    let _ = tx.send(TaskNotification::ScheduleFired {
                        task_id: tid.clone(),
                        label: display_label.clone(),
                        prompt: prompt.clone(),
                        iteration,
                        finished,
                    });
                }
                if finished {
                    return;
                }
            }
        });

        Ok(summary)
    }

    /// Lists all user-visible background tasks in creation order (refreshing live output tails).
    #[must_use]
    pub fn list_tasks(&self) -> Vec<TaskSummary> {
        let Ok(guard) = self.inner.lock() else {
            return Vec::new();
        };
        guard
            .order
            .iter()
            .filter_map(|id| guard.tasks.get(id))
            .filter(|entry| entry.visible)
            .map(refresh_summary_snapshot)
            .collect()
    }

    /// Returns a refreshed [`TaskSummary`] with up to `tail_lines` tail lines for `task_id`.
    pub fn task_status(&self, task_id: &str, tail_lines: usize) -> Result<TaskSummary, String> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| "task registry lock poisoned".to_owned())?;
        let entry = guard
            .tasks
            .get(task_id)
            .ok_or_else(|| format!("unknown task_id '{task_id}'"))?;
        let mut summary = refresh_summary_snapshot(entry);
        if let TaskRuntime::Shell { ring, .. } = &entry.runtime {
            summary.output_tail = read_ring_tail(ring, tail_lines.max(1));
        }
        Ok(summary)
    }

    /// Returns scrollable output lines for the TUI **Task Details** screen.
    #[must_use]
    pub fn task_output_lines(&self, task_id: &str) -> Vec<String> {
        let (log_path, fallback_tail, prompt_line) = {
            let Ok(guard) = self.inner.lock() else {
                return Vec::new();
            };
            let Some(entry) = guard.tasks.get(task_id) else {
                return Vec::new();
            };
            let tail = match &entry.runtime {
                TaskRuntime::Shell { ring, .. } => read_ring_tail(ring, 500),
                TaskRuntime::Timer { .. } | TaskRuntime::Cron { .. } => {
                    entry.summary.output_tail.clone()
                }
            };
            (
                entry.summary.log_path.clone(),
                tail,
                entry.summary.prompt.clone(),
            )
        };
        if let Some(path) = log_path
            && let Ok(raw) = std::fs::read_to_string(path)
        {
            let lines: Vec<String> = raw.lines().map(str::to_owned).collect();
            if !lines.is_empty() {
                return lines;
            }
        }
        if !fallback_tail.is_empty() {
            return fallback_tail.lines().map(str::to_owned).collect();
        }
        if let Some(prompt) = prompt_line {
            return vec![format!("Prompt: {prompt}")];
        }
        Vec::new()
    }

    /// Writes `input` bytes to a running shell task's stdin (model-driven via `manage_task`).
    pub fn send_input(&self, task_id: &str, input: &str) -> Result<(), String> {
        let proc = {
            let guard = self
                .inner
                .lock()
                .map_err(|_| "task registry lock poisoned".to_owned())?;
            let entry = guard
                .tasks
                .get(task_id)
                .ok_or_else(|| format!("unknown task_id '{task_id}'"))?;
            if entry.summary.status != TaskStatus::Running {
                return Err(format!(
                    "cannot send_input to task '{task_id}' in state '{}'",
                    entry.summary.status.label()
                ));
            }
            match &entry.runtime {
                TaskRuntime::Shell { proc, .. } => Arc::clone(proc),
                _ => {
                    return Err(format!(
                        "task '{task_id}' is not a shell process and has no stdin"
                    ));
                }
            }
        };
        proc.write_stdin(input.as_bytes())
            .map_err(|err| format!("failed writing stdin to '{task_id}': {err}"))
    }

    /// Terminates a running shell process group or cancels a running timer/cron.
    pub fn kill_task(&self, task_id: &str) -> Result<TaskSummary, String> {
        enum KillTarget {
            Shell(Arc<SpawnedSandboxProcess>),
            Schedule(Arc<AtomicBool>, Arc<(Mutex<bool>, Condvar)>),
        }

        let target = {
            let mut guard = self
                .inner
                .lock()
                .map_err(|_| "task registry lock poisoned".to_owned())?;
            let entry = guard
                .tasks
                .get_mut(task_id)
                .ok_or_else(|| format!("unknown task_id '{task_id}'"))?;
            if entry.summary.status != TaskStatus::Running {
                return Err(format!(
                    "task '{task_id}' is already {}",
                    entry.summary.status.label()
                ));
            }
            entry.summary.status = TaskStatus::Killed;
            entry.summary.finished_unix_secs = Some(now_unix_secs());
            match &entry.runtime {
                TaskRuntime::Shell { proc, .. } => KillTarget::Shell(Arc::clone(proc)),
                TaskRuntime::Timer { cancel, wake, .. } | TaskRuntime::Cron { cancel, wake } => {
                    KillTarget::Schedule(Arc::clone(cancel), Arc::clone(wake))
                }
            }
        };

        match target {
            KillTarget::Shell(proc) => proc.kill(),
            KillTarget::Schedule(cancel, wake) => {
                cancel.store(true, Ordering::Release);
                signal_condvar(&wake);
            }
        }
        self.task_status(task_id, 40)
    }

    /// Removes a completed/terminal task from `/tasks` (`x` action). Refuses if still `Running`.
    pub fn delete_done_task(&self, task_id: &str) -> Result<(), String> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| "task registry lock poisoned".to_owned())?;
        let Some(entry) = guard.tasks.get(task_id) else {
            return Err(format!("unknown task_id '{task_id}'"));
        };
        if !entry.summary.status.is_done() {
            return Err(format!(
                "cannot delete running task '{task_id}' (kill it first with 'k')"
            ));
        }
        guard.tasks.remove(task_id);
        guard.order.retain(|id| id != task_id);
        Ok(())
    }

    /// Kills all active tasks and cancels all running timers/crons cleanly before exit.
    pub fn shutdown_all(&self) {
        let running_ids: Vec<String> = self
            .list_tasks()
            .into_iter()
            .filter(|t| t.status == TaskStatus::Running)
            .map(|t| t.id)
            .collect();
        for id in running_ids {
            let _ = self.kill_task(&id);
        }
    }

    fn finish_shell_task(
        &self,
        task_id: &str,
        terminal: TaskStatus,
        exit_code: i32,
        tail: String,
    ) -> Option<mpsc::Sender<TaskNotification>> {
        let mut guard = self.inner.lock().ok()?;
        let inbox = guard.inbox_tx.clone();
        if let Some(entry) = guard.tasks.get_mut(task_id) {
            entry.summary.status = terminal;
            entry.summary.exit_code = Some(exit_code);
            entry.summary.finished_unix_secs = Some(now_unix_secs());
            entry.summary.output_tail = tail;
        }
        inbox
    }

    fn take_completed_sync(&self, task_id: &str) -> Option<i32> {
        let mut guard = self.inner.lock().ok()?;
        let entry = guard.tasks.remove(task_id)?;
        guard.order.retain(|id| id != task_id);
        entry.summary.exit_code
    }

    fn mark_schedule_finished(
        &self,
        task_id: &str,
        terminal: TaskStatus,
        prompt: &str,
    ) -> Option<mpsc::Sender<TaskNotification>> {
        let mut guard = self.inner.lock().ok()?;
        let inbox = guard.inbox_tx.clone();
        if let Some(entry) = guard.tasks.get_mut(task_id) {
            entry.summary.status = terminal;
            entry.summary.finished_unix_secs = Some(now_unix_secs());
            entry.summary.output_tail = format!("Fired prompt: {prompt}");
        }
        inbox
    }

    fn record_cron_trigger(
        &self,
        task_id: &str,
        iteration: u64,
        prompt: &str,
        finished: bool,
    ) -> Option<mpsc::Sender<TaskNotification>> {
        let mut guard = self.inner.lock().ok()?;
        let inbox = guard.inbox_tx.clone();
        if let Some(entry) = guard.tasks.get_mut(task_id) {
            entry.summary.output_tail = format!("Trigger #{iteration}: {prompt}");
            if finished {
                entry.summary.status = TaskStatus::Fired;
                entry.summary.finished_unix_secs = Some(now_unix_secs());
            }
        }
        inbox
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_fires_and_sends_notification() -> Result<(), Box<dyn std::error::Error>> {
        let reg = TaskRegistry::new();
        let (tx, rx) = mpsc::channel();
        reg.set_inbox_tx(tx);

        let summary =
            reg.schedule_timer(1, "check status".to_owned(), TimerCondition::Never, None)?;
        assert_eq!(summary.status, TaskStatus::Running);
        let ev = rx.recv_timeout(Duration::from_secs(3))?;
        assert!(matches!(
            ev,
            TaskNotification::ScheduleFired {
                task_id,
                finished: true,
                ..
            } if task_id == summary.id
        ));
        let final_status = reg.task_status(&summary.id, 10)?;
        assert_eq!(final_status.status, TaskStatus::Fired);
        reg.delete_done_task(&summary.id)?;
        assert!(reg.list_tasks().is_empty());
        Ok(())
    }

    #[test]
    fn timer_cancels_early_on_matching_condition_or_kill() -> Result<(), Box<dyn std::error::Error>>
    {
        let reg = TaskRegistry::new();
        let (tx, rx) = mpsc::channel();
        reg.set_inbox_tx(tx);

        let s1 = reg.schedule_timer(30, "ping".to_owned(), TimerCondition::Any, None)?;
        reg.notify_external_event("task-other");
        let st1 = reg.task_status(&s1.id, 10)?;
        assert_eq!(st1.status, TaskStatus::Cancelled);

        let s2 = reg.schedule_timer(30, "pong".to_owned(), TimerCondition::Never, None)?;
        let killed = reg.kill_task(&s2.id)?;
        assert_eq!(killed.status, TaskStatus::Killed);
        assert!(rx.try_recv().is_err());
        Ok(())
    }

    #[test]
    fn bash_task_auto_backgrounds_accepts_send_input_and_emits_exit_wakeup()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join(format!("rein_task_bg_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;

        let reg = TaskRegistry::new();
        let (tx, rx) = mpsc::channel();
        reg.set_inbox_tx(tx);

        let spec = SandboxSpec {
            workspace_root: dir.clone(),
            program: "sh".to_owned(),
            args: vec!["-c".to_owned(), "read -r line; echo GOT:$line".to_owned()],
            output_dir: Some(dir.join("logs")),
        };
        let outcome = reg.spawn_bash_task(
            spec,
            "read -r line; echo GOT:$line".to_owned(),
            Some(500),
            false,
        )?;
        let BashSpawnOutcome::Backgrounded { task_id, .. } = outcome else {
            return Err("expected command waiting on stdin to background after 500ms".into());
        };

        reg.send_input(&task_id, "hello-task\n")?;
        let ev = rx.recv_timeout(Duration::from_secs(3))?;
        assert!(matches!(
            ev,
            TaskNotification::TaskExited {
                status: TaskStatus::Completed,
                exit_code: Some(0),
                ref tail,
                ..
            } if tail.contains("GOT:hello-task")
        ));

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
