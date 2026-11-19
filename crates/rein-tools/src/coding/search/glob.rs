// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-19
// Purpose: Advanced GlobTool supporting brace expansion, character classes, exclusions, mtime sort, and pagination

use std::cmp::Reverse;
use std::path::Path;

use rein_policy::{AccessMode, Capability, WorkspaceSandbox};
use serde_json::{Value, json};

use crate::coding::read::extract_str;
use crate::coding::search::walk::{WalkOptions, collect_workspace_entries};
use crate::coding::search::{format_paginated_lines, parse_limit_and_offset};
use crate::registry::{Tool, ToolError, ToolSchema};

const MAX_BRACE_EXPANSIONS: usize = 128;

/// Finds workspace files matching glob patterns with braces, character classes, exclusions, and pagination.
#[derive(Debug, Clone)]
pub struct GlobTool {
    sandbox: WorkspaceSandbox,
}

impl GlobTool {
    /// Creates a [`GlobTool`] rooted at `sandbox`.
    #[must_use]
    pub fn new(sandbox: WorkspaceSandbox) -> Self {
        Self { sandbox }
    }
}

impl Tool for GlobTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "glob".to_owned(),
            description: "List workspace files matching a glob pattern (supports '**', '*', '?', '{a,b}', and '[a-z]', respecting .gitignore). Prefer this over bash find/ls.".to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["pattern"],
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern (e.g. '**/*.rs', 'crates/{core,tools}/**/*.rs')" },
                    "path": { "type": "string", "description": "Relative directory root (default '.')" },
                    "exclude": {
                        "description": "Glob pattern or array of glob patterns to exclude",
                        "oneOf": [
                            { "type": "string" },
                            { "type": "array", "items": { "type": "string" } }
                        ]
                    },
                    "sort": { "type": "string", "enum": ["path", "mtime"], "description": "Sort by 'path' (default) or 'mtime' (newest first)" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 1000, "description": "Maximum results per page (default 200)" },
                    "offset": { "type": "integer", "minimum": 0, "description": "Zero-based result offset for pagination" },
                    "include_hidden": { "type": "boolean", "description": "Include dotfiles/dotdirs (default false unless pattern explicitly targets hidden paths)" },
                    "include_ignored": { "type": "boolean", "description": "Include .gitignore'd files (default false)" }
                }
            }),
        }
    }

    fn required_capability(&self, arguments: &Value) -> Result<Capability, ToolError> {
        let rel = arguments.get("path").and_then(Value::as_str).unwrap_or(".");
        Ok(Capability::FileSystem {
            path: rel.into(),
            mode: AccessMode::Read,
        })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let pattern = extract_str(arguments, "pattern", "glob")?;
        let rel = arguments.get("path").and_then(Value::as_str).unwrap_or(".");
        let start_dir = self.sandbox.resolve_within(Path::new(rel))?;

        let include_hidden = arguments
            .get("include_hidden")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| pattern_targets_hidden(pattern));
        let include_ignored = arguments
            .get("include_ignored")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let excludes = parse_exclude_patterns(arguments);
        let sort_mtime = arguments.get("sort").and_then(Value::as_str) == Some("mtime");
        let (limit, offset) = parse_limit_and_offset(arguments);

        let mut entries = collect_workspace_entries(
            &start_dir,
            self.sandbox.root(),
            WalkOptions {
                include_hidden,
                include_ignored,
            },
        )?;

        entries.retain(|entry| {
            wildcard_match(pattern, &entry.rel_path)
                && !excludes
                    .iter()
                    .any(|ex| wildcard_match(ex, &entry.rel_path))
        });

        if sort_mtime {
            entries.sort_by_key(|entry| (Reverse(entry.mtime), entry.rel_path.clone()));
        }

        let paths: Vec<String> = entries.into_iter().map(|e| e.rel_path).collect();
        Ok(format_paginated_lines(paths, offset, limit, "matches"))
    }
}

