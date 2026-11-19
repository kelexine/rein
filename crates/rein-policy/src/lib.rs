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
    LocalTimeParts, OUTPUT_CAP_BYTES, SandboxCommandOutput, SandboxSpec, SandboxStatus,
    SharedLogWriter, SpawnedSandboxProcess, WorkspaceSandbox, detach_controlling_terminal,
    drain_pipe_to_log_and_sink, harden_process_dumpable, local_time_parts, probe_kernel_sandbox,
    run_sandboxed_command, signal_process_group, spawn_sandboxed_command,
};
