// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Auto-creates ~/.rein and <workspace>/.rein layouts and merges JSON configs & commands.json allowlists

use std::path::{Path, PathBuf};

use rein_tools::McpServerConfig;
use serde::{Deserialize, Serialize};

/// Selected cloud LLM provider backend configured in `config.json` or `REIN_PROVIDER`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// Anthropic Messages SSE API (`https://api.anthropic.com/v1/messages`).
    #[default]
    Anthropic,
    /// Google Antigravity Cloud Code PA (`https://daily-cloudcode-pa.googleapis.com`).
    Antigravity,
}

impl ProviderKind {
    /// Parses a case-insensitive provider name (`"anthropic"` or `"antigravity"`).
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "anthropic" => Some(Self::Anthropic),
            "antigravity" | "agy" => Some(Self::Antigravity),
            _ => None,
        }
    }

    /// Returns the canonical lowercase provider identifier.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Antigravity => "antigravity",
        }
    }
}

/// Resolved filesystem layout for global (`~/.rein`) and repository (`<workspace>/.rein`) state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReinPaths {
    /// Global user directory (`~/.rein`).
    pub global_dir: PathBuf,
    /// Repository workspace directory (`<workspace>/.rein`).
    pub repo_dir: PathBuf,
    /// Canonical workspace directory (`<workspace>`) owning this session group.
    pub workspace_dir: PathBuf,
    /// Global personal memory JSONL (`~/.rein/memory.jsonl`).
    pub global_memory: PathBuf,
    /// Repository project memory JSONL (`<workspace>/.rein/memory.jsonl`).
    pub repo_memory: PathBuf,
    /// Global personal Markdown memory vault & AutoDream index (`~/.rein/memory/`).
    pub global_memory_dir: PathBuf,
    /// Repository project Markdown memory vault & AutoDream index (`<workspace>/.rein/memory/`).
    pub repo_memory_dir: PathBuf,
    /// Global session-scoped todo checklists (`~/.rein/todos/`).
    pub todos_dir: PathBuf,
    /// Global personal notes JSONL (`~/.rein/notes.jsonl`).
    pub notes: PathBuf,
    /// Global personal calendar JSONL (`~/.rein/calendar.jsonl`).
    pub calendar: PathBuf,
    /// Global personal email spool JSONL (`~/.rein/email.jsonl`).
    pub email: PathBuf,
    /// Directory storing per-session UID event logs grouped by workspace directory (`~/.rein/sessions/<dir-bucket>/`).
    pub sessions_dir: PathBuf,
    /// Marker file holding the most recently active session UID in this workspace group.
    pub latest_session: PathBuf,
    /// Legacy single-file session transcript (`<workspace>/.rein/session.jsonl`).
    pub session_log: PathBuf,
    /// Legacy uncompacted conversation archive (`<workspace>/.rein/full-transcript.jsonl`).
    pub full_transcript: PathBuf,
    /// Repository policy audit log (`<workspace>/.rein/audit.jsonl`).
    pub audit_log: PathBuf,
    /// Global JSON config (`~/.rein/config.json`).
    pub global_config: PathBuf,
    /// Repository JSON config (`<workspace>/.rein/config.json`).
    pub repo_config: PathBuf,
    /// Global accepted shell command array (`~/.rein/commands.json`).
    pub global_commands: PathBuf,
    /// Per-workspace accepted shell command array (`~/.rein/sessions/<dir-bucket>/commands.json`).
    pub repo_commands: PathBuf,
    /// Global skills directory (`~/.rein/skills/`).
    pub global_skills_dir: PathBuf,
    /// Repository skills directory (`<workspace>/.rein/skills/`).
    pub repo_skills_dir: PathBuf,
}

impl ReinPaths {
    /// Constructs resolved paths from explicit `global_dir` and `repo_dir`, grouping sessions by workspace directory.
    #[must_use]
    pub fn from_dirs(global_dir: PathBuf, repo_dir: PathBuf) -> Self {
        let raw_workspace = repo_dir.parent().unwrap_or(&repo_dir).to_path_buf();
        let workspace_dir = std::fs::canonicalize(&raw_workspace).unwrap_or(raw_workspace);
        let bucket = workspace_session_bucket(&workspace_dir);
        let sessions_dir = global_dir.join("sessions").join(bucket);
        let latest_session = sessions_dir.join("latest");
        Self {
            global_memory: global_dir.join("memory.jsonl"),
            global_memory_dir: global_dir.join("memory"),
            todos_dir: global_dir.join("todos"),
            notes: global_dir.join("notes.jsonl"),
            calendar: global_dir.join("calendar.jsonl"),
            email: global_dir.join("email.jsonl"),
            global_config: global_dir.join("config.json"),
            global_commands: global_dir.join("commands.json"),
            global_skills_dir: global_dir.join("skills"),
            repo_memory: repo_dir.join("memory.jsonl"),
            repo_memory_dir: repo_dir.join("memory"),
            sessions_dir: sessions_dir.clone(),
            latest_session,
            session_log: repo_dir.join("session.jsonl"),
            full_transcript: repo_dir.join("full-transcript.jsonl"),
            audit_log: repo_dir.join("audit.jsonl"),
            repo_config: repo_dir.join("config.json"),
            repo_commands: sessions_dir.join("commands.json"),
            repo_skills_dir: repo_dir.join("skills"),
            global_dir,
            repo_dir,
            workspace_dir,
        }
    }

