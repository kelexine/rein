// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Auto-creates ~/.rein and <workspace>/.rein layouts and merges JSON configs & commands.json allowlists

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rein_core::{
    AnthropicConfig, AnthropicProvider, AntigravityConfig, AntigravityProvider, Provider, Role,
    SessionEvent, resolve_token,
};
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
    /// Repository accepted shell command array (`<workspace>/.rein/commands.json`).
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
            sessions_dir,
            latest_session,
            session_log: repo_dir.join("session.jsonl"),
            full_transcript: repo_dir.join("full-transcript.jsonl"),
            audit_log: repo_dir.join("audit.jsonl"),
            repo_config: repo_dir.join("config.json"),
            repo_commands: repo_dir.join("commands.json"),
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

fn workspace_session_bucket(workspace_dir: &Path) -> String {
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

static SESSION_SEQ: AtomicU64 = AtomicU64::new(0);

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

/// Returns `true` when `raw` is a safe session identifier (`4..=32` ASCII `[a-z0-9_-]`).
#[must_use]
pub fn is_valid_session_uid(raw: &str) -> bool {
    let len = raw.len();
    (4..=32).contains(&len)
        && raw
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// Generates a unique 8-char hex session UID not yet present in `paths.sessions_dir`.
#[must_use]
pub fn generate_session_uid(paths: &ReinPaths) -> String {
    let mut seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0u64, |d| u64::try_from(d.as_nanos()).unwrap_or(d.as_secs()));
    seed ^= u64::from(std::process::id()).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for byte in paths.repo_dir.to_string_lossy().as_bytes() {
        seed ^= u64::from(*byte);
        seed = seed.wrapping_mul(0x1000_0000_01B3);
    }

    loop {
        let seq = SESSION_SEQ.fetch_add(1, Ordering::Relaxed);
        let mixed = splitmix64(seed.wrapping_add(seq));
        let uid = format!("{:08x}", u32::try_from(mixed & 0xFFFF_FFFF).unwrap_or(0));
        let (log_path, _) = paths.session_paths_for(&uid);
        if !log_path.exists() {
            return uid;
        }
        seed = mixed;
    }
}

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Records `uid` as the active/latest session UID in `.rein/sessions/latest`.
pub fn write_latest_session_uid(paths: &ReinPaths, uid: &str) -> Result<()> {
    if !is_valid_session_uid(uid) {
        anyhow::bail!("invalid session uid '{uid}'");
    }
    std::fs::create_dir_all(&paths.sessions_dir).with_context(|| {
        format!(
            "failed to create sessions dir '{}'",
            paths.sessions_dir.display()
        )
    })?;
    std::fs::write(&paths.latest_session, format!("{uid}\n")).with_context(|| {
        format!(
            "failed to write latest session marker '{}'",
            paths.latest_session.display()
        )
    })
}

/// Reads the most recently recorded session UID from `.rein/sessions/latest`.
#[must_use]
pub fn read_latest_session_uid(paths: &ReinPaths) -> Option<String> {
    let raw = std::fs::read_to_string(&paths.latest_session).ok()?;
    let uid = raw.trim().to_owned();
    is_valid_session_uid(&uid).then_some(uid)
}

/// Resolves `--resume [uid|prefix]` to a [`SessionSummary`], or starts a fresh UID if bare `--resume` has no saved sessions.
pub fn resolve_resume_session(
    paths: &ReinPaths,
    target: Option<&str>,
) -> Result<(SessionSummary, bool)> {
    if let Some(raw) = target.map(str::trim).filter(|s| !s.is_empty()) {
        let lower = raw.to_ascii_lowercase();
        if !lower
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        {
            anyhow::bail!("invalid session uid '{raw}' (expected lowercase hex/alphanumeric id)");
        }
        let exact = paths.session_paths_for(&lower);
        if is_valid_session_uid(&lower) && exact.0.exists() {
            return Ok((summarize_session_file(&lower, exact.0, exact.1), true));
        }
        let saved = list_sessions(paths)?;
        let matches: Vec<SessionSummary> = saved
            .into_iter()
            .filter(|s| s.uid.starts_with(&lower))
            .collect();
        return match matches.as_slice() {
            [] => {
                if let Some(other_dir) = find_session_owner_dir(paths, &lower) {
                    anyhow::bail!(
                        "session '{raw}' belongs to directory '{other_dir}'; run rein there to resume it"
                    );
                }
                anyhow::bail!(
                    "no saved session matching uid '{raw}' in '{}'",
                    paths.workspace_dir.display()
                );
            }
            [single] => Ok((single.clone(), true)),
            many => {
                let ids: Vec<&str> = many.iter().map(|s| s.uid.as_str()).collect();
                anyhow::bail!(
                    "ambiguous session prefix '{raw}' matches: {}",
                    ids.join(", ")
                )
            }
        };
    }

    if let Some(latest_uid) = read_latest_session_uid(paths) {
        let (log, full) = paths.session_paths_for(&latest_uid);
        if log.exists() {
            return Ok((summarize_session_file(&latest_uid, log, full), true));
        }
    }

    if let Some(newest) = list_sessions(paths)?.into_iter().next() {
        return Ok((newest, true));
    }

    if paths.session_log.exists() {
        return Ok((
            summarize_session_file(
                "legacy",
                paths.session_log.clone(),
                paths.full_transcript.clone(),
            ),
            true,
        ));
    }

    let uid = generate_session_uid(paths);
    let (log, full) = paths.session_paths_for(&uid);
    Ok((
        SessionSummary {
            uid,
            event_log: log,
            full_transcript: full,
            preview: None,
            turn_count: 0,
        },
        false,
    ))
}

/// Lists saved sessions under `.rein/sessions/` sorted by most recently modified first.
pub fn list_sessions(paths: &ReinPaths) -> Result<Vec<SessionSummary>> {
    if !paths.sessions_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&paths.sessions_dir).with_context(|| {
        format!(
            "failed to read sessions dir '{}'",
            paths.sessions_dir.display()
        )
    })? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !name.ends_with(".jsonl") || name.ends_with(".full.jsonl") {
            continue;
        }
        let uid = name.trim_end_matches(".jsonl").to_owned();
        if !is_valid_session_uid(&uid) {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(UNIX_EPOCH);
        let (_, full) = paths.session_paths_for(&uid);
        entries.push((modified, summarize_session_file(&uid, path, full)));
    }
    entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.uid.cmp(&b.1.uid)));
    Ok(entries.into_iter().map(|(_, s)| s).collect())
}

