// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Typestate marker structs for Turn transitions

use crate::message::ToolCall;

/// Typestate marker: turn is idle and waiting for user input.
#[derive(Debug)]
pub struct Idle;

/// Typestate marker: turn is actively streaming a model response.
#[derive(Debug)]
pub struct Streaming {
    pub(crate) accumulated_text: String,
}

/// Typestate marker: model requested a tool call that must pass policy evaluation first.
#[derive(Debug)]
pub struct AwaitingPolicy {
    pub(crate) pending_call: ToolCall,
}

/// Typestate marker: policy explicitly approved the tool call for sandboxed execution.
#[derive(Debug)]
pub struct Executing {
    pub(crate) approved_call: ToolCall,
}
