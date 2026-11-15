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
            description: "Read a UTF-8 file inside the workspace sandbox with optional 1-based line bounds. Prefer this over bash cat/head/tail for inspecting files.".to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": { "type": "string" },
                    "start_line": { "type": "integer", "minimum": 1 },
                    "end_line": { "type": "integer", "minimum": 1 }
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
        .unwrap_or(start + DEFAULT_MAX_READ_LINES - 1);

    raw.lines()
        .enumerate()
        .skip(start - 1)
        .take(end.saturating_sub(start) + 1)
        .map(|(idx, line)| format!("{:>4}: {line}", idx + 1))
        .collect::<Vec<_>>()
        .join("\n")
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
