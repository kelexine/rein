// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-19
// Purpose: Pure-Rust subagent workspace isolation modes (inherit, share, branch) without git shelling out

use std::path::{Path, PathBuf};

use rein_tools::ToolError;

const SHARE_SKIP_DIRS: &[&str] = &[".git", ".rein", "target", "node_modules"];
const BRANCH_SKIP_DIRS: &[&str] = &[".rein", "target", "node_modules"];

/// Workspace directory isolation mode for spawned subagents.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WorkspaceIsolation {
    /// Share parent workspace directory directly (default for read-only research/verification).
    #[default]
    Inherit,
    /// Isolated working copy omitting `.git` / build caches via pure-Rust `std::fs::copy`.
    Share,
    /// Full isolated workspace copy including `.git` (omitting build caches) for independent git/branch state.
    Branch,
}

impl WorkspaceIsolation {
    /// Parses `"inherit"`, `"share"`, or `"branch"` (defaults to `Inherit` when omitted).
    pub fn parse(raw: Option<&str>) -> Result<Self, ToolError> {
        match raw.map(str::trim).filter(|s| !s.is_empty()) {
            None => Ok(Self::Inherit),
            Some(s) if s.eq_ignore_ascii_case("inherit") => Ok(Self::Inherit),
            Some(s) if s.eq_ignore_ascii_case("share") => Ok(Self::Share),
            Some(s) if s.eq_ignore_ascii_case("branch") => Ok(Self::Branch),
            Some(other) => Err(ToolError::InvalidArguments {
                tool: "invoke_subagent".to_owned(),
                reason: format!(
                    "unknown workspace mode '{other}' (expected 'inherit', 'share', or 'branch')"
                ),
            }),
        }
    }

    /// Lowercase wire identifier.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Share => "share",
            Self::Branch => "branch",
        }
    }
}

/// Prepares `(effective_workspace_root, optional_isolated_dir_to_cleanup)` without spawning subprocesses.
pub fn prepare_subagent_workspace(
    parent_root: &Path,
    workspaces_root: &Path,
    sub_id: &str,
    mode: WorkspaceIsolation,
) -> Result<(PathBuf, Option<PathBuf>), ToolError> {
    match mode {
        WorkspaceIsolation::Inherit => Ok((parent_root.to_path_buf(), None)),
        WorkspaceIsolation::Share => {
            let dest = workspaces_root.join(sub_id);
            copy_tree_filtered(parent_root, &dest, SHARE_SKIP_DIRS)?;
            Ok((dest.clone(), Some(dest)))
        }
        WorkspaceIsolation::Branch => {
            let dest = workspaces_root.join(sub_id);
            copy_tree_filtered(parent_root, &dest, BRANCH_SKIP_DIRS)?;
            Ok((dest.clone(), Some(dest)))
        }
    }
}

/// Removes an isolated subagent workspace directory if present.
pub fn cleanup_subagent_workspace(isolated_dir: Option<&Path>) {
    if let Some(dir) = isolated_dir {
        let _ = std::fs::remove_dir_all(dir);
    }
}

fn copy_tree_filtered(src: &Path, dst: &Path, skip_dirs: &[&str]) -> Result<(), ToolError> {
    std::fs::create_dir_all(dst).map_err(|source| ToolError::Io {
        path: dst.to_path_buf(),
        source,
    })?;
    let entries = std::fs::read_dir(src).map_err(|source| ToolError::Io {
        path: src.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| ToolError::Io {
            path: src.to_path_buf(),
            source,
        })?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if skip_dirs.iter().any(|skip| *skip == name_str) {
            continue;
        }
        let src_path = entry.path();
        let dst_path = dst.join(&name);
        let ftype = entry.file_type().map_err(|source| ToolError::Io {
            path: src_path.clone(),
            source,
        })?;
        if ftype.is_symlink() {
            continue;
        }
        if ftype.is_dir() {
            copy_tree_filtered(&src_path, &dst_path, skip_dirs)?;
        } else if ftype.is_file() {
            std::fs::copy(&src_path, &dst_path).map_err(|source| ToolError::Io {
                path: dst_path,
                source,
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_and_branch_isolate_trees_pure_rust_and_cleanup()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("rein_ws_iso_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("src_repo");
        let pool = root.join("workspaces");
        std::fs::create_dir_all(src.join("src"))?;
        std::fs::create_dir_all(src.join(".git"))?;
        std::fs::create_dir_all(src.join("target/debug"))?;
        std::fs::write(src.join("src/lib.rs"), "pub fn ok() {}\n")?;
        std::fs::write(src.join(".git/HEAD"), "ref: refs/heads/main\n")?;
        std::fs::write(src.join("target/debug/bin"), "ignored")?;

        let (inh, inh_iso) =
            prepare_subagent_workspace(&src, &pool, "sub-inh", WorkspaceIsolation::Inherit)?;
        assert_eq!(inh, src);
        assert!(inh_iso.is_none());

        let (sh_dir, sh_iso) =
            prepare_subagent_workspace(&src, &pool, "sub-share", WorkspaceIsolation::Share)?;
        assert!(sh_dir.join("src/lib.rs").exists());
        assert!(!sh_dir.join(".git").exists());
        assert!(!sh_dir.join("target").exists());
        cleanup_subagent_workspace(sh_iso.as_deref());
        assert!(!sh_dir.exists());

        let (br_dir, br_iso) =
            prepare_subagent_workspace(&src, &pool, "sub-branch", WorkspaceIsolation::Branch)?;
        assert!(br_dir.join("src/lib.rs").exists());
        assert!(br_dir.join(".git/HEAD").exists());
        assert!(!br_dir.join("target").exists());
        cleanup_subagent_workspace(br_iso.as_deref());
        assert!(!br_dir.exists());

        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }
}
