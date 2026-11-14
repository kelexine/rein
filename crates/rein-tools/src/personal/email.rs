// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Native Rust JSONL email inbox/draft/send tool gated behind interactive policy confirmation

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use rein_policy::{AccessMode, Capability, PersonalResource};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::registry::{Tool, ToolError, ToolSchema};

/// Delivery status of a stored [`EmailMessage`] in the local JSONL spool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmailStatus {
    /// Received message in inbox.
    Received,
    /// Staged draft not yet transmitted.
    Draft,
    /// Approved and dispatched message.
    Sent,
}

/// Email message envelope for native inbox/outbox operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailMessage {
    /// Recipient address.
    pub to: String,
    /// Subject line.
    pub subject: String,
    /// Plaintext body.
    pub body: String,
    /// Mailbox status (`received`, `draft`, or `sent`).
    pub status: EmailStatus,
}

/// Interactive personal email tool gated behind [`PersonalResource::Email`].
#[derive(Debug, Clone)]
pub struct EmailTool {
    spool_path: PathBuf,
}

impl EmailTool {
    /// Creates an [`EmailTool`] using `spool_path` for local JSONL inbox/outbox storage.
    #[must_use]
    pub fn new(spool_path: impl Into<PathBuf>) -> Self {
        Self {
            spool_path: spool_path.into(),
        }
    }

    fn read_messages(&self) -> Result<Vec<EmailMessage>, ToolError> {
        if !self.spool_path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(&self.spool_path).map_err(|source| ToolError::Io {
            path: self.spool_path.clone(),
            source,
        })?;
        let mut messages = Vec::new();
        for line in BufReader::new(file).lines() {
            let raw = line.map_err(|source| ToolError::Io {
                path: self.spool_path.clone(),
                source,
            })?;
            if !raw.trim().is_empty() {
                messages.push(serde_json::from_str(&raw)?);
            }
        }
        Ok(messages)
    }

    fn append_message(&self, msg: &EmailMessage) -> Result<(), ToolError> {
        if let Some(parent) = self.spool_path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ToolError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.spool_path)
            .map_err(|source| ToolError::Io {
                path: self.spool_path.clone(),
                source,
            })?;
        let line = serde_json::to_string(msg)?;
        writeln!(file, "{line}").map_err(|source| ToolError::Io {
            path: self.spool_path.clone(),
            source,
        })
    }
}

impl Tool for EmailTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "email".to_owned(),
            description: "Read inbox, stage a draft, or send email behind explicit user permission"
                .to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": { "type": "string", "enum": ["inbox", "draft", "send"] },
                    "to": { "type": "string" },
                    "subject": { "type": "string" },
                    "body": { "type": "string" }
                }
            }),
        }
    }

    fn required_capability(&self, arguments: &Value) -> Result<Capability, ToolError> {
        let mode = match arguments.get("action").and_then(Value::as_str) {
            Some("inbox") => AccessMode::Read,
            Some("draft" | "send") => AccessMode::Write,
            _ => {
                return Err(ToolError::InvalidArguments {
                    tool: "email".to_owned(),
                    reason: "expected action 'inbox', 'draft', or 'send'".to_owned(),
                });
            }
        };
        Ok(Capability::Personal {
            resource: PersonalResource::Email,
            mode,
        })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        match arguments.get("action").and_then(Value::as_str) {
            Some("inbox") => Ok(serde_json::to_string(&self.read_messages()?)?),
            Some(act @ ("draft" | "send")) => {
                let status = if act == "draft" {
                    EmailStatus::Draft
                } else {
                    EmailStatus::Sent
                };
                let msg = EmailMessage {
                    to: arguments
                        .get("to")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    subject: arguments
                        .get("subject")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    body: arguments
                        .get("body")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    status,
                };
                self.append_message(&msg)?;
                Ok(serde_json::to_string(&msg)?)
            }
            _ => Err(ToolError::InvalidArguments {
                tool: "email".to_owned(),
                reason: "unsupported email action".to_owned(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_and_send_persist_with_status_and_write_capability()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir().join("rein_email_tool_test.jsonl");
        let _ = std::fs::remove_file(&path);
        let tool = EmailTool::new(&path);
        tool.execute(
            &json!({"action": "draft", "to": "a@example.invalid", "subject": "hi", "body": "x"}),
        )?;
        tool.execute(
            &json!({"action": "send", "to": "b@example.invalid", "subject": "yo", "body": "y"}),
        )?;

        let inbox: Vec<EmailMessage> =
            serde_json::from_str(&tool.execute(&json!({"action": "inbox"}))?)?;
        assert_eq!(inbox.len(), 2);
        assert_eq!(inbox[0].status, EmailStatus::Draft);
        assert_eq!(inbox[1].status, EmailStatus::Sent);
        assert!(matches!(
            tool.required_capability(&json!({"action": "send"}))?,
            Capability::Personal {
                resource: PersonalResource::Email,
                mode: AccessMode::Write
            }
        ));
        assert!(
            tool.required_capability(&json!({"action": "nope"}))
                .is_err()
        );
        let _ = std::fs::remove_file(&path);
        Ok(())
    }
}
