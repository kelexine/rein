// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Public exports for workspace pre-checks and kernel Landlock sandboxing

/// Linux Landlock kernel probe and self-restriction syscalls.
pub mod linux;
/// Re-exec sandbox specification and child runner.
pub mod spec;
/// Workspace root canonicalization and escape pre-check.
pub mod workspace;

pub use linux::{apply_sandbox_in_child, probe_kernel_sandbox};
pub use spec::{
    DEFAULT_READ_ONLY_DIRS, DEFAULT_RW_DEVICE_PATHS, OUTPUT_CAP_BYTES, SANDBOX_CHILD_FLAG,
    SandboxCommandOutput, SandboxSpec, SandboxStatus, default_sandbox_read_only_paths,
    default_sandbox_read_write_paths, execute_sandbox_child, run_sandboxed_command,
};
pub use workspace::WorkspaceSandbox;
