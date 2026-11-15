// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Re-exec SandboxSpec, streaming output capture, kernel availability status, and child runner

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::engine::PolicyError;
use crate::sandbox::linux::{apply_sandbox_in_child, probe_kernel_sandbox};

/// CLI flag used when the single binary re-execs itself as a sandboxed child worker.
pub const SANDBOX_CHILD_FLAG: &str = "--sandbox-child";

/// Default read-only system directories exposed to sandboxed child processes.
pub const DEFAULT_READ_ONLY_DIRS: &[&str] = &[
    "/usr", "/bin", "/lib", "/lib64", "/etc", "/proc", "/sys", "/opt", "/nix", "/dev",
];

/// Character device files that require read/write access even inside a read-only sandbox (e.g. `2>/dev/null`).
pub const DEFAULT_RW_DEVICE_PATHS: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/urandom",
    "/dev/random",
    "/dev/full",
    "/dev/tty",
];

/// Resolves read+execute system, `$PATH`, and developer toolchain paths for Landlock sandboxing without exposing `$HOME` broadly.
#[must_use]
pub fn default_sandbox_read_only_paths(workspace_root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for dir in DEFAULT_READ_ONLY_DIRS {
        push_sandbox_path(&mut paths, PathBuf::from(dir), workspace_root);
    }

    if let Some(raw_path) = std::env::var_os("PATH") {
        for entry in std::env::split_paths(&raw_path) {
            if !entry.as_os_str().is_empty() && entry != Path::new(".") {
                push_sandbox_path(&mut paths, entry, workspace_root);
            }
        }
    }

    for env_var in ["CARGO_HOME", "RUSTUP_HOME", "BUN_INSTALL", "GOPATH"] {
        if let Some(val) = std::env::var_os(env_var) {
            push_sandbox_path(&mut paths, PathBuf::from(val), workspace_root);
        }
    }

    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for rel in [
            ".cargo",
            ".rustup",
            ".bun",
            ".local",
            "go",
            ".nvm",
            ".fnm",
            ".pyenv",
            ".asdf",
            ".mise",
            ".volta",
            ".gitconfig",
            ".config/git",
        ] {
            push_sandbox_path(&mut paths, home.join(rel), workspace_root);
        }
    }

    paths
}

/// Resolves safe writable scratch/device paths (such as `/dev/null`, `/tmp`, and Cargo's lockfile) for Landlock sandboxing.
#[must_use]
pub fn default_sandbox_read_write_paths(
    workspace_root: &Path,
    read_only_workspace: bool,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for dev in DEFAULT_RW_DEVICE_PATHS {
        let p = PathBuf::from(dev);
        if p.exists() && !paths.contains(&p) {
            paths.push(p);
        }
    }

    let cargo_cache = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")))
        .map(|cargo_home| cargo_home.join(".package-cache"));
    if let Some(cache) = cargo_cache
        && cache.exists()
        && !paths.contains(&cache)
    {
        paths.push(cache);
    }

    let mut scratch_dirs = vec![std::env::temp_dir()];
    if let Some(tmpdir) = std::env::var_os("TMPDIR").map(PathBuf::from) {
        scratch_dirs.push(tmpdir);
    }
    for dir in scratch_dirs {
        if !dir.exists() {
            continue;
        }
        // Keep read-only workspace boundaries strict when tests place a workspace under `/tmp`.
        if read_only_workspace && workspace_root.starts_with(&dir) {
            continue;
        }
        if !paths.contains(&dir) {
            paths.push(dir);
        }
    }

    paths
}

fn push_sandbox_path(out: &mut Vec<PathBuf>, candidate: PathBuf, workspace_root: &Path) {
    if !candidate.exists() {
        return;
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
        && candidate == home
    {
        return;
    }
    if workspace_root.starts_with(&candidate) || candidate.starts_with(workspace_root) {
        return;
    }
    if !out.contains(&candidate) {
        out.push(candidate);
    }
}

/// Maximum bytes retained inline per stream (`stdout` / `stderr`) in [`SandboxCommandOutput`].
pub const OUTPUT_CAP_BYTES: usize = 32 * 1024;

static OUTPUT_SEQ: AtomicU64 = AtomicU64::new(0);

/// Kernel-level process isolation availability detected at startup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SandboxStatus {
    /// Linux Landlock LSM is active and enforced with the detected ABI version.
    Enforced {
        /// Supported Landlock ABI version.
        abi_version: u32,
    },
    /// Kernel sandboxing is unavailable; bash tools refuse to run by default.
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
    /// When true, Landlock restricts even `workspace_root` to read-only access (used for read-only commands).
    pub read_only_workspace: bool,
    /// System and toolchain paths granted read-only and execute access.
    pub read_only_paths: Vec<PathBuf>,
    /// Safe scratch/device paths granted read-write access (e.g. `/dev/null`, `/tmp`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_write_paths: Vec<PathBuf>,
    /// Target executable to run after Landlock is applied.
    pub program: String,
    /// Arguments passed to `program`.
    pub args: Vec<String>,
    /// Optional directory where full streamed `stdout`/`stderr` log files are written by the parent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<PathBuf>,
}

/// Captured bounded output from a sandboxed subprocess plus path to the full streamed log file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxCommandOutput {
    /// Process exit status code.
    pub status_code: i32,
    /// Capped UTF-8 standard output excerpt.
    pub stdout: String,
    /// Capped UTF-8 standard error excerpt.
    pub stderr: String,
    /// `true` when either stream exceeded [`OUTPUT_CAP_BYTES`] and was truncated inline.
    #[serde(default)]
    pub truncated: bool,
    /// Path to the complete un-truncated streamed log file on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_file: Option<PathBuf>,
}