fn find_session_owner_dir(paths: &ReinPaths, uid_or_prefix: &str) -> Option<String> {
    let sessions_root = paths.global_dir.join("sessions");
    let entries = std::fs::read_dir(sessions_root).ok()?;
    for entry in entries.flatten() {
        let bucket = entry.path();
        if bucket == paths.sessions_dir || !bucket.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&bucket) else {
            continue;
        };
        let matched = files.flatten().any(|f| {
            let name = f.file_name();
            let s = name.to_string_lossy();
            s.ends_with(".jsonl")
                && !s.ends_with(".full.jsonl")
                && s.trim_end_matches(".jsonl").starts_with(uid_or_prefix)
        });
        if matched {
            let owner = std::fs::read_to_string(bucket.join(".workspace"))
                .ok()
                .map(|raw| raw.trim().to_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| bucket.display().to_string());
            return Some(owner);
        }
    }
    None
}

fn summarize_session_file(
    uid: &str,
    event_log: PathBuf,
    full_transcript: PathBuf,
) -> SessionSummary {
    let mut preview: Option<String> = None;
    let mut turn_count = 0usize;
    let source = if full_transcript.exists() {
        &full_transcript
    } else {
        &event_log
    };
    if let Ok(file) = std::fs::File::open(source) {
        let reader = BufReader::new(file);
        for line in reader.lines().map_while(Result::ok) {
            let Ok(SessionEvent::MessageRecorded { message }) =
                serde_json::from_str::<SessionEvent>(&line)
            else {
                continue;
            };
            if message.role != Role::User {
                continue;
            }
            let content = message.content.trim();
            if content.is_empty() || content == "[tool_result_continuation]" {
                continue;
            }
            turn_count += 1;
            if preview.is_none() {
                let first_line = content.lines().next().unwrap_or(content).trim();
                let clipped: String = first_line.chars().take(52).collect();
                let ellipsis = if first_line.chars().count() > 52 {
                    "…"
                } else {
                    ""
                };
                preview = Some(format!("{clipped}{ellipsis}"));
            }
        }
    }
    SessionSummary {
        uid: uid.to_owned(),
        event_log,
        full_transcript,
        preview,
        turn_count,
    }
}

