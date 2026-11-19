// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Public exports for the in-process event-sourced agent loop

/// In-process agent loop orchestrating Core, Policy, Tools, and Modes.
pub mod runner;
/// Per-turn workspace snapshots and `/rewind` restoration.
pub mod snapshot;
/// Policy-confined sub-agent tool (`agent`), live TUI status tracker, and child runner.
pub mod subagent;

pub use runner::{
    AgentLoop, ApprovalAction, ApprovalDecision, ApprovalPrompter, ApprovalRequest, HydratedEvent,
    MAX_TOOL_ROUNDS,
};
pub use snapshot::{
    TurnCheckpoint, WorkspaceSnapshot, capture_workspace_snapshot, format_relative_age,
    now_unix_secs, restore_workspace_snapshot,
};
pub use subagent::{
    AgentTool, DEFAULT_SUBAGENT_MAX_ROUNDS, LiveSubagentStatus, ProviderFactory, SubagentEnv,
    SubagentRunState, SubagentSessionState, SubagentTracker,
};
