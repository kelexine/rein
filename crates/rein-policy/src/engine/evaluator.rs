// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: PolicyEngine evaluating capabilities against kernel sandbox status, classifier, allowlists, and audit log

use rein_core::{MemoryScope, ModeId};

use crate::audit::{AuditEntry, AuditLog};
use crate::capability::{AccessMode, Capability, PersonalResource};
use crate::classifier::{EffectLevel, ShellClassifier, TargetScope};
use crate::engine::{PolicyDecision, PolicyError, PolicyRuleSet};
use crate::sandbox::{SandboxStatus, WorkspaceSandbox, probe_kernel_sandbox};

/// Sandbox-first policy evaluator that logs every capability check to the JSONL [`AuditLog`].
#[derive(Debug)]
pub struct PolicyEngine {
    rules: PolicyRuleSet,
    sandbox: WorkspaceSandbox,
    sandbox_status: SandboxStatus,
    audit: AuditLog,
}

impl PolicyEngine {
    /// Constructs a [`PolicyEngine`] and probes kernel Landlock availability at creation.
    #[must_use]
    pub fn new(rules: PolicyRuleSet, sandbox: WorkspaceSandbox, audit: AuditLog) -> Self {
        Self::with_sandbox_status(rules, sandbox, probe_kernel_sandbox(), audit)
    }

    /// Constructs a [`PolicyEngine`] with an explicit [`SandboxStatus`] (useful for deterministic tests).
    #[must_use]
    pub fn with_sandbox_status(
        rules: PolicyRuleSet,
        sandbox: WorkspaceSandbox,
        sandbox_status: SandboxStatus,
        audit: AuditLog,
    ) -> Self {
        Self {
            rules,
            sandbox,
            sandbox_status,
            audit,
        }
    }

    /// Swaps the active [`PolicyRuleSet`] when switching profiles between Coding and Personal modes.
    pub fn set_rules(&mut self, rules: PolicyRuleSet) {
        self.rules = rules;
    }

    /// Returns a reference to the active [`PolicyRuleSet`].
    #[must_use]
    pub fn rules(&self) -> &PolicyRuleSet {
        &self.rules
    }

    /// Returns a reference to the [`WorkspaceSandbox`].
    #[must_use]
    pub fn sandbox(&self) -> &WorkspaceSandbox {
        &self.sandbox
    }

    /// Returns the detected kernel [`SandboxStatus`].
    #[must_use]
    pub fn sandbox_status(&self) -> &SandboxStatus {
        &self.sandbox_status
    }

    /// Evaluates a [`Capability`] and appends an immutable [`AuditEntry`] to the JSONL audit log.
    pub fn evaluate(
        &self,
        tool_name: &str,
        capability: &Capability,
    ) -> Result<PolicyDecision, PolicyError> {
        let decision = self.decide(capability);
        self.audit.record(&AuditEntry {
            mode: self.rules.mode,
            tool_name: tool_name.to_owned(),
            capability: capability.clone(),
            decision: decision.clone(),
        })?;
        Ok(decision)
    }

    fn decide(&self, capability: &Capability) -> PolicyDecision {
        match capability {
            Capability::Memory { scope, mode } => self.decide_memory(*scope, *mode),
            Capability::FileSystem { path, mode } => self.decide_fs(path, *mode),
            Capability::Personal { resource, mode } => self.decide_personal(*resource, *mode),
            Capability::Shell { program, args } => self.decide_shell(program, args),
            Capability::Mcp { server, tool } => PolicyDecision::Ask {
                prompt: format!("Allow MCP call '{server}::{tool}'?"),
            },
        }
    }

    fn decide_shell(&self, program: &str, args: &[String]) -> PolicyDecision {
        if let SandboxStatus::Unavailable { reason } = &self.sandbox_status
            && self.rules.require_kernel_sandbox
        {
            return PolicyDecision::Deny {
                reason: format!("refusing unsandboxed shell command '{program}': {reason}"),
            };
        }
        let classification =
            ShellClassifier::classify_with_sandbox(program, args, Some(&self.sandbox));
        if classification.target == TargetScope::Protected
            && classification.effect > EffectLevel::ReadOnly
        {
            return PolicyDecision::Deny {
                reason: format!(
                    "refusing mutation of protected target: {}",
                    classification.reason
                ),
            };
        }
        if classification.effect < EffectLevel::Unknown
            && classification.target == TargetScope::Workspace
            && self.is_command_allowlisted(program, args)
        {
            return PolicyDecision::Allow {
                reason: "matched configured global/repo allowed_commands list".to_owned(),
            };
        }
        match classification.effect {
            EffectLevel::ReadOnly if classification.target == TargetScope::Workspace => {
                PolicyDecision::Allow {
                    reason: classification.reason,
                }
            }
            EffectLevel::ReadOnly
            | EffectLevel::WorkspaceWrite
            | EffectLevel::OutsideWrite
            | EffectLevel::Network
            | EffectLevel::Exec => PolicyDecision::Ask {
                prompt: format!(
                    "[{:?} | {:?}] Allow '{} {}' ({})?",
                    classification.effect,
                    classification.target,
                    program,
                    args.join(" "),
                    classification.reason
                ),
            },
            EffectLevel::Unknown => PolicyDecision::Deny {
                reason: classification.reason,
            },
        }
    }

    fn is_command_allowlisted(&self, program: &str, args: &[String]) -> bool {
        let full = if args.is_empty() {
            program.to_owned()
        } else if matches!(program.rsplit('/').next(), Some("sh" | "bash" | "zsh"))
            && args.len() >= 2
            && args[0] == "-c"
        {
            args[1].trim().to_owned()
        } else {
            format!("{program} {}", args.join(" "))
        };
        self.rules.allowed_commands.iter().any(|pattern| {
            let pat = pattern.trim();
            !pat.is_empty()
                && (full == pat || full.starts_with(&format!("{pat} ")) || program == pat)
        })
    }

