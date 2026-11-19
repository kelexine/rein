// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-19
// Purpose: Pure-Rust per-turn workspace snapshots and symlink-safe /rewind restoration

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const IGNORED_DIRS: &[&str] = &[".git", ".rein", "target", "node_modules"];

/// Captured workspace state prior to a single user turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceSnapshot {
    /// Legacy checkpoint format retained solely for backwards JSON deserialization.
    GitTree {
        /// Historical git tree SHA-1 identifier.
        tree_sha: String,
    },
    /// Pure-Rust in-memory file manifest capturing workspace files without shelling out.
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

/// Captures a pure-Rust workspace snapshot without modifying `.git` or shelling out.
pub fn capture_workspace_snapshot(workspace_root: &Path) -> Result<WorkspaceSnapshot> {
    let mut files = BTreeMap::new();
    collect_files(workspace_root, workspace_root, &mut files)?;
    Ok(WorkspaceSnapshot::InMemory { files })
}

/// Restores the workspace to `snapshot`, reverting edits, deletions, and newly created files safely.
pub fn restore_workspace_snapshot(
    workspace_root: &Path,
    snapshot: &WorkspaceSnapshot,
) -> Result<()> {
    match snapshot {
        WorkspaceSnapshot::GitTree { tree_sha } => {
            bail!(
                "cannot restore legacy git_tree snapshot '{tree_sha}' (git shelling out is disabled)"
            )
        }
        WorkspaceSnapshot::InMemory { files } => restore_in_memory(workspace_root, files),
    }
}

fn validate_restore_target(
    sandbox: &rein_policy::WorkspaceSandbox,
    workspace_root: &Path,
    rel: &str,
) -> Result<PathBuf> {
    let candidate = workspace_root.join(rel);
    let resolved = sandbox
        .resolve_within(&candidate)
        .map_err(|err| anyhow::anyhow!("refusing snapshot path outside workspace: {err}"))?;
    if std::fs::symlink_metadata(&resolved).is_ok_and(|meta| meta.file_type().is_symlink()) {
        bail!(
            "refusing to modify symlink path during restore: {}",
            resolved.display()
        );
    }
    Ok(resolved)
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
        if file_type.is_symlink() {
            continue;
        }
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
    let sandbox = rein_policy::WorkspaceSandbox::new(workspace_root);
    let mut current = BTreeMap::new();
    collect_files(workspace_root, workspace_root, &mut current)?;
    for rel in current.keys() {
        if !target_files.contains_key(rel)
            && let Ok(full) = validate_restore_target(&sandbox, workspace_root, rel)
        {
            let _ = std::fs::remove_file(full);
        }
    }
    for (rel, bytes) in target_files {
        let full = validate_restore_target(&sandbox, workspace_root, rel)?;
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
    fn snapshot_and_restore_work_without_git_and_preserve_git_dir()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join(format!("rein_snap_pure_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git"))?;
        std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n")?;
        std::fs::write(dir.join("a.txt"), "original a\n")?;
        std::fs::write(dir.join("b.txt"), "original b\n")?;

        let snap = capture_workspace_snapshot(&dir)?;
        assert!(matches!(snap, WorkspaceSnapshot::InMemory { .. }));

        std::fs::write(dir.join("a.txt"), "modified a\n")?;
        std::fs::remove_file(dir.join("b.txt"))?;
        std::fs::write(dir.join("c.txt"), "created c\n")?;
        std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/dev\n")?;

        restore_workspace_snapshot(&dir, &snap)?;
        assert_eq!(std::fs::read_to_string(dir.join("a.txt"))?, "original a\n");
        assert_eq!(std::fs::read_to_string(dir.join("b.txt"))?, "original b\n");
        assert!(!dir.join("c.txt").exists());
        // `.git/` is never overwritten or touched by snapshots.
        assert_eq!(
            std::fs::read_to_string(dir.join(".git/HEAD"))?,
            "ref: refs/heads/dev\n"
        );

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
