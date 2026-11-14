// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Append-only JSONL memory store supporting Project and Personal scopes

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use rein_core::MemoryScope;
use serde::{Deserialize, Serialize};

use crate::registry::ToolError;

/// A single scoped key-value memory record persisted in JSONL format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRecord {
    /// Scope owning this memory entry (`Project` or `Personal`).
    pub scope: MemoryScope,
    /// Lookup key or topic tag.
    pub key: String,
    /// Stored memory content.
    pub value: String,
}

/// JSONL-backed store holding [`MemoryRecord`] items without SQL dependencies.
#[derive(Debug, Clone)]
pub struct JsonlMemoryStore {
    path: PathBuf,
}

impl JsonlMemoryStore {
    /// Binds a [`JsonlMemoryStore`] to the specified JSONL file path.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Appends a [`MemoryRecord`] to the JSONL store.
    pub fn append(&self, record: &MemoryRecord) -> Result<(), ToolError> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|source| ToolError::Io {
                path: self.path.clone(),
                source,
            })?;
        let line = serde_json::to_string(record)?;
        writeln!(file, "{line}").map_err(|source| ToolError::Io {
            path: self.path.clone(),
            source,
        })
    }

    /// Queries records matching `scope` and optional substring `query`.
    pub fn query(&self, scope: MemoryScope, query: &str) -> Result<Vec<MemoryRecord>, ToolError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(&self.path).map_err(|source| ToolError::Io {
            path: self.path.clone(),
            source,
        })?;
        let mut matches = Vec::new();
        for line in BufReader::new(file).lines() {
            let raw = line.map_err(|source| ToolError::Io {
                path: self.path.clone(),
                source,
            })?;
            if raw.trim().is_empty() {
                continue;
            }
            let record: MemoryRecord = serde_json::from_str(&raw)?;
            if record.scope == scope
                && (query.is_empty() || record.key.contains(query) || record.value.contains(query))
            {
                matches.push(record);
            }
        }
        Ok(matches)
    }
}