    fn decide_memory(&self, scope: MemoryScope, mode: AccessMode) -> PolicyDecision {
        match (scope, mode) {
            (MemoryScope::Project, _) => PolicyDecision::Allow {
                reason: "project memory access permitted".to_owned(),
            },
            (MemoryScope::Personal, AccessMode::Read) if self.rules.allow_personal_memory_read => {
                PolicyDecision::Allow {
                    reason: "personal memory read permitted across coding and personal profiles"
                        .to_owned(),
                }
            }
            (MemoryScope::Personal, AccessMode::Write)
                if self.rules.allow_personal_memory_write =>
            {
                PolicyDecision::Allow {
                    reason: "personal memory write permitted in personal profile".to_owned(),
                }
            }
            (MemoryScope::Personal, AccessMode::Write) => PolicyDecision::Ask {
                prompt: "Allow writing to personal memory from current profile?".to_owned(),
            },
            (MemoryScope::Personal, AccessMode::Read) => PolicyDecision::Deny {
                reason: "personal memory read disabled by rule set".to_owned(),
            },
        }
    }

    fn decide_fs(&self, path: &std::path::Path, mode: AccessMode) -> PolicyDecision {
        if self.sandbox.resolve_within(path).is_err() {
            return PolicyDecision::Deny {
                reason: format!("path '{}' is outside workspace sandbox", path.display()),
            };
        }
        match mode {
            AccessMode::Read => PolicyDecision::Allow {
                reason: "workspace read within sandbox".to_owned(),
            },
            AccessMode::Write if self.rules.allow_workspace_write => PolicyDecision::Allow {
                reason: "workspace write within sandbox".to_owned(),
            },
            AccessMode::Write => PolicyDecision::Ask {
                prompt: format!("Allow writing file '{}'?", path.display()),
            },
        }
    }

    fn decide_personal(&self, resource: PersonalResource, mode: AccessMode) -> PolicyDecision {
        if self.rules.mode != ModeId::Personal {
            return PolicyDecision::Deny {
                reason: format!("personal resource '{resource:?}' requires personal mode profile"),
            };
        }
        match (resource, mode) {
            (_, AccessMode::Read) | (PersonalResource::Notes, AccessMode::Write) => {
                PolicyDecision::Allow {
                    reason: format!("personal '{resource:?}' '{mode:?}' allowed in personal mode"),
                }
            }
            (PersonalResource::Calendar | PersonalResource::Email, AccessMode::Write) => {
                PolicyDecision::Ask {
                    prompt: format!("Confirm mutating personal {resource:?} action?"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_shell_when_kernel_sandbox_unavailable() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir();
        let audit_path = temp_dir.join("rein_policy_fail_closed_audit.jsonl");
        let _ = std::fs::remove_file(&audit_path);

        let engine = PolicyEngine::with_sandbox_status(
            PolicyRuleSet::coding_default(),
            WorkspaceSandbox::new(&temp_dir),
            SandboxStatus::Unavailable {
                reason: "kernel compiled without CONFIG_SECURITY_LANDLOCK".to_owned(),
            },
            AuditLog::new(&audit_path),
        );
        let decision = engine.evaluate(
            "shell",
            &Capability::Shell {
                program: "head".to_owned(),
                args: vec!["-n".to_owned(), "5".to_owned(), "Cargo.toml".to_owned()],
            },
        )?;
        assert!(matches!(decision, PolicyDecision::Deny { .. }));
        let _ = std::fs::remove_file(&audit_path);
        Ok(())
    }

    #[test]
    fn allows_readonly_workspace_commands_directly_when_enforced()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir();
        let audit_path = temp_dir.join("rein_policy_readonly_allow_audit.jsonl");
        let _ = std::fs::remove_file(&audit_path);

        let engine = PolicyEngine::with_sandbox_status(
            PolicyRuleSet::coding_default(),
            WorkspaceSandbox::new(&temp_dir),
            SandboxStatus::Enforced { abi_version: 5 },
            AuditLog::new(&audit_path),
        );
        let decision = engine.evaluate(
            "shell",
            &Capability::Shell {
                program: "tail".to_owned(),
                args: vec!["-n".to_owned(), "10".to_owned(), "Cargo.toml".to_owned()],
            },
        )?;
        assert!(matches!(decision, PolicyDecision::Allow { .. }));
        let _ = std::fs::remove_file(&audit_path);
        Ok(())
    }

    #[test]
    fn allows_configured_global_and_repo_commands_inside_sandbox()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir();
        let audit_path = temp_dir.join("rein_policy_allowlist_audit.jsonl");
        let _ = std::fs::remove_file(&audit_path);

        let mut rules = PolicyRuleSet::coding_default();
        rules.allowed_commands = vec!["cargo test".to_owned()];
        let engine = PolicyEngine::with_sandbox_status(
            rules,
            WorkspaceSandbox::new(&temp_dir),
            SandboxStatus::Enforced { abi_version: 5 },
            AuditLog::new(&audit_path),
        );
        let decision = engine.evaluate(
            "shell",
            &Capability::Shell {
                program: "cargo".to_owned(),
                args: vec!["test".to_owned(), "--lib".to_owned()],
            },
        )?;
        assert!(matches!(decision, PolicyDecision::Allow { .. }));
        let _ = std::fs::remove_file(&audit_path);
        Ok(())
    }
}