/// Automatic memory consolidation (`AutoDream`) scheduling configuration.
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
}

impl ReinConfig {
    /// Starter configuration written to `~/.rein/config.json` on first launch.
    #[must_use]
    pub fn global_starter() -> Self {
        Self {
            api_key_env: Some("ANTHROPIC_API_KEY".to_owned()),
            ..Self::default()
        }
    }

    /// Merges `repo` overrides cleanly on top of `self` (global).
    #[must_use]
    pub fn merge_with(&self, repo: &Self) -> Self {
        let mut mcp_servers = self.mcp_servers.clone();
        for server in &repo.mcp_servers {
            if !mcp_servers
                .iter()
                .any(|existing| existing.name == server.name)
            {
                mcp_servers.push(server.clone());
            }
        }
        Self {
            provider: repo.provider.or(self.provider),
            model: repo.model.clone().or_else(|| self.model.clone()),
            endpoint: repo.endpoint.clone().or_else(|| self.endpoint.clone()),
            max_tokens: repo.max_tokens.or(self.max_tokens),
            api_key_env: repo
                .api_key_env
                .clone()
                .or_else(|| self.api_key_env.clone()),
            antigravity_base_url: repo
                .antigravity_base_url
                .clone()
                .or_else(|| self.antigravity_base_url.clone()),
            antigravity_token_path: repo
                .antigravity_token_path
                .clone()
                .or_else(|| self.antigravity_token_path.clone()),
            antigravity_project_id: repo
                .antigravity_project_id
                .clone()
                .or_else(|| self.antigravity_project_id.clone()),
            ics_dir: repo.ics_dir.clone().or_else(|| self.ics_dir.clone()),
            maildir: repo.maildir.clone().or_else(|| self.maildir.clone()),
            mcp_servers,
            dream: repo.dream.clone().or_else(|| self.dream.clone()),
        }
    }

    /// Resolves the active [`ProviderKind`], honoring `REIN_PROVIDER` before config fields.
    #[must_use]
    pub fn resolved_provider(&self) -> ProviderKind {
        if let Ok(env_val) = std::env::var("REIN_PROVIDER")
            && let Some(kind) = ProviderKind::parse(&env_val)
        {
            return kind;
        }
        self.provider.unwrap_or_default()
    }

    /// Converts merged configuration into [`AnthropicConfig`], preserving env overrides when unset.
    #[must_use]
    pub fn to_anthropic_config(&self) -> AnthropicConfig {
        let mut cfg = AnthropicConfig::default();
        if let Some(model) = &self.model {
            cfg.model.clone_from(model);
        }
        if let Some(endpoint) = &self.endpoint {
            cfg.endpoint.clone_from(endpoint);
        }
        if let Some(max_tokens) = self.max_tokens {
            cfg.max_tokens = max_tokens;
        }
        if let Some(api_key_env) = &self.api_key_env {
            cfg.api_key_env.clone_from(api_key_env);
        }
        cfg
    }

    /// Converts merged configuration into [`AntigravityConfig`], preserving env overrides when unset.
    #[must_use]
    pub fn to_antigravity_config(&self) -> AntigravityConfig {
        let mut cfg = AntigravityConfig::default();
        if let Some(base_url) = &self.antigravity_base_url {
            cfg.base_url.clone_from(base_url);
        }
        if let Some(model) = &self.model {
            cfg.model.clone_from(model);
        }
        if let Some(max_tokens) = self.max_tokens {
            cfg.max_tokens = max_tokens;
        }
        if let Some(project_id) = &self.antigravity_project_id {
            cfg.project_id = Some(project_id.clone());
        }
        if let Some(token_path) = &self.antigravity_token_path {
            cfg.token_path = Some(token_path.clone());
        }
        cfg
    }
}

/// Updates the `provider` (and optional `model`) persisted inside `config_path` (e.g. `~/.rein/config.json`).
pub fn save_provider_selection(
    config_path: &Path,
    provider: ProviderKind,
    model: Option<&str>,
) -> Result<()> {
    let mut cfg: ReinConfig = if config_path.exists() {
        read_json_file(config_path)?
    } else {
        ReinConfig::global_starter()
    };
    cfg.provider = Some(provider);
    if let Some(m) = model.filter(|s| !s.trim().is_empty()) {
        cfg.model = Some(m.trim().to_owned());
    }
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory '{}'", parent.display()))?;
    }
    let encoded = serde_json::to_string_pretty(&cfg).context("failed to serialize config JSON")?;
    std::fs::write(config_path, format!("{encoded}\n"))
        .with_context(|| format!("failed to write config '{}'", config_path.display()))?;
    Ok(())
}

