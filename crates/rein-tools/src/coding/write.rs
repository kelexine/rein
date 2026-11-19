// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Sandboxed workspace file write tool with protected-path rejection

use std::path::Path;

use rein_policy::{AccessMode, Capability, TargetScope, WorkspaceSandbox, classify_path_token};
use serde_json::{Value, json};

use crate::coding::read::extract_str;
use crate::registry::{Tool, ToolError, ToolSchema};

/// Writes UTF-8 files confined within the [`WorkspaceSandbox`], rejecting protected paths.
#[derive(Debug, Clone)]
pub struct WriteTool {
    sandbox: WorkspaceSandbox,
}

impl WriteTool {
    /// Creates a [`WriteTool`] confined to `sandbox`.
    #[must_use]
    pub fn new(sandbox: WorkspaceSandbox) -> Self {
        Self { sandbox }
    }
}

impl Tool for WriteTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "write".to_owned(),
            description: "Create or overwrite a file inside the workspace sandbox (rejects protected .git/.vscode/.rein paths). Prefer edit for modifying existing files.".to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["path", "content"],
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                }
            }),
        }
    }

    fn required_capability(&self, arguments: &Value) -> Result<Capability, ToolError> {
        let path_str = extract_str(arguments, "path", "write")?;
        Ok(Capability::FileSystem {
            path: path_str.into(),
            mode: AccessMode::Write,
        })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let path_str = extract_str(arguments, "path", "write")?;
        if classify_path_token(path_str, Some(&self.sandbox)) == TargetScope::Protected
            || self.sandbox.is_protected(Path::new(path_str))
        {
            return Err(ToolError::InvalidArguments {
                tool: "write".to_owned(),
                reason: format!("refusing to write protected path '{path_str}'"),
            });
        }
        let content = extract_str(arguments, "content", "write")?;
        let resolved = self.sandbox.resolve_within(Path::new(path_str))?;
        if std::fs::symlink_metadata(&resolved).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(ToolError::InvalidArguments {
                tool: "write".to_owned(),
                reason: format!("refusing to write through symlink '{path_str}'"),
            });
        }
        if let Some(parent) = resolved.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ToolError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        std::fs::write(&resolved, content).map_err(|source| ToolError::Io {
            path: resolved,
            source,
        })?;
        Ok(format!("wrote {} bytes to '{path_str}'", content.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_creates_nested_files_and_rejects_protected_or_escape_paths()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("rein_write_tool_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;

        let tool = WriteTool::new(WorkspaceSandbox::new(&dir));
        let cap = tool.required_capability(&json!({"path": "src/main.rs", "content": ""}))?;
        assert!(matches!(
            cap,
            Capability::FileSystem {
                mode: AccessMode::Write,
                ..
            }
        ));

        let msg =
            tool.execute(&json!({"path": "nested/dir/hello.txt", "content": "hello world"}))?;
        assert!(msg.contains("11 bytes"));
        assert_eq!(
            std::fs::read_to_string(dir.join("nested/dir/hello.txt"))?,
            "hello world"
        );

        tool.execute(&json!({"path": "nested/dir/hello.txt", "content": "updated"}))?;
        assert_eq!(
            std::fs::read_to_string(dir.join("nested/dir/hello.txt"))?,
            "updated"
        );

        for protected in [
            ".git/hooks/pre-commit",
            ".rein/config.json",
            ".vscode/settings.json",
        ] {
            assert!(
                tool.execute(&json!({"path": protected, "content": "bad"}))
                    .is_err(),
                "expected protected path {protected} to be rejected"
            );
        }

        assert!(
            tool.execute(&json!({"path": "../outside.txt", "content": "bad"}))
                .is_err()
        );

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
