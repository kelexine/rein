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
        if normalized.starts_with(&self.root) {
            Ok(normalized)
        } else {
            Err(PolicyError::SandboxEscape {
                root: self.root.clone(),
                attempted: normalized,
            })
        }
    }
}

fn canonicalize_existing_prefix(path: &Path) -> PathBuf {
    let lexical = normalize_lexical(path);
    let mut current = lexical.as_path();
    let mut tail = Vec::new();
    while !current.exists() {
        if let Some(name) = current.file_name() {
            tail.push(name.to_os_string());
        }
        match current.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => current = parent,
            _ => return lexical,
        }
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
}
