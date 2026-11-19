// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-19
// Purpose: Advanced GrepTool supporting NFA regex, case-insensitivity, file filters, context lines, and output modes

use std::collections::BTreeSet;
use std::path::Path;

use rein_policy::{AccessMode, Capability, WorkspaceSandbox};
use serde_json::{Value, json};

use crate::coding::search::glob::{pattern_targets_hidden, wildcard_match};
use crate::coding::search::regex::Regex;
use crate::coding::search::walk::{WalkOptions, collect_workspace_entries};
use crate::coding::search::{format_paginated_lines, parse_limit_and_offset};
use crate::registry::{Tool, ToolError, ToolSchema};

const MAX_CONTEXT_LINES: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputMode {
    Content,
    FilesWithMatches,
    Count,
}

enum Matcher {
    Literal {
        query: String,
        lower_query: String,
        case_insensitive: bool,
    },
    Regex(Regex),
}

impl Matcher {
    fn is_match(&self, line: &str) -> bool {
        match self {
            Self::Literal {
                query,
                lower_query,
                case_insensitive,
            } => {
                if *case_insensitive {
                    line.to_ascii_lowercase().contains(lower_query)
                } else {
                    line.contains(query)
                }
            }
            Self::Regex(re) => re.is_match(line),
        }
    }
}

/// Searches UTF-8 workspace files for literal or regex matches with file filters, context lines, and output modes.
#[derive(Debug, Clone)]
pub struct GrepTool {
    sandbox: WorkspaceSandbox,
}

impl GrepTool {
    /// Creates a [`GrepTool`] rooted at `sandbox`.
    #[must_use]
    pub fn new(sandbox: WorkspaceSandbox) -> Self {
        Self { sandbox }
    }
}

impl Tool for GrepTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "grep".to_owned(),
            description: "Search UTF-8 workspace files with literal or regex matching, glob/type filters, context lines, and output modes, respecting .gitignore. Prefer this over bash grep/rg.".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Literal substring or regex pattern to search for" },
                    "pattern": { "type": "string", "description": "Alias for 'query'" },
                    "path": { "type": "string", "description": "Relative file or directory root (default '.')" },
                    "regex": { "type": "boolean", "description": "Treat query/pattern as a regular expression (default false)" },
                    "case_insensitive": { "type": "boolean", "description": "Case-insensitive matching (default false)" },
                    "glob": { "type": "string", "description": "Filter files by glob pattern (e.g. '*.rs', 'crates/{core,tools}/**/*.rs')" },
                    "include": { "type": "string", "description": "Alias for 'glob'" },
                    "type": { "type": "string", "description": "Language shorthand filter (e.g. 'rust', 'ts', 'py', 'toml', 'md', 'json', 'sh')" },
                    "output_mode": {
                        "type": "string",
                        "enum": ["content", "files_with_matches", "count"],
                        "description": "Output mode: 'content' (default, file:line:text), 'files_with_matches', or 'count'"
                    },
                    "before_context": { "type": "integer", "minimum": 0, "maximum": 20, "description": "Lines to show before each match" },
                    "after_context": { "type": "integer", "minimum": 0, "maximum": 20, "description": "Lines to show after each match" },
                    "context": { "type": "integer", "minimum": 0, "maximum": 20, "description": "Lines to show before and after each match" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 1000, "description": "Maximum matches/entries per page (default 200)" },
                    "offset": { "type": "integer", "minimum": 0, "description": "Zero-based result offset for pagination" },
                    "include_hidden": { "type": "boolean", "description": "Include dotfiles/dotdirs (default false)" },
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
        let query = extract_query(arguments)?;
        let rel = arguments.get("path").and_then(Value::as_str).unwrap_or(".");
        let start_path = self.sandbox.resolve_within(Path::new(rel))?;

        let case_insensitive = arguments
            .get("case_insensitive")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let use_regex = arguments
            .get("regex")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let matcher = build_matcher(query, use_regex, case_insensitive)?;

        let file_glob = parse_file_filter(arguments);
        let include_hidden = arguments
            .get("include_hidden")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| {
                file_glob.as_deref().is_some_and(pattern_targets_hidden)
                    || pattern_targets_hidden(rel)
            });
        let include_ignored = arguments
            .get("include_ignored")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let output_mode = parse_output_mode(arguments)?;
        let (before_ctx, after_ctx) = parse_context_bounds(arguments);
        let (limit, offset) = parse_limit_and_offset(arguments);

        let files = collect_target_files(
            self.sandbox.root(),
            &start_path,
            file_glob.as_deref(),
            WalkOptions {
                include_hidden,
                include_ignored,
            },
        )?;

        match output_mode {
            OutputMode::FilesWithMatches => {
                let items = collect_matching_files(self.sandbox.root(), &files, &matcher);
                Ok(format_paginated_lines(items, offset, limit, "files"))
            }
            OutputMode::Count => {
                let items = collect_match_counts(self.sandbox.root(), &files, &matcher);
                Ok(format_paginated_lines(items, offset, limit, "files"))
            }
            OutputMode::Content => Ok(collect_content_matches(
                self.sandbox.root(),
                &files,
                &matcher,
                before_ctx,
                after_ctx,
                offset,
                limit,
            )),
        }
    }
}

