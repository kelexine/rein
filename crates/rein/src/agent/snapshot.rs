// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-19
// Purpose: Per-turn workspace snapshots and /rewind restoration across Git and non-Git workspaces

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

static SNAP_SEQ: AtomicU64 = AtomicU64::new(1);

const IGNORED_DIRS: &[&str] = &[".git", ".rein", "target", "node_modules"];

/// Captured workspace state prior to a single user turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceSnapshot {
    /// Git tree object created via a temporary `GIT_INDEX_FILE` (leaves real `.git/index` untouched).
    GitTree {
        /// Git tree SHA-1 representing all tracked and untracked non-ignored files.
        tree_sha: String,
    },
    /// Fallback in-memory file manifest for non-Git workspaces.
    InMemory {
        /// Relative workspace paths mapped to file contents.
        files: BTreeMap<String, Vec<u8>>,
    },
}

/// Recorded pre-turn checkpoint enabling `/rewind` of messages and/or workspace code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnCheckpoint {
    /// Zero-based user turn index within the active session.
    pub turn_index: usize,
    /// Single-line preview of the user prompt for this turn.
    pub prompt_preview: String,
    /// Unix timestamp (seconds) when this turn started.
    pub timestamp_secs: u64,
    /// `AgentLoop::history` length immediately before this turn's user message.
    pub history_len: usize,
    /// Number of JSONL lines in `session.jsonl` immediately before this turn.
    pub event_count: usize,
    /// Number of JSONL lines in `full-transcript.jsonl` immediately before this turn.
    pub full_transcript_count: usize,
    /// Pre-turn workspace snapshot, if captured.
    pub snapshot: Option<WorkspaceSnapshot>,
}

/// Returns the current Unix timestamp in seconds.
#[must_use]
pub fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Formats a relative elapsed time label like `"3m ago"` or `"just now"`.
#[must_use]
pub fn format_relative_age(timestamp_secs: u64, now_secs: u64) -> String {
    let elapsed = now_secs.saturating_sub(timestamp_secs);
    if elapsed < 60 {
        "just now".to_owned()
    } else if elapsed < 3_600 {
        format!("{}m ago", elapsed / 60)
    } else if elapsed < 86_400 {
        format!("{}h ago", elapsed / 3_600)
    } else {
        format!("{}d ago", elapsed / 86_400)
    }
}

/// Captures a workspace snapshot without modifying `.git/index` or `HEAD`.
pub fn capture_workspace_snapshot(workspace_root: &Path) -> Result<WorkspaceSnapshot> {
    if is_git_workspace(workspace_root) {
        let tree_sha = capture_git_tree(workspace_root)?;
        return Ok(WorkspaceSnapshot::GitTree { tree_sha });
    }
    let mut files = BTreeMap::new();
    collect_files(workspace_root, workspace_root, &mut files)?;
    Ok(WorkspaceSnapshot::InMemory { files })
}

/// Restores the workspace to `snapshot`, reverting edits, deletions, and newly created files.
pub fn restore_workspace_snapshot(
    workspace_root: &Path,
    snapshot: &WorkspaceSnapshot,
) -> Result<()> {
    match snapshot {
        WorkspaceSnapshot::GitTree { tree_sha } => restore_git_tree(workspace_root, tree_sha),
        WorkspaceSnapshot::InMemory { files } => restore_in_memory(workspace_root, files),
    }
}

fn is_git_workspace(workspace_root: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .is_ok_and(|out| out.status.success())
}

struct TempIndexGuard(PathBuf);

impl TempIndexGuard {
    fn new() -> Self {
        let seq = SNAP_SEQ.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("rein_snap_idx_{}_{}", std::process::id(), seq));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempIndexGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn capture_git_tree(workspace_root: &Path) -> Result<String> {
    let idx = TempIndexGuard::new();
    let add = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .env("GIT_INDEX_FILE", idx.path())
        .args(["add", "-A"])
        .output()
        .context("failed to run git add -A for snapshot")?;
    if !add.status.success() {
        bail!(
            "git add -A snapshot failed: {}",
            String::from_utf8_lossy(&add.stderr).trim()
        );
    }

    let write = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .env("GIT_INDEX_FILE", idx.path())
        .arg("write-tree")
        .output()
        .context("failed to run git write-tree for snapshot")?;
    if !write.status.success() {
        bail!(
            "git write-tree snapshot failed: {}",
            String::from_utf8_lossy(&write.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&write.stdout).trim().to_owned())
}

fn restore_git_tree(workspace_root: &Path, target_tree: &str) -> Result<()> {
    let current_tree = capture_git_tree(workspace_root)?;
    if current_tree == target_tree {
        return Ok(());
    }

    let diff = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .args([
            "diff-tree",
            "-r",
            "--name-status",
            "-z",
            target_tree,
            &current_tree,
        ])
        .output()
        .context("failed to diff snapshot trees")?;
    if !diff.status.success() {
        bail!(
            "git diff-tree failed: {}",
            String::from_utf8_lossy(&diff.stderr).trim()
        );
    }

    // `diff-tree -z` emits `<status>\0<path>\0...`; remove files added since `target_tree`.
    let mut parts = diff.stdout.split(|&b| b == 0).filter(|s| !s.is_empty());
    while let Some(status) = parts.next() {
        let Some(path_bytes) = parts.next() else {
            break;
        };
        if status.starts_with(b"A") {
            let rel = String::from_utf8_lossy(path_bytes);
            let full = workspace_root.join(rel.as_ref());
            if full.exists() {
                let _ = std::fs::remove_file(&full);
            }
        }
    }

    let idx = TempIndexGuard::new();
    let read_tree = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .env("GIT_INDEX_FILE", idx.path())
        .args(["read-tree", target_tree])
        .output()
        .context("failed to run git read-tree for restore")?;
    if !read_tree.status.success() {
        bail!(
            "git read-tree restore failed: {}",
            String::from_utf8_lossy(&read_tree.stderr).trim()
        );
    }

    let checkout = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .env("GIT_INDEX_FILE", idx.path())
        .args(["checkout-index", "-a", "-f"])
        .output()
        .context("failed to run git checkout-index for restore")?;
    if !checkout.status.success() {
        bail!(
            "git checkout-index restore failed: {}",
            String::from_utf8_lossy(&checkout.stderr).trim()
        );
    }
    Ok(())
}

fn collect_files(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) -> Result<()> {
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("failed to read directory {}", dir.display()))?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if IGNORED_DIRS.iter().any(|ignored| *ignored == name_str) {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_files(root, &path, out)?;
        } else if file_type.is_file() {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            let bytes = std::fs::read(&path)
                .with_context(|| format!("failed to read snapshot file {}", path.display()))?;
            out.insert(rel, bytes);
        }
    }
    Ok(())
}

