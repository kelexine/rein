// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-19
// Purpose: .gitignore-aware workspace file walker for GlobTool and GrepTool

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::coding::search::glob::{glob_match_path, glob_match_segment};
use crate::registry::ToolError;

/// Built-in directories always skipped during workspace search traversal.
const HARD_IGNORED_DIRS: &[&str] = &[".git", "target", "node_modules", ".rein"];

/// Traversal options controlling hidden files and `.gitignore` filtering.
#[derive(Debug, Clone, Copy, Default)]
pub struct WalkOptions {
    /// Include dotfiles and dot-directories (still skips `.git` and `.rein`).
    pub include_hidden: bool,
    /// Include files matched by `.gitignore` rules.
    pub include_ignored: bool,
}

/// Discovered workspace file entry with workspace-relative path and optional modification time.
#[derive(Debug, Clone)]
pub struct WalkEntry {
    /// Forward-slash relative path from `WorkspaceSandbox::root()`.
    pub rel_path: String,
    /// Last modification timestamp when available.
    pub mtime: Option<SystemTime>,
}

#[derive(Debug, Clone)]
struct IgnoreRule {
    base_rel: String,
    pattern: String,
    dir_only: bool,
    anchored: bool,
    negated: bool,
}

impl IgnoreRule {
    fn parse(base_rel: &str, raw_line: &str) -> Option<Self> {
        let trimmed = raw_line.trim_end_matches(['\r', '\n']);
        if trimmed.trim().is_empty() || trimmed.starts_with('#') {
            return None;
        }
        let mut pat = trimmed.trim();
        let negated = if let Some(rest) = pat.strip_prefix('!') {
            pat = rest;
            true
        } else {
            false
        };
        let dir_only = pat.ends_with('/');
        pat = pat.trim_end_matches('/');
        let anchored = pat.starts_with('/') || pat.contains('/');
        pat = pat.trim_start_matches('/');
        if pat.is_empty() {
            return None;
        }
        Some(Self {
            base_rel: base_rel.to_owned(),
            pattern: pat.to_owned(),
            dir_only,
            anchored,
            negated,
        })
    }

    fn matches(&self, rel_path: &str, is_dir: bool) -> bool {
        if self.dir_only && !is_dir {
            return false;
        }
        let scoped_rel = if self.base_rel.is_empty() {
            rel_path
        } else {
            let Some(rest) = rel_path.strip_prefix(&self.base_rel) else {
                return false;
            };
            rest.strip_prefix('/').unwrap_or(rest)
        };
        if scoped_rel.is_empty() {
            return false;
        }
        if self.anchored {
            glob_match_path(&self.pattern, scoped_rel)
                || self
                    .pattern
                    .strip_prefix("**/")
                    .is_some_and(|sub| glob_match_path(sub, scoped_rel))
        } else {
            let file_name = scoped_rel.rsplit('/').next().unwrap_or(scoped_rel);
            glob_match_segment(&self.pattern, file_name)
                || glob_match_path(&self.pattern, scoped_rel)
        }
    }
}

/// Collects workspace files under `start_dir` (relative to `root`), respecting `.gitignore` and symlinks.
pub fn collect_workspace_entries(
    start_dir: &Path,
    root: &Path,
    opts: WalkOptions,
) -> Result<Vec<WalkEntry>, ToolError> {
    let mut rules = load_ancestor_gitignores(start_dir, root);
    let mut out = Vec::new();
    walk_dir(start_dir, root, opts, &mut rules, &mut out)?;
    Ok(out)
}

fn load_ancestor_gitignores(start_dir: &Path, root: &Path) -> Vec<IgnoreRule> {
    let mut dirs = Vec::new();
    let mut cur = start_dir.parent();
    while let Some(dir) = cur {
        if !dir.starts_with(root) {
            break;
        }
        dirs.push(dir.to_path_buf());
        if dir == root {
            break;
        }
        cur = dir.parent();
    }
    dirs.reverse();
    let mut rules = Vec::new();
    for dir in dirs {
        append_gitignore_rules(&dir, root, &mut rules);
    }
    rules
}

fn append_gitignore_rules(dir: &Path, root: &Path, rules: &mut Vec<IgnoreRule>) {
    let gitignore = dir.join(".gitignore");
    let Ok(raw) = std::fs::read_to_string(gitignore) else {
        return;
    };
    let base_rel = dir
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    for line in raw.lines() {
        if let Some(rule) = IgnoreRule::parse(&base_rel, line) {
            rules.push(rule);
        }
    }
}

fn is_gitignored(rules: &[IgnoreRule], rel_path: &str, is_dir: bool) -> bool {
    let mut ignored = false;
    for rule in rules {
        if rule.matches(rel_path, is_dir) {
            ignored = !rule.negated;
        }
    }
    ignored
}

fn walk_dir(
    dir: &Path,
    root: &Path,
    opts: WalkOptions,
    rules: &mut Vec<IgnoreRule>,
    out: &mut Vec<WalkEntry>,
) -> Result<(), ToolError> {
    let saved_len = rules.len();
    if !opts.include_ignored {
        append_gitignore_rules(dir, root, rules);
    }

    let entries = std::fs::read_dir(dir).map_err(|source| ToolError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|d| d.path())).collect();
    paths.sort();

    for path in paths {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if HARD_IGNORED_DIRS.contains(&name) {
            continue;
        }
        if !opts.include_hidden && name.starts_with('.') {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel_path = rel.to_string_lossy().replace('\\', "/");
        let is_dir = meta.is_dir();

        if !opts.include_ignored && is_gitignored(rules, &rel_path, is_dir) {
            continue;
        }
        if is_dir {
            walk_dir(&path, root, opts, rules, out)?;
        } else if meta.is_file() {
            out.push(WalkEntry {
                rel_path,
                mtime: meta.modified().ok(),
            });
        }
    }

    rules.truncate(saved_len);
    Ok(())
}