fn extract_query(arguments: &Value) -> Result<&str, ToolError> {
    arguments
        .get("query")
        .or_else(|| arguments.get("pattern"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ToolError::InvalidArguments {
            tool: "grep".to_owned(),
            reason: "missing non-empty string field 'query' (or 'pattern')".to_owned(),
        })
}

fn build_matcher(
    query: &str,
    use_regex: bool,
    case_insensitive: bool,
) -> Result<Matcher, ToolError> {
    if use_regex {
        let compiled = Regex::compile(query, case_insensitive).map_err(|reason| {
            ToolError::InvalidArguments {
                tool: "grep".to_owned(),
                reason: format!("invalid regex '{query}': {reason}"),
            }
        })?;
        Ok(Matcher::Regex(compiled))
    } else {
        Ok(Matcher::Literal {
            query: query.to_owned(),
            lower_query: query.to_ascii_lowercase(),
            case_insensitive,
        })
    }
}

fn parse_file_filter(arguments: &Value) -> Option<String> {
    if let Some(glob) = arguments
        .get("glob")
        .or_else(|| arguments.get("include"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Some(glob.to_owned());
    }
    let file_type = arguments
        .get("type")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_ascii_lowercase())?;
    type_to_glob(&file_type).map(str::to_owned)
}

fn type_to_glob(file_type: &str) -> Option<&'static str> {
    match file_type {
        "rust" | "rs" => Some("*.rs"),
        "ts" | "typescript" => Some("*.{ts,tsx,mts,cts}"),
        "js" | "javascript" => Some("*.{js,jsx,mjs,cjs}"),
        "py" | "python" => Some("*.py"),
        "go" => Some("*.go"),
        "c" => Some("*.{c,h}"),
        "cpp" | "c++" => Some("*.{cpp,cc,cxx,hpp,hh,h}"),
        "java" => Some("*.java"),
        "toml" => Some("*.toml"),
        "json" => Some("*.json"),
        "yaml" | "yml" => Some("*.{yml,yaml}"),
        "md" | "markdown" => Some("*.md"),
        "sh" | "bash" | "zsh" => Some("*.{sh,bash,zsh}"),
        _ => None,
    }
}

fn parse_output_mode(arguments: &Value) -> Result<OutputMode, ToolError> {
    match arguments.get("output_mode").and_then(Value::as_str) {
        None | Some("content") => Ok(OutputMode::Content),
        Some("files_with_matches" | "files") => Ok(OutputMode::FilesWithMatches),
        Some("count") => Ok(OutputMode::Count),
        Some(other) => Err(ToolError::InvalidArguments {
            tool: "grep".to_owned(),
            reason: format!(
                "unknown output_mode '{other}' (expected content, files_with_matches, or count)"
            ),
        }),
    }
}

fn parse_context_bounds(arguments: &Value) -> (usize, usize) {
    let base = arguments
        .get("context")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(0)
        .min(MAX_CONTEXT_LINES);
    let before = arguments
        .get("before_context")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .map_or(base, |n| n.min(MAX_CONTEXT_LINES));
    let after = arguments
        .get("after_context")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .map_or(base, |n| n.min(MAX_CONTEXT_LINES));
    (before, after)
}

