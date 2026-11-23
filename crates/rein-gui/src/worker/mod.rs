// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Worker module exports for rein-gui

/// Background worker thread owning `AgentLoop` and `Provider`.
pub mod bridge;
/// Typed commands, events, and interactive approval/question prompters.
pub mod events;
/// Iced subscription polling worker events, task wakeups, and keyboard shortcuts.
pub mod subscription;

pub use bridge::WorkerHandle;
pub use events::{PendingApprovalPrompt, PendingQuestionPrompt, WorkerCommand, WorkerEvent};
pub use subscription::app_subscription;