fn parse_exclude_patterns(arguments: &Value) -> Vec<String> {
    let Some(val) = arguments.get("exclude") else {
        return Vec::new();
    };
    if let Some(single) = val.as_str() {
        return vec![single.to_owned()];
    }
    val.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn pattern_targets_hidden(pattern: &str) -> bool {
    pattern
        .split('/')
        .any(|seg| seg.starts_with('.') && seg != "." && seg != "..")
}

/// Expands brace expressions like `*.{rs,toml}` into concrete glob patterns.
#[must_use]
pub fn expand_braces(pattern: &str) -> Vec<String> {
    let mut queue = vec![pattern.to_owned()];
    let mut expanded = Vec::new();

    while let Some(cur) = queue.pop() {
        if let Some((start, end, options)) = find_innermost_brace(&cur) {
            let prefix = &cur[..start];
            let suffix = &cur[end + 1..];
            for opt in options {
                if expanded.len() + queue.len() >= MAX_BRACE_EXPANSIONS {
                    expanded.push(cur);
                    return expanded;
                }
                queue.push(format!("{prefix}{opt}{suffix}"));
            }
        } else {
            expanded.push(cur);
        }
    }
    expanded.reverse();
    expanded
}

fn find_innermost_brace(pattern: &str) -> Option<(usize, usize, Vec<&str>)> {
    let bytes = pattern.as_bytes();
    let mut last_open = None;
    for (idx, &b) in bytes.iter().enumerate() {
        match b {
            b'{' => last_open = Some(idx),
            b'}' => {
                if let Some(open) = last_open {
                    let inner = &pattern[open + 1..idx];
                    let options: Vec<&str> = inner.split(',').collect();
                    return Some((open, idx, options));
                }
            }
            _ => {}
        }
    }
    None
}

/// Matches `rel_path` against a glob pattern supporting `**`, `*`, `?`, `{a,b}`, and `[a-z]`.
#[must_use]
pub fn wildcard_match(pattern: &str, rel_path: &str) -> bool {
    expand_braces(pattern)
        .iter()
        .any(|expanded| single_wildcard_match(expanded, rel_path))
}

fn single_wildcard_match(pattern: &str, rel_path: &str) -> bool {
    let pat = pattern.trim().trim_start_matches("./");
    if pat.is_empty() || pat == "*" || pat == "**" || pat == "**/*" {
        return true;
    }
    let file_name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    if pat.contains('*') || pat.contains('?') || pat.contains('[') {
        if let Some(sub_pat) = pat.strip_prefix("**/")
            && (glob_match_path(sub_pat, rel_path) || glob_match_segment(sub_pat, file_name))
        {
            return true;
        }
        if pat.contains('/') {
            glob_match_path(pat, rel_path)
        } else {
            glob_match_segment(pat, file_name) || glob_match_path(pat, rel_path)
        }
    } else {
        rel_path.contains(pat) || file_name == pat
    }
}

pub(crate) fn glob_match_path(pattern: &str, path: &str) -> bool {
    let pat_segs: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let path_segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match_path_segments(&pat_segs, &path_segs)
}

fn match_path_segments(pat: &[&str], path: &[&str]) -> bool {
    if pat.is_empty() {
        return path.is_empty();
    }
    if pat[0] == "**" {
        return match_path_segments(&pat[1..], path)
            || (!path.is_empty() && match_path_segments(pat, &path[1..]));
    }
    if path.is_empty() {
        return false;
    }
    glob_match_segment(pat[0], path[0]) && match_path_segments(&pat[1..], &path[1..])
}

pub(crate) fn glob_match_segment(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let mut memo: Vec<Vec<Option<bool>>> = vec![vec![None; t.len() + 1]; p.len() + 1];
    match_seg_rec(&p, &t, 0, 0, &mut memo)
}

fn match_seg_rec(
    p: &[char],
    t: &[char],
    pi: usize,
    ti: usize,
    memo: &mut [Vec<Option<bool>>],
) -> bool {
    if let Some(cached) = memo[pi][ti] {
        return cached;
    }
    let res = if pi == p.len() {
        ti == t.len()
    } else if p[pi] == '*' {
        match_seg_rec(p, t, pi + 1, ti, memo)
            || (ti < t.len() && match_seg_rec(p, t, pi, ti + 1, memo))
    } else if ti < t.len() && p[pi] == '?' {
        match_seg_rec(p, t, pi + 1, ti + 1, memo)
    } else if p[pi] == '['
        && let Some((matched, next_pi)) = match_char_class(p, pi, t.get(ti).copied())
    {
        matched && match_seg_rec(p, t, next_pi, ti + 1, memo)
    } else {
        ti < t.len() && p[pi] == t[ti] && match_seg_rec(p, t, pi + 1, ti + 1, memo)
    };
    memo[pi][ti] = Some(res);
    res
}

fn match_char_class(p: &[char], open_idx: usize, ch: Option<char>) -> Option<(bool, usize)> {
    let ch = ch?;
    let mut idx = open_idx + 1;
    if idx >= p.len() {
        return None;
    }
    let negated = p[idx] == '!' || p[idx] == '^';
    if negated {
        idx += 1;
    }
    let mut found = false;
    let mut first = true;
    while idx < p.len() {
        if p[idx] == ']' && !first {
            return Some((if negated { !found } else { found }, idx + 1));
        }
        first = false;
        if idx + 2 < p.len() && p[idx + 1] == '-' && p[idx + 2] != ']' {
            let start = p[idx];
            let end = p[idx + 2];
            if start <= ch && ch <= end {
                found = true;
            }
            idx += 3;
        } else {
            if p[idx] == ch {
                found = true;
            }
            idx += 1;
        }
    }
    None
}
