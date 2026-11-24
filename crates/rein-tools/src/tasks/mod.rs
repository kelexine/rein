// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-19
// Purpose: Session TaskRegistry, cron parser, manage_task tool, and schedule tool

/// Standard 5-field cron expression parser and local wall-clock evaluator.
pub mod cron;
mod io_helpers;
/// Model-facing `manage_task` tool (`list`, `status`, `send_input`, `kill`).
pub mod manage;
/// Thread-safe background shell, timer, and cron registry.
pub mod registry;
/// Time-based `schedule` tool for timers and crons.
pub mod schedule;

pub use cron::CronSchedule;
pub use manage::ManageTaskTool;
pub use registry::{
    BashSpawnOutcome, DEFAULT_WAIT_MS_BEFORE_ASYNC, MAX_WAIT_MS_BEFORE_ASYNC,
    MIN_WAIT_MS_BEFORE_ASYNC, TASK_OUTPUT_CAP_BYTES, TaskKind, TaskNotification, TaskRegistry,
    TaskStatus, TaskSummary, TimerCondition,
};
pub use schedule::ScheduleTool;
