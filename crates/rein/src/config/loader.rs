// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Auto-creates ~/.rein and <workspace>/.rein layouts and merges JSON configs & commands.json allowlists

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rein_core::{
    AnthropicConfig, AnthropicProvider, AntigravityConfig, AntigravityProvider, Provider,
    resolve_token,
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
    /// Global personal memory JSONL (`~/.rein/memory.jsonl`).
    pub global_memory: PathBuf,
    /// Repository project memory JSONL (`<workspace>/.rein/memory.jsonl`).
    pub repo_memory: PathBuf,
    /// Global personal notes JSONL (`~/.rein/notes.jsonl`).
    pub notes: PathBuf,
    /// Global personal calendar JSONL (`~/.rein/calendar.jsonl`).
    pub calendar: PathBuf,
    /// Global personal email spool JSONL (`~/.rein/email.jsonl`).
    pub email: PathBuf,
    /// Repository event-sourced session transcript (`<workspace>/.rein/session.jsonl`).
    pub session_log: PathBuf,
    /// Append-only full uncompacted conversation archive (`<workspace>/.rein/full-transcript.jsonl`).
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
    /// Constructs resolved paths from explicit `global_dir` and `repo_dir`.
    #[must_use]
    pub fn from_dirs(global_dir: PathBuf, repo_dir: PathBuf) -> Self {
        Self {
            global_memory: global_dir.join("memory.jsonl"),
            notes: global_dir.join("notes.jsonl"),
            calendar: global_dir.join("calendar.jsonl"),
            email: global_dir.join("email.jsonl"),
            global_config: global_dir.join("config.json"),
            global_commands: global_dir.join("commands.json"),
            global_skills_dir: global_dir.join("skills"),
            repo_memory: repo_dir.join("memory.jsonl"),
            session_log: repo_dir.join("session.jsonl"),
            full_transcript: repo_dir.join("full-transcript.jsonl"),
            audit_log: repo_dir.join("audit.jsonl"),
            repo_config: repo_dir.join("config.json"),
            repo_commands: repo_dir.join("commands.json"),
            repo_skills_dir: repo_dir.join("skills"),
            global_dir,
            repo_dir,
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
    for dir in [&paths.global_skills_dir, &paths.repo_skills_dir] {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create directory '{}'", dir.display()))?;
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
}
