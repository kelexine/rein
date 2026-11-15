// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Native Rust workspace glob file finder and line-level grep search tools

use std::path::{Path, PathBuf};

use rein_policy::{AccessMode, Capability, WorkspaceSandbox};
use serde_json::{Value, json};

use crate::coding::read::extract_str;
use crate::registry::{Tool, ToolError, ToolSchema};

/// Maximum number of matches returned by `glob` or `grep` in a single call.
pub const MAX_SEARCH_RESULTS: usize = 200;

/// Directories skipped during recursive workspace traversal.
const IGNORED_DIRS: &[&str] = &[".git", "target", "node_modules", ".rein"];

/// Finds workspace files whose relative path matches a pattern or suffix.
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
            description: "List workspace files matching a filename/extension pattern (e.g. '*.rs', 'Cargo.toml'). Prefer this over bash find/ls.".to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["pattern"],
                "properties": {
                    "pattern": { "type": "string" },
                    "path": { "type": "string" }
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
        let mut files = Vec::new();
        collect_files(&start_dir, self.sandbox.root(), &mut files)?;
        let matched: Vec<String> = files
            .into_iter()
            .filter(|rel_path| wildcard_match(pattern, rel_path))
            .take(MAX_SEARCH_RESULTS)
            .collect();
        Ok(matched.join("\n"))
    }
}

/// Searches workspace files for lines containing a substring query.
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
            description: "Search UTF-8 workspace files for lines containing a literal query string. Prefer this over bash grep/rg for fast workspace text lookup."
                .to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["query"],
                "properties": {
                    "query": { "type": "string" },
                    "path": { "type": "string" }
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
        let query = extract_str(arguments, "query", "grep")?;
        let rel = arguments.get("path").and_then(Value::as_str).unwrap_or(".");
        let start_path = self.sandbox.resolve_within(Path::new(rel))?;
        let mut files = Vec::new();
        if start_path.is_file() {
            if let Ok(stripped) = start_path.strip_prefix(self.sandbox.root()) {
                files.push(stripped.to_string_lossy().into_owned());
            }
        } else {
            collect_files(&start_path, self.sandbox.root(), &mut files)?;
        }
        let hits = search_files_for_query(self.sandbox.root(), &files, query);
        Ok(hits.join("\n"))
    }
}

fn search_files_for_query(root: &Path, rel_files: &[String], query: &str) -> Vec<String> {
    let mut hits = Vec::new();
    for rel in rel_files {
        let full = root.join(rel);
        let Ok(content) = std::fs::read_to_string(&full) else {
            continue;
        };
        for (idx, line) in content.lines().enumerate() {
            if line.contains(query) {
                hits.push(format!("{rel}:{}:{line}", idx + 1));
                if hits.len() >= MAX_SEARCH_RESULTS {
                    return hits;
                }
            }
        }
    }
    hits
}

fn collect_files(dir: &Path, root: &Path, out: &mut Vec<String>) -> Result<(), ToolError> {
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
        if IGNORED_DIRS.contains(&name) {
            continue;
        }
        if path.is_dir() {
            collect_files(&path, root, out)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_string_lossy().into_owned());
        }
    }
    Ok(())
}

fn wildcard_match(pattern: &str, rel_path: &str) -> bool {
    let file_name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    if let Some(ext) = pattern.strip_prefix("*.") {
        file_name.ends_with(&format!(".{ext}"))
    } else {
        rel_path.contains(pattern) || file_name == pattern
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
