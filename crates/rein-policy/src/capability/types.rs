// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Fine-grained capability descriptors required by tools before execution

use std::path::PathBuf;

use rein_core::{MemoryScope, ModeId};
use serde::{Deserialize, Serialize};

/// Read or write access intent for a capability check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    /// Read-only inspection.
    Read,
    /// State-mutating write or send action.
    Write,
}

/// Personal-mode resource domains gated behind explicit user permissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonalResource {
    /// Personal markdown/JSONL notes.
    Notes,
    /// Native `.ics` / CalDAV calendar events.
    Calendar,
    /// Native IMAP/SMTP email messages.
    Email,
}

/// Capability required by a tool invocation and evaluated by the policy engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "domain", rename_all = "snake_case")]
pub enum Capability {
    /// Filesystem access to a target path.
    FileSystem {
        /// Target file or directory path.
        path: PathBuf,
        /// Read or write access mode.
        mode: AccessMode,
    },
    /// Subprocess command execution inside the workspace sandbox, inspected by [`crate::ShellClassifier`].
    Shell {
        /// Program binary name.
        program: String,
        /// Command-line arguments inspected by the read-only dictionary and shell classifier.
        #[serde(default)]
        args: Vec<String>,
    },
    /// Scoped memory access (`Project` or `Personal`).
    Memory {
        /// Target memory scope.
        scope: MemoryScope,
        /// Read or write access mode.
        mode: AccessMode,
    },
    /// Interactive personal assistant resource access.
    Personal {
        /// Resource kind (notes, calendar, email).
        resource: PersonalResource,
        /// Read or write access mode.
        mode: AccessMode,
    },
    /// External MCP server tool invocation.
    Mcp {
        /// Logical MCP server identifier.
        server: String,
        /// Tool name exposed by the MCP server.
        tool: String,
    },
    /// Runtime profile transition initiated via `switch_mode`.
    ModeSwitch {
        /// Target operating mode (`coding`, `plan`, or `personal`).
        target: ModeId,
    },
    /// Session-scoped global todo checklist operation (`todo_read`, `todo_write`).
    Todo {
        /// Read or write access mode.
        mode: AccessMode,
    },
    /// Delegated sub-agent execution confined to the spawning session's mode and policy.
    Subagent {
        /// Sub-agent type (`research`, `plan`, `coder`, `personal`).
        agent_type: String,
        /// Short 2–5 word sub-agent role title.
        role: String,
        /// Target operating mode required by this sub-agent.
        target_mode: ModeId,
    },
    /// Background task management and time-scheduled prompt triggers (`manage_task`, `schedule`).
    Task {
        /// Task operation (`list`, `status`, `send_input`, `kill`, `schedule`).
        action: String,
        /// Read or write access mode.
        mode: AccessMode,
    },
    /// Interactive multiple-choice prompt asking the user clarifying questions (`ask_question`).
    AskUser,
}
