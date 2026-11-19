// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-19
// Purpose: `schedule` tool creating one-shot timers and recurring 5-field cron schedules in TaskRegistry

use rein_policy::{AccessMode, Capability};
use serde_json::{Value, json};

use crate::registry::{Tool, ToolError, ToolSchema};
use crate::tasks::{TaskRegistry, TimerCondition};

/// Schedules one-shot timers (`duration_seconds`) or recurring crons (`cron_expression`).
#[derive(Debug, Clone)]
pub struct ScheduleTool {
    registry: TaskRegistry,
}

impl ScheduleTool {
    /// Creates a [`ScheduleTool`] bound to the active session's `registry`.
    #[must_use]
    pub fn new(registry: TaskRegistry) -> Self {
        Self { registry }
    }
}

impl Tool for ScheduleTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "schedule".to_owned(),
            description: "Schedule a one-shot timer (duration_seconds) or recurring 5-field cron (cron_expression) that wakes the session with 'prompt'.".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "duration_seconds": {
                        "type": "integer",
                        "description": "One-shot delay in seconds (mutually exclusive with cron_expression)."
                    },
                    "cron_expression": {
                        "type": "string",
                        "description": "Standard 5-field cron expression (mutually exclusive with duration_seconds)."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Instruction delivered back to wake the session when the schedule fires."
                    },
                    "timer_condition": {
                        "type": "string",
                        "description": "One-shot timer early cancellation condition: 'never' (default), 'any', or a specific task_id."
                    },
                    "exact_timing": {
                        "type": "boolean",
                        "description": "Cron only: omit default 0-4s jitter when true."
                    },
                    "max_iterations": {
                        "type": "integer",
                        "description": "Cron only: optional maximum number of triggers before stopping."
                    },
                    "label": {
                        "type": "string",
                        "description": "Optional short label shown in the task strip."
                    }
                },
                "required": ["prompt"]
            }),
        }
    }

    fn required_capability(&self, _arguments: &Value) -> Result<Capability, ToolError> {
        Ok(Capability::Task {
            action: "schedule".to_owned(),
            mode: AccessMode::Write,
        })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let prompt = arguments
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ToolError::InvalidArguments {
                tool: "schedule".to_owned(),
                reason: "expected non-empty string 'prompt'".to_owned(),
            })?
            .to_owned();
        let label = arguments
            .get("label")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let duration_secs = arguments.get("duration_seconds").and_then(Value::as_u64);
        let cron_expr = arguments
            .get("cron_expression")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());

        match (duration_secs, cron_expr) {
            (Some(secs), None) => {
                let condition =
                    TimerCondition::parse(arguments.get("timer_condition").and_then(Value::as_str));
                let summary = self
                    .registry
                    .schedule_timer(secs, prompt, condition, label)?;
                Ok(serde_json::to_string(&summary)?)
            }
            (None, Some(expr)) => {
                let exact = arguments
                    .get("exact_timing")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let max_iterations = arguments.get("max_iterations").and_then(Value::as_u64);
                let summary =
                    self.registry
                        .schedule_cron(expr, prompt, max_iterations, exact, label)?;
                Ok(serde_json::to_string(&summary)?)
            }
            (Some(_), Some(_)) => Err(ToolError::InvalidArguments {
                tool: "schedule".to_owned(),
                reason: "duration_seconds and cron_expression are mutually exclusive".to_owned(),
            }),
            (None, None) => Err(ToolError::InvalidArguments {
                tool: "schedule".to_owned(),
                reason: "provide either duration_seconds or cron_expression".to_owned(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_mutual_exclusion_and_registers_timer_and_cron()
    -> Result<(), Box<dyn std::error::Error>> {
        let reg = TaskRegistry::new();
        let tool = ScheduleTool::new(reg.clone());

        assert!(
            tool.execute(&json!({
                "duration_seconds": 10,
                "cron_expression": "* * * * *",
                "prompt": "conflict"
            }))
            .is_err()
        );
        assert!(
            tool.execute(&json!({ "prompt": "missing target" }))
                .is_err()
        );

        let t_raw = tool.execute(&json!({
            "duration_seconds": 30,
            "prompt": "check build",
            "timer_condition": "any"
        }))?;
        let t_val: Value = serde_json::from_str(&t_raw)?;
        let tid = t_val["id"].as_str().ok_or("missing timer id")?.to_owned();

        let c_raw = tool.execute(&json!({
            "cron_expression": "*/5 * * * *",
            "prompt": "poll status",
            "max_iterations": 2,
            "exact_timing": true
        }))?;
        let c_val: Value = serde_json::from_str(&c_raw)?;
        let cid = c_val["id"].as_str().ok_or("missing cron id")?.to_owned();

        reg.shutdown_all();
        reg.delete_done_task(&tid)?;
        reg.delete_done_task(&cid)?;
        Ok(())
    }
}
