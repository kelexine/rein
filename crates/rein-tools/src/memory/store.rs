// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Scoped JSONL memory store supporting upsert, delete, deduplication, and salience metadata

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rein_core::MemoryScope;
use serde::{Deserialize, Serialize};

use crate::registry::ToolError;

/// Semantic category for a stored [`MemoryRecord`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// General durable note or fact.
    #[default]
    General,
    /// User role, goals, preferences, or collaboration style.
    User,
    /// User correction or guidance on approach.
    Feedback,
    /// Non-code repository context, decisions, or deadlines.
    Project,
    /// External resource pointer (issue tracker, dashboard, docs).
    Reference,
}

impl MemoryKind {
    /// Parses a case-insensitive [`MemoryKind`] string, defaulting to [`MemoryKind::General`] if unrecognized.
    #[must_use]
    pub fn parse_lossy(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "user" => Self::User,
            "feedback" => Self::Feedback,
            "project" => Self::Project,
            "reference" => Self::Reference,
            _ => Self::General,
        }
    }

    /// Returns the lowercase canonical label for this memory kind.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::User => "user",
            Self::Feedback => "feedback",
            Self::Project => "project",
            Self::Reference => "reference",
        }
    }
}

fn default_salience() -> f32 {
    1.0
}

/// Returns current Unix epoch seconds (`0` if system clock precedes the epoch).
#[must_use]
pub fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A single scoped key-value memory record persisted in JSONL format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryRecord {
    /// Scope owning this memory entry (`Project` or `Personal`).
    pub scope: MemoryScope,
    /// Lookup key or topic tag.
    pub key: String,
    /// Stored memory content.
    pub value: String,
    /// Semantic memory category.
    #[serde(default)]
    pub kind: MemoryKind,
    /// Unix epoch timestamp (seconds) of the last write or consolidation.
    #[serde(default)]
    pub updated_at: u64,
    /// Current salience score (`FLOOR_SALIENCE..=1.0`), decayed for unconsolidated items.
    #[serde(default = "default_salience")]
    pub salience: f32,
    /// Exempt from temporal salience decay once promoted by AutoDream consolidation.
    #[serde(default)]
    pub consolidated: bool,
}

