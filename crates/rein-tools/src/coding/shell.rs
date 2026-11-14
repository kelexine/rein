// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Classifier-driven ShellTool executing commands inside a Landlock re-exec sandbox

use std::path::PathBuf;

use rein_policy::{
    Capability, DEFAULT_READ_ONLY_DIRS, SandboxSpec, ShellClassifier, WorkspaceSandbox,
    run_sandboxed_command,
};
use serde_json::{Value, json};

use crate::registry::{Tool, ToolError, ToolSchema};

/// Executes commands via `rein --sandbox-child` under Landlock, driven by [`ShellClassifier`].
#[derive(Debug, Clone)]
pub struct ShellTool {
    sandbox: WorkspaceSandbox,
}

impl ShellTool {
    /// Creates a [`ShellTool`] rooted at `sandbox`.
    #[must_use]
    pub fn new(sandbox: WorkspaceSandbox) -> Self {
        Self { sandbox }
    }
}

impl Tool for ShellTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "shell".to_owned(),
            description: "Execute a command inside the Linux Landlock sandbox (read-only dictionary commands run directly)".to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["program"],
                "properties": {
                    "program": { "type": "string" },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                }
            }),
        }
    }

    fn required_capability(&self, arguments: &Value) -> Result<Capability, ToolError> {
        let (program, args) = parse_command(arguments)?;
        Ok(Capability::Shell { program, args })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let (program, args) = parse_command(arguments)?;
        let classification = ShellClassifier::classify(&program, &args);
        let spec = SandboxSpec {
            workspace_root: self.sandbox.root().to_path_buf(),
            read_only_workspace: classification.is_read_only(),
            read_only_paths: DEFAULT_READ_ONLY_DIRS.iter().map(PathBuf::from).collect(),
            program,
            args,
        };
        let output = run_sandboxed_command(&spec)?;
        Ok(serde_json::to_string(&output)?)
    }
}

fn parse_command(arguments: &Value) -> Result<(String, Vec<String>), ToolError> {
    let program = arguments
        .get("program")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::InvalidArguments {
            tool: "shell".to_owned(),
            reason: "missing string field 'program'".to_owned(),
        })?
        .to_owned();
    let args = arguments
        .get("args")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Ok((program, args))
}
