// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Mode identifiers, memory scopes, and conversational roles

use serde::{Deserialize, Serialize};

/// Operating mode identifier used by profiles without coupling core to mode implementations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeId {
    /// Software engineering and workspace manipulation mode.
    Coding,
    /// Interactive personal assistant mode (memory, notes, calendar, email).
    Personal,
}

/// Logical memory partition enforced by the policy layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    /// Repository or workspace-local memory.
    Project,
    /// Cross-session personal preferences, notes, and context.
    Personal,
}

/// Conversational role in a model turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// System prompt instruction.
    System,
    /// End-user message.
    User,
    /// Assistant response.
    Assistant,
    /// Tool execution output returned to the model.
    Tool,
}