fn restore_in_memory(
    workspace_root: &Path,
    target_files: &BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    let mut current = BTreeMap::new();
    collect_files(workspace_root, workspace_root, &mut current)?;
    for rel in current.keys() {
        if !target_files.contains_key(rel) {
            let _ = std::fs::remove_file(workspace_root.join(rel));
        }
    }
    for (rel, bytes) in target_files {
        let full = workspace_root.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&full, bytes)
            .with_context(|| format!("failed to restore file {}", full.display()))?;
    }
    Ok(())
}

/// Truncates a JSONL file to at most `keep_lines` lines.
pub fn truncate_jsonl_lines(path: &Path, keep_lines: usize) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let kept: Vec<&str> = raw.lines().take(keep_lines).collect();
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    std::fs::write(path, out).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Counts the number of lines in a JSONL file (0 if missing).
#[must_use]
pub fn count_jsonl_lines(path: &Path) -> usize {
    std::fs::read_to_string(path).map_or(0, |raw| raw.lines().count())
}

/// Returns the sidecar JSONL path storing [`TurnCheckpoint`] records for `event_log_path`.
#[must_use]
pub fn checkpoints_path_for(event_log_path: &Path) -> PathBuf {
    event_log_path.with_extension("checkpoints.jsonl")
}

/// Appends one [`TurnCheckpoint`] record as a JSONL line.
pub fn append_checkpoint(path: &Path, checkpoint: &TurnCheckpoint) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open checkpoint log {}", path.display()))?;
    let line = serde_json::to_string(checkpoint)?;
    writeln!(file, "{line}")
        .with_context(|| format!("failed to write checkpoint log {}", path.display()))
}

/// Loads persisted [`TurnCheckpoint`] records from `path` (empty if missing).
pub fn load_checkpoints(path: &Path) -> Result<Vec<TurnCheckpoint>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read checkpoint log {}", path.display()))?;
    let mut out = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(cp) = serde_json::from_str::<TurnCheckpoint>(trimmed) {
            out.push(cp);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_snapshot_and_restore_reverts_edits_deletes_and_new_files()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join(format!("rein_snap_git_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;

        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&dir)
                .arg("init")
                .status()?
                .success()
        );
        std::fs::write(dir.join("a.txt"), "original a\n")?;
        std::fs::write(dir.join("b.txt"), "original b\n")?;

        let snap = capture_workspace_snapshot(&dir)?;

        // Mutate workspace via direct file ops (simulating edit, write, or bash sed/python/rm).
        std::fs::write(dir.join("a.txt"), "modified a\n")?;
        std::fs::remove_file(dir.join("b.txt"))?;
        std::fs::write(dir.join("c.txt"), "created c\n")?;

        restore_workspace_snapshot(&dir, &snap)?;
        assert_eq!(std::fs::read_to_string(dir.join("a.txt"))?, "original a\n");
        assert_eq!(std::fs::read_to_string(dir.join("b.txt"))?, "original b\n");
        assert!(!dir.join("c.txt").exists());

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn in_memory_snapshot_and_restore_works_without_git() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = std::env::temp_dir().join(format!("rein_snap_nogit_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;

        std::fs::write(dir.join("main.py"), "print(1)\n")?;
        let snap = capture_workspace_snapshot(&dir)?;
        assert!(matches!(snap, WorkspaceSnapshot::InMemory { .. }));

        std::fs::write(dir.join("main.py"), "print(2)\n")?;
        std::fs::write(dir.join("extra.py"), "x = 1\n")?;

        restore_workspace_snapshot(&dir, &snap)?;
        assert_eq!(std::fs::read_to_string(dir.join("main.py"))?, "print(1)\n");
        assert!(!dir.join("extra.py").exists());

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
