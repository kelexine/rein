// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Path target scope analyzer (workspace vs outside_workspace vs protected)

use std::path::Path;

use crate::classifier::effect::TargetScope;
use crate::sandbox::WorkspaceSandbox;

/// Protected workspace-relative directory and file prefixes that agents must not silently tamper with.
const PROTECTED_COMPONENTS: &[&str] = &[".git", ".vscode", ".rein"];

/// Classifies a path token into [`TargetScope::Workspace`], [`TargetScope::OutsideWorkspace`], or [`TargetScope::Protected`].
#[must_use]
pub fn classify_path_token(raw_token: &str, sandbox: Option<&WorkspaceSandbox>) -> TargetScope {
    let token = raw_token.strip_prefix("file://").unwrap_or(raw_token);
    let token = strip_pwd_prefix(token);
    if token.is_empty() || token == "/dev/null" || token == "/dev/zero" {
        return TargetScope::Workspace;
    }
    if is_protected_path(token) {
        return TargetScope::Protected;
    }
    if is_explicit_outside_path(token) {
        return TargetScope::OutsideWorkspace;
    }
    if let Some(sb) = sandbox {
        if sb.resolve_within(Path::new(token)).is_err() {
            return TargetScope::OutsideWorkspace;
        }
    } else if token.starts_with("..") || token.contains("/..") {
        return TargetScope::OutsideWorkspace;
    }
    TargetScope::Workspace
}

/// Checks if a path references `.git`, `.vscode`, or `.rein` protected trees.
#[must_use]
pub fn is_protected_path(token: &str) -> bool {
    let trimmed = token.trim_start_matches("./");
    for protected in PROTECTED_COMPONENTS {
        if trimmed == *protected
            || trimmed.starts_with(&format!("{protected}/"))
            || trimmed.contains(&format!("/{protected}/"))
        {
            return true;
        }
    }
    false
}

fn strip_pwd_prefix(token: &str) -> &str {
    if token == "$PWD" || token == "${PWD}" {
        return ".";
    }
    token
        .strip_prefix("$PWD/")
        .or_else(|| token.strip_prefix("${PWD}/"))
        .unwrap_or(token)
}

fn is_explicit_outside_path(token: &str) -> bool {
    token.starts_with('~')
        || token.starts_with("$HOME")
        || token.starts_with("${HOME}")
        || token.starts_with('/')
        || token.starts_with("../")
        || token == ".."
}

/// Aggregates the maximum [`TargetScope`] across a list of candidate path arguments.
#[must_use]
pub fn classify_args_scope(args: &[String], sandbox: Option<&WorkspaceSandbox>) -> TargetScope {
    let mut scope = TargetScope::Workspace;
    for arg in args {
        if arg.starts_with('-') && !arg.contains('=') {
            continue;
        }
        let candidate = arg.split_once('=').map_or(arg.as_str(), |(_, v)| v);
        if looks_like_path(candidate) {
            scope = scope.max(classify_path_token(candidate, sandbox));
        }
    }
    scope
}

/// Heuristic to distinguish file/path arguments from plain words or flags.
#[must_use]
pub fn looks_like_path(token: &str) -> bool {
    token.starts_with('/')
        || token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with('~')
        || token.starts_with("$HOME")
        || token.starts_with("file://")
        || token.starts_with('.')
        || token.contains('/')
        || token.ends_with(".txt")
        || token.ends_with(".rs")
        || token.ends_with(".toml")
        || token.ends_with(".lock")
        || token.ends_with(".sh")
        || token.ends_with(".tar")
        || token.ends_with(".json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_workspace_outside_and_protected_path_tokens() {
        assert_eq!(classify_path_token("", None), TargetScope::Workspace);
        assert_eq!(
            classify_path_token("/dev/null", None),
            TargetScope::Workspace
        );
        assert_eq!(
            classify_path_token("$PWD/src/main.rs", None),
            TargetScope::Workspace
        );
        assert_eq!(
            classify_path_token("file://src/lib.rs", None),
            TargetScope::Workspace
        );

        assert_eq!(
            classify_path_token(".git/config", None),
            TargetScope::Protected
        );
        assert_eq!(
            classify_path_token("./.vscode/settings.json", None),
            TargetScope::Protected
        );
        assert_eq!(
            classify_path_token("sub/.rein/commands.json", None),
            TargetScope::Protected
        );

        assert_eq!(
            classify_path_token("~/.ssh/id_rsa", None),
            TargetScope::OutsideWorkspace
        );
        assert_eq!(
            classify_path_token("$HOME/.bashrc", None),
            TargetScope::OutsideWorkspace
        );
        assert_eq!(
            classify_path_token("../secret.txt", None),
            TargetScope::OutsideWorkspace
        );
        assert_eq!(
            classify_path_token("/etc/passwd", None),
            TargetScope::OutsideWorkspace
        );
    }

    #[test]
    fn classifies_args_scope_and_sandbox_boundaries() -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("rein_policy_scope_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src"))?;
        std::fs::write(dir.join("src/main.rs"), "fn main() {}")?;
        let sandbox = WorkspaceSandbox::new(&dir);

        assert_eq!(
            classify_args_scope(
                &["--check".to_owned(), "src/main.rs".to_owned()],
                Some(&sandbox)
            ),
            TargetScope::Workspace
        );
        assert_eq!(
            classify_args_scope(&["--output=../escape.txt".to_owned()], Some(&sandbox)),
            TargetScope::OutsideWorkspace
        );
        assert_eq!(
            classify_args_scope(&[".git/hooks/pre-commit".to_owned()], Some(&sandbox)),
            TargetScope::Protected
        );

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
