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
    /// Read-only architecture exploration and implementation planning mode.
    Plan,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_enums_serialize_and_deserialize_in_snake_case()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(serde_json::to_string(&ModeId::Coding)?, "\"coding\"");
        assert_eq!(serde_json::to_string(&ModeId::Plan)?, "\"plan\"");
        assert_eq!(serde_json::to_string(&ModeId::Personal)?, "\"personal\"");
        assert_eq!(
            serde_json::from_str::<ModeId>("\"personal\"")?,
            ModeId::Personal
        );

        assert_eq!(serde_json::to_string(&MemoryScope::Project)?, "\"project\"");
        assert_eq!(
            serde_json::to_string(&MemoryScope::Personal)?,
            "\"personal\""
        );
        assert_eq!(
            serde_json::from_str::<MemoryScope>("\"project\"")?,
            MemoryScope::Project
        );

        assert_eq!(serde_json::to_string(&Role::System)?, "\"system\"");
        assert_eq!(serde_json::to_string(&Role::User)?, "\"user\"");
        assert_eq!(serde_json::to_string(&Role::Assistant)?, "\"assistant\"");
        assert_eq!(serde_json::to_string(&Role::Tool)?, "\"tool\"");
        assert_eq!(serde_json::from_str::<Role>("\"tool\"")?, Role::Tool);
        Ok(())
    }
}
