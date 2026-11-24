// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: PolicyEngine evaluating capabilities against sandbox status, classifier, allowlists, and audit log

use rein_core::{MemoryScope, ModeId};

use crate::audit::{AuditEntry, AuditLog};
use crate::capability::{AccessMode, Capability, PersonalResource};
use crate::classifier::{EffectLevel, ShellClassifier, TargetScope, classify_parsed_command};
use crate::engine::{PolicyDecision, PolicyError, PolicyRuleSet};
use crate::sandbox::{SandboxStatus, WorkspaceSandbox, probe_kernel_sandbox};

use super::suggest::{extract_stages, stage_is_auto_allowed, stage_matches_pattern};

/// Sandbox-first policy evaluator that logs every capability check to the JSONL [`AuditLog`].
#[derive(Debug)]
pub struct PolicyEngine {
    rules: PolicyRuleSet,
    sandbox: WorkspaceSandbox,
    sandbox_status: SandboxStatus,
    audit: AuditLog,
    bypass_permissions: bool,
}

impl PolicyEngine {
    /// Constructs a [`PolicyEngine`] and probes the active [`SandboxStatus`] at creation.
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
            bypass_permissions: false,
        }
    }

    /// Enables or disables explicit permission bypass (`--dangerously-bypass-permission` / `--yolo`).
    #[must_use]
    pub fn with_bypass_permissions(mut self, bypass: bool) -> Self {
        self.bypass_permissions = bypass;
        self
    }

    /// Returns `true` when explicit permission bypass is active.
    #[must_use]
    pub fn is_bypass_permissions(&self) -> bool {
        self.bypass_permissions
    }

    /// Swaps the active [`PolicyRuleSet`] when switching profiles between Coding and Personal modes.
    pub fn set_rules(&mut self, rules: PolicyRuleSet) {
        self.rules = rules;
    }

    /// Adds a newly persisted command pattern (`cargo *`, exact command, etc.) to active rules.
    pub fn allow_command_pattern(&mut self, pattern: impl Into<String>) {
        let pat = pattern.into();
        if !self.rules.allowed_commands.iter().any(|p| p == &pat) {
            self.rules.allowed_commands.push(pat);
        }
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

    /// Returns the detected [`SandboxStatus`].
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
        let decision = self.decide(tool_name, capability);
        self.audit.record(&AuditEntry {
            mode: self.rules.mode,
            tool_name: tool_name.to_owned(),
            capability: capability.clone(),
            decision: decision.clone(),
        })?;
        Ok(decision)
    }

    fn decide(&self, tool_name: &str, capability: &Capability) -> PolicyDecision {
        if let Capability::Subagent {
            agent_type,
            role,
            target_mode,
        } = capability
        {
            return self.decide_subagent(agent_type, role, *target_mode);
        }
        if self.bypass_permissions {
            return PolicyDecision::Allow {
                reason: format!(
                    "auto-approved '{tool_name}' via --dangerously-bypass-permission / --yolo"
                ),
            };
        }
        match capability {
            Capability::Memory { scope, mode } => self.decide_memory(tool_name, *scope, *mode),
            Capability::FileSystem { path, mode } => self.decide_fs(tool_name, path, *mode),
            Capability::Personal { resource, mode } => {
                self.decide_personal(tool_name, *resource, *mode)
            }
            Capability::Shell { program, args } => self.decide_shell(program, args),
            Capability::ModeSwitch { target } => PolicyDecision::Allow {
                reason: format!("mode switch to {target:?} permitted"),
            },
            Capability::Todo { mode } => PolicyDecision::Allow {
                reason: format!("global session todo checklist {mode:?} permitted"),
            },
            Capability::Mcp { server, tool } => {
                if self.is_tool_allowlisted(tool_name) {
                    PolicyDecision::Allow {
                        reason: format!("tool '{tool_name}' matched configured allowlist"),
                    }
                } else {
                    PolicyDecision::Ask {
                        prompt: format!("Allow MCP call '{server}::{tool}'?"),
                    }
                }
            }
            Capability::Subagent {
                agent_type,
                role,
                target_mode,
            } => self.decide_subagent(agent_type, role, *target_mode),
            Capability::Task { action, mode } => PolicyDecision::Allow {
                reason: format!("session task operation '{action}' ({mode:?}) permitted"),
            },
            Capability::AskUser => PolicyDecision::Allow {
                reason: "interactive user clarification prompt permitted".to_owned(),
            },
            Capability::WebSearch { .. } => PolicyDecision::Allow {
                reason: "read-only web search permitted".to_owned(),
            },
            Capability::WebFetch { url, method } => {
                if self.rules.mode == ModeId::Plan && method != "GET" {
                    PolicyDecision::Deny {
                        reason: format!(
                            "plan mode is read-only and cannot perform {method} requests to '{url}'"
                        ),
                    }
                } else if self.is_tool_allowlisted(tool_name) {
                    PolicyDecision::Allow {
                        reason: format!("tool '{tool_name}' matched configured allowlist"),
                    }
                } else {
                    PolicyDecision::Ask {
                        prompt: format!("Allow {method} web_fetch to '{url}'?"),
                    }
                }
            }
        }
    }

    fn decide_subagent(&self, agent_type: &str, role: &str, target_mode: ModeId) -> PolicyDecision {
        if self.rules.mode == ModeId::Plan && target_mode != ModeId::Plan {
            return PolicyDecision::Deny {
                reason: format!(
                    "plan mode confines subagents to read-only research (cannot spawn '{agent_type}' in {target_mode:?} mode)"
                ),
            };
        }
        if target_mode == ModeId::Coding && self.rules.mode != ModeId::Coding {
            return PolicyDecision::Deny {
                reason: format!(
                    "coding subagents require active Coding mode (current mode: {:?})",
                    self.rules.mode
                ),
            };
        }
        if target_mode == ModeId::Personal && self.rules.mode != ModeId::Personal {
            return PolicyDecision::Deny {
                reason: format!(
                    "personal subagents require active Personal mode (current mode: {:?})",
                    self.rules.mode
                ),
            };
        }
        if target_mode == ModeId::Plan {
            return PolicyDecision::Allow {
                reason: format!("read-only research subagent '{agent_type}' ({role}) permitted"),
            };
        }
        PolicyDecision::Allow {
            reason: format!(
                "subagent '{agent_type}' ({role}) permitted under active {:?} session policy",
                self.rules.mode
            ),
        }
    }

    fn decide_shell(&self, program: &str, args: &[String]) -> PolicyDecision {
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
        if self.rules.mode == ModeId::Plan && classification.effect > EffectLevel::ReadOnly {
            return PolicyDecision::Deny {
                reason: format!(
                    "plan mode permits read-only inspection only (denied '{program}': {})",
                    classification.reason
                ),
            };
        }
        if classification.effect < EffectLevel::Unknown
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
        let Some(stages) = extract_stages(program, args, &self.sandbox) else {
            return false;
        };
        if stages.is_empty() {
            return false;
        }
        stages.iter().all(|stage| {
            let class = classify_parsed_command(stage, Some(&self.sandbox));
            stage_is_auto_allowed(stage, &class)
                || self
                    .rules
                    .allowed_commands
                    .iter()
                    .any(|pat| stage_matches_pattern(stage, &class, pat))
        })
    }

    fn is_tool_allowlisted(&self, tool_name: &str) -> bool {
        let key = format!("tool:{tool_name}");
        self.rules.allowed_commands.iter().any(|pat| pat == &key)
    }

    fn decide_memory(
        &self,
        tool_name: &str,
        scope: MemoryScope,
        mode: AccessMode,
    ) -> PolicyDecision {
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
            (MemoryScope::Personal, AccessMode::Write) if self.is_tool_allowlisted(tool_name) => {
                PolicyDecision::Allow {
                    reason: format!("tool '{tool_name}' matched configured allowlist"),
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

    fn decide_fs(
        &self,
        tool_name: &str,
        path: &std::path::Path,
        mode: AccessMode,
    ) -> PolicyDecision {
        if self.sandbox.resolve_within(path).is_err() {
            return PolicyDecision::Deny {
                reason: format!("path '{}' is outside workspace sandbox", path.display()),
            };
        }
        if mode == AccessMode::Write && self.sandbox.is_protected(path) {
            return PolicyDecision::Deny {
                reason: format!("refusing write to protected path '{}'", path.display()),
            };
        }
        match mode {
            AccessMode::Read => PolicyDecision::Allow {
                reason: "workspace read within sandbox".to_owned(),
            },
            AccessMode::Write if self.rules.mode == ModeId::Plan => PolicyDecision::Deny {
                reason: format!("plan mode forbids filesystem writes ('{}')", path.display()),
            },
            AccessMode::Write if self.rules.allow_workspace_write => PolicyDecision::Allow {
                reason: "workspace write within sandbox".to_owned(),
            },
            AccessMode::Write if self.is_tool_allowlisted(tool_name) => PolicyDecision::Allow {
                reason: format!("sandboxed tool '{tool_name}' matched configured allowlist"),
            },
            AccessMode::Write => PolicyDecision::Ask {
                prompt: format!("Allow writing file '{}'?", path.display()),
            },
        }
    }

    fn decide_personal(
        &self,
        tool_name: &str,
        resource: PersonalResource,
        mode: AccessMode,
    ) -> PolicyDecision {
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
            (PersonalResource::Calendar | PersonalResource::Email, AccessMode::Write)
                if self.is_tool_allowlisted(tool_name) =>
            {
                PolicyDecision::Allow {
                    reason: format!("tool '{tool_name}' matched configured allowlist"),
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

pub(super) const MULTI_SUBCOMMAND_TOOLS: &[&str] = &[
    "cargo", "git", "npm", "pnpm", "yarn", "bun", "go", "make", "just", "pytest", "uv", "pip",
    "ruff", "rustup", "rustc",
];

#[cfg(test)]
mod tests {
    use super::super::suggest::suggest_command_allow_pattern;
    use super::*;

    #[test]
    fn allows_readonly_shell_without_kernel_landlock() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir();
        let audit_path = temp_dir.join("rein_policy_no_landlock_audit.jsonl");
        let _ = std::fs::remove_file(&audit_path);

        let engine = PolicyEngine::with_sandbox_status(
            PolicyRuleSet::coding_default(),
            WorkspaceSandbox::new(&temp_dir),
            SandboxStatus::PolicyOnly,
            AuditLog::new(&audit_path),
        );
        let decision = engine.evaluate(
            "shell",
            &Capability::Shell {
                program: "head".to_owned(),
                args: vec!["-n".to_owned(), "5".to_owned(), "Cargo.toml".to_owned()],
            },
        )?;
        assert!(matches!(decision, PolicyDecision::Allow { .. }));
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

    #[test]
    fn wildcard_cargo_allowlist_does_not_allow_install_or_publish()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir();
        let audit_path = temp_dir.join("rein_policy_cargo_scope_audit.jsonl");
        let _ = std::fs::remove_file(&audit_path);

        let mut rules = PolicyRuleSet::coding_default();
        rules.allowed_commands = vec!["cargo *".to_owned()];
        let engine = PolicyEngine::with_sandbox_status(
            rules,
            WorkspaceSandbox::new(&temp_dir),
            SandboxStatus::Enforced { abi_version: 5 },
            AuditLog::new(&audit_path),
        );

        let check = engine.evaluate(
            "bash",
            &Capability::Shell {
                program: "bash".to_owned(),
                args: vec!["-c".to_owned(), "cargo check --all-targets".to_owned()],
            },
        )?;
        assert!(matches!(check, PolicyDecision::Allow { .. }));

        let install = engine.evaluate(
            "bash",
            &Capability::Shell {
                program: "bash".to_owned(),
                args: vec!["-c".to_owned(), "cargo install ripgrep".to_owned()],
            },
        )?;
        assert!(matches!(install, PolicyDecision::Ask { .. }));

        let publish = engine.evaluate(
            "bash",
            &Capability::Shell {
                program: "bash".to_owned(),
                args: vec!["-c".to_owned(), "cargo publish".to_owned()],
            },
        )?;
        assert!(matches!(publish, PolicyDecision::Ask { .. }));

        let chained = engine.evaluate(
            "bash",
            &Capability::Shell {
                program: "bash".to_owned(),
                args: vec![
                    "-c".to_owned(),
                    "cargo check && rm -rf /tmp/rein_nonexistent".to_owned(),
                ],
            },
        )?;
        assert!(matches!(chained, PolicyDecision::Ask { .. }));

        let _ = std::fs::remove_file(&audit_path);
        Ok(())
    }

    #[test]
    fn exact_cargo_install_allowlist_does_not_broaden_to_other_cargo_commands()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir();
        let audit_path = temp_dir.join("rein_policy_exact_install_audit.jsonl");
        let _ = std::fs::remove_file(&audit_path);

        let sandbox = WorkspaceSandbox::new(&temp_dir);
        let (_, install_pat) = suggest_command_allow_pattern(
            "bash",
            &["-c".to_owned(), "cargo install ripgrep".to_owned()],
            &sandbox,
        );
        assert_eq!(install_pat, "cargo install ripgrep");

        let (_, test_pat) = suggest_command_allow_pattern(
            "bash",
            &["-c".to_owned(), "cargo test --lib".to_owned()],
            &sandbox,
        );
        assert_eq!(test_pat, "cargo *");

        let mut rules = PolicyRuleSet::coding_default();
        rules.allowed_commands = vec![install_pat];
        let engine = PolicyEngine::with_sandbox_status(
            rules,
            sandbox,
            SandboxStatus::Enforced { abi_version: 5 },
            AuditLog::new(&audit_path),
        );

        let allowed = engine.evaluate(
            "bash",
            &Capability::Shell {
                program: "bash".to_owned(),
                args: vec!["-c".to_owned(), "cargo install ripgrep".to_owned()],
            },
        )?;
        assert!(matches!(allowed, PolicyDecision::Allow { .. }));

        let other_install = engine.evaluate(
            "bash",
            &Capability::Shell {
                program: "bash".to_owned(),
                args: vec!["-c".to_owned(), "cargo install cargo-edit".to_owned()],
            },
        )?;
        assert!(matches!(other_install, PolicyDecision::Ask { .. }));

        let other_cargo = engine.evaluate(
            "bash",
            &Capability::Shell {
                program: "bash".to_owned(),
                args: vec!["-c".to_owned(), "cargo test".to_owned()],
            },
        )?;
        assert!(matches!(other_cargo, PolicyDecision::Ask { .. }));

        let _ = std::fs::remove_file(&audit_path);
        Ok(())
    }

    #[test]
    fn tool_allowlist_applies_only_within_sandbox_path() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join("rein_policy_tool_sandbox_scope");
        std::fs::create_dir_all(&temp_dir)?;
        let audit_path = temp_dir.join("audit.jsonl");
        let _ = std::fs::remove_file(&audit_path);

        let mut rules = PolicyRuleSet::personal_default();
        rules.allowed_commands = vec!["tool:write".to_owned()];
        let engine = PolicyEngine::with_sandbox_status(
            rules,
            WorkspaceSandbox::new(&temp_dir),
            SandboxStatus::Enforced { abi_version: 5 },
            AuditLog::new(&audit_path),
        );

        let inside = engine.evaluate(
            "write",
            &Capability::FileSystem {
                path: temp_dir.join("note.txt"),
                mode: AccessMode::Write,
            },
        )?;
        assert!(matches!(inside, PolicyDecision::Allow { .. }));

        let outside = engine.evaluate(
            "write",
            &Capability::FileSystem {
                path: std::path::PathBuf::from("/etc/passwd"),
                mode: AccessMode::Write,
            },
        )?;
        assert!(matches!(outside, PolicyDecision::Deny { .. }));

        let _ = std::fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    #[test]
    fn pipeline_readonly_consumers_do_not_block_allowlist_and_env_hijack_is_refused()
    -> Result<(), Box<dyn std::error::Error>> {
        let ws = std::env::temp_dir().join("rein_policy_pipeline_env_audit_ws");
        std::fs::create_dir_all(&ws)?;
        let audit_path = ws.join("audit.jsonl");
        let _ = std::fs::remove_file(&audit_path);

        let sandbox = WorkspaceSandbox::new(&ws);
        let (_, pat) = suggest_command_allow_pattern(
            "bash",
            &[
                "-c".to_owned(),
                "TMPDIR=$PWD/target/tmp cargo test --workspace 2>&1 | tail -80".to_owned(),
            ],
            &sandbox,
        );
        assert_eq!(pat, "cargo *");

        let mut rules = PolicyRuleSet::coding_default();
        rules.allowed_commands = vec![pat];
        let engine = PolicyEngine::with_sandbox_status(
            rules,
            sandbox,
            SandboxStatus::Enforced { abi_version: 5 },
            AuditLog::new(&audit_path),
        );

        let allowed_pipe = engine.evaluate(
            "bash",
            &Capability::Shell {
                program: "bash".to_owned(),
                args: vec![
                    "-c".to_owned(),
                    "TMPDIR=$PWD/target/tmp cargo test --workspace 2>&1 | tail -80".to_owned(),
                ],
            },
        )?;
        assert!(matches!(allowed_pipe, PolicyDecision::Allow { .. }));

        let hijacked = engine.evaluate(
            "bash",
            &Capability::Shell {
                program: "bash".to_owned(),
                args: vec!["-c".to_owned(), "RUSTC_WRAPPER=sh cargo check".to_owned()],
            },
        )?;
        assert!(matches!(hijacked, PolicyDecision::Ask { .. }));

        let safe_export = engine.evaluate(
            "bash",
            &Capability::Shell {
                program: "bash".to_owned(),
                args: vec![
                    "-c".to_owned(),
                    "export TMPDIR=$PWD/target/tmp; cargo fmt --check | head -50".to_owned(),
                ],
            },
        )?;
        assert!(matches!(safe_export, PolicyDecision::Allow { .. }));

        let path_export = engine.evaluate(
            "bash",
            &Capability::Shell {
                program: "bash".to_owned(),
                args: vec!["-c".to_owned(), "export PATH=./bin:$PATH".to_owned()],
            },
        )?;
        assert!(matches!(path_export, PolicyDecision::Deny { .. }));

        let _ = std::fs::remove_dir_all(&ws);
        Ok(())
    }

    #[test]
    fn plan_mode_enforces_strict_readonly_policy_and_allows_mode_switch()
    -> Result<(), Box<dyn std::error::Error>> {
        let ws = std::env::temp_dir().join("rein_policy_plan_mode_test");
        std::fs::create_dir_all(&ws)?;
        let audit_path = ws.join("audit.jsonl");
        let _ = std::fs::remove_file(&audit_path);

        let mut rules = PolicyRuleSet::plan_default();
        rules.allowed_commands = vec!["cargo *".to_owned(), "tool:write".to_owned()];
        let engine = PolicyEngine::with_sandbox_status(
            rules,
            WorkspaceSandbox::new(&ws),
            SandboxStatus::Enforced { abi_version: 5 },
            AuditLog::new(&audit_path),
        );

        let write_decision = engine.evaluate(
            "write",
            &Capability::FileSystem {
                path: ws.join("src/lib.rs"),
                mode: AccessMode::Write,
            },
        )?;
        assert!(matches!(write_decision, PolicyDecision::Deny { .. }));

        let mutating_shell = engine.evaluate(
            "bash",
            &Capability::Shell {
                program: "bash".to_owned(),
                args: vec!["-c".to_owned(), "cargo test".to_owned()],
            },
        )?;
        assert!(matches!(mutating_shell, PolicyDecision::Deny { .. }));

        let readonly_shell = engine.evaluate(
            "bash",
            &Capability::Shell {
                program: "bash".to_owned(),
                args: vec!["-c".to_owned(), "git status".to_owned()],
            },
        )?;
        assert!(matches!(readonly_shell, PolicyDecision::Allow { .. }));

        let switch_decision = engine.evaluate(
            "switch_mode",
            &Capability::ModeSwitch {
                target: ModeId::Coding,
            },
        )?;
        assert!(matches!(switch_decision, PolicyDecision::Allow { .. }));

        let _ = std::fs::remove_dir_all(&ws);
        Ok(())
    }

    #[test]
    fn subagent_policy_confines_plan_mode_to_research_and_requires_coding_mode_for_coder()
    -> Result<(), Box<dyn std::error::Error>> {
        let ws = std::env::temp_dir().join("rein_policy_subagent_confinement");
        let _ = std::fs::remove_dir_all(&ws);
        std::fs::create_dir_all(&ws)?;
        let audit_path = ws.join("audit.jsonl");

        // Plan mode denies coding/personal subagents even if bypass_permissions is true.
        let plan_engine = PolicyEngine::with_sandbox_status(
            PolicyRuleSet::plan_default(),
            WorkspaceSandbox::new(&ws),
            SandboxStatus::Enforced { abi_version: 5 },
            AuditLog::new(&audit_path),
        )
        .with_bypass_permissions(true);

        let plan_research = plan_engine.evaluate(
            "agent",
            &Capability::Subagent {
                agent_type: "research".to_owned(),
                role: "Codebase Explorer".to_owned(),
                target_mode: ModeId::Plan,
            },
        )?;
        assert!(matches!(plan_research, PolicyDecision::Allow { .. }));

        let plan_coder = plan_engine.evaluate(
            "agent",
            &Capability::Subagent {
                agent_type: "coder".to_owned(),
                role: "Fix Bug".to_owned(),
                target_mode: ModeId::Coding,
            },
        )?;
        assert!(matches!(plan_coder, PolicyDecision::Deny { .. }));

        // Coding mode allows both research and coding subagents, and denies personal subagents.
        let coding_engine = PolicyEngine::with_sandbox_status(
            PolicyRuleSet::coding_default(),
            WorkspaceSandbox::new(&ws),
            SandboxStatus::Enforced { abi_version: 5 },
            AuditLog::new(&audit_path),
        );
        let coding_coder = coding_engine.evaluate(
            "agent",
            &Capability::Subagent {
                agent_type: "coder".to_owned(),
                role: "Implement Feature".to_owned(),
                target_mode: ModeId::Coding,
            },
        )?;
        assert!(matches!(coding_coder, PolicyDecision::Allow { .. }));

        let coding_personal = coding_engine.evaluate(
            "agent",
            &Capability::Subagent {
                agent_type: "personal".to_owned(),
                role: "Send Email".to_owned(),
                target_mode: ModeId::Personal,
            },
        )?;
        assert!(matches!(coding_personal, PolicyDecision::Deny { .. }));

        let _ = std::fs::remove_dir_all(&ws);
        Ok(())
    }

    #[test]
    fn web_search_allows_and_web_fetch_asks_or_denies_mutating_in_plan()
    -> Result<(), Box<dyn std::error::Error>> {
        let ws = std::env::temp_dir().join("rein_policy_web_tools_test");
        let _ = std::fs::remove_dir_all(&ws);
        std::fs::create_dir_all(&ws)?;
        let audit_path = ws.join("audit.jsonl");

        let coding = PolicyEngine::with_sandbox_status(
            PolicyRuleSet::coding_default(),
            WorkspaceSandbox::new(&ws),
            SandboxStatus::Enforced { abi_version: 5 },
            AuditLog::new(&audit_path),
        );
        let search_dec = coding.evaluate(
            "web_search",
            &Capability::WebSearch {
                query: "rust ureq".to_owned(),
            },
        )?;
        assert!(matches!(search_dec, PolicyDecision::Allow { .. }));

        let fetch_get = coding.evaluate(
            "web_fetch",
            &Capability::WebFetch {
                url: "https://example.com".to_owned(),
                method: "GET".to_owned(),
            },
        )?;
        assert!(matches!(fetch_get, PolicyDecision::Ask { .. }));

        let plan = PolicyEngine::with_sandbox_status(
            PolicyRuleSet::plan_default(),
            WorkspaceSandbox::new(&ws),
            SandboxStatus::Enforced { abi_version: 5 },
            AuditLog::new(&audit_path),
        );
        let plan_get = plan.evaluate(
            "web_fetch",
            &Capability::WebFetch {
                url: "https://example.com".to_owned(),
                method: "GET".to_owned(),
            },
        )?;
        assert!(matches!(plan_get, PolicyDecision::Ask { .. }));

        let plan_post = plan.evaluate(
            "web_fetch",
            &Capability::WebFetch {
                url: "https://example.com".to_owned(),
                method: "POST".to_owned(),
            },
        )?;
        assert!(matches!(plan_post, PolicyDecision::Deny { .. }));

        let _ = std::fs::remove_dir_all(&ws);
        Ok(())
    }
}
