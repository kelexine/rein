// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Append-only JSONL AuditLog for policy evaluations

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use rein_core::ModeId;
use serde::{Deserialize, Serialize};

use crate::capability::Capability;
use crate::engine::{PolicyDecision, PolicyError};

/// Single audit record persisted in the JSONL audit log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Active mode when the capability was checked.
    pub mode: ModeId,
    /// Name of the tool requesting the capability.
    pub tool_name: String,
    /// Evaluated capability descriptor.
    pub capability: Capability,
    /// Resulting policy decision.
    pub decision: PolicyDecision,
}

/// Append-only JSONL audit log recording every policy decision.
#[derive(Debug, Clone)]
pub struct AuditLog {
    path: PathBuf,
}

impl AuditLog {
    /// Creates an [`AuditLog`] backed by the given JSONL path.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Returns the JSONL file path of the audit log.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends an [`AuditEntry`] to the JSONL audit file.
    pub fn record(&self, entry: &AuditEntry) -> Result<(), PolicyError> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|source| PolicyError::Io {
                path: self.path.clone(),
                source,
            })?;
        let line = serde_json::to_string(entry)?;
        writeln!(file, "{line}").map_err(|source| PolicyError::Io {
            path: self.path.clone(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::capability::AccessMode;

    #[test]
    fn audit_log_appends_and_deserializes_entries() -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("rein_policy_audit_log_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("audit.jsonl");
        let log = AuditLog::new(&path);
        assert_eq!(log.path(), path);

        let entry1 = AuditEntry {
            mode: ModeId::Coding,
            tool_name: "read".to_owned(),
            capability: Capability::FileSystem {
                path: PathBuf::from("Cargo.toml"),
                mode: AccessMode::Read,
            },
            decision: PolicyDecision::Allow {
                reason: "within workspace".to_owned(),
            },
        };
        let entry2 = AuditEntry {
            mode: ModeId::Plan,
            tool_name: "write".to_owned(),
            capability: Capability::FileSystem {
                path: PathBuf::from("src/lib.rs"),
                mode: AccessMode::Write,
            },
            decision: PolicyDecision::Deny {
                reason: "plan mode is read-only".to_owned(),
            },
        };

        log.record(&entry1)?;
        log.record(&entry2)?;

        let raw = std::fs::read_to_string(&path)?;
        let lines: Vec<AuditEntry> = raw
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()?;
        assert_eq!(lines, vec![entry1, entry2]);

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
