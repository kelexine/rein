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
    ics_dir: Option<PathBuf>,
}

impl CalendarTool {
    /// Creates a [`CalendarTool`] backed by a local JSONL calendar file.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            ics_dir: None,
        }
    }

    /// Attaches an external `.ics` directory to read/import calendar events from.
    #[must_use]
    pub fn with_ics_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.ics_dir = Some(dir.into());
        self
    }

    fn read_ics_dir(&self) -> Result<Vec<CalendarEvent>, ToolError> {
        let Some(dir) = &self.ics_dir else {
            return Ok(Vec::new());
        };
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut events = Vec::new();
        for entry in std::fs::read_dir(dir).map_err(|source| ToolError::Io {
            path: dir.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| ToolError::Io {
                path: dir.clone(),
                source,
            })?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("ics") {
                let raw = std::fs::read_to_string(&path).map_err(|source| ToolError::Io {
                    path: path.clone(),
                    source,
                })?;
                events.extend(parse_ics_events(&raw));
            }
        }
        Ok(events)
    }

    fn read_all_events(&self) -> Result<Vec<CalendarEvent>, ToolError> {
        let mut combined = self.read_events()?;
        for ext in self.read_ics_dir()? {
            if !combined.iter().any(|ev| ev == &ext) {
                combined.push(ext);
            }
        }
        Ok(combined)
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
            description: "List, export RFC 5545 .ics, sync external .ics directory, or schedule calendar events".to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": { "type": "string", "enum": ["list", "export_ics", "schedule", "sync_ics"] },
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
            Some("schedule" | "sync_ics") => AccessMode::Write,
            _ => {
                return Err(ToolError::InvalidArguments {
                    tool: "calendar".to_owned(),
                    reason: "expected action 'list', 'export_ics', 'schedule', or 'sync_ics'"
                        .to_owned(),
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
            Some("list") => Ok(serde_json::to_string(&self.read_all_events()?)?),
            Some("export_ics") => {
                let mut ics = String::from(
                    "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Rein//Personal//EN\r\n",
                );
                for ev in self.read_all_events()? {
                    ics.push_str(&ev.to_vevent());
                }
                ics.push_str("END:VCALENDAR\r\n");
                Ok(ics)
            }
            Some("sync_ics") => {
                let existing = self.read_events()?;
                let mut imported = 0usize;
                for ext in self.read_ics_dir()? {
                    if !existing.iter().any(|ev| ev == &ext) {
                        self.append_event(&ext)?;
                        imported += 1;
                    }
                }
                Ok(serde_json::to_string(&json!({ "imported": imported }))?)
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
                validate_new_event(&event, &self.read_all_events()?)?;
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

fn parse_ics_events(raw: &str) -> Vec<CalendarEvent> {
    let mut events = Vec::new();
    let (mut in_vevent, mut summary, mut start_iso, mut end_iso) =
        (false, String::new(), String::new(), String::new());
    for raw_line in raw.lines() {
        let line = raw_line.trim();
        match line {
            "BEGIN:VEVENT" => {
                in_vevent = true;
                summary.clear();
                start_iso.clear();
                end_iso.clear();
            }
            "END:VEVENT" if in_vevent => {
                in_vevent = false;
                if !summary.is_empty() && !start_iso.is_empty() && !end_iso.is_empty() {
                    events.push(CalendarEvent {
                        summary: summary.clone(),
                        start_iso: start_iso.clone(),
                        end_iso: end_iso.clone(),
                    });
                }
            }
            _ if in_vevent => {
                if let Some(val) = line.strip_prefix("SUMMARY:") {
                    summary = val.to_owned();
                } else if let Some(val) = extract_ics_prop(line, "DTSTART") {
                    start_iso = normalize_ics_timestamp(val);
                } else if let Some(val) = extract_ics_prop(line, "DTEND") {
                    end_iso = normalize_ics_timestamp(val);
                }
            }
            _ => {}
        }
    }
    events
}

fn extract_ics_prop<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(key)?;
    let (_, value) = rest.split_once(':')?;
    Some(value.trim())
}

fn normalize_ics_timestamp(raw: &str) -> String {
    if raw.len() == 16 && raw.ends_with('Z') && raw.as_bytes()[8] == b'T' {
        format!(
            "{}-{}-{}T{}:{}:{}Z",
            &raw[0..4],
            &raw[4..6],
            &raw[6..8],
            &raw[9..11],
            &raw[11..13],
            &raw[13..15]
        )
    } else {
        raw.to_owned()
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

    #[test]
    fn imports_events_from_ics_directory() -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("rein_ics_dir_test");
        let _ = std::fs::remove_dir_all(&dir);
        let ics_dir = dir.join("ics");
        std::fs::create_dir_all(&ics_dir)?;
        std::fs::write(
            ics_dir.join("work.ics"),
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:Design Review\r\nDTSTART;TZID=UTC:20261120T140000Z\r\nDTEND:20261120T150000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        )?;

        let tool = CalendarTool::new(dir.join("cal.jsonl")).with_ics_dir(&ics_dir);
        let listed: Vec<CalendarEvent> =
            serde_json::from_str(&tool.execute(&json!({"action": "list"}))?)?;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].summary, "Design Review");
        assert_eq!(listed[0].start_iso, "2026-11-20T14:00:00Z");

        let synced: Value = serde_json::from_str(&tool.execute(&json!({"action": "sync_ics"}))?)?;
        assert_eq!(synced["imported"], 1);
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
