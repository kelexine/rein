// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: PolicyDecision outcome and mode-aware PolicyRuleSet configuration

use rein_core::ModeId;
use serde::{Deserialize, Serialize};

/// Outcome of evaluating a [`crate::capability::Capability`] against the active [`PolicyRuleSet`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum PolicyDecision {
    /// Execution is permitted automatically within the sandbox.
    Allow {
        /// Reason recorded in the audit log.
        reason: String,
    },
    /// Interactive user confirmation required in the TUI before proceeding.
    Ask {
        /// Prompt displayed to the user explaining the requested capability.
        prompt: String,
    },
    /// Execution is strictly forbidden by the active profile policy.
    Deny {
        /// Reason for denial returned to the model and recorded in the audit log.
        reason: String,
    },
}

/// Declarative rule set bundled inside a mode profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyRuleSet {
    /// Mode owning this rule set.
    pub mode: ModeId,
    /// Whether reading personal memory is permitted in this mode (true for both Coding and Personal).
    pub allow_personal_memory_read: bool,
    /// Whether writing personal memory is permitted without interactive confirmation.
    pub allow_personal_memory_write: bool,
    /// Whether workspace filesystem writes are permitted without interactive confirmation.
    pub allow_workspace_write: bool,
    /// Legacy compatibility flag (`false`: shell commands inherit user configs under PolicyEngine).
    #[serde(default)]
    pub require_kernel_sandbox: bool,
    /// Merged global (`~/.rein/commands.json`) and session allowlist.
    #[serde(default)]
    pub allowed_commands: Vec<String>,
}

impl PolicyRuleSet {
    /// Default policy for Coding mode (reads personal memory, gates shell writes/exec via classifier + allowlists).
    #[must_use]
    pub fn coding_default() -> Self {
        Self {
            mode: ModeId::Coding,
            allow_personal_memory_read: true,
            allow_personal_memory_write: false,
            allow_workspace_write: true,
            require_kernel_sandbox: false,
            allowed_commands: Vec::new(),
        }
    }

    /// Default policy for Plan mode (read-only exploration; denies workspace writes and mutating shell commands).
    #[must_use]
    pub fn plan_default() -> Self {
        Self {
            mode: ModeId::Plan,
            allow_personal_memory_read: true,
            allow_personal_memory_write: false,
            allow_workspace_write: false,
            require_kernel_sandbox: false,
            allowed_commands: Vec::new(),
        }
    }

    /// Default policy for Personal mode (full personal memory, asks on email/calendar writes).
    #[must_use]
    pub fn personal_default() -> Self {
        Self {
            mode: ModeId::Personal,
            allow_personal_memory_read: true,
            allow_personal_memory_write: true,
            allow_workspace_write: false,
            require_kernel_sandbox: false,
            allowed_commands: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_rule_sets_enforce_mode_least_privilege() {
        let coding = PolicyRuleSet::coding_default();
        assert_eq!(coding.mode, ModeId::Coding);
        assert!(coding.allow_personal_memory_read);
        assert!(!coding.allow_personal_memory_write);
        assert!(coding.allow_workspace_write);
        assert!(!coding.require_kernel_sandbox);

        let plan = PolicyRuleSet::plan_default();
        assert_eq!(plan.mode, ModeId::Plan);
        assert!(plan.allow_personal_memory_read);
        assert!(!plan.allow_personal_memory_write);
        assert!(!plan.allow_workspace_write);
        assert!(!plan.require_kernel_sandbox);

        let personal = PolicyRuleSet::personal_default();
        assert_eq!(personal.mode, ModeId::Personal);
        assert!(personal.allow_personal_memory_read);
        assert!(personal.allow_personal_memory_write);
        assert!(!personal.allow_workspace_write);
        assert!(!personal.require_kernel_sandbox);
    }

    #[test]
    fn policy_decision_serializes_with_decision_tag() -> Result<(), Box<dyn std::error::Error>> {
        let ask = PolicyDecision::Ask {
            prompt: "Confirm?".to_owned(),
        };
        let encoded = serde_json::to_string(&ask)?;
        assert!(encoded.contains("\"decision\":\"ask\""));
        let decoded: PolicyDecision = serde_json::from_str(&encoded)?;
        assert_eq!(decoded, ask);
        Ok(())
    }
}
