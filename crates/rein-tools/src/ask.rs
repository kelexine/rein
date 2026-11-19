// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-19
// Purpose: `ask_question` tool prompting the user with structured single/multi-select questions in the UI

use std::sync::Arc;

use rein_policy::Capability;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::registry::{Tool, ToolError, ToolSchema};

/// Single structured question presented to the user via `ask_question`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionSpec {
    /// Question prompt displayed to the user.
    pub question: String,
    /// Answer options offered cleanly to the user.
    pub options: Vec<String>,
    /// When true, the user may toggle multiple options before confirming.
    #[serde(default)]
    pub is_multi_select: bool,
}

/// User selection / free-form answer returned for one [`QuestionSpec`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionAnswer {
    /// Original question string.
    pub question: String,
    /// Selected option label(s).
    pub selected: Vec<String>,
    /// Optional custom free-form response (`Other`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_text: Option<String>,
}

/// Callback invoked by [`AskQuestionTool`] to collect answers synchronously from the active UI.
pub type QuestionHandler =
    Arc<dyn Fn(Vec<QuestionSpec>) -> Result<Vec<QuestionAnswer>, ToolError> + Send + Sync>;

/// Tool pausing the turn loop cleanly to ask the user structured multiple-choice questions.
#[derive(Clone)]
pub struct AskQuestionTool {
    handler: QuestionHandler,
}

impl std::fmt::Debug for AskQuestionTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AskQuestionTool").finish_non_exhaustive()
    }
}

impl AskQuestionTool {
    /// Creates an [`AskQuestionTool`] backed by `handler`.
    #[must_use]
    pub fn new(handler: QuestionHandler) -> Self {
        Self { handler }
    }
}

impl Tool for AskQuestionTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "ask_question".to_owned(),
            description: "Ask the user one or more structured multiple-choice questions (with optional multi-select and custom answer) to clarify requirements before proceeding.".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": { "type": "string" },
                                "options": {
                                    "type": "array",
                                    "items": { "type": "string" }
                                },
                                "is_multi_select": { "type": "boolean" }
                            },
                            "required": ["question", "options"]
                        }
                    }
                },
                "required": ["questions"]
            }),
        }
    }

    fn required_capability(&self, arguments: &Value) -> Result<Capability, ToolError> {
        let _ = parse_questions(arguments)?;
        Ok(Capability::AskUser)
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let questions = parse_questions(arguments)?;
        let answers = (self.handler)(questions)?;
        Ok(serde_json::to_string(&json!({ "answers": answers }))?)
    }
}

fn parse_questions(arguments: &Value) -> Result<Vec<QuestionSpec>, ToolError> {
    let raw = arguments
        .get("questions")
        .and_then(Value::as_array)
        .filter(|arr| !arr.is_empty())
        .ok_or_else(|| ToolError::InvalidArguments {
            tool: "ask_question".to_owned(),
            reason: "expected non-empty array 'questions'".to_owned(),
        })?;
    let mut specs = Vec::with_capacity(raw.len());
    for item in raw {
        let question = item
            .get("question")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ToolError::InvalidArguments {
                tool: "ask_question".to_owned(),
                reason: "each question must have a non-empty 'question' string".to_owned(),
            })?
            .to_owned();
        let options: Vec<String> = item
            .get("options")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        if options.is_empty() {
            return Err(ToolError::InvalidArguments {
                tool: "ask_question".to_owned(),
                reason: format!("question '{question}' must include at least one option"),
            });
        }
        let is_multi_select = item
            .get("is_multi_select")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        specs.push(QuestionSpec {
            question,
            options,
            is_multi_select,
        });
    }
    Ok(specs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_questions_and_returns_handler_answers() -> Result<(), Box<dyn std::error::Error>> {
        let tool = AskQuestionTool::new(Arc::new(|qs| {
            Ok(qs
                .into_iter()
                .map(|q| QuestionAnswer {
                    question: q.question,
                    selected: vec![q.options[0].clone()],
                    custom_text: None,
                })
                .collect())
        }));

        assert!(
            tool.required_capability(&json!({ "questions": [] }))
                .is_err()
        );
        let raw = tool.execute(&json!({
            "questions": [{
                "question": "Pick runtime",
                "options": ["std::thread", "tokio"],
                "is_multi_select": false
            }]
        }))?;
        assert!(raw.contains("std::thread"));
        Ok(())
    }
}
