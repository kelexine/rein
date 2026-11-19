// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Direct subprocess execution with inherited shell configs, process groups, and streamed log capture

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::engine::PolicyError;
use crate::sandbox::linux::{
    configure_sandbox_child_cmd, harden_process_dumpable, signal_process_group,
};

/// Maximum bytes retained inline per stream (`stdout` / `stderr`) in [`SandboxCommandOutput`].
pub const OUTPUT_CAP_BYTES: usize = 32 * 1024;

static OUTPUT_SEQ: AtomicU64 = AtomicU64::new(0);

/// Policy/process isolation status reported at startup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SandboxStatus {
    /// Policy-gated shell execution inheriting user shell configs and environment.
    PolicyOnly,
    /// Optional legacy enforced marker kept for compatibility.
    Enforced {
        /// Kernel ABI version.
        abi_version: u32,
    },
    /// Optional legacy unavailable marker.
    Unavailable {
        /// Diagnostic reason.
        reason: String,
    },
}

impl SandboxStatus {
    /// Returns `true` when policy-gated execution is active.
    #[must_use]
    pub fn is_enforced(&self) -> bool {
        !matches!(self, Self::Unavailable { .. })
    }
}

/// Specification for running a child command with inherited shell configs and streamed log capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxSpec {
    /// Working directory for the executed command.
    pub workspace_root: PathBuf,
    /// Target executable to run.
    pub program: String,
    /// Arguments passed to `program`.
    pub args: Vec<String>,
    /// Optional directory where full streamed `stdout`/`stderr` log files are written by the parent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<PathBuf>,
}

/// Captured bounded output from a subprocess plus path to the full streamed log file.
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

/// Handle to an active subprocess running in its own session/process group.
pub struct SpawnedSandboxProcess {
    child: Mutex<Option<Child>>,
    pgid: i32,
    stdin: Mutex<Option<ChildStdin>>,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    log_path: PathBuf,
    log_writer: SharedLogWriter,
    exited: AtomicBool,
    killed: AtomicBool,
}

impl SpawnedSandboxProcess {
    /// Returns the OS process group ID assigned via `setsid()`.
    #[must_use]
    pub fn pgid(&self) -> i32 {
        self.pgid
    }

    /// Returns the streamed log file path on disk.
    #[must_use]
    pub fn log_path(&self) -> &Path {
        &self.log_path
    }

    /// Returns a clone of the shared buffered log writer.
    #[must_use]
    pub fn log_writer(&self) -> SharedLogWriter {
        Arc::clone(&self.log_writer)
    }

    /// Takes ownership of `stdout` and `stderr` pipes for background reader threads.
    pub fn take_stdout_stderr(&mut self) -> (Option<ChildStdout>, Option<ChildStderr>) {
        (self.stdout.take(), self.stderr.take())
    }

    /// Writes `bytes` to the child process standard input and flushes immediately.
    pub fn write_stdin(&self, bytes: &[u8]) -> Result<(), PolicyError> {
        let mut guard = self
            .stdin
            .lock()
            .map_err(|_| PolicyError::SandboxApply("failed to lock child stdin".to_owned()))?;
        let Some(stdin) = guard.as_mut() else {
            return Err(PolicyError::SandboxApply(
                "task standard input is closed".to_owned(),
            ));
        };
        stdin.write_all(bytes).map_err(|source| PolicyError::Io {
            path: self.log_path.clone(),
            source,
        })?;
        stdin.flush().map_err(|source| PolicyError::Io {
            path: self.log_path.clone(),
            source,
        })
    }

    /// Closes standard input so waiting processes receive EOF.
    pub fn close_stdin(&self) {
        if let Ok(mut guard) = self.stdin.lock() {
            let _ = guard.take();
        }
    }