    /// Returns `(event_log_path, full_transcript_path)` for the given validated session `uid`.
    #[must_use]
    pub fn session_paths_for(&self, uid: &str) -> (PathBuf, PathBuf) {
        (
            self.sessions_dir.join(format!("{uid}.jsonl")),
            self.sessions_dir.join(format!("{uid}.full.jsonl")),
        )
    }
}

pub(crate) fn workspace_session_bucket(workspace_dir: &Path) -> String {
    let mut parts: Vec<String> = workspace_dir
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(os) => {
                let cleaned: String = os
                    .to_string_lossy()
                    .chars()
                    .map(|ch| {
                        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                            ch.to_ascii_lowercase()
                        } else {
                            '-'
                        }
                    })
                    .collect();
                let trimmed = cleaned.trim_matches('-').to_owned();
                (!trimmed.is_empty()).then_some(trimmed)
            }
            _ => None,
        })
        .collect();
    if parts.len() > 2 {
        parts = parts.split_off(parts.len() - 2);
    }
    let slug = if parts.is_empty() {
        "root".to_owned()
    } else {
        parts.join("-")
    };
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in workspace_dir.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!(
        "{slug}-{:08x}",
        u32::try_from(hash & 0xFFFF_FFFF).unwrap_or(0)
    )
}

/// Metadata summary for a saved workspace session in `.rein/sessions/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    /// Short lowercase hash identifier resumable with `--resume <uid>`.
    pub uid: String,
    /// Path to the session's `EventLog` JSONL file.
    pub event_log: PathBuf,
    /// Path to the session's uncompacted full-transcript JSONL archive.
    pub full_transcript: PathBuf,
    /// First non-continuation user prompt recorded in this session, if any.
    pub preview: Option<String>,
    /// Number of user turns recorded in this session.
    pub turn_count: usize,
}

/// Automatic background memory consolidation (`AutoDream`) configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DreamConfig {
    /// Enable automatic background memory consolidation (defaults to `true`).
    #[serde(default = "default_dream_enabled")]
    pub enabled: bool,
    /// Minimum hours between automatic consolidation passes (defaults to `24`).
    #[serde(default = "default_dream_min_hours")]
    pub min_hours: u64,
    /// Minimum newly modified session logs required to trigger consolidation (defaults to `5`).
    #[serde(default = "default_dream_min_sessions")]
    pub min_sessions: usize,
    /// Maximum tool-execution turns allowed during a single dream pass (defaults to `12`).
    #[serde(default = "default_dream_max_turns")]
    pub max_turns: usize,
}

fn default_dream_enabled() -> bool {
    true
}

fn default_dream_min_hours() -> u64 {
    24
}

fn default_dream_min_sessions() -> usize {
    5
}

fn default_dream_max_turns() -> usize {
    12
}

impl Default for DreamConfig {
    fn default() -> Self {
        Self {
            enabled: default_dream_enabled(),
            min_hours: default_dream_min_hours(),
            min_sessions: default_dream_min_sessions(),
            max_turns: default_dream_max_turns(),
        }
    }
}

/// User and workspace configuration schema stored as JSON in `config.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReinConfig {
    /// Preferred provider backend (`"anthropic"` or `"antigravity"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderKind>,
    /// Optional model identifier override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Optional Anthropic-compatible API endpoint override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Optional max output tokens override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Environment variable name supplying the API key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// Optional Antigravity base URL override (`https://daily-cloudcode-pa.googleapis.com`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antigravity_base_url: Option<String>,
    /// Optional explicit OAuth token JSON path for Antigravity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antigravity_token_path: Option<PathBuf>,
    /// Optional Cloud AI companion project ID (`aicode-consumers`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antigravity_project_id: Option<String>,
    /// Optional directory containing external `.ics` calendar files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ics_dir: Option<PathBuf>,
    /// Optional local Maildir root (`new/`, `cur/`) for native inbox inspection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maildir: Option<PathBuf>,
    /// Configured stdio MCP servers discovered at startup.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<McpServerConfig>,
    /// Optional background AutoDream memory consolidation settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dream: Option<DreamConfig>,
    /// Interactive UI and runtime preferences managed via `/config`.
    #[serde(default)]
    pub settings: UiSettings,
}

/// Interactive UI & runtime preferences persisted in `~/.rein/config.json` via `/config`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    /// Automatically compact long conversation histories.
    pub auto_compact: bool,
    /// Enable extended thinking / reasoning tokens when supported by the model.
    pub thinking_mode: bool,
    /// Expand tool output blocks by default in the transcript.
    pub verbose_output: bool,
    /// Show elapsed duration on completed turns.
    pub show_turn_duration: bool,
    /// Automatically scroll transcript to bottom on new output.
    pub auto_scroll: bool,
    /// Respect `.gitignore` rules inside the `@` file picker.
    pub respect_gitignore_picker: bool,
    /// Default permission policy (`"policy-gated"` or `"auto-bypass"`).
    pub default_permission_mode: String,
    /// Input editor mode (`"normal"` or `"vi"`).
    pub editor_mode: String,
    /// Show contextual hints in the footer/empty state.
    pub show_tips: bool,
    /// Controls when follow-up messages typed while busy are sent (`"send_immediately"` or `"queue"`).
    pub queued_messages: String,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            auto_compact: true,
            thinking_mode: true,
            verbose_output: false,
            show_turn_duration: true,
            auto_scroll: true,
            respect_gitignore_picker: true,
            default_permission_mode: "policy-gated".to_owned(),
            editor_mode: "normal".to_owned(),
            show_tips: true,
            queued_messages: "send_immediately".to_owned(),
        }
    }
}
