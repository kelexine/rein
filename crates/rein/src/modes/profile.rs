// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Mode Profile bundling tools, policy rules, system prompt, memory scopes, and context strategy

use rein_core::{MemoryScope, ModeId};
use rein_policy::PolicyRuleSet;
use serde::{Deserialize, Serialize};

use crate::modes::ContextStrategy;

/// Tool names enabled in the Coding profile.
pub const CODING_TOOLS: &[&str] = &[
    "read",
    "write",
    "edit",
    "glob",
    "grep",
    "bash",
    "memory",
    "todo_write",
    "todo_read",
];

/// Tool names enabled in the Personal profile.
pub const PERSONAL_TOOLS: &[&str] = &[
    "memory",
    "notes",
    "calendar",
    "email",
    "todo_write",
    "todo_read",
];

/// A self-contained mode profile keeping `rein-core` completely mode-agnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// Mode identifier (`Coding` or `Personal`).
    pub mode: ModeId,
    /// System prompt injected into provider turns.
    pub system_prompt: String,
    /// Tool names enabled in this profile.
    pub allowed_tools: Vec<String>,
    /// Policy rules enforced when this profile is active.
    pub policy_rules: PolicyRuleSet,
    /// Memory scopes injected into context (Coding reads Personal + Project).
    pub readable_memory_scopes: Vec<MemoryScope>,
    /// Memory scopes the profile writes to without confirmation.
    pub writable_memory_scopes: Vec<MemoryScope>,
    /// Context window strategy for this profile.
    pub context_strategy: ContextStrategy,
}

impl Profile {
    /// Coding profile: workspace tools, sees Project + Personal memory, writes Project memory.
    #[must_use]
    pub fn coding() -> Self {
        Self {
            mode: ModeId::Coding,
            system_prompt:
                "You are Rein in Coding mode. Work inside the workspace sandbox, prefer \
                            read-only inspection before edits, use todo_write to track multi-step \
                            tasks, and keep changes minimal and verified."
                    .to_owned(),
            allowed_tools: to_owned_list(CODING_TOOLS),
            policy_rules: PolicyRuleSet::coding_default(),
            readable_memory_scopes: vec![MemoryScope::Project, MemoryScope::Personal],
            writable_memory_scopes: vec![MemoryScope::Project],
            context_strategy: ContextStrategy::coding(),
        }
    }

    /// Personal profile: memory, notes, calendar, and email behind permissions.
    #[must_use]
    pub fn personal() -> Self {
        Self {
            mode: ModeId::Personal,
            system_prompt: "You are Rein in Personal mode. Help with notes, calendar, and email. \
                            Always confirm before sending email or changing calendar events."
                .to_owned(),
            allowed_tools: to_owned_list(PERSONAL_TOOLS),
            policy_rules: PolicyRuleSet::personal_default(),
            readable_memory_scopes: vec![MemoryScope::Personal],
            writable_memory_scopes: vec![MemoryScope::Personal],
            context_strategy: ContextStrategy::personal(),
        }
    }

    /// Returns the built-in profile for `mode`.
    #[must_use]
    pub fn for_mode(mode: ModeId) -> Self {
        match mode {
            ModeId::Coding => Self::coding(),
            ModeId::Personal => Self::personal(),
        }
    }

    /// Returns `true` if `tool_name` is enabled in this profile.
    #[must_use]
    pub fn allows_tool(&self, tool_name: &str) -> bool {
        self.allowed_tools.iter().any(|t| t == tool_name)
    }

    /// Attaches global + repository `commands.json` allowlisted shell commands to this profile.
    #[must_use]
    pub fn with_allowed_commands(mut self, commands: Vec<String>) -> Self {
        self.policy_rules.allowed_commands = commands;
        self
    }

    /// Appends additional tool names (such as discovered MCP tools) to `allowed_tools`.
    #[must_use]
    pub fn with_extra_tools(mut self, extra: &[String]) -> Self {
        for name in extra {
            if !self.allowed_tools.iter().any(|t| t == name) {
                self.allowed_tools.push(name.clone());
            }
        }
        self
    }
}

fn to_owned_list(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}
