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
