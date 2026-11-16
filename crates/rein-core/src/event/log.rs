// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Append-only JSONL EventLog implementation

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use crate::event::{CoreError, SessionEvent};

/// Append-only JSONL session log supporting deterministic replay.
#[derive(Debug)]
pub struct EventLog {
    path: PathBuf,
}

impl EventLog {
    /// Binds an [`EventLog`] to a JSONL file path on disk.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Returns the underlying JSONL file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends a single [`SessionEvent`] as one JSONL record.
    pub fn append(&self, event: &SessionEvent) -> Result<(), CoreError> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|source| CoreError::Io {
                path: self.path.clone(),
                source,
            })?;
        let line = serde_json::to_string(event)?;
        writeln!(file, "{line}").map_err(|source| CoreError::Io {
            path: self.path.clone(),
            source,
        })
    }

    /// Reads all recorded [`SessionEvent`] entries in deterministic order.
    pub fn read_all(&self) -> Result<Vec<SessionEvent>, CoreError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(&self.path).map_err(|source| CoreError::Io {
            path: self.path.clone(),
            source,
        })?;
        let reader = BufReader::new(file);
        let mut events = Vec::new();
        for line in reader.lines() {
            let raw = line.map_err(|source| CoreError::Io {
                path: self.path.clone(),
                source,
            })?;
            if !raw.trim().is_empty() {
                events.push(serde_json::from_str(&raw)?);
            }
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use crate::message::{Message, Role};

    #[test]
    fn event_log_returns_empty_when_missing_and_ignores_blank_lines()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("rein_core_event_log_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("events.jsonl");
        let log = EventLog::new(&path);

        assert_eq!(log.path(), path);
        assert!(log.read_all()?.is_empty());

        log.append(&SessionEvent::MessageRecorded {
            message: Message::new(Role::User, "first"),
        })?;
        let mut f = OpenOptions::new().append(true).open(&path)?;
        writeln!(f, "   ")?;
        log.append(&SessionEvent::StreamDelta {
            delta: "second".to_owned(),
        })?;

        let read_back = log.read_all()?;
        assert_eq!(read_back.len(), 2);
        assert!(matches!(
            &read_back[0],
            SessionEvent::MessageRecorded { message } if message.content == "first"
        ));
        assert!(matches!(
            &read_back[1],
            SessionEvent::StreamDelta { delta } if delta == "second"
        ));

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