impl MemoryRecord {
    /// Creates a new [`MemoryRecord`] stamped at the current Unix timestamp with default salience.
    #[must_use]
    pub fn new(scope: MemoryScope, key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            scope,
            key: key.into(),
            value: value.into(),
            kind: MemoryKind::General,
            updated_at: now_unix_secs(),
            salience: default_salience(),
            consolidated: false,
        }
    }

    /// Sets the semantic [`MemoryKind`] on this record.
    #[must_use]
    pub fn with_kind(mut self, kind: MemoryKind) -> Self {
        self.kind = kind;
        self
    }

    /// Marks whether this record has been promoted/consolidated by AutoDream.
    #[must_use]
    pub fn with_consolidated(mut self, consolidated: bool) -> Self {
        self.consolidated = consolidated;
        self
    }
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

    /// Returns the JSONL file backing `scope`.
    #[must_use]
    pub fn path_for(&self, scope: MemoryScope) -> &Path {
        match scope {
            MemoryScope::Project => &self.project_path,
            MemoryScope::Personal => &self.personal_path,
        }
    }

    /// Appends a [`MemoryRecord`] to the JSONL store for `record.scope`.
    pub fn append(&self, record: &MemoryRecord) -> Result<(), ToolError> {
        let path = self.path_for(record.scope);
        ensure_parent_dir(path)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|source| ToolError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        let line = serde_json::to_string(record)?;
        writeln!(file, "{line}").map_err(|source| ToolError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Inserts or replaces a record matching `(record.scope, record.key)` case-insensitively.
    pub fn upsert(&self, record: &MemoryRecord) -> Result<(), ToolError> {
        let mut existing = self.read_raw_all_scopes(record.scope)?;
        let needle = record.key.trim().to_ascii_lowercase();
        existing.retain(|item| {
            !(item.scope == record.scope && item.key.trim().to_ascii_lowercase() == needle)
        });
        let mut saved = record.clone();
        if saved.updated_at == 0 {
            saved.updated_at = now_unix_secs();
        }
        existing.push(saved);
        write_atomic_jsonl(self.path_for(record.scope), &existing)
    }

    /// Deletes records matching `(scope, key)` case-insensitively; returns `true` if any were removed.
    pub fn delete(&self, scope: MemoryScope, key: &str) -> Result<bool, ToolError> {
        let mut existing = self.read_raw_all_scopes(scope)?;
        let needle = key.trim().to_ascii_lowercase();
        let before = existing.len();
        existing.retain(|item| {
            !(item.scope == scope && item.key.trim().to_ascii_lowercase() == needle)
        });
        if existing.len() == before {
            return Ok(false);
        }
        write_atomic_jsonl(self.path_for(scope), &existing)?;
        Ok(true)
    }

    /// Atomically replaces all records belonging to `scope` (preserving other scopes if sharing one file).
    pub fn replace_scope(
        &self,
        scope: MemoryScope,
        records: &[MemoryRecord],
    ) -> Result<(), ToolError> {
        let mut kept: Vec<MemoryRecord> = self
            .read_raw_all_scopes(scope)?
            .into_iter()
            .filter(|r| r.scope != scope)
            .collect();
        kept.extend(records.iter().cloned());
        write_atomic_jsonl(self.path_for(scope), &kept)
    }

    /// Queries deduplicated records (latest-wins per key) matching `scope` and optional case-insensitive `query`.
    pub fn query(&self, scope: MemoryScope, query: &str) -> Result<Vec<MemoryRecord>, ToolError> {
        let raw = self.read_raw_all_scopes(scope)?;
        let mut deduped: Vec<MemoryRecord> = Vec::new();
        for rec in raw.into_iter().filter(|r| r.scope == scope) {
            let needle = rec.key.trim().to_ascii_lowercase();
            deduped.retain(|prev| prev.key.trim().to_ascii_lowercase() != needle);
            deduped.push(rec);
        }
        let q = query.trim().to_ascii_lowercase();
        if q.is_empty() {
            return Ok(deduped);
        }
        Ok(deduped
            .into_iter()
            .filter(|r| {
                r.key.to_ascii_lowercase().contains(&q)
                    || r.value.to_ascii_lowercase().contains(&q)
                    || r.kind.as_str().contains(&q)
            })
            .collect())
    }

    fn read_raw_all_scopes(&self, scope: MemoryScope) -> Result<Vec<MemoryRecord>, ToolError> {
        let path = self.path_for(scope);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(path).map_err(|source| ToolError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let mut records = Vec::new();
        for line in BufReader::new(file).lines() {
            let raw = line.map_err(|source| ToolError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            if !raw.trim().is_empty() {
                records.push(serde_json::from_str(&raw)?);
            }
        }
        Ok(records)
    }
}

fn ensure_parent_dir(path: &Path) -> Result<(), ToolError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|source| ToolError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

static ATOMIC_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn unique_tmp_path(path: &Path) -> PathBuf {
    let seq = ATOMIC_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("memory.jsonl");
    path.with_file_name(format!(".{file_name}.{pid}.{seq}.tmp"))
}

fn write_atomic_jsonl(path: &Path, records: &[MemoryRecord]) -> Result<(), ToolError> {
    ensure_parent_dir(path)?;
    let tmp = unique_tmp_path(path);
    {
        let mut file = File::create(&tmp).map_err(|source| ToolError::Io {
            path: tmp.clone(),
            source,
        })?;
        for record in records {
            let line = serde_json::to_string(record)?;
            writeln!(file, "{line}").map_err(|source| ToolError::Io {
                path: tmp.clone(),
                source,
            })?;
        }
    }
    std::fs::rename(&tmp, path).map_err(|source| ToolError::Io {
        path: path.to_path_buf(),
        source,
    })
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
        store.append(&MemoryRecord::new(
            MemoryScope::Project,
            "build",
            "cargo test",
        ))?;
        store.append(&MemoryRecord::new(MemoryScope::Personal, "editor", "nvim"))?;

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

    #[test]
    fn upsert_deduplicates_delete_removes_and_legacy_lines_parse()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("rein_upsert_memory_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("mem.jsonl");
        std::fs::write(
            &path,
            "{\"scope\":\"project\",\"key\":\"MSRV\",\"value\":\"1.85\"}\n",
        )?;

        let store = JsonlMemoryStore::new(&path);
        assert_eq!(store.query(MemoryScope::Project, "msrv")?[0].value, "1.85");

        store.upsert(
            &MemoryRecord::new(MemoryScope::Project, "msrv", "1.99").with_kind(MemoryKind::Project),
        )?;
        let all = store.query(MemoryScope::Project, "")?;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].value, "1.99");
        assert_eq!(all[0].kind, MemoryKind::Project);

        assert!(store.delete(MemoryScope::Project, "MSRV")?);
        assert!(!store.delete(MemoryScope::Project, "MSRV")?);
        assert!(store.query(MemoryScope::Project, "")?.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