/// Instantiates the active [`Provider`] and status-bar display label (`<provider>/<model>`).
#[must_use]
pub fn build_provider(kind: ProviderKind, config: &ReinConfig) -> (Box<dyn Provider>, String) {
    match kind {
        ProviderKind::Anthropic => {
            let cfg = config.to_anthropic_config();
            let label = format!("anthropic/{}", cfg.model);
            (Box::new(AnthropicProvider::new(cfg)), label)
        }
        ProviderKind::Antigravity => {
            let cfg = config.to_antigravity_config();
            let label = format!("antigravity/{}", cfg.model);
            (Box::new(AntigravityProvider::new(cfg)), label)
        }
    }
}

/// Returns a non-fatal startup notice when credentials for `kind` are absent.
#[must_use]
pub fn provider_startup_notice(kind: ProviderKind, config: &ReinConfig) -> Option<String> {
    match kind {
        ProviderKind::Anthropic => {
            let cfg = config.to_anthropic_config();
            if std::env::var_os(&cfg.api_key_env).is_none() {
                Some(format!(
                    "{} not set · export it or run /login for Antigravity",
                    cfg.api_key_env
                ))
            } else {
                None
            }
        }
        ProviderKind::Antigravity => {
            let cfg = config.to_antigravity_config();
            if resolve_token(cfg.token_path.as_deref()).is_err() {
                Some("Antigravity credentials missing · run `/login` or `rein login`".to_owned())
            } else {
                None
            }
        }
    }
}

/// Resolves a `/model <spec>` argument into `(ProviderKind, model_id)`.
#[must_use]
pub fn resolve_model_target(config: &ReinConfig, raw_spec: &str) -> (ProviderKind, String) {
    let trimmed = raw_spec.trim();
    if let Some((prefix, rest)) = trimmed.split_once('/')
        && let Some(kind) = ProviderKind::parse(prefix)
        && !rest.trim().is_empty()
    {
        return (kind, rest.trim().to_owned());
    }
    if is_antigravity_model_id(trimmed) {
        return (ProviderKind::Antigravity, trimmed.to_owned());
    }
    if is_anthropic_model_id(trimmed) {
        return (ProviderKind::Anthropic, trimmed.to_owned());
    }
    (config.resolved_provider(), trimmed.to_owned())
}

fn is_antigravity_model_id(model: &str) -> bool {
    model.starts_with("gemini-")
        || model.starts_with("gpt-oss")
        || model.ends_with("-medium")
        || model.ends_with("-high")
        || model.ends_with("-low")
        || model.ends_with("-minimal")
        || model.ends_with("-tiered")
}

fn is_anthropic_model_id(model: &str) -> bool {
    model.starts_with("claude-") && !is_antigravity_model_id(model)
}

/// Updates in-memory `config` when switching providers interactively without carrying incompatible model IDs.
pub fn apply_provider_switch(
    config: &mut ReinConfig,
    kind: ProviderKind,
    explicit_model: Option<&str>,
) {
    config.provider = Some(kind);
    if let Some(model) = explicit_model.filter(|s| !s.trim().is_empty()) {
        config.model = Some(model.trim().to_owned());
        return;
    }
    if let Some(current) = config.model.as_deref() {
        let mismatch = match kind {
            ProviderKind::Anthropic => is_antigravity_model_id(current),
            ProviderKind::Antigravity => is_anthropic_model_id(current),
        };
        if mismatch {
            config.model = None;
        }
    }
}

/// Complete loaded startup state combining paths, merged config, and merged command allowlist.
#[derive(Debug, Clone)]
pub struct LoadedReinState {
    /// Resolved global and repo file paths.
    pub paths: ReinPaths,
    /// Merged configuration (`global` overridden by `repo`).
    pub config: ReinConfig,
    /// Deduplicated union of global and repo `commands.json` JSON arrays.
    pub allowed_commands: Vec<String>,
}

