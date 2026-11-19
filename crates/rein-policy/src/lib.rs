// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Public exports for policy engine, classifier, capabilities, sandbox, and JSONL audit log

//! Sandbox-first capability, permission engine, command classifier, and JSONL audit layer for Rein.
// Crate-wide unsafe prohibition; `sandbox::linux` is the sole `#[allow(unsafe_code)]` FFI module for Landlock/prctl syscalls.
#![deny(unsafe_code)]

/// Append-only JSONL audit logging.
pub mod audit;
/// Fine-grained capability descriptors.
pub mod capability;
/// Flag/option/argument-aware Shell/Bash effect classifier.
pub mod classifier;
/// Policy evaluation engine and rule sets.
pub mod engine;
/// Workspace path confinement and kernel Landlock sandbox boundary.
pub mod sandbox;

pub use audit::{AuditEntry, AuditLog};
pub use capability::{AccessMode, Capability, PersonalResource};
pub use classifier::{
    EffectLevel, ParsedCommand, Redirection, ShellClassification, ShellClassifier, ShellPipeline,
    TargetScope, classify_args_scope, classify_parsed_command, classify_path_token,
};
pub use engine::{
    PolicyDecision, PolicyEngine, PolicyError, PolicyRuleSet, suggest_command_allow_pattern,
};
pub use sandbox::{
    DEFAULT_READ_ONLY_DIRS, DEFAULT_RW_DEVICE_PATHS, OUTPUT_CAP_BYTES, SANDBOX_CHILD_FLAG,
    SandboxCommandOutput, SandboxSpec, SandboxStatus, WorkspaceSandbox, apply_sandbox_in_child,
    default_sandbox_read_only_paths, default_sandbox_read_write_paths, execute_sandbox_child,
    harden_process_dumpable, probe_kernel_sandbox, run_sandboxed_command,
};
