// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Native Rust calendar tool with JSONL persistence, overlap check, and RFC 5545 .ics export

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use rein_policy::{AccessMode, Capability, PersonalResource};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::registry::{Tool, ToolError, ToolSchema};

/// Native calendar event representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarEvent {
    /// Event summary/title.
    pub summary: String,
    /// ISO-8601 start timestamp (`YYYY-MM-DDTHH:MM:SSZ`).
    pub start_iso: String,
    /// ISO-8601 end timestamp (`YYYY-MM-DDTHH:MM:SSZ`).
    pub end_iso: String,
}

impl CalendarEvent {
    /// Checks whether two lexicographically comparable ISO-8601 intervals overlap.
    #[must_use]
    pub fn overlaps_with(&self, other: &Self) -> bool {
        self.start_iso < other.end_iso && other.start_iso < self.end_iso
    }

    /// Formats the event as an RFC 5545 `VEVENT` block.
    #[must_use]
    pub fn to_vevent(&self) -> String {
        let dt_start = self.start_iso.replace(['-', ':'], "");
        let dt_end = self.end_iso.replace(['-', ':'], "");
        format!(
            "BEGIN:VEVENT\r\nSUMMARY:{}\r\nDTSTART:{dt_start}\r\nDTEND:{dt_end}\r\nEND:VEVENT\r\n",
            self.summary
        )
    }
}

/// Interactive personal calendar tool gated behind [`PersonalResource::Calendar`].
#[derive(Debug, Clone)]
pub struct CalendarTool {
    path: PathBuf,
}

impl CalendarTool {
    /// Creates a [`CalendarTool`] backed by a local JSONL calendar file.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn read_events(&self) -> Result<Vec<CalendarEvent>, ToolError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(&self.path).map_err(|source| ToolError::Io {
            path: self.path.clone(),
            source,
        })?;
        let mut events = Vec::new();
        for line in BufReader::new(file).lines() {
            let raw = line.map_err(|source| ToolError::Io {
                path: self.path.clone(),
                source,
            })?;
            if !raw.trim().is_empty() {
                events.push(serde_json::from_str(&raw)?);
            }
        }
        Ok(events)
    }

    fn append_event(&self, event: &CalendarEvent) -> Result<(), ToolError> {
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
        let line = serde_json::to_string(event)?;
        writeln!(file, "{line}").map_err(|source| ToolError::Io {
            path: self.path.clone(),
            source,
        })
    }
}

impl Tool for CalendarTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "calendar".to_owned(),
            description: "List, export RFC 5545 .ics, or schedule calendar events".to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": { "type": "string", "enum": ["list", "export_ics", "schedule"] },
                    "summary": { "type": "string" },
                    "start_iso": { "type": "string" },
                    "end_iso": { "type": "string" }
                }
            }),
        }
    }

    fn required_capability(&self, arguments: &Value) -> Result<Capability, ToolError> {
        let mode = match arguments.get("action").and_then(Value::as_str) {
            Some("list" | "export_ics") => AccessMode::Read,
            Some("schedule") => AccessMode::Write,
            _ => {
                return Err(ToolError::InvalidArguments {
                    tool: "calendar".to_owned(),
                    reason: "expected action 'list', 'export_ics', or 'schedule'".to_owned(),
                });
            }
        };
        Ok(Capability::Personal {
            resource: PersonalResource::Calendar,
            mode,
        })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        match arguments.get("action").and_then(Value::as_str) {
            Some("list") => Ok(serde_json::to_string(&self.read_events()?)?),
            Some("export_ics") => {
                let mut ics = String::from(
                    "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Rein//Personal//EN\r\n",
                );
                for ev in self.read_events()? {
                    ics.push_str(&ev.to_vevent());
                }
                ics.push_str("END:VCALENDAR\r\n");
                Ok(ics)
            }
            Some("schedule") => {
                let event = CalendarEvent {
                    summary: arguments
                        .get("summary")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    start_iso: arguments
                        .get("start_iso")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    end_iso: arguments
                        .get("end_iso")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                };
                validate_new_event(&event, &self.read_events()?)?;
                self.append_event(&event)?;
                Ok(serde_json::to_string(&event)?)
            }
            _ => Err(ToolError::InvalidArguments {
                tool: "calendar".to_owned(),
                reason: "unsupported calendar action".to_owned(),
            }),
        }
    }
}

fn validate_new_event(event: &CalendarEvent, existing: &[CalendarEvent]) -> Result<(), ToolError> {
    if event.start_iso.is_empty() || event.end_iso.is_empty() || event.start_iso >= event.end_iso {
        return Err(ToolError::InvalidArguments {
            tool: "calendar".to_owned(),
            reason: "start_iso and end_iso are required and start must precede end".to_owned(),
        });
    }
    if let Some(conflict) = existing.iter().find(|ev| ev.overlaps_with(event)) {
        return Err(ToolError::InvalidArguments {
            tool: "calendar".to_owned(),
            reason: format!(
                "conflicts with '{}' ({} to {})",
                conflict.summary, conflict.start_iso, conflict.end_iso
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedules_rejects_overlap_and_exports_ics() -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir().join("rein_calendar_tool_test.jsonl");
        let _ = std::fs::remove_file(&path);
        let tool = CalendarTool::new(&path);

        tool.execute(&json!({"action": "schedule", "summary": "standup",
            "start_iso": "2026-11-15T09:00:00Z", "end_iso": "2026-11-15T09:30:00Z"}))?;
        let clash = tool.execute(&json!({"action": "schedule", "summary": "clash",
            "start_iso": "2026-11-15T09:15:00Z", "end_iso": "2026-11-15T10:00:00Z"}));
        assert!(clash.is_err());
        let inverted = tool.execute(&json!({"action": "schedule", "summary": "bad",
            "start_iso": "2026-11-15T12:00:00Z", "end_iso": "2026-11-15T11:00:00Z"}));
        assert!(inverted.is_err());

        let ics = tool.execute(&json!({"action": "export_ics"}))?;
        assert!(ics.starts_with("BEGIN:VCALENDAR"));
        assert!(ics.contains("SUMMARY:standup"));
        assert!(ics.contains("DTSTART:20261115T090000Z"));
        assert!(!ics.contains("clash"));
        let _ = std::fs::remove_file(&path);
        Ok(())
    }
}
