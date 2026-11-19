// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Mode Profile bundling tools, policy rules, system prompt, memory scopes, and context strategy

use rein_core::{MemoryScope, ModeId};
use rein_policy::PolicyRuleSet;
use serde::{Deserialize, Serialize};

use crate::modes::{ContextStrategy, prompts};

/// Tool names enabled in the Coding profile.
pub const CODING_TOOLS: &[&str] = &[
    "read",
    "write",
    "edit",
    "glob",
    "grep",
    "bash",
    "manage_task",
    "schedule",
    "memory",
    "todo_write",
    "todo_read",
    "switch_mode",
    "agent",
];

/// Tool names enabled in the read-only Plan profile.
pub const PLAN_TOOLS: &[&str] = &[
    "read",
    "glob",
    "grep",
    "bash",
    "manage_task",
    "schedule",
    "memory",
    "todo_write",
    "todo_read",
    "switch_mode",
    "agent",
];

/// Tool names enabled in the Personal profile.
pub const PERSONAL_TOOLS: &[&str] = &[
    "memory",
    "notes",
    "calendar",
    "email",
    "manage_task",
    "schedule",
    "todo_write",
    "todo_read",
    "switch_mode",
    "agent",
];

/// A self-contained mode profile keeping `rein-core` completely mode-agnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// Mode identifier (`Coding`, `Plan`, or `Personal`).
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
            system_prompt: prompts::build_system_prompt(ModeId::Coding),
            allowed_tools: to_owned_list(CODING_TOOLS),
            policy_rules: PolicyRuleSet::coding_default(),
            readable_memory_scopes: vec![MemoryScope::Project, MemoryScope::Personal],
            writable_memory_scopes: vec![MemoryScope::Project],
            context_strategy: ContextStrategy::coding(),
        }
    }

    /// Plan profile: strictly read-only inspection and architectural planning before coding changes.
    #[must_use]
    pub fn plan() -> Self {
        Self {
            mode: ModeId::Plan,
            system_prompt: prompts::build_system_prompt(ModeId::Plan),
            allowed_tools: to_owned_list(PLAN_TOOLS),
            policy_rules: PolicyRuleSet::plan_default(),
            readable_memory_scopes: vec![MemoryScope::Project, MemoryScope::Personal],
            writable_memory_scopes: vec![MemoryScope::Project],
            context_strategy: ContextStrategy::plan(),
        }
    }

    /// Personal profile: memory, notes, calendar, and email behind permissions.
    #[must_use]
    pub fn personal() -> Self {
        Self {
            mode: ModeId::Personal,
            system_prompt: prompts::build_system_prompt(ModeId::Personal),
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
            ModeId::Plan => Self::plan(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_enforce_distinct_tool_and_memory_boundaries() {
        let coding = Profile::for_mode(ModeId::Coding);
        assert!(coding.allows_tool("read"));
        assert!(coding.allows_tool("write"));
        assert!(coding.allows_tool("edit"));
        assert!(coding.allows_tool("bash"));
        assert!(!coding.allows_tool("notes"));
        assert_eq!(
            coding.readable_memory_scopes,
            vec![MemoryScope::Project, MemoryScope::Personal]
        );
        assert_eq!(coding.writable_memory_scopes, vec![MemoryScope::Project]);

        let plan = Profile::for_mode(ModeId::Plan);
        assert!(plan.allows_tool("read"));
        assert!(plan.allows_tool("grep"));
        assert!(plan.allows_tool("bash"));
        assert!(!plan.allows_tool("write"));
        assert!(!plan.allows_tool("edit"));
        assert_eq!(
            plan.readable_memory_scopes,
            vec![MemoryScope::Project, MemoryScope::Personal]
        );

        let personal = Profile::for_mode(ModeId::Personal);
        assert!(personal.allows_tool("memory"));
        assert!(personal.allows_tool("notes"));
        assert!(personal.allows_tool("calendar"));
        assert!(personal.allows_tool("email"));
        assert!(!personal.allows_tool("bash"));
        assert!(!personal.allows_tool("read"));
        assert_eq!(personal.readable_memory_scopes, vec![MemoryScope::Personal]);
        assert_eq!(personal.writable_memory_scopes, vec![MemoryScope::Personal]);
    }

    #[test]
    fn with_extra_tools_deduplicates_and_with_allowed_commands_sets_rules() {
        let profile = Profile::coding()
            .with_extra_tools(&[
                "read".to_owned(),
                "mcp__github__search".to_owned(),
                "mcp__github__search".to_owned(),
            ])
            .with_allowed_commands(vec!["cargo test".to_owned()]);

        let read_count = profile
            .allowed_tools
            .iter()
            .filter(|t| t.as_str() == "read")
            .count();
        let mcp_count = profile
            .allowed_tools
            .iter()
            .filter(|t| t.as_str() == "mcp__github__search")
            .count();
        assert_eq!(read_count, 1);
        assert_eq!(mcp_count, 1);
        assert_eq!(
            profile.policy_rules.allowed_commands,
            vec!["cargo test".to_owned()]
        );
    }
}
