// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Fast user-space path canonicalization pre-check (paired with Landlock enforcement)

use std::path::{Component, Path, PathBuf};

use crate::engine::PolicyError;

/// Fast user-space pre-check for workspace confinement; Landlock provides kernel enforcement.
#[derive(Debug, Clone)]
pub struct WorkspaceSandbox {
    root: PathBuf,
}

impl WorkspaceSandbox {
    /// Creates a [`WorkspaceSandbox`] anchored at `root` (canonicalized when present on disk).
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let raw = root.into();
        let resolved = std::fs::canonicalize(&raw).unwrap_or_else(|_| normalize_lexical(&raw));
        Self { root: resolved }
    }

    /// Returns the workspace root path.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Pre-checks `candidate` by resolving existing symlink ancestors and lexical components.
    pub fn resolve_within(&self, candidate: &Path) -> Result<PathBuf, PolicyError> {
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root.join(candidate)
        };
        let normalized = canonicalize_existing_prefix(&joined);
        if !normalized.starts_with(&self.root) {
            return Err(PolicyError::SandboxEscape {
                root: self.root.clone(),
                attempted: normalized,
            });
        }
        // Reject symlinks (including dangling symlinks) whose target escapes `self.root`.
        if let Some(target) = resolve_symlink_target_if_any(&joined, &self.root)
            && !target.starts_with(&self.root)
        {
            return Err(PolicyError::SandboxEscape {
                root: self.root.clone(),
                attempted: target,
            });
        }
        Ok(normalized)
    }

    /// Returns `true` when `candidate` (after normalizing `..` and resolving symlinks) hits `.git`, `.vscode`, or `.rein`.
    #[must_use]
    pub fn is_protected(&self, candidate: &Path) -> bool {
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root.join(candidate)
        };
        let normalized = canonicalize_existing_prefix(&joined);
        let rel = normalized.strip_prefix(&self.root).unwrap_or(candidate);
        if has_protected_component(rel) {
            return true;
        }
        if let Some(target) = resolve_symlink_target_if_any(&joined, &self.root) {
            let target_rel = target.strip_prefix(&self.root).unwrap_or(&target);
            if has_protected_component(target_rel) {
                return true;
            }
        }
        false
    }
}

fn has_protected_component(path: &Path) -> bool {
    path.components().any(|c| {
        matches!(
            c,
            Component::Normal(name)
                if name == ".git" || name == ".vscode" || name == ".rein"
        )
    })
}

fn resolve_symlink_target_if_any(path: &Path, root: &Path) -> Option<PathBuf> {
    let mut current = normalize_lexical(path);
    for _ in 0..16 {
        let meta = std::fs::symlink_metadata(&current).ok()?;
        if !meta.file_type().is_symlink() {
            return Some(canonicalize_existing_prefix(&current));
        }
        let target = std::fs::read_link(&current).ok()?;
        let next = if target.is_absolute() {
            target
        } else {
            current.parent().unwrap_or(root).join(target)
        };
        current = normalize_lexical(&next);
    }
    Some(current)
}

fn canonicalize_existing_prefix(path: &Path) -> PathBuf {
    let lexical = normalize_lexical(path);
    let mut current = lexical.as_path();
    let mut tail = Vec::new();
    while std::fs::symlink_metadata(current).is_err() {
        if let Some(name) = current.file_name() {
            tail.push(name.to_os_string());
        }
        match current.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => current = parent,
            _ => return lexical,
        }
    }
    if let Ok(meta) = std::fs::symlink_metadata(current)
        && meta.file_type().is_symlink()
        && let Ok(link_target) = std::fs::read_link(current)
    {
        let resolved_link = if link_target.is_absolute() {
            link_target
        } else {
            current
                .parent()
                .map_or_else(|| link_target.clone(), |p| p.join(&link_target))
        };
        let mut base = canonicalize_existing_prefix(&resolved_link);
        for part in tail.into_iter().rev() {
            base.push(part);
        }
        return base;
    }
    let mut base = std::fs::canonicalize(current).unwrap_or_else(|_| current.to_path_buf());
    for part in tail.into_iter().rev() {
        base.push(part);
    }
    base
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_parent_traversal_escape() {
        let sandbox = WorkspaceSandbox::new("/home/kelexine/dev/rein");
        assert!(
            sandbox
                .resolve_within(Path::new("crates/rein-core/Cargo.toml"))
                .is_ok()
        );
        assert!(
            sandbox
                .resolve_within(Path::new("../../etc/passwd"))
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape_for_existing_and_nonexistent_targets()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join("rein_workspace_symlink_test");
        let _ = std::fs::remove_dir_all(&base);
        let ws = base.join("ws");
        let outside = base.join("outside");
        std::fs::create_dir_all(ws.join("sub"))?;
        std::fs::create_dir_all(&outside)?;
        std::fs::write(ws.join("sub/inside.txt"), "safe")?;
        std::fs::write(outside.join("secret.txt"), "secret")?;

        symlink(&outside, ws.join("escape_link"))?;
        symlink(ws.join("sub/inside.txt"), ws.join("safe_link.txt"))?;

        let sandbox = WorkspaceSandbox::new(&ws);
        assert!(matches!(
            sandbox.resolve_within(Path::new("escape_link/secret.txt")),
            Err(PolicyError::SandboxEscape { .. })
        ));
        assert!(matches!(
            sandbox.resolve_within(Path::new("escape_link/new_dir/created.txt")),
            Err(PolicyError::SandboxEscape { .. })
        ));
        assert!(sandbox.resolve_within(Path::new("safe_link.txt")).is_ok());

        let _ = std::fs::remove_dir_all(&base);
        Ok(())
    }
}