fn collect_target_files(
    root: &Path,
    start_path: &Path,
    file_glob: Option<&str>,
    opts: WalkOptions,
) -> Result<Vec<String>, ToolError> {
    let mut files = if start_path.is_file() {
        start_path
            .strip_prefix(root)
            .map(|p| vec![p.to_string_lossy().replace('\\', "/")])
            .unwrap_or_default()
    } else {
        collect_workspace_entries(start_path, root, opts)?
            .into_iter()
            .map(|e| e.rel_path)
            .collect()
    };
    if let Some(glob) = file_glob {
        files.retain(|rel| wildcard_match(glob, rel));
    }
    Ok(files)
}

fn collect_matching_files(root: &Path, rel_files: &[String], matcher: &Matcher) -> Vec<String> {
    let mut matched = Vec::new();
    for rel in rel_files {
        let Ok(content) = std::fs::read_to_string(root.join(rel)) else {
            continue;
        };
        if content.lines().any(|line| matcher.is_match(line)) {
            matched.push(rel.clone());
        }
    }
    matched
}

fn collect_match_counts(root: &Path, rel_files: &[String], matcher: &Matcher) -> Vec<String> {
    let mut counts = Vec::new();
    for rel in rel_files {
        let Ok(content) = std::fs::read_to_string(root.join(rel)) else {
            continue;
        };
        let n = content
            .lines()
            .filter(|line| matcher.is_match(line))
            .count();
        if n > 0 {
            counts.push(format!("{rel}: {n}"));
        }
    }
    counts
}

struct ContentHit {
    rel_idx: usize,
    line_idx: usize,
}

fn collect_content_matches(
    root: &Path,
    rel_files: &[String],
    matcher: &Matcher,
    before_ctx: usize,
    after_ctx: usize,
    offset: usize,
    limit: usize,
) -> String {
    let mut file_lines: Vec<Option<Vec<String>>> = Vec::with_capacity(rel_files.len());
    let mut hits: Vec<ContentHit> = Vec::new();

    for (rel_idx, rel) in rel_files.iter().enumerate() {
        let Ok(content) = std::fs::read_to_string(root.join(rel)) else {
            file_lines.push(None);
            continue;
        };
        let lines: Vec<String> = content.lines().map(str::to_owned).collect();
        for (line_idx, line) in lines.iter().enumerate() {
            if matcher.is_match(line) {
                hits.push(ContentHit { rel_idx, line_idx });
            }
        }
        file_lines.push(Some(lines));
    }

    let total = hits.len();
    if total == 0 || offset >= total {
        return String::new();
    }
    let end = (offset.saturating_add(limit)).min(total);
    let slice = &hits[offset..end];

    let rendered = if before_ctx == 0 && after_ctx == 0 {
        slice
            .iter()
            .filter_map(|hit| {
                let lines = file_lines[hit.rel_idx].as_ref()?;
                let rel = &rel_files[hit.rel_idx];
                let line = lines.get(hit.line_idx)?;
                Some(format!("{rel}:{}:{line}", hit.line_idx + 1))
            })
            .collect::<Vec<_>>()
    } else {
        render_with_context(rel_files, &file_lines, slice, before_ctx, after_ctx)
    };

    let mut out = rendered.join("\n");
    if end < total || offset > 0 {
        let shown_start = offset + 1;
        if end < total {
            out.push_str(&format!(
                "\n\n[showing {shown_start}-{end} of {total} matches; pass offset={end} to see more]"
            ));
        } else {
            out.push_str(&format!(
                "\n\n[showing {shown_start}-{end} of {total} matches]"
            ));
        }
    }
    out
}