/// Resolves `~/.rein` + `<workspace_root>/.rein`, auto-creates missing files, and loads state.
pub fn ensure_dirs_and_load(workspace_root: &Path) -> Result<LoadedReinState> {
    let global_dir = resolve_global_dir()?;
    let repo_dir = workspace_root.join(".rein");
    ensure_dirs_and_load_in(&global_dir, &repo_dir)
}

/// Auto-creates `global_dir` and `repo_dir` layouts if missing and loads merged config + commands.
pub fn ensure_dirs_and_load_in(global_dir: &Path, repo_dir: &Path) -> Result<LoadedReinState> {
    std::fs::create_dir_all(global_dir)
        .with_context(|| format!("failed to create global dir '{}'", global_dir.display()))?;
    std::fs::create_dir_all(repo_dir)
        .with_context(|| format!("failed to create repo dir '{}'", repo_dir.display()))?;

    let paths = ReinPaths::from_dirs(global_dir.to_path_buf(), repo_dir.to_path_buf());
    for dir in [
        &paths.global_skills_dir,
        &paths.repo_skills_dir,
        &paths.global_memory_dir,
        &paths.repo_memory_dir,
        &paths.todos_dir,
        &paths.sessions_dir,
    ] {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create directory '{}'", dir.display()))?;
    }
    let marker = paths.sessions_dir.join(".workspace");
    if !marker.exists() {
        let _ = std::fs::write(&marker, format!("{}\n", paths.workspace_dir.display()));
    }
    let starter_global = serde_json::to_string_pretty(&ReinConfig::global_starter())?;
    ensure_file_if_missing(&paths.global_config, &format!("{starter_global}\n"))?;
    ensure_file_if_missing(&paths.repo_config, "{}\n")?;
    ensure_file_if_missing(&paths.global_commands, "[]\n")?;
    ensure_file_if_missing(&paths.repo_commands, "[]\n")?;

    let global_cfg: ReinConfig = read_json_file(&paths.global_config)?;
    let repo_cfg: ReinConfig = read_json_file(&paths.repo_config)?;
    let global_cmds: Vec<String> = read_json_file(&paths.global_commands)?;
    let repo_cmds: Vec<String> = read_json_file(&paths.repo_commands)?;

    let mut allowed_commands = global_cmds;
    for cmd in repo_cmds {
        if !allowed_commands.contains(&cmd) {
            allowed_commands.push(cmd);
        }
    }

    Ok(LoadedReinState {
        paths,
        config: global_cfg.merge_with(&repo_cfg),
        allowed_commands,
    })
}

/// Appends `pattern` to a `commands.json` JSON array file (`~/.rein/commands.json` or `.rein/commands.json`) if absent.
pub fn append_allowed_command(path: &Path, pattern: &str) -> Result<()> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory '{}'", parent.display()))?;
    }
    let mut commands: Vec<String> = if path.exists() {
        read_json_file(path)?
    } else {
        Vec::new()
    };
    if !commands.iter().any(|existing| existing == trimmed) {
        commands.push(trimmed.to_owned());
        let encoded = serde_json::to_string_pretty(&commands)
            .context("failed to serialize commands allowlist JSON")?;
        std::fs::write(path, format!("{encoded}\n"))
            .with_context(|| format!("failed to write allowlist '{}'", path.display()))?;
    }
    Ok(())
}

fn resolve_global_dir() -> Result<PathBuf> {
    if let Some(custom) = std::env::var_os("REIN_HOME") {
        return Ok(PathBuf::from(custom));
    }
    let home = std::env::var_os("HOME").context("HOME environment variable is not set")?;
    Ok(PathBuf::from(home).join(".rein"))
}

