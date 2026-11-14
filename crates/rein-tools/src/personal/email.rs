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
    maildir: Option<PathBuf>,
}

impl EmailTool {
    /// Creates an [`EmailTool`] using `spool_path` for local JSONL inbox/outbox storage.
    #[must_use]
    pub fn new(spool_path: impl Into<PathBuf>) -> Self {
        Self {
            spool_path: spool_path.into(),
            maildir: None,
        }
    }

    /// Attaches a local Maildir root (`new/` and `cur/`) read alongside the JSONL spool.
    #[must_use]
    pub fn with_maildir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.maildir = Some(dir.into());
        self
    }

    fn read_maildir(&self) -> Result<Vec<EmailMessage>, ToolError> {
        let Some(root) = &self.maildir else {
            return Ok(Vec::new());
        };
        let mut maildir_msgs = Vec::new();
        for sub in ["new", "cur"] {
            let dir = root.join(sub);
            if !dir.exists() {
                continue;
            }
            for entry in std::fs::read_dir(&dir).map_err(|source| ToolError::Io {
                path: dir.clone(),
                source,
            })? {
                let entry = entry.map_err(|source| ToolError::Io {
                    path: dir.clone(),
                    source,
                })?;
                let path = entry.path();
                if path.is_file() {
                    let raw = std::fs::read_to_string(&path).map_err(|source| ToolError::Io {
                        path: path.clone(),
                        source,
                    })?;
                    maildir_msgs.push(parse_rfc2822_summary(&raw));
                }
            }
        }
        Ok(maildir_msgs)
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
            Some("inbox") => {
                let mut combined = self.read_messages()?;
                combined.extend(self.read_maildir()?);
                Ok(serde_json::to_string(&combined)?)
            }
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

fn parse_rfc2822_summary(raw: &str) -> EmailMessage {
    let (mut to, mut from, mut subject) = (String::new(), String::new(), String::new());
    let mut lines = raw.lines();
    for line in lines.by_ref() {
        let trimmed = line.trim_end_matches('\r');
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            match k.to_ascii_lowercase().as_str() {
                "to" => to = v.trim().to_owned(),
                "from" => from = v.trim().to_owned(),
                "subject" => subject = v.trim().to_owned(),
                _ => {}
            }
        }
    }
    let body_preview: String = lines.take(8).collect::<Vec<_>>().join("\n");
    let body = if from.is_empty() {
        body_preview
    } else {
        format!("From: {from}\n{body_preview}")
    };
    EmailMessage {
        to,
        subject,
        body,
        status: EmailStatus::Received,
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

    #[test]
    fn reads_maildir_new_and_cur_messages_in_inbox() -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("rein_maildir_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("maildir/new"))?;
        std::fs::create_dir_all(dir.join("maildir/cur"))?;
        std::fs::write(
            dir.join("maildir/new/msg1.eml"),
            "From: alice@example.invalid\nTo: kelexine@example.invalid\nSubject: Kernel Patch\n\nPlease review.",
        )?;

        let tool = EmailTool::new(dir.join("email.jsonl")).with_maildir(dir.join("maildir"));
        let inbox: Vec<EmailMessage> =
            serde_json::from_str(&tool.execute(&json!({"action": "inbox"}))?)?;
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].subject, "Kernel Patch");
        assert_eq!(inbox[0].status, EmailStatus::Received);
        assert!(inbox[0].body.contains("alice@example.invalid"));
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
