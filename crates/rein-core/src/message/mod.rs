// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Public exports for core message, role, scope, and tool call types

/// Mode identifiers, memory scopes, and conversational roles.
pub mod scope;
/// Tool call request and output structures.
pub mod tool;
/// Conversation transcript message structure.
pub mod transcript;

pub use scope::{MemoryScope, ModeId, Role};
pub use tool::{ToolCall, ToolOutput};
pub use transcript::Message;
