// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Ordered EffectLevel lattice, TargetScope, and ShellClassification

use serde::{Deserialize, Serialize};

/// Ordered effect lattice (low -> high):
/// `ReadOnly < WorkspaceWrite < OutsideWrite < Network < Exec < Unknown`
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EffectLevel {
    /// Pure read-only inspection; runs directly in a read-only Landlock sandbox.
    ReadOnly,
    /// Writes, modifies, or deletes files inside the workspace boundary.
    WorkspaceWrite,
    /// Writes, modifies, or deletes files outside the workspace boundary.
    OutsideWrite,
    /// Performs network communication (HTTP, DNS, SSH, Git remote, `/dev/tcp`).
    Network,
    /// Executes arbitrary code, interpreters, build scripts, or subprocess hooks.
    Exec,
    /// Unparseable, obfuscated, or statically unresolvable command (fails closed).
    Unknown,
}

/// Scope of filesystem or configuration targets touched by a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetScope {
    /// All paths resolve inside the normal workspace tree.
    Workspace,
    /// At least one read, write, or symlink target resolves outside the workspace.
    OutsideWorkspace,
    /// Touches protected paths (`.git/`, `.vscode/`, `.rein/`, or git hook/fsmonitor config).
    Protected,
}

/// Complete classification of a shell command or pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellClassification {
    /// Computed effect level in the lattice.
    pub effect: EffectLevel,
    /// Most sensitive target scope touched by the command.
    pub target: TargetScope,
    /// Human-readable audit explanation.
    pub reason: String,
}

impl ShellClassification {
    /// Constructs a [`ShellClassification`] defaulting to [`TargetScope::Workspace`].
    #[must_use]
    pub fn new(effect: EffectLevel, reason: impl Into<String>) -> Self {
        Self {
            effect,
            target: TargetScope::Workspace,
            reason: reason.into(),
        }
    }

    /// Constructs a [`ShellClassification`] with an explicit [`TargetScope`].
    #[must_use]
    pub fn with_target(
        effect: EffectLevel,
        target: TargetScope,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            effect,
            target,
            reason: reason.into(),
        }
    }

    /// Combines two classifications by taking the maximum [`EffectLevel`] and [`TargetScope`].
    #[must_use]
    pub fn merge(self, other: Self) -> Self {
        let target = self.target.max(other.target);
        if other.effect >= self.effect {
            Self {
                effect: other.effect,
                target,
                reason: other.reason,
            }
        } else {
            Self {
                effect: self.effect,
                target,
                reason: self.reason,
            }
        }
    }

    /// Returns `true` when the command is purely `ReadOnly` within `Workspace` scope.
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.effect == EffectLevel::ReadOnly && self.target == TargetScope::Workspace
    }
}
