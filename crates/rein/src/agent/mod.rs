// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Public exports for the in-process event-sourced agent loop

/// In-process agent loop orchestrating Core, Policy, Tools, and Modes.
pub mod runner;

pub use runner::{AgentLoop, ApprovalPrompter, MAX_TOOL_ROUNDS};
