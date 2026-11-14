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
    /// Fail-closed default: refuse shell execution if kernel sandboxing (Landlock) is unavailable.
    pub require_kernel_sandbox: bool,
}

impl PolicyRuleSet {
    /// Default sandbox policy for Coding mode (requires Landlock for shell, reads personal memory).
    #[must_use]
    pub fn coding_default() -> Self {
        Self {
            mode: ModeId::Coding,
            allow_personal_memory_read: true,
            allow_personal_memory_write: false,
            allow_workspace_write: true,
            require_kernel_sandbox: true,
        }
    }

    /// Default sandbox policy for Personal mode (full personal memory, asks on email/calendar writes).
    #[must_use]
    pub fn personal_default() -> Self {
        Self {
            mode: ModeId::Personal,
            allow_personal_memory_read: true,
            allow_personal_memory_write: true,
            allow_workspace_write: false,
            require_kernel_sandbox: true,
        }
    }
}
