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
            description: "Execute a command inside the Linux Landlock sandbox (accepts either 'command' shell string or 'program' + 'args')".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command or pipeline executed via sh -c"
                    },
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
        let classification =
            ShellClassifier::classify_with_sandbox(&program, &args, Some(&self.sandbox));
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
    if let Some(command) = arguments.get("command").and_then(Value::as_str)
        && !command.trim().is_empty()
    {
        return Ok(("sh".to_owned(), vec!["-c".to_owned(), command.to_owned()]));
    }
    let program = arguments
        .get("program")
        .and_then(Value::as_str)
        .filter(|p| !p.trim().is_empty())
        .ok_or_else(|| ToolError::InvalidArguments {
            tool: "shell".to_owned(),
            reason: "expected non-empty string 'command' or 'program'".to_owned(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_command_string_and_program_args_equally() -> Result<(), Box<dyn std::error::Error>> {
        let sandbox = WorkspaceSandbox::new(std::env::temp_dir());
        let tool = ShellTool::new(sandbox);

        let cap_cmd = tool.required_capability(&json!({"command": "git status | head -n 5"}))?;
        assert_eq!(
            cap_cmd,
            Capability::Shell {
                program: "sh".to_owned(),
                args: vec!["-c".to_owned(), "git status | head -n 5".to_owned()],
            }
        );

        let cap_prog = tool.required_capability(&json!({"program": "rg", "args": ["--files"]}))?;
        assert_eq!(
            cap_prog,
            Capability::Shell {
                program: "rg".to_owned(),
                args: vec!["--files".to_owned()],
            }
        );

        assert!(tool.required_capability(&json!({})).is_err());
        Ok(())
    }
}
