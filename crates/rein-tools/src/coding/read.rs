// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Sandboxed workspace file read tool with 1-based line range slicing

use std::path::Path;

use rein_policy::{AccessMode, Capability, WorkspaceSandbox};
use serde_json::{Value, json};

use crate::registry::{Tool, ToolError, ToolSchema};

/// Default maximum lines returned by `read` when no `end_line` is specified.
pub const DEFAULT_MAX_READ_LINES: usize = 500;

/// Reads UTF-8 files confined within the [`WorkspaceSandbox`] with optional 1-based line bounds.
#[derive(Debug, Clone)]
pub struct ReadTool {
    sandbox: WorkspaceSandbox,
}

impl ReadTool {
    /// Creates a [`ReadTool`] confined to `sandbox`.
    #[must_use]
    pub fn new(sandbox: WorkspaceSandbox) -> Self {
        Self { sandbox }
    }
}

impl Tool for ReadTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "read".to_owned(),
            description: "Read a UTF-8 file inside the workspace sandbox with 1-based line pagination (defaults to lines 1-500 when no range is specified). Prefer this over bash cat/head/tail for inspecting files.".to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": { "type": "string" },
                    "start_line": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "1-based start line (default 1)"
                    },
                    "end_line": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "1-based inclusive end line (defaults to start_line + 499)"
                    }
                }
            }),
        }
    }

    fn required_capability(&self, arguments: &Value) -> Result<Capability, ToolError> {
        let path_str = extract_str(arguments, "path", "read")?;
        Ok(Capability::FileSystem {
            path: path_str.into(),
            mode: AccessMode::Read,
        })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let path_str = extract_str(arguments, "path", "read")?;
        let resolved = self.sandbox.resolve_within(Path::new(path_str))?;
        let raw = std::fs::read_to_string(&resolved).map_err(|source| ToolError::Io {
            path: resolved,
            source,
        })?;
        Ok(slice_lines(&raw, arguments))
    }
}

fn slice_lines(raw: &str, arguments: &Value) -> String {
    let total_lines = raw.lines().count();
    let start = arguments
        .get("start_line")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(1)
        .max(1);
    let end = arguments
        .get("end_line")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or_else(|| start.saturating_add(DEFAULT_MAX_READ_LINES - 1));

    if end < start || start > total_lines {
        return String::new();
    }

    let count = end.saturating_sub(start).saturating_add(1);
    let actual_end = end.min(total_lines);
    let mut body = raw
        .lines()
        .enumerate()
        .skip(start - 1)
        .take(count)
        .map(|(idx, line)| format!("{:>4}: {line}", idx + 1))
        .collect::<Vec<_>>()
        .join("\n");

    if actual_end < total_lines {
        let next = actual_end + 1;
        body.push_str(&format!(
            "\n\n[showing lines {start}-{actual_end} of {total_lines}; pass start_line={next} to read more]"
        ));
    }
    body
}

pub(crate) fn extract_str<'a>(
    arguments: &'a Value,
    field: &str,
    tool: &str,
) -> Result<&'a str, ToolError> {
    arguments
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::InvalidArguments {
            tool: tool.to_owned(),
            reason: format!("missing string field '{field}'"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_slices_1_based_lines_and_rejects_escape() -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("rein_read_tool_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("notes.txt"), "alpha\nbeta\ngamma\ndelta\n")?;

        let tool = ReadTool::new(WorkspaceSandbox::new(&dir));
        let cap = tool.required_capability(&json!({"path": "notes.txt"}))?;
        assert!(matches!(
            cap,
            Capability::FileSystem {
                mode: AccessMode::Read,
                ..
            }
        ));

        let full = tool.execute(&json!({"path": "notes.txt"}))?;
        assert_eq!(full, "   1: alpha\n   2: beta\n   3: gamma\n   4: delta");

        let slice = tool.execute(&json!({"path": "notes.txt", "start_line": 2, "end_line": 3}))?;
        assert_eq!(
            slice,
            "   2: beta\n   3: gamma\n\n[showing lines 2-3 of 4; pass start_line=4 to read more]"
        );

        assert!(tool.execute(&json!({"path": "missing.txt"})).is_err());
        assert!(tool.execute(&json!({"path": "../outside.txt"})).is_err());
        assert!(tool.execute(&json!({})).is_err());

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn read_paginates_large_files_at_500_lines_by_default_and_allows_explicit_ranges()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("rein_read_pagination_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;

        let content: String = (1..=650)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.join("big.txt"), content)?;

        let tool = ReadTool::new(WorkspaceSandbox::new(&dir));

        let default_page = tool.execute(&json!({"path": "big.txt"}))?;
        assert!(default_page.contains("   1: line-1"));
        assert!(default_page.contains(" 500: line-500"));
        assert!(!default_page.contains(" 501: line-501"));
        assert!(default_page.contains("showing lines 1-500 of 650; pass start_line=501"));

        let mid_slice =
            tool.execute(&json!({"path": "big.txt", "start_line": 50, "end_line": 80}))?;
        assert!(mid_slice.contains("  50: line-50"));
        assert!(mid_slice.contains("  80: line-80"));
        assert!(!mid_slice.contains("  81: line-81"));
        assert!(mid_slice.contains("showing lines 50-80 of 650; pass start_line=81"));

        let tail = tool.execute(&json!({"path": "big.txt", "start_line": 501, "end_line": 650}))?;
        assert!(tail.contains(" 501: line-501"));
        assert!(tail.contains(" 650: line-650"));
        assert!(!tail.contains("showing lines"));

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
