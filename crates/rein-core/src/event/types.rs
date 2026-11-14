// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Immutable SessionEvent enum persisted to JSONL logs

use serde::{Deserialize, Serialize};

use crate::message::{MemoryScope, Message, ModeId, ToolCall, ToolOutput};

/// Immutable event recorded in the JSONL session log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEvent {
    /// Active profile switched between coding and personal modes.
    ModeSwitched {
        /// New active mode.
        mode: ModeId,
    },
    /// Message appended to the session transcript.
    MessageRecorded {
        /// Recorded message.
        message: Message,
    },
    /// Streaming text delta emitted by the model provider.
    StreamDelta {
        /// Incremental text chunk.
        delta: String,
    },
    /// Model requested a tool execution.
    ToolRequested {
        /// Requested tool call details.
        call: ToolCall,
    },
    /// Policy layer evaluated a tool call capability request.
    PolicyEvaluated {
        /// Tool call identifier.
        call_id: String,
        /// Whether execution was permitted.
        allowed: bool,
        /// Human-readable audit reason.
        reason: String,
    },
    /// Tool execution finished and returned output.
    ToolCompleted {
        /// Result of the tool call.
        output: ToolOutput,
    },
    /// Memory entry read or written across scopes.
    MemoryAccessed {
        /// Target memory scope.
        scope: MemoryScope,
        /// Operation kind (`read` or `write`).
        operation: String,
        /// Key or query string.
        key: String,
    },
}