/// Spawns `current_exe() --sandbox-child <json>` without timeouts, streaming full logs to disk.
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
    let mut child = Command::new(&exe)
        .arg(SANDBOX_CHILD_FLAG)
        .arg(payload)
        .current_dir(&spec.workspace_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| PolicyError::Io { path: exe, source })?;

    let (log_path, log_writer) = open_stream_log(spec.output_dir.as_deref())?;
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let (out_res, err_res) = std::thread::scope(|scope| {
        let log_out = Arc::clone(&log_writer);
        let h_out = scope.spawn(move || drain_pipe(stdout_pipe, "[stdout]\n", &log_out));
        let log_err = Arc::clone(&log_writer);
        let h_err = scope.spawn(move || drain_pipe(stderr_pipe, "[stderr]\n", &log_err));
        let out_res = h_out
            .join()
            .unwrap_or_else(|_| Err(std::io::Error::other("stdout reader panicked")));
        let err_res = h_err
            .join()
            .unwrap_or_else(|_| Err(std::io::Error::other("stderr reader panicked")));
        (out_res, err_res)
    });

    if let Ok(mut guard) = log_writer.lock() {
        let _ = guard.flush();
    }
    let exit_status = child.wait().map_err(|source| PolicyError::Io {
        path: PathBuf::from(&spec.program),
        source,
    })?;

    let (out_bytes, out_trunc) = out_res.map_err(|source| PolicyError::Io {
        path: log_path.clone(),
        source,
    })?;
    let (err_bytes, err_trunc) = err_res.map_err(|source| PolicyError::Io {
        path: log_path.clone(),
        source,
    })?;

    Ok(SandboxCommandOutput {
        status_code: exit_status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out_bytes).into_owned(),
        stderr: String::from_utf8_lossy(&err_bytes).into_owned(),
        truncated: out_trunc || err_trunc,
        output_file: Some(log_path),
    })
}

/// Entrypoint executed inside the `--sandbox-child` process: applies Landlock, then runs command.
pub fn execute_sandbox_child(raw_spec: &str) -> Result<i32, PolicyError> {
    let spec: SandboxSpec = serde_json::from_str(raw_spec)?;
    apply_sandbox_in_child(&spec)?;
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args).current_dir(&spec.workspace_root);
    if !spec.read_only_workspace {
        // Keep temp scratch on the workspace filesystem so cross-mount renames
        // from tmpfs `/tmp` into `target/` do not fail with EXDEV.
        let ws_tmp = spec.workspace_root.join(".rein").join("tmp");
        if std::fs::create_dir_all(&ws_tmp).is_ok() {
            cmd.env("TMPDIR", &ws_tmp);
        }
    }
    let status = cmd.status().map_err(|source| PolicyError::Io {
        path: PathBuf::from(&spec.program),
        source,
    })?;
    Ok(status.code().unwrap_or(1))
}

type SharedLogWriter = Arc<Mutex<BufWriter<File>>>;

fn open_stream_log(output_dir: Option<&Path>) -> Result<(PathBuf, SharedLogWriter), PolicyError> {
    let dir = output_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::env::temp_dir().join("rein-bash-outputs"));
    std::fs::create_dir_all(&dir).map_err(|source| PolicyError::Io {
        path: dir.clone(),
        source,
    })?;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = OUTPUT_SEQ.fetch_add(1, Ordering::Relaxed);
    let log_path = dir.join(format!("bash-{ts}-{seq}.log"));
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_path)
        .map_err(|source| PolicyError::Io {
            path: log_path.clone(),
            source,
        })?;
    Ok((log_path, Arc::new(Mutex::new(BufWriter::new(file)))))
}

fn drain_pipe<R: Read>(
    pipe: Option<R>,
    header: &str,
    log: &Arc<Mutex<BufWriter<File>>>,
) -> std::io::Result<(Vec<u8>, bool)> {
    let Some(mut reader) = pipe else {
        return Ok((Vec::new(), false));
    };
    let mut buf = [0u8; 4096];
    let mut inline = Vec::new();
    let mut truncated = false;
    let mut wrote_header = false;

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let chunk = &buf[..n];
        if let Ok(mut guard) = log.lock() {
            if !wrote_header {
                guard.write_all(header.as_bytes())?;
                wrote_header = true;
            }
            guard.write_all(chunk)?;
            guard.flush()?;
        }
        if inline.len() < OUTPUT_CAP_BYTES {
            let room = OUTPUT_CAP_BYTES - inline.len();
            if n > room {
                inline.extend_from_slice(&chunk[..room]);
                truncated = true;
            } else {
                inline.extend_from_slice(chunk);
            }
        } else {
            truncated = true;
        }
    }
    Ok((inline, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_default_paths_expose_dev_null_and_toolchains_without_home_root() {
        let ws = std::env::temp_dir().join("rein_spec_paths_test_ws");
        let ro = default_sandbox_read_only_paths(&ws);
        assert!(ro.contains(&PathBuf::from("/usr")));
        assert!(ro.contains(&PathBuf::from("/dev")));
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            assert!(!ro.contains(&home), "must not expose $HOME broadly");
        }

        let rw_for_ro_ws = default_sandbox_read_write_paths(&ws, true);
        assert!(rw_for_ro_ws.contains(&PathBuf::from("/dev/null")));
        assert!(!rw_for_ro_ws.contains(&std::env::temp_dir()));

        let non_tmp_ws = PathBuf::from("/nonexistent_rein_workspace_root_for_test");
        let rw_for_real_ws = default_sandbox_read_write_paths(&non_tmp_ws, false);
        assert!(rw_for_real_ws.contains(&PathBuf::from("/dev/null")));
        assert!(rw_for_real_ws.contains(&std::env::temp_dir()));
    }
}
