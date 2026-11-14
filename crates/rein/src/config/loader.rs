// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Auto-creates ~/.rein and <workspace>/.rein layouts and merges JSON configs & commands.json allowlists

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rein_core::AnthropicConfig;
use rein_tools::McpServerConfig;
use serde::{Deserialize, Serialize};

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
            repo_memory: repo_dir.join("memory.jsonl"),
            session_log: repo_dir.join("session.jsonl"),
            audit_log: repo_dir.join("audit.jsonl"),
            repo_config: repo_dir.join("config.json"),
            repo_commands: repo_dir.join("commands.json"),
            global_dir,
            repo_dir,
        }
    }
}

/// User and workspace configuration schema stored as JSON in `config.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReinConfig {
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
            model: repo.model.clone().or_else(|| self.model.clone()),
            endpoint: repo.endpoint.clone().or_else(|| self.endpoint.clone()),
            max_tokens: repo.max_tokens.or(self.max_tokens),
            api_key_env: repo
                .api_key_env
                .clone()
                .or_else(|| self.api_key_env.clone()),
            ics_dir: repo.ics_dir.clone().or_else(|| self.ics_dir.clone()),
            maildir: repo.maildir.clone().or_else(|| self.maildir.clone()),
            mcp_servers,
        }
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
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }
}
