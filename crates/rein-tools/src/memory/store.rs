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
///
/// Supports separating `Project` records (`<workspace>/.rein/memory.jsonl`) from
/// `Personal` records (`~/.rein/memory.jsonl`) via [`JsonlMemoryStore::dual`].
#[derive(Debug, Clone)]
pub struct JsonlMemoryStore {
    project_path: PathBuf,
    personal_path: PathBuf,
}

impl JsonlMemoryStore {
    /// Binds both `Project` and `Personal` scopes to the same JSONL file path.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let shared = path.into();
        Self {
            project_path: shared.clone(),
            personal_path: shared,
        }
    }

    /// Binds `Project` and `Personal` memory scopes to separate JSONL files.
    #[must_use]
    pub fn dual(project_path: impl Into<PathBuf>, personal_path: impl Into<PathBuf>) -> Self {
        Self {
            project_path: project_path.into(),
            personal_path: personal_path.into(),
        }
    }

    fn path_for(&self, scope: MemoryScope) -> &PathBuf {
        match scope {
            MemoryScope::Project => &self.project_path,
            MemoryScope::Personal => &self.personal_path,
        }
    }

    /// Appends a [`MemoryRecord`] to the JSONL store for `record.scope`.
    pub fn append(&self, record: &MemoryRecord) -> Result<(), ToolError> {
        let path = self.path_for(record.scope);
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|source| ToolError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|source| ToolError::Io {
                path: path.clone(),
                source,
            })?;
        let line = serde_json::to_string(record)?;
        writeln!(file, "{line}").map_err(|source| ToolError::Io {
            path: path.clone(),
            source,
        })
    }

    /// Queries records matching `scope` and optional substring `query`.
    pub fn query(&self, scope: MemoryScope, query: &str) -> Result<Vec<MemoryRecord>, ToolError> {
        let path = self.path_for(scope);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(path).map_err(|source| ToolError::Io {
            path: path.clone(),
            source,
        })?;
        let mut matches = Vec::new();
        for line in BufReader::new(file).lines() {
            let raw = line.map_err(|source| ToolError::Io {
                path: path.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dual_store_isolates_project_and_personal_files() -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("rein_dual_memory_test");
        let _ = std::fs::remove_dir_all(&dir);
        let project_file = dir.join("repo/memory.jsonl");
        let personal_file = dir.join("global/memory.jsonl");

        let store = JsonlMemoryStore::dual(&project_file, &personal_file);
        store.append(&MemoryRecord {
            scope: MemoryScope::Project,
            key: "build".to_owned(),
            value: "cargo test".to_owned(),
        })?;
        store.append(&MemoryRecord {
            scope: MemoryScope::Personal,
            key: "editor".to_owned(),
            value: "nvim".to_owned(),
        })?;

        let proj = store.query(MemoryScope::Project, "")?;
        let pers = store.query(MemoryScope::Personal, "")?;
        assert_eq!(proj.len(), 1);
        assert_eq!(proj[0].key, "build");
        assert_eq!(pers.len(), 1);
        assert_eq!(pers[0].key, "editor");
        assert!(project_file.exists());
        assert!(personal_file.exists());
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