fn ensure_file_if_missing(path: &Path, default_contents: &str) -> Result<()> {
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => file
            .write_all(default_contents.as_bytes())
            .with_context(|| format!("failed to write starter file '{}'", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(err) => Err(err).with_context(|| format!("failed to initialize '{}'", path.display())),
    }
}

fn read_json_file<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read JSON file '{}'", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse JSON file '{}'", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_creates_global_and_repo_files_and_merges_commands() -> Result<()> {
        let root = std::env::temp_dir().join("rein_config_loader_test");
        let _ = std::fs::remove_dir_all(&root);
        let global = root.join("home/.rein");
        let repo = root.join("repo/.rein");

        let initial = ensure_dirs_and_load_in(&global, &repo)?;
        assert!(initial.paths.global_config.exists());
        assert!(initial.paths.repo_config.exists());
        assert!(initial.paths.global_commands.exists());
        assert!(initial.paths.repo_commands.exists());
        assert!(initial.paths.global_skills_dir.is_dir());
        assert!(initial.paths.repo_skills_dir.is_dir());
        assert!(initial.allowed_commands.is_empty());

        std::fs::write(&initial.paths.global_commands, r#"["cargo", "git"]"#)?;
        std::fs::write(&initial.paths.repo_commands, r#"["bun", "git"]"#)?;
        std::fs::write(
            &initial.paths.global_config,
            r#"{"model":"claude-sonnet-4-20250514","max_tokens":4096}"#,
        )?;
        std::fs::write(&initial.paths.repo_config, r#"{"max_tokens":2048}"#)?;

        let merged = ensure_dirs_and_load_in(&global, &repo)?;
        assert_eq!(merged.allowed_commands, vec!["cargo", "git", "bun"]);
        assert_eq!(
            merged.config.model.as_deref(),
            Some("claude-sonnet-4-20250514")
        );
        assert_eq!(merged.config.max_tokens, Some(2048));

        append_allowed_command(&initial.paths.repo_commands, "cargo *")?;
        append_allowed_command(&initial.paths.repo_commands, "cargo *")?;
        let updated: Vec<String> = read_json_file(&initial.paths.repo_commands)?;
        assert_eq!(updated, vec!["bun", "git", "cargo *"]);

        save_provider_selection(
            &initial.paths.global_config,
            ProviderKind::Antigravity,
            Some("gemini-3.8-flash-medium"),
        )?;
        let reloaded = ensure_dirs_and_load_in(&global, &repo)?;
        assert_eq!(reloaded.config.provider, Some(ProviderKind::Antigravity));
        assert_eq!(
            reloaded.config.model.as_deref(),
            Some("gemini-3.8-flash-medium")
        );

        let mut switched = reloaded.config.clone();
        apply_provider_switch(&mut switched, ProviderKind::Anthropic, None);
        assert_eq!(switched.provider, Some(ProviderKind::Anthropic));
        assert_eq!(switched.model, None);

        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    #[test]
    fn generates_unique_session_uids_and_resolves_prefix_and_latest() -> Result<()> {
        let root = std::env::temp_dir().join("rein_session_uid_test");
        let _ = std::fs::remove_dir_all(&root);
        let global = root.join("home/.rein");
        let repo = root.join("repo/.rein");
        let loaded = ensure_dirs_and_load_in(&global, &repo)?;

        assert!(loaded.paths.sessions_dir.is_dir());
        let u1 = generate_session_uid(&loaded.paths);
        let u2 = generate_session_uid(&loaded.paths);
        assert_ne!(u1, u2);
        assert!(is_valid_session_uid(&u1));
        assert!(!is_valid_session_uid("../etc"));

        let (p1, full1) = loaded.paths.session_paths_for(&u1);
        let ev1 = SessionEvent::MessageRecorded {
            message: rein_core::Message::new(Role::User, "analyze workspace architecture"),
        };
        std::fs::write(&p1, format!("{}\n", serde_json::to_string(&ev1)?))?;
        std::fs::write(&full1, format!("{}\n", serde_json::to_string(&ev1)?))?;
        write_latest_session_uid(&loaded.paths, &u1)?;

        let (latest_res, found_latest) = resolve_resume_session(&loaded.paths, None)?;
        assert!(found_latest);
        assert_eq!(latest_res.uid, u1);
        assert_eq!(latest_res.turn_count, 1);
        assert_eq!(
            latest_res.preview.as_deref(),
            Some("analyze workspace architecture")
        );

        let prefix = &u1[..4];
        let (pref_res, found_pref) = resolve_resume_session(&loaded.paths, Some(prefix))?;
        assert!(found_pref);
        assert_eq!(pref_res.uid, u1);

        assert!(resolve_resume_session(&loaded.paths, Some("deadbeef99")).is_err());

        let other_repo = root.join("other/.rein");
        let other = ensure_dirs_and_load_in(&global, &other_repo)?;
        assert_ne!(loaded.paths.sessions_dir, other.paths.sessions_dir);
        let err = resolve_resume_session(&other.paths, Some(&u1))
            .expect_err("must refuse cross-directory resume");
        assert!(err.to_string().contains("belongs to directory"));

        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }
}