    /// Terminates the process group cleanly (`SIGTERM`, 1.5s grace period, then `SIGKILL`).
    pub fn kill(&self) {
        self.killed.store(true, Ordering::Release);
        self.close_stdin();
        if self.exited.load(Ordering::Acquire) {
            return;
        }
        let _ = signal_process_group(self.pgid, false);
        for _ in 0..30 {
            if self.exited.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if !self.exited.load(Ordering::Acquire) {
            let _ = signal_process_group(self.pgid, true);
        }
    }

    /// Returns `true` if [`Self::kill`] was invoked for this process.
    #[must_use]
    pub fn was_killed(&self) -> bool {
        self.killed.load(Ordering::Acquire)
    }

    /// Waits for the child process to exit, returning its exit status code.
    pub fn wait(&self) -> Result<i32, PolicyError> {
        let taken = self
            .child
            .lock()
            .map_err(|_| PolicyError::SandboxApply("failed to lock child handle".to_owned()))?
            .take();
        let Some(mut child) = taken else {
            return Ok(-1);
        };
        let status = child.wait().map_err(|source| PolicyError::Io {
            path: self.log_path.clone(),
            source,
        });
        self.exited.store(true, Ordering::Release);
        if let Ok(mut guard) = self.log_writer.lock() {
            let _ = guard.flush();
        }
        status.map(|s| s.code().unwrap_or(-1))
    }
}

fn build_child_cmd(spec: &SandboxSpec, piped_stdin: bool) -> Command {
    harden_process_dumpable();
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args)
        .current_dir(&spec.workspace_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if piped_stdin {
        cmd.stdin(Stdio::piped());
    }
    configure_sandbox_child_cmd(&mut cmd);
    cmd
}

/// Spawns `spec.program` directly in its own session/process group with inherited shell environment and configs.
pub fn spawn_sandboxed_command(spec: &SandboxSpec) -> Result<SpawnedSandboxProcess, PolicyError> {
    let mut cmd = build_child_cmd(spec, true);
    let mut child = cmd.spawn().map_err(|source| PolicyError::Io {
        path: PathBuf::from(&spec.program),
        source,
    })?;
    let pgid = i32::try_from(child.id()).unwrap_or(-1);
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (log_path, log_writer) = open_stream_log(spec.output_dir.as_deref())?;

    Ok(SpawnedSandboxProcess {
        child: Mutex::new(Some(child)),
        pgid,
        stdin: Mutex::new(stdin),
        stdout,
        stderr,
        log_path,
        log_writer,
        exited: AtomicBool::new(false),
        killed: AtomicBool::new(false),
    })
}

/// Runs `spec.program` directly with full inherited shell configs and streams full logs to disk.
pub fn run_sandboxed_command(spec: &SandboxSpec) -> Result<SandboxCommandOutput, PolicyError> {
    let mut cmd = build_child_cmd(spec, false);
    let mut child = cmd.spawn().map_err(|source| PolicyError::Io {
        path: PathBuf::from(&spec.program),
        source,
    })?;

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

/// Public handle to the parent-owned buffered log file writer.
pub type SharedLogWriter = Arc<Mutex<BufWriter<File>>>;

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
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = OUTPUT_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let log_path = dir.join(format!("bash-{pid}-{ts}-{seq}.log"));

    let mut opts = OpenOptions::new();
    opts.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    let file = opts.open(&log_path).map_err(|source| PolicyError::Io {
        path: log_path.clone(),
        source,
    })?;
    Ok((log_path, Arc::new(Mutex::new(BufWriter::new(file)))))
}

/// Streams `pipe` bytes into both `log` and a bounded ring-tailed `sink` (`max_sink_bytes`).
pub fn drain_pipe_to_log_and_sink<R: Read>(
    pipe: Option<R>,
    header: &str,
    log: &SharedLogWriter,
    sink: &Arc<Mutex<Vec<u8>>>,
    max_sink_bytes: usize,
) -> std::io::Result<bool> {
    let Some(mut reader) = pipe else {
        return Ok(false);
    };
    let mut buf = [0u8; 4096];
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
        if let Ok(mut ring) = sink.lock() {
            ring.extend_from_slice(chunk);
            if ring.len() > max_sink_bytes {
                let excess = ring.len() - max_sink_bytes;
                ring.drain(..excess);
                truncated = true;
            }
        }
    }
    Ok(truncated)
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
    fn runs_direct_command_with_inherited_env_and_log() -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join(format!("rein_direct_run_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;

        let out = run_sandboxed_command(&SandboxSpec {
            workspace_root: dir.clone(),
            program: "sh".to_owned(),
            args: vec!["-c".to_owned(), "echo direct-ok".to_owned()],
            output_dir: Some(dir.join("logs")),
        })?;
        assert_eq!(out.status_code, 0);
        assert_eq!(out.stdout.trim(), "direct-ok");
        assert!(out.output_file.as_ref().is_some_and(|p| p.exists()));

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