fn render_with_context(
    rel_files: &[String],
    file_lines: &[Option<Vec<String>>],
    slice: &[ContentHit],
    before_ctx: usize,
    after_ctx: usize,
) -> Vec<String> {
    let mut by_file: std::collections::BTreeMap<usize, BTreeSet<usize>> =
        std::collections::BTreeMap::new();
    for hit in slice {
        by_file.entry(hit.rel_idx).or_default().insert(hit.line_idx);
    }

    let mut out = Vec::new();
    for (rel_idx, match_set) in by_file {
        let Some(lines) = file_lines[rel_idx].as_ref() else {
            continue;
        };
        let rel = &rel_files[rel_idx];
        let mut emit_set = BTreeSet::new();
        for &m in &match_set {
            let lo = m.saturating_sub(before_ctx);
            let hi = (m + after_ctx).min(lines.len().saturating_sub(1));
            for i in lo..=hi {
                emit_set.insert(i);
            }
        }
        let mut prev: Option<usize> = None;
        for line_idx in emit_set {
            if let Some(p) = prev
                && line_idx > p + 1
            {
                out.push("--".to_owned());
            }
            let sep = if match_set.contains(&line_idx) {
                ':'
            } else {
                '-'
            };
            out.push(format!(
                "{rel}{sep}{}{sep}{}",
                line_idx + 1,
                lines[line_idx]
            ));
            prev = Some(line_idx);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding::search::{GlobTool, MAX_SEARCH_RESULTS};

    #[test]
    fn glob_and_grep_skip_ignored_dirs() -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("rein_search_tool_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src"))?;
        std::fs::create_dir_all(dir.join("target/debug"))?;
        std::fs::write(
            dir.join("src/main.rs"),
            "fn main() {\n    // TODO: wire loop\n}\n",
        )?;
        std::fs::write(dir.join("Cargo.toml"), "[package]\n")?;
        std::fs::write(dir.join("target/debug/junk.rs"), "// TODO: ignored\n")?;
        let sandbox = WorkspaceSandbox::new(&dir);

        let globbed = GlobTool::new(sandbox.clone()).execute(&json!({"pattern": "*.rs"}))?;
        assert_eq!(globbed, "src/main.rs");
        let named = GlobTool::new(sandbox.clone()).execute(&json!({"pattern": "Cargo.toml"}))?;
        assert_eq!(named, "Cargo.toml");

        let hits = GrepTool::new(sandbox.clone()).execute(&json!({"query": "TODO"}))?;
        assert_eq!(hits, "src/main.rs:2:    // TODO: wire loop");
        let single = GrepTool::new(sandbox.clone())
            .execute(&json!({"query": "fn", "path": "src/main.rs"}))?;
        assert!(single.starts_with("src/main.rs:1:"));
        assert!(
            GrepTool::new(sandbox)
                .execute(&json!({"query": "x", "path": "../.."}))
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn glob_and_grep_handle_empty_results_limits_and_missing_args()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("rein_search_tool_edges_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub"))?;

        let mut lines = String::new();
        for i in 0..(MAX_SEARCH_RESULTS + 20) {
            lines.push_str(&format!("hit line {i}\n"));
        }
        std::fs::write(dir.join("sub/many.txt"), lines)?;
        let sandbox = WorkspaceSandbox::new(&dir);

        let none = GlobTool::new(sandbox.clone()).execute(&json!({"pattern": "*.py"}))?;
        assert!(none.is_empty());

        let sub_glob =
            GlobTool::new(sandbox.clone()).execute(&json!({"pattern": "*.txt", "path": "sub"}))?;
        assert_eq!(sub_glob, "sub/many.txt");

        let capped = GrepTool::new(sandbox.clone()).execute(&json!({"query": "hit"}))?;
        assert!(capped.contains("[showing 1-200 of 220 matches; pass offset=200 to see more]"));

        let page2 =
            GrepTool::new(sandbox.clone()).execute(&json!({"query": "hit", "offset": 200}))?;
        assert!(page2.contains("hit line 200"));
        assert!(page2.contains("[showing 201-220 of 220 matches]"));

        let zero_hits =
            GrepTool::new(sandbox.clone()).execute(&json!({"query": "NO_SUCH_TEXT"}))?;
        assert!(zero_hits.is_empty());

        assert!(GlobTool::new(sandbox.clone()).execute(&json!({})).is_err());
        assert!(GrepTool::new(sandbox).execute(&json!({})).is_err());

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn glob_supports_braces_char_classes_gitignore_exclude_and_mtime_sort()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join(format!("rein_search_adv_glob_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("crates/core/src"))?;
        std::fs::create_dir_all(dir.join("crates/tools/src"))?;
        std::fs::create_dir_all(dir.join("dist"))?;
        std::fs::write(dir.join(".gitignore"), "dist/\n*.log\n!keep.log\n")?;
        std::fs::write(dir.join("Cargo.toml"), "[workspace]\n")?;
        std::fs::write(dir.join("crates/core/Cargo.toml"), "[package]\n")?;
        std::fs::write(dir.join("crates/core/src/lib.rs"), "pub fn a() {}\n")?;
        std::fs::write(dir.join("crates/tools/src/mod1.rs"), "pub fn b() {}\n")?;
        std::fs::write(dir.join("crates/tools/src/mod2.rs"), "pub fn c() {}\n")?;
        std::fs::write(dir.join("dist/bundle.js"), "ignored\n")?;
        std::fs::write(dir.join("debug.log"), "ignored\n")?;
        std::fs::write(dir.join("keep.log"), "kept\n")?;

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&dir, dir.join("crates/core/cycle_link"))?;
            std::os::unix::fs::symlink("/etc/hosts", dir.join("crates/core/hosts_link.rs"))?;
        }

        let sandbox = WorkspaceSandbox::new(&dir);
        let glob = GlobTool::new(sandbox);

        let braced = glob.execute(&json!({"pattern": "*.{rs,toml}"}))?;
        assert!(braced.contains("Cargo.toml"));
        assert!(braced.contains("crates/core/src/lib.rs"));
        assert!(!braced.contains("bundle.js"));

        let excluded = glob.execute(&json!({
            "pattern": "**/*.rs",
            "exclude": ["crates/tools/**"]
        }))?;
        assert_eq!(excluded, "crates/core/src/lib.rs");

        let char_class = glob.execute(&json!({"pattern": "mod[12].rs"}))?;
        assert_eq!(
            char_class.lines().collect::<Vec<_>>(),
            vec!["crates/tools/src/mod1.rs", "crates/tools/src/mod2.rs"]
        );

        let logs = glob.execute(&json!({"pattern": "*.log"}))?;
        assert_eq!(logs, "keep.log");

        let with_ignored = glob.execute(&json!({
            "pattern": "*.log",
            "include_ignored": true
        }))?;
        assert_eq!(
            with_ignored.lines().collect::<Vec<_>>(),
            vec!["debug.log", "keep.log"]
        );

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn grep_supports_regex_case_insensitive_filters_context_and_output_modes()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join(format!("rein_search_adv_grep_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src"))?;
        std::fs::write(
            dir.join("src/lib.rs"),
            "/// Header\npub fn alpha(x: u32) -> u32 {\n    x + 1\n}\n\nfn helper() {}\npub struct Beta;\n",
        )?;
        std::fs::write(dir.join("README.md"), "# ALPHA\npub fn ignored_in_md()\n")?;

        let sandbox = WorkspaceSandbox::new(&dir);
        let grep = GrepTool::new(sandbox);

        // Regex + type filter + pattern alias
        let regex_hits = grep.execute(&json!({
            "pattern": r"^pub\s+(fn|struct)\s+\w+",
            "regex": true,
            "type": "rust"
        }))?;
        assert_eq!(
            regex_hits.lines().collect::<Vec<_>>(),
            vec![
                "src/lib.rs:2:pub fn alpha(x: u32) -> u32 {",
                "src/lib.rs:7:pub struct Beta;",
            ]
        );

        // Case-insensitive literal across files_with_matches
        let files_mode = grep.execute(&json!({
            "query": "alpha",
            "case_insensitive": true,
            "output_mode": "files_with_matches"
        }))?;
        assert_eq!(
            files_mode.lines().collect::<Vec<_>>(),
            vec!["README.md", "src/lib.rs"]
        );

        // Count mode with glob filter
        let count_mode = grep.execute(&json!({
            "query": "fn",
            "glob": "*.rs",
            "output_mode": "count"
        }))?;
        assert_eq!(count_mode, "src/lib.rs: 2");

        // Context lines with hunk separator
        let ctx = grep.execute(&json!({
            "query": r"pub\s+(fn|struct)",
            "regex": true,
            "type": "rust",
            "before_context": 1,
            "after_context": 1
        }))?;
        assert_eq!(
            ctx.lines().collect::<Vec<_>>(),
            vec![
                "src/lib.rs-1-/// Header",
                "src/lib.rs:2:pub fn alpha(x: u32) -> u32 {",
                "src/lib.rs-3-    x + 1",
                "--",
                "src/lib.rs-6-fn helper() {}",
                "src/lib.rs:7:pub struct Beta;",
            ]
        );

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
