// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Conversation transcript message representation

use serde::{Deserialize, Serialize};

use crate::message::{Role, ToolCall};

/// A single message in the conversation context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// Sender role.
    pub role: Role,
    /// Textual message content.
    pub content: String,
    /// Optional tool calls requested by the assistant in this message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
}

impl Message {
    /// Constructs a plain text message for the given [`Role`].
    #[must_use]
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
        }
    }
}
