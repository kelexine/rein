// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Public exports for workspace pre-checks and process-group execution

/// POSIX process group, local wall-clock time, and process hardening helpers.
pub mod linux;
/// Subprocess specification and streaming runner.
pub mod spec;
/// Workspace root canonicalization and escape pre-check.
pub mod workspace;

pub use linux::{
    LocalTimeParts, detach_controlling_terminal, harden_process_dumpable, local_time_parts,
    probe_kernel_sandbox, signal_process_group,
};
pub use spec::{
    OUTPUT_CAP_BYTES, SandboxCommandOutput, SandboxSpec, SandboxStatus, SharedLogWriter,
    SpawnedSandboxProcess, drain_pipe_to_log_and_sink, run_sandboxed_command,
    spawn_sandboxed_command,
};
pub use workspace::WorkspaceSandbox;
