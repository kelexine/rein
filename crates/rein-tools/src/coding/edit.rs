// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Exact string-replacement file editing tool inside the workspace sandbox

use std::path::Path;

use rein_policy::{AccessMode, Capability, TargetScope, WorkspaceSandbox, classify_path_token};
use serde_json::{Value, json};

use crate::coding::read::extract_str;
use crate::registry::{Tool, ToolError, ToolSchema};

/// Applies deterministic string replacements inside a workspace file, rejecting protected paths.
#[derive(Debug, Clone)]
pub struct EditTool {
    sandbox: WorkspaceSandbox,
}

impl EditTool {
    /// Creates an [`EditTool`] confined to `sandbox`.
    #[must_use]
    pub fn new(sandbox: WorkspaceSandbox) -> Self {
        Self { sandbox }
    }
}

impl Tool for EditTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "edit".to_owned(),
            description: "Replace an exact string occurrence inside a workspace file".to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["path", "old_text", "new_text"],
                "properties": {
                    "path": { "type": "string" },
                    "old_text": { "type": "string" },
                    "new_text": { "type": "string" }
                }
            }),
        }
    }

    fn required_capability(&self, arguments: &Value) -> Result<Capability, ToolError> {
        let path_str = extract_str(arguments, "path", "edit")?;
        Ok(Capability::FileSystem {
            path: path_str.into(),
            mode: AccessMode::Write,
        })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let path_str = extract_str(arguments, "path", "edit")?;
        if classify_path_token(path_str, Some(&self.sandbox)) == TargetScope::Protected {
            return Err(ToolError::InvalidArguments {
                tool: "edit".to_owned(),
                reason: format!("refusing to edit protected path '{path_str}'"),
            });
        }
        let old_text = extract_str(arguments, "old_text", "edit")?;
        let new_text = extract_str(arguments, "new_text", "edit")?;
        let resolved = self.sandbox.resolve_within(Path::new(path_str))?;
        let original = std::fs::read_to_string(&resolved).map_err(|source| ToolError::Io {
            path: resolved.clone(),
            source,
        })?;
        let updated = replace_unique(&original, old_text, new_text, path_str)?;
        std::fs::write(&resolved, updated).map_err(|source| ToolError::Io {
            path: resolved,
            source,
        })?;
        Ok(format!("edited '{path_str}'"))
    }
}

fn replace_unique(
    original: &str,
    old_text: &str,
    new_text: &str,
    path_str: &str,
) -> Result<String, ToolError> {
    let matches = original.matches(old_text).count();
    if matches == 0 {
        return Err(ToolError::InvalidArguments {
            tool: "edit".to_owned(),
            reason: format!("target snippet not found in '{path_str}'"),
        });
    }
    if matches > 1 {
        return Err(ToolError::InvalidArguments {
            tool: "edit".to_owned(),
            reason: format!(
                "target snippet matched {matches} times in '{path_str}'; provide more surrounding context"
            ),
        });
    }
    Ok(original.replacen(old_text, new_text, 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_unique_match_and_rejects_ambiguous_missing_protected()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("rein_edit_tool_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git"))?;
        std::fs::write(
            dir.join("a.rs"),
            "fn a() {}\nfn b() {}\nlet x = 1;\nlet x = 1;\n",
        )?;
        std::fs::write(dir.join(".git/config"), "[core]\n")?;
        let tool = EditTool::new(WorkspaceSandbox::new(&dir));

        tool.execute(
            &json!({"path": "a.rs", "old_text": "fn a() {}", "new_text": "fn a() -> u8 { 1 }"}),
        )?;
        assert!(std::fs::read_to_string(dir.join("a.rs"))?.contains("fn a() -> u8 { 1 }"));
        assert!(
            tool.execute(&json!({"path": "a.rs", "old_text": "let x = 1;", "new_text": "y"}))
                .is_err()
        );
        assert!(
            tool.execute(&json!({"path": "a.rs", "old_text": "missing", "new_text": "y"}))
                .is_err()
        );
        assert!(
            tool.execute(&json!({"path": ".git/config", "old_text": "[core]", "new_text": "x"}))
                .is_err()
        );
        assert!(
            tool.execute(&json!({"path": "../escape.rs", "old_text": "a", "new_text": "b"}))
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
