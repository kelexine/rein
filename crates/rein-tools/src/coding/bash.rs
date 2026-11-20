// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Classifier-driven BashTool executing policy-gated commands with streamed output files

use std::path::PathBuf;

use rein_policy::{Capability, SandboxSpec, WorkspaceSandbox, run_sandboxed_command};
use serde_json::{Value, json};

use crate::registry::{Tool, ToolError, ToolSchema};
use crate::tasks::{BashSpawnOutcome, TaskRegistry};

/// Executes policy-gated bash commands with full inherited shell configs and streamed output files.
#[derive(Debug, Clone)]
pub struct BashTool {
    sandbox: WorkspaceSandbox,
    output_dir: Option<PathBuf>,
    task_registry: Option<TaskRegistry>,
}

impl BashTool {
    /// Creates a [`BashTool`] rooted at `sandbox`, streaming full logs to `<sandbox>/.rein/bash-outputs`.
    #[must_use]
    pub fn new(sandbox: WorkspaceSandbox) -> Self {
        let output_dir = Some(sandbox.root().join(".rein").join("bash-outputs"));
        Self {
            sandbox,
            output_dir,
            task_registry: None,
        }
    }

    /// Overrides the directory where full streamed `stdout`/`stderr` log files are written.
    #[must_use]
    pub fn with_output_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.output_dir = Some(dir.into());
        self
    }

    /// Attaches a [`TaskRegistry`] enabling `wait_ms_before_async` auto-backgrounding and `is_daemon` execution.
    #[must_use]
    pub fn with_task_registry(mut self, registry: TaskRegistry) -> Self {
        self.task_registry = Some(registry);
        self
    }
}

impl Tool for BashTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "bash".to_owned(),
            description: "Execute a bash command in the workspace (accepts either 'command' via bash -c or 'program' + 'args', inheriting user shell configs with no execution timeout). Commands exceeding wait_ms_before_async (500-10000ms) or with is_daemon=true auto-background into TaskRegistry and wake the session on completion.".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Bash command or pipeline executed via bash -c"
                    },
                    "program": { "type": "string" },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "wait_ms_before_async": {
                        "type": "integer",
                        "description": "Milliseconds (500-10000, default 10000) to wait synchronously before promoting to a background task."
                    },
                    "is_daemon": {
                        "type": "boolean",
                        "description": "Set true for long-running servers/watchers that should immediately start in the background."
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
        let label = format_command_label(arguments, &program, &args);
        let spec = SandboxSpec {
            workspace_root: self.sandbox.root().to_path_buf(),
            program,
            args,
            output_dir: self.output_dir.clone(),
        };
        if let Some(registry) = &self.task_registry {
            let wait_ms = arguments
                .get("wait_ms_before_async")
                .and_then(Value::as_u64);
            let is_daemon = arguments
                .get("is_daemon")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            return match registry.spawn_bash_task(spec, label, wait_ms, is_daemon)? {
                BashSpawnOutcome::Completed {
                    exit_code,
                    stdout,
                    stderr,
                    truncated,
                    log_path,
                } => Ok(serde_json::to_string(&json!({
                    "status_code": exit_code,
                    "stdout": stdout,
                    "stderr": stderr,
                    "truncated": truncated,
                    "output_file": log_path,
                }))?),
                BashSpawnOutcome::Backgrounded {
                    task_id,
                    is_daemon,
                    partial_output,
                    log_path,
                } => Ok(serde_json::to_string(&json!({
                    "status": "backgrounded",
                    "task_id": task_id,
                    "is_daemon": is_daemon,
                    "partial_output": partial_output,
                    "output_file": log_path,
                }))?),
            };
        }
        let output = run_sandboxed_command(&spec)?;
        Ok(serde_json::to_string(&output)?)
    }
}

fn format_command_label(arguments: &Value, program: &str, args: &[String]) -> String {
    if let Some(cmd) = arguments
        .get("command")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return cmd.to_owned();
    }
    if args.is_empty() {
        program.to_owned()
    } else {
        format!("{program} {}", args.join(" "))
    }
}

fn parse_command(arguments: &Value) -> Result<(String, Vec<String>), ToolError> {
    if let Some(command) = arguments.get("command").and_then(Value::as_str)
        && !command.trim().is_empty()
    {
        return Ok(("bash".to_owned(), vec!["-c".to_owned(), command.to_owned()]));
    }
    let program = arguments
        .get("program")
        .and_then(Value::as_str)
        .filter(|p| !p.trim().is_empty())
        .ok_or_else(|| ToolError::InvalidArguments {
            tool: "bash".to_owned(),
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
    fn parses_command_string_as_bash_c_and_program_args_equally()
    -> Result<(), Box<dyn std::error::Error>> {
        let sandbox = WorkspaceSandbox::new(std::env::temp_dir());
        let tool = BashTool::new(sandbox);
        assert_eq!(tool.schema().name, "bash");

        let cap_cmd = tool.required_capability(&json!({"command": "git status | head -n 5"}))?;
        assert_eq!(
            cap_cmd,
            Capability::Shell {
                program: "bash".to_owned(),
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

    #[test]
    fn rejects_whitespace_only_command_and_defaults_missing_args()
    -> Result<(), Box<dyn std::error::Error>> {
        let sandbox = WorkspaceSandbox::new(std::env::temp_dir());
        let tool = BashTool::new(sandbox);

        assert!(
            tool.required_capability(&json!({"command": "   \t\n"}))
                .is_err()
        );
        assert!(tool.required_capability(&json!({"program": ""})).is_err());

        let cap = tool.required_capability(&json!({"program": "pwd"}))?;
        assert_eq!(
            cap,
            Capability::Shell {
                program: "pwd".to_owned(),
                args: Vec::new(),
            }
        );

        let mixed =
            tool.required_capability(&json!({"program": "ls", "args": ["-la", 42, "src"]}))?;
        assert_eq!(
            mixed,
            Capability::Shell {
                program: "ls".to_owned(),
                args: vec!["-la".to_owned(), "src".to_owned()],
            }
        );
        Ok(())
    }
}
