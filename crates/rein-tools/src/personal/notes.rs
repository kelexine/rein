// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Native Rust JSONL notes tool for interactive personal mode

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use rein_policy::{AccessMode, Capability, PersonalResource};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::registry::{Tool, ToolError, ToolSchema};

/// Structured personal note stored in JSONL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteEntry {
    /// Note title.
    pub title: String,
    /// Markdown body.
    pub body: String,
    /// Optional classification tags.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Native personal notes tool gated behind [`PersonalResource::Notes`].
#[derive(Debug, Clone)]
pub struct NotesTool {
    path: PathBuf,
}

impl NotesTool {
    /// Creates a [`NotesTool`] storing notes at `path`.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn read_entries(&self) -> Result<Vec<NoteEntry>, ToolError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(&self.path).map_err(|source| ToolError::Io {
            path: self.path.clone(),
            source,
        })?;
        let mut out = Vec::new();
        for line in BufReader::new(file).lines() {
            let raw = line.map_err(|source| ToolError::Io {
                path: self.path.clone(),
                source,
            })?;
            if !raw.trim().is_empty() {
                out.push(serde_json::from_str(&raw)?);
            }
        }
        Ok(out)
    }

    fn append_entry(&self, entry: &NoteEntry) -> Result<(), ToolError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ToolError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|source| ToolError::Io {
                path: self.path.clone(),
                source,
            })?;
        let line = serde_json::to_string(entry)?;
        writeln!(file, "{line}").map_err(|source| ToolError::Io {
            path: self.path.clone(),
            source,
        })
    }
}

impl Tool for NotesTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "notes".to_owned(),
            description: "List, search, or append personal notes in JSONL format (action='add' takes title, body, and optional tags).".to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": { "type": "string", "enum": ["list", "search", "add"] },
                    "query": { "type": "string" },
                    "title": { "type": "string" },
                    "body": { "type": "string" },
                    "tags": { "type": "array", "items": { "type": "string" } }
                }
            }),
        }
    }

    fn required_capability(&self, arguments: &Value) -> Result<Capability, ToolError> {
        let mode = match arguments.get("action").and_then(Value::as_str) {
            Some("list" | "search") => AccessMode::Read,
            Some("add") => AccessMode::Write,
            _ => {
                return Err(ToolError::InvalidArguments {
                    tool: "notes".to_owned(),
                    reason: "expected action 'list', 'search', or 'add'".to_owned(),
                });
            }
        };
        Ok(Capability::Personal {
            resource: PersonalResource::Notes,
            mode,
        })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        match arguments.get("action").and_then(Value::as_str) {
            Some("list") => Ok(serde_json::to_string(&self.read_entries()?)?),
            Some("search") => {
                let query = arguments.get("query").and_then(Value::as_str).unwrap_or("");
                let filtered: Vec<NoteEntry> = self
                    .read_entries()?
                    .into_iter()
                    .filter(|n| {
                        n.title.contains(query)
                            || n.body.contains(query)
                            || n.tags.iter().any(|t| t.contains(query))
                    })
                    .collect();
                Ok(serde_json::to_string(&filtered)?)
            }
            Some("add") => {
                let tags = arguments
                    .get("tags")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                let entry = NoteEntry {
                    title: arguments
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("Untitled")
                        .to_owned(),
                    body: arguments
                        .get("body")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    tags,
                };
                self.append_entry(&entry)?;
                Ok(serde_json::to_string(&entry)?)
            }
            _ => Err(ToolError::InvalidArguments {
                tool: "notes".to_owned(),
                reason: "unsupported action".to_owned(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_lists_and_searches_by_tag() -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir().join("rein_notes_tool_test.jsonl");
        let _ = std::fs::remove_file(&path);
        let tool = NotesTool::new(&path);
        tool.execute(
            &json!({"action": "add", "title": "groceries", "body": "milk", "tags": ["home"]}),
        )?;
        tool.execute(&json!({"action": "add", "title": "kernel", "body": "bore tuning", "tags": ["thevoid"]}))?;

        let all: Vec<NoteEntry> = serde_json::from_str(&tool.execute(&json!({"action": "list"}))?)?;
        assert_eq!(all.len(), 2);
        let hits: Vec<NoteEntry> =
            serde_json::from_str(&tool.execute(&json!({"action": "search", "query": "thevoid"}))?)?;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "kernel");
        assert!(matches!(
            tool.required_capability(&json!({"action": "add"}))?,
            Capability::Personal {
                mode: AccessMode::Write,
                ..
            }
        ));
        let _ = std::fs::remove_file(&path);
        Ok(())
    }
}
