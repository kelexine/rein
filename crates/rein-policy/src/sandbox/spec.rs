// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Re-exec SandboxSpec, kernel availability status, and child process runner

use std::path::PathBuf;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::engine::PolicyError;
use crate::sandbox::linux::{apply_sandbox_in_child, probe_kernel_sandbox};

/// CLI flag used when the single binary re-execs itself as a sandboxed child worker.
pub const SANDBOX_CHILD_FLAG: &str = "--sandbox-child";

/// Default read-only system directories exposed to sandboxed child processes.
pub const DEFAULT_READ_ONLY_DIRS: &[&str] = &["/usr", "/bin", "/lib", "/lib64", "/etc", "/proc"];

/// Kernel-level process isolation availability detected at startup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SandboxStatus {
    /// Linux Landlock LSM is active and enforced with the detected ABI version.
    Enforced {
        /// Supported Landlock ABI version.
        abi_version: u32,
    },
    /// Kernel sandboxing is unavailable; shell tools refuse to run by default.
    Unavailable {
        /// Diagnostic reason.
        reason: String,
    },
}

impl SandboxStatus {
    /// Returns `true` if kernel-level Landlock enforcement is available.
    #[must_use]
    pub fn is_enforced(&self) -> bool {
        matches!(self, Self::Enforced { .. })
    }
}

/// Serializable specification passed to `rein --sandbox-child` for self-restriction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxSpec {
    /// Workspace directory granted access.
    pub workspace_root: PathBuf,
    /// When true, Landlock restricts even `workspace_root` to read-only access (used for read-only dictionary commands).
    pub read_only_workspace: bool,
    /// System paths granted read-only and execute access.
    pub read_only_paths: Vec<PathBuf>,
    /// Target executable to run after Landlock is applied.
    pub program: String,
    /// Arguments passed to `program`.
    pub args: Vec<String>,
}

/// Captured output from a sandboxed subprocess execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxCommandOutput {
    /// Process exit status code.
    pub status_code: i32,
    /// Standard output UTF-8 text.
    pub stdout: String,
    /// Standard error UTF-8 text.
    pub stderr: String,
}

/// Spawns `current_exe() --sandbox-child <json>` so the child applies Landlock before exec.
pub fn run_sandboxed_command(spec: &SandboxSpec) -> Result<SandboxCommandOutput, PolicyError> {
    let status = probe_kernel_sandbox();
    if let SandboxStatus::Unavailable { reason } = status {
        return Err(PolicyError::SandboxUnavailable(reason));
    }
    let exe = std::env::current_exe().map_err(|source| PolicyError::Io {
        path: PathBuf::from("current_exe"),
        source,
    })?;
    let payload = serde_json::to_string(spec)?;
    let output = Command::new(&exe)
        .arg(SANDBOX_CHILD_FLAG)
        .arg(payload)
        .current_dir(&spec.workspace_root)
        .output()
        .map_err(|source| PolicyError::Io { path: exe, source })?;
    Ok(SandboxCommandOutput {
        status_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Entrypoint executed inside the `--sandbox-child` process: applies Landlock, then runs command.
pub fn execute_sandbox_child(raw_spec: &str) -> Result<i32, PolicyError> {
    let spec: SandboxSpec = serde_json::from_str(raw_spec)?;
    apply_sandbox_in_child(&spec)?;
    let status = Command::new(&spec.program)
        .args(&spec.args)
        .current_dir(&spec.workspace_root)
        .status()
        .map_err(|source| PolicyError::Io {
            path: PathBuf::from(&spec.program),
            source,
        })?;
    Ok(status.code().unwrap_or(1))
}
