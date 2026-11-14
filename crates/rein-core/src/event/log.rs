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
