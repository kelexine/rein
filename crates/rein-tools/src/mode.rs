// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-15
// Purpose: Model-callable tool requesting a runtime transition between coding, plan, and personal modes

use rein_core::ModeId;
use rein_policy::Capability;
use serde_json::{Value, json};

use crate::registry::{Tool, ToolError, ToolSchema};

/// Tool allowing the model to request an audited transition between `coding`, `plan`, and `personal` modes.
#[derive(Debug, Default, Clone, Copy)]
pub struct SwitchModeTool;

impl SwitchModeTool {
    /// Creates a new [`SwitchModeTool`].
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// Parses a `"coding"`, `"plan"`, or `"personal"` mode string into [`ModeId`].
pub fn parse_mode_id(raw: &str) -> Result<ModeId, ToolError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "coding" | "code" => Ok(ModeId::Coding),
        "plan" => Ok(ModeId::Plan),
        "personal" => Ok(ModeId::Personal),
        other => Err(ToolError::InvalidArguments {
            tool: "switch_mode".to_owned(),
            reason: format!("unknown mode '{other}' (expected coding, plan, or personal)"),
        }),
    }
}

fn extract_mode_arg(arguments: &Value) -> Result<ModeId, ToolError> {
    let raw = arguments
        .get("mode")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::InvalidArguments {
            tool: "switch_mode".to_owned(),
            reason: "missing string field 'mode'".to_owned(),
        })?;
    parse_mode_id(raw)
}

impl Tool for SwitchModeTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "switch_mode".to_owned(),
            description: "Switch Rein's active mode (`coding`, `plan`, or `personal`) to enable that mode's toolset and security policy.".to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["mode"],
                "properties": {
                    "mode": {
                        "type": "string",
                        "enum": ["coding", "plan", "personal"],
                        "description": "Target operating mode."
                    },
                    "reason": {
                        "type": "string",
                        "description": "Optional brief rationale for switching modes."
                    }
                }
            }),
        }
    }

    fn required_capability(&self, arguments: &Value) -> Result<Capability, ToolError> {
        let target = extract_mode_arg(arguments)?;
        Ok(Capability::ModeSwitch { target })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let target = extract_mode_arg(arguments)?;
        let reason = arguments
            .get("reason")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("mode switched as requested");
        Ok(json!({
            "status": "switched",
            "mode": target,
            "reason": reason,
        })
        .to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_modes_and_rejects_unknown_mode() -> Result<(), Box<dyn std::error::Error>> {
        let tool = SwitchModeTool::new();
        assert_eq!(
            tool.required_capability(&json!({ "mode": "plan" }))?,
            Capability::ModeSwitch {
                target: ModeId::Plan
            }
        );
        let out = tool.execute(&json!({ "mode": "personal", "reason": "check notes" }))?;
        assert!(out.contains("\"mode\":\"personal\""));
        assert!(out.contains("check notes"));

        let err = tool.required_capability(&json!({ "mode": "admin" }));
        assert!(err.is_err());
        Ok(())
    }
}
