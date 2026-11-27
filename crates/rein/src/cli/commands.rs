// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-27
// Purpose: Executes non-interactive CLI subcommands (one-shot prompt, config, models, auth, sessions, allowlist, mcp, plugins, doctor, replay)

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rein_core::{
    EventLog, ReplayProvider, Role, SessionEvent, ToolCall, UsageStats, login_with_browser,
    resolve_token,
};
use rein_policy::{AuditLog, PolicyEngine, WorkspaceSandbox, probe_kernel_sandbox};
use rein_tools::{
    JsonlMemoryStore, McpCatalog, McpClient, McpServerConfig, TaskRegistry, TodoStore,
};

use crate::agent::{
    AgentLoop, ApprovalAction, ApprovalDecision, ApprovalPrompter, ApprovalRequest, HydratedEvent,
};
use crate::config::{
    LoadedReinState, ProviderKind, ReinConfig, append_allowed_command, apply_provider_switch,
    build_provider, delete_session, list_sessions, load_cached_models, read_command_list,
    read_config_file, remove_allowed_command, resolve_model_target, resolve_resume_session,
    save_cached_models, save_config_file, save_provider_selection,
};
use crate::modes::Profile;
use crate::plugins::PluginManager;
use crate::runtime::{ReinSession, ResumeSpec, build_registry, format_sandbox_label};

use super::args::{
    AllowlistAction, AuthAction, CliCommand, ConfigAction, McpAction, ModelsAction, OneShotOpts,
    PluginsAction, SessionsAction, format_help_text, format_version_text,
};

struct HeadlessPrompter {
    stream_stdout: bool,
    bypass: bool,
    tool_calls: Vec<String>,
    usage: UsageStats,
}

impl HeadlessPrompter {
    fn new(stream_stdout: bool, bypass: bool) -> Self {
        Self {
            stream_stdout,
            bypass,
            tool_calls: Vec::new(),
            usage: UsageStats::default(),
        }
    }
}

impl ApprovalPrompter for HeadlessPrompter {
    fn confirm(&mut self, request: &ApprovalRequest) -> Result<ApprovalDecision> {
        if self.bypass {
            return Ok(ApprovalDecision {
                action: ApprovalAction::AllowOnce,
                feedback: None,
            });
        }
        eprintln!(
            "[policy] denied in non-interactive mode (pass --yolo or `rein allowlist add \"{}\"`): {}",
            request.pattern, request.excerpt
        );
        Ok(ApprovalDecision {
            action: ApprovalAction::Deny,
            feedback: None,
        })
    }

    fn on_delta(&mut self, delta: &str) {
        if self.stream_stdout {
            print!("{delta}");
            let _ = std::io::stdout().flush();
        }
    }

    fn on_tool_start(&mut self, call: &ToolCall) {
        self.tool_calls.push(call.name.clone());
    }

    fn on_usage(&mut self, usage: UsageStats) {
        self.usage.add_assign_turn(usage);
    }
}

/// Executes any non-TUI [`CliCommand`] and returns `Ok(true)` if handled, or `Ok(false)` to launch the TUI.
pub fn execute_cli_command(
    cmd: CliCommand,
    workspace_root: &Path,
    loaded: &LoadedReinState,
) -> Result<bool> {
    match cmd {
        CliCommand::InteractiveTui => Ok(false),
        CliCommand::Help => {
            print!("{}", format_help_text());
            Ok(true)
        }
        CliCommand::Version => {
            println!("{}", format_version_text());
            Ok(true)
        }
        CliCommand::OneShot(opts) => {
            run_oneshot_cli(workspace_root, loaded.clone(), opts)?;
            Ok(true)
        }
        CliCommand::Replay {
            path,
            bypass_permissions,
        } => {
            run_replay_cli(&path, workspace_root, loaded, bypass_permissions)?;
            Ok(true)
        }
        CliCommand::Config(action) => {
            run_config_cli(loaded, action)?;
            Ok(true)
        }
        CliCommand::Models(action) => {
            run_models_cli(loaded, action)?;
            Ok(true)
        }
        CliCommand::Auth(action) => {
            run_auth_cli(loaded, action)?;
            Ok(true)
        }
        CliCommand::Sessions(action) => {
            run_sessions_cli(loaded, action)?;
            Ok(true)
        }
        CliCommand::Allowlist(action) => {
            run_allowlist_cli(loaded, action)?;
            Ok(true)
        }
        CliCommand::Mcp(action) => {
            run_mcp_cli(loaded, action)?;
            Ok(true)
        }
        CliCommand::Plugins(action) => {
            run_plugins_cli(loaded, action)?;
            Ok(true)
        }
        CliCommand::Doctor => {
            run_doctor_cli(workspace_root, loaded)?;
            Ok(true)
        }
    }
}

fn run_oneshot_cli(
    workspace_root: &Path,
    mut loaded: LoadedReinState,
    opts: OneShotOpts,
) -> Result<()> {
    if let Some(kind) = opts.provider {
        apply_provider_switch(&mut loaded.config, kind, None);
    }
    if let Some(model) = &opts.model {
        let (kind, model_id) = resolve_model_target(&loaded.config, model);
        apply_provider_switch(&mut loaded.config, kind, Some(&model_id));
    }
    let bypass =
        opts.bypass_permissions || loaded.config.settings.default_permission_mode == "auto-bypass";
    let mut session =
        ReinSession::from_loaded(workspace_root, loaded, bypass, ResumeSpec::NewSession)?;
    if let Some(mode) = opts.mode {
        session.agent.set_mode(mode)?;
    }

    let mut prompter = HeadlessPrompter::new(!opts.json, bypass);
    let reply =
        session
            .agent
            .run_user_prompt(&opts.prompt, session.provider.as_mut(), &mut prompter)?;

    if opts.json {
        let payload = serde_json::json!({
            "session_uid": session.session_uid,
            "model": session.model_label,
            "mode": format!("{:?}", session.agent.active_profile().mode).to_ascii_lowercase(),
            "response": reply,
            "tool_calls": prompter.tool_calls,
            "usage": {
                "input_tokens": prompter.usage.input_tokens,
                "output_tokens": prompter.usage.output_tokens,
                "total_tokens": prompter.usage.total_tokens,
            }
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if !reply.ends_with('\n') {
        println!();
    }
    Ok(())
}

/// Applies a dotted configuration key/value update to `cfg`.
pub fn apply_config_key_value(cfg: &mut ReinConfig, key: &str, value: &str) -> Result<()> {
    let parse_bool = |v: &str| -> Result<bool> {
        match v.trim().to_ascii_lowercase().as_str() {
            "true" | "on" | "1" | "yes" => Ok(true),
            "false" | "off" | "0" | "no" => Ok(false),
            _ => bail!("expected boolean (true/false/on/off) for '{key}', got '{v}'"),
        }
    };
    let trimmed = value.trim();
    match key {
        "provider" => {
            let kind = ProviderKind::parse(trimmed).ok_or_else(|| {
                anyhow::anyhow!("invalid provider '{trimmed}' (use anthropic or antigravity)")
            })?;
            apply_provider_switch(cfg, kind, None);
        }
        "model" => {
            cfg.model = (!trimmed.is_empty() && trimmed != "none" && trimmed != "default")
                .then(|| trimmed.to_owned());
        }
        "endpoint" => {
            cfg.endpoint = (!trimmed.is_empty() && trimmed != "none").then(|| trimmed.to_owned());
        }
        "max_tokens" => {
            if trimmed.is_empty() || trimmed == "none" || trimmed == "default" {
                cfg.max_tokens = None;
            } else {
                let n: u32 = trimmed
                    .parse()
                    .with_context(|| format!("invalid max_tokens integer '{trimmed}'"))?;
                cfg.max_tokens = Some(n);
            }
        }
        "api_key_env" => {
            cfg.api_key_env =
                (!trimmed.is_empty() && trimmed != "none").then(|| trimmed.to_owned());
        }
        "antigravity_base_url" => {
            cfg.antigravity_base_url =
                (!trimmed.is_empty() && trimmed != "none").then(|| trimmed.to_owned());
        }
        "antigravity_project_id" => {
            cfg.antigravity_project_id =
                (!trimmed.is_empty() && trimmed != "none").then(|| trimmed.to_owned());
        }
        "antigravity_token_path" => {
            cfg.antigravity_token_path =
                (!trimmed.is_empty() && trimmed != "none").then(|| PathBuf::from(trimmed));
        }
        "ics_dir" => {
            cfg.ics_dir =
                (!trimmed.is_empty() && trimmed != "none").then(|| PathBuf::from(trimmed));
        }
        "maildir" => {
            cfg.maildir =
                (!trimmed.is_empty() && trimmed != "none").then(|| PathBuf::from(trimmed));
        }
        "dream.enabled" => {
            let mut d = cfg.dream.clone().unwrap_or_default();
            d.enabled = parse_bool(trimmed)?;
            cfg.dream = Some(d);
        }
        "dream.min_hours" => {
            let mut d = cfg.dream.clone().unwrap_or_default();
            d.min_hours = trimmed
                .parse()
                .context("expected u64 for dream.min_hours")?;
            cfg.dream = Some(d);
        }
        "dream.min_sessions" => {
            let mut d = cfg.dream.clone().unwrap_or_default();
            d.min_sessions = trimmed
                .parse()
                .context("expected usize for dream.min_sessions")?;
            cfg.dream = Some(d);
        }
        "dream.max_turns" => {
            let mut d = cfg.dream.clone().unwrap_or_default();
            d.max_turns = trimmed
                .parse()
                .context("expected usize for dream.max_turns")?;
            cfg.dream = Some(d);
        }
        "settings.auto_compact" | "auto_compact" => {
            cfg.settings.auto_compact = parse_bool(trimmed)?
        }
        "settings.thinking_mode" | "thinking_mode" => {
            cfg.settings.thinking_mode = parse_bool(trimmed)?;
        }
        "settings.verbose_output" | "verbose_output" => {
            cfg.settings.verbose_output = parse_bool(trimmed)?;
        }
        "settings.show_turn_duration" | "show_turn_duration" => {
            cfg.settings.show_turn_duration = parse_bool(trimmed)?;
        }
        "settings.auto_scroll" | "auto_scroll" => cfg.settings.auto_scroll = parse_bool(trimmed)?,
        "settings.respect_gitignore_picker" | "respect_gitignore_picker" => {
            cfg.settings.respect_gitignore_picker = parse_bool(trimmed)?;
        }
        "settings.show_tips" | "show_tips" => cfg.settings.show_tips = parse_bool(trimmed)?,
        "settings.default_permission_mode" | "default_permission_mode" => {
            if !matches!(trimmed, "policy-gated" | "auto-bypass") {
                bail!("default_permission_mode must be 'policy-gated' or 'auto-bypass'");
            }
            cfg.settings.default_permission_mode = trimmed.to_owned();
        }
        "settings.editor_mode" | "editor_mode" => {
            if !matches!(trimmed, "normal" | "vi") {
                bail!("editor_mode must be 'normal' or 'vi'");
            }
            cfg.settings.editor_mode = trimmed.to_owned();
        }
        "settings.queued_messages" | "queued_messages" => {
            if !matches!(trimmed, "send_immediately" | "queue") {
                bail!("queued_messages must be 'send_immediately' or 'queue'");
            }
            cfg.settings.queued_messages = trimmed.to_owned();
        }
        other => {
            bail!("unknown config key '{other}' (run `rein config list` to see all supported keys)")
        }
    }
    Ok(())
}

/// Reads a single dotted configuration key from `cfg`.
pub fn read_config_key(cfg: &ReinConfig, key: &str) -> Result<String> {
    let dream = cfg.dream.clone().unwrap_or_default();
    let val = match key {
        "provider" => cfg.resolved_provider().as_str().to_owned(),
        "model" => cfg
            .model
            .clone()
            .unwrap_or_else(|| "(provider default)".to_owned()),
        "endpoint" => cfg
            .endpoint
            .clone()
            .unwrap_or_else(|| "(default)".to_owned()),
        "max_tokens" => cfg
            .max_tokens
            .map_or_else(|| "(default)".to_owned(), |n| n.to_string()),
        "api_key_env" => cfg
            .api_key_env
            .clone()
            .unwrap_or_else(|| "ANTHROPIC_API_KEY".to_owned()),
        "antigravity_base_url" => cfg
            .antigravity_base_url
            .clone()
            .unwrap_or_else(|| "(default)".to_owned()),
        "antigravity_project_id" => cfg
            .antigravity_project_id
            .clone()
            .unwrap_or_else(|| "(auto)".to_owned()),
        "antigravity_token_path" => cfg
            .antigravity_token_path
            .as_ref()
            .map_or_else(|| "(default)".to_owned(), |p| p.display().to_string()),
        "ics_dir" => cfg
            .ics_dir
            .as_ref()
            .map_or_else(|| "(default)".to_owned(), |p| p.display().to_string()),
        "maildir" => cfg
            .maildir
            .as_ref()
            .map_or_else(|| "(default)".to_owned(), |p| p.display().to_string()),
        "dream.enabled" => dream.enabled.to_string(),
        "dream.min_hours" => dream.min_hours.to_string(),
        "dream.min_sessions" => dream.min_sessions.to_string(),
        "dream.max_turns" => dream.max_turns.to_string(),
        "settings.auto_compact" | "auto_compact" => cfg.settings.auto_compact.to_string(),
        "settings.thinking_mode" | "thinking_mode" => cfg.settings.thinking_mode.to_string(),
        "settings.verbose_output" | "verbose_output" => cfg.settings.verbose_output.to_string(),
        "settings.show_turn_duration" | "show_turn_duration" => {
            cfg.settings.show_turn_duration.to_string()
        }
        "settings.auto_scroll" | "auto_scroll" => cfg.settings.auto_scroll.to_string(),
        "settings.respect_gitignore_picker" | "respect_gitignore_picker" => {
            cfg.settings.respect_gitignore_picker.to_string()
        }
        "settings.show_tips" | "show_tips" => cfg.settings.show_tips.to_string(),
        "settings.default_permission_mode" | "default_permission_mode" => {
            cfg.settings.default_permission_mode.clone()
        }
        "settings.editor_mode" | "editor_mode" => cfg.settings.editor_mode.clone(),
        "settings.queued_messages" | "queued_messages" => cfg.settings.queued_messages.clone(),
        other => bail!("unknown config key '{other}'"),
    };
    Ok(val)
}

fn run_config_cli(loaded: &LoadedReinState, action: ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Path => {
            println!("global_dir:      {}", loaded.paths.global_dir.display());
            println!("global_config:   {}", loaded.paths.global_config.display());
            println!(
                "global_commands: {}",
                loaded.paths.global_commands.display()
            );
            println!("repo_dir:        {}", loaded.paths.repo_dir.display());
            println!("repo_config:     {}", loaded.paths.repo_config.display());
            println!("repo_commands:   {}", loaded.paths.repo_commands.display());
            println!("sessions_dir:    {}", loaded.paths.sessions_dir.display());
        }
        ConfigAction::List { json } => {
            if json {
                println!("{}", serde_json::to_string_pretty(&loaded.config)?);
                return Ok(());
            }
            let keys = [
                "provider",
                "model",
                "max_tokens",
                "endpoint",
                "api_key_env",
                "antigravity_base_url",
                "antigravity_project_id",
                "antigravity_token_path",
                "ics_dir",
                "maildir",
                "dream.enabled",
                "dream.min_hours",
                "dream.min_sessions",
                "dream.max_turns",
                "settings.thinking_mode",
                "settings.auto_compact",
                "settings.verbose_output",
                "settings.show_turn_duration",
                "settings.auto_scroll",
                "settings.respect_gitignore_picker",
                "settings.default_permission_mode",
                "settings.editor_mode",
                "settings.queued_messages",
                "settings.show_tips",
            ];
            for k in keys {
                println!("{:<34} = {}", k, read_config_key(&loaded.config, k)?);
            }
        }
        ConfigAction::Get { key } => {
            println!("{}", read_config_key(&loaded.config, &key)?);
        }
        ConfigAction::Set { key, value, repo } => {
            if repo
                && !matches!(
                    key.as_str(),
                    "model"
                        | "max_tokens"
                        | "dream.enabled"
                        | "dream.min_hours"
                        | "dream.min_sessions"
                        | "dream.max_turns"
                )
            {
                bail!(
                    "security policy: repo config only permits overriding `model`, `max_tokens`, and `dream.*` (omit --repo to save '{key}' globally)"
                );
            }
            let target_path = if repo {
                &loaded.paths.repo_config
            } else {
                &loaded.paths.global_config
            };
            let mut cfg = read_config_file(target_path)?;
            apply_config_key_value(&mut cfg, &key, &value)?;
            save_config_file(target_path, &cfg)?;
            println!("Saved {key} = {value} in {}", target_path.display());
        }
    }
    Ok(())
}

fn run_models_cli(loaded: &LoadedReinState, action: ModelsAction) -> Result<()> {
    match action {
        ModelsAction::Set { spec } => {
            let (kind, model_id) = resolve_model_target(&loaded.config, &spec);
            save_provider_selection(&loaded.paths.global_config, kind, Some(&model_id))?;
            println!(
                "Active model set to {}/{model_id} in {}",
                kind.as_str(),
                loaded.paths.global_config.display()
            );
        }
        ModelsAction::List { refresh, provider } => {
            let kind = provider.unwrap_or_else(|| loaded.config.resolved_provider());
            let (_, active_label) = build_provider(kind, &loaded.config);
            let mut models = if refresh {
                Vec::new()
            } else {
                load_cached_models(&loaded.paths.global_dir, kind)
            };
            if models.is_empty() {
                let (mut p, _) = build_provider(kind, &loaded.config);
                if let Ok(live) = p.list_models() {
                    if !live.is_empty() {
                        save_cached_models(&loaded.paths.global_dir, kind, &live);
                    }
                    models = live;
                }
            }
            println!("Provider: {} (active: {active_label})", kind.as_str());
            if models.is_empty() {
                println!("  (no models returned; check credentials with `rein auth status`)");
            } else {
                for m in models {
                    let quota = m
                        .quota_percent
                        .map_or_else(String::new, |pct| format!(" [{pct}% quota]"));
                    println!("  {:<36} {}{}", m.id, m.display_name, quota);
                }
            }
        }
    }
    Ok(())
}

fn run_auth_cli(loaded: &LoadedReinState, action: AuthAction) -> Result<()> {
    match action {
        AuthAction::Login => {
            let token_path = loaded
                .config
                .antigravity_token_path
                .clone()
                .unwrap_or_else(|| loaded.paths.global_dir.join("antigravity-oauth.json"));
            let result = login_with_browser(&token_path, |url| {
                println!("Opening Google Sign-In for Antigravity…");
                println!("If your browser did not open, visit:\n{url}\n");
            })?;
            save_provider_selection(&loaded.paths.global_config, ProviderKind::Antigravity, None)?;
            let who = result.email.as_deref().unwrap_or("Google account");
            println!(
                "Authenticated {who}; credentials saved to {} and provider set to `antigravity` in {}.",
                result.token_path.display(),
                loaded.paths.global_config.display(),
            );
        }
        AuthAction::Status => {
            let active = loaded.config.resolved_provider();
            let anth_cfg = loaded.config.to_anthropic_config();
            let anth_set = std::env::var_os(&anth_cfg.api_key_env).is_some();
            let agy_cfg = loaded.config.to_antigravity_config();
            let agy_ok = resolve_token(agy_cfg.token_path.as_deref()).is_ok();
            println!("Active provider: {}", active.as_str());
            println!(
                "  anthropic   ({}): {}",
                anth_cfg.api_key_env,
                if anth_set { "configured" } else { "not set" }
            );
            println!(
                "  antigravity (OAuth token): {}",
                if agy_ok {
                    "authenticated"
                } else {
                    "missing (run `rein auth login`)"
                }
            );
        }
    }
    Ok(())
}

fn run_sessions_cli(loaded: &LoadedReinState, action: SessionsAction) -> Result<()> {
    match action {
        SessionsAction::List => {
            let sessions = list_sessions(&loaded.paths)?;
            if sessions.is_empty() {
                println!(
                    "No saved sessions in {}",
                    loaded.paths.workspace_dir.display()
                );
            } else {
                println!(
                    "Saved sessions for {}:",
                    loaded.paths.workspace_dir.display()
                );
                for s in sessions {
                    let preview = s.preview.as_deref().unwrap_or("(empty session)");
                    println!("  {:<10} {:>3} turns  {}", s.uid, s.turn_count, preview);
                }
            }
        }
        SessionsAction::Show { uid } => {
            let (summary, found) = resolve_resume_session(&loaded.paths, Some(&uid))?;
            if !found {
                bail!("no saved session matching '{uid}'");
            }
            let sandbox = WorkspaceSandbox::new(&loaded.paths.workspace_dir);
            let memory =
                JsonlMemoryStore::dual(&loaded.paths.repo_memory, &loaded.paths.global_memory);
            let profile = Profile::coding();
            let policy = PolicyEngine::new(
                profile.policy_rules.clone(),
                sandbox,
                AuditLog::new(&loaded.paths.audit_log),
            );
            let mut agent = AgentLoop::new(
                EventLog::new(&summary.event_log),
                policy,
                rein_tools::ToolRegistry::new(),
                memory,
                profile,
            )
            .with_full_transcript_path(&summary.full_transcript);
            let events = agent.hydrate_structured()?;
            println!("Session {} ({} events):", summary.uid, events.len());
            for ev in events {
                match ev {
                    HydratedEvent::User(t) => println!("\n[User]\n{t}"),
                    HydratedEvent::Assistant(t) => println!("\n[Assistant]\n{t}"),
                    HydratedEvent::Tool { call, output } => {
                        let status = if output.is_error { "ERR" } else { "OK" };
                        println!("  -> tool: {} [{}]", call.name, status);
                    }
                    HydratedEvent::Note(n) => println!("  · {n}"),
                }
            }
        }
        SessionsAction::Delete { uid } => {
            let deleted = delete_session(&loaded.paths, &uid)?;
            println!("Deleted session {deleted}");
        }
    }
    Ok(())
}

fn run_allowlist_cli(loaded: &LoadedReinState, action: AllowlistAction) -> Result<()> {
    match action {
        AllowlistAction::List => {
            let global = read_command_list(&loaded.paths.global_commands)?;
            let repo = read_command_list(&loaded.paths.repo_commands)?;
            println!(
                "Global allowlist ({}):",
                loaded.paths.global_commands.display()
            );
            if global.is_empty() {
                println!("  (empty)");
            } else {
                for c in global {
                    println!("  - {c}");
                }
            }
            println!(
                "Workspace allowlist ({}):",
                loaded.paths.repo_commands.display()
            );
            if repo.is_empty() {
                println!("  (empty)");
            } else {
                for c in repo {
                    println!("  - {c}");
                }
            }
        }
        AllowlistAction::Add { pattern, global } => {
            let path = if global {
                &loaded.paths.global_commands
            } else {
                &loaded.paths.repo_commands
            };
            append_allowed_command(path, &pattern)?;
            println!("Added '{pattern}' to {}", path.display());
        }
        AllowlistAction::Remove { pattern, global } => {
            let path = if global {
                &loaded.paths.global_commands
            } else {
                &loaded.paths.repo_commands
            };
            if remove_allowed_command(path, &pattern)? {
                println!("Removed '{pattern}' from {}", path.display());
            } else {
                println!("Pattern '{pattern}' was not present in {}", path.display());
            }
        }
    }
    Ok(())
}

fn run_mcp_cli(loaded: &LoadedReinState, action: McpAction) -> Result<()> {
    match action {
        McpAction::List => {
            if loaded.config.mcp_servers.is_empty() {
                println!(
                    "No MCP servers configured in {}",
                    loaded.paths.global_config.display()
                );
            } else {
                for srv in &loaded.config.mcp_servers {
                    println!("  {:<20} {} {}", srv.name, srv.command, srv.args.join(" "));
                }
            }
        }
        McpAction::Add {
            name,
            command,
            args,
        } => {
            let mut cfg = read_config_file(&loaded.paths.global_config)?;
            cfg.mcp_servers.retain(|s| s.name != name);
            cfg.mcp_servers.push(McpServerConfig {
                name: name.clone(),
                command,
                args,
            });
            save_config_file(&loaded.paths.global_config, &cfg)?;
            println!(
                "Added MCP server '{name}' to {}",
                loaded.paths.global_config.display()
            );
        }
        McpAction::Remove { name } => {
            let mut cfg = read_config_file(&loaded.paths.global_config)?;
            let before = cfg.mcp_servers.len();
            cfg.mcp_servers.retain(|s| s.name != name);
            if cfg.mcp_servers.len() == before {
                println!("MCP server '{name}' not found");
            } else {
                save_config_file(&loaded.paths.global_config, &cfg)?;
                println!("Removed MCP server '{name}'");
            }
        }
    }
    Ok(())
}

fn run_plugins_cli(loaded: &LoadedReinState, action: PluginsAction) -> Result<()> {
    let mgr = PluginManager::new(loaded.paths.global_dir.join("plugins"));
    match action {
        PluginsAction::List => {
            let installed = mgr.installed_plugins();
            if installed.is_empty() {
                println!(
                    "No plugins installed (run `rein plugins refresh` then `rein plugins install <name>`)."
                );
            } else {
                for rec in installed {
                    let state = if rec.enabled { "enabled" } else { "disabled" };
                    println!(
                        "  {:<24} [{state}] ({}) {}",
                        rec.name, rec.marketplace, rec.description
                    );
                }
            }
        }
        PluginsAction::Marketplaces => {
            for mp in mgr.list_marketplaces() {
                println!("  {:<26} {} ({})", mp.name, mp.url, mp.description);
            }
        }
        PluginsAction::MarketAdd { spec } => {
            let entry = mgr.add_marketplace(&spec).map_err(|e| anyhow::anyhow!(e))?;
            println!("Added marketplace '{}' ({})", entry.name, entry.url);
        }
        PluginsAction::Refresh { name } => {
            let targets: Vec<String> = if let Some(n) = name {
                vec![n]
            } else {
                mgr.list_marketplaces()
                    .into_iter()
                    .map(|m| m.name)
                    .collect()
            };
            for t in targets {
                let count = mgr
                    .refresh_marketplace(&t)
                    .map_err(|e| anyhow::anyhow!(e))?;
                println!("Synced marketplace '{t}' ({count} plugins available)");
            }
        }
        PluginsAction::Install { name } => {
            let catalog = mgr.discover_catalog(None);
            let entry = catalog
                .into_iter()
                .find(|p| p.name == name)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "plugin '{name}' not found in local catalog (run `rein plugins refresh` first)"
                    )
                })?;
            let rec = mgr.install_plugin(&entry).map_err(|e| anyhow::anyhow!(e))?;
            println!(
                "Installed plugin '{}' to {}",
                rec.name,
                rec.install_path.display()
            );
        }
        PluginsAction::Toggle { name } => {
            let enabled = mgr
                .toggle_plugin_enabled(&name)
                .map_err(|e| anyhow::anyhow!(e))?;
            println!(
                "Plugin '{name}' is now {}",
                if enabled { "enabled" } else { "disabled" }
            );
        }
        PluginsAction::Uninstall { name } => {
            mgr.uninstall_plugin(&name)
                .map_err(|e| anyhow::anyhow!(e))?;
            println!("Uninstalled plugin '{name}'");
        }
    }
    Ok(())
}

fn run_doctor_cli(workspace_root: &Path, loaded: &LoadedReinState) -> Result<()> {
    let sandbox_status = probe_kernel_sandbox();
    let sandbox_label = format_sandbox_label(&sandbox_status);
    let active_kind = loaded.config.resolved_provider();
    let (_, model_label) = build_provider(active_kind, &loaded.config);
    let anth_cfg = loaded.config.to_anthropic_config();
    let anth_ok = std::env::var_os(&anth_cfg.api_key_env).is_some();
    let agy_cfg = loaded.config.to_antigravity_config();
    let agy_ok = resolve_token(agy_cfg.token_path.as_deref()).is_ok();
    let sessions_count = list_sessions(&loaded.paths).map_or(0, |s| s.len());
    let plugin_mgr = PluginManager::new(loaded.paths.global_dir.join("plugins"));
    let installed_plugins = plugin_mgr.installed_plugins().len();

    println!("Rein Doctor — Environment & Runtime Diagnostics");
    println!("  version:            {}", env!("CARGO_PKG_VERSION"));
    println!("  workspace:          {}", workspace_root.display());
    println!("  kernel sandbox:     {sandbox_label} ({sandbox_status:?})");
    println!("  active provider:    {}", active_kind.as_str());
    println!("  active model:       {model_label}");
    println!(
        "  permission mode:    {}",
        loaded.config.settings.default_permission_mode
    );
    println!(
        "  anthropic auth:     {}",
        if anth_ok {
            format!("ok ({})", anth_cfg.api_key_env)
        } else {
            format!("missing ({})", anth_cfg.api_key_env)
        }
    );
    println!(
        "  antigravity auth:   {}",
        if agy_ok {
            "ok (OAuth token valid)"
        } else {
            "missing"
        }
    );
    println!(
        "  allowed commands:   {} merged patterns",
        loaded.allowed_commands.len()
    );
    println!("  saved sessions:     {sessions_count} in current workspace");
    println!("  installed plugins:  {installed_plugins}");
    println!(
        "  mcp servers:        {} configured",
        loaded.config.mcp_servers.len()
    );
    for srv in &loaded.config.mcp_servers {
        let client = McpClient::new(srv.clone());
        match client.list_tools() {
            Ok(tools) => println!("    - {}: ok ({} tools)", srv.name, tools.len()),
            Err(err) => println!("    - {}: error ({err})", srv.name),
        }
    }
    Ok(())
}

struct HeadlessDenier;
impl ApprovalPrompter for HeadlessDenier {
    fn confirm(&mut self, _request: &ApprovalRequest) -> Result<ApprovalDecision> {
        Ok(ApprovalDecision {
            action: ApprovalAction::Deny,
            feedback: None,
        })
    }
}

fn run_replay_cli(
    replay_path: &Path,
    workspace_root: &Path,
    loaded: &LoadedReinState,
    bypass_permissions: bool,
) -> Result<()> {
    let sandbox = WorkspaceSandbox::new(workspace_root);
    let memory = JsonlMemoryStore::dual(&loaded.paths.repo_memory, &loaded.paths.global_memory);
    let todo_store = TodoStore::for_session(&loaded.paths.todos_dir, "replay");
    let task_registry = TaskRegistry::new();
    let mcp_catalog = McpCatalog::new(loaded.paths.sessions_dir.join("mcp-schemas"));
    let (registry, mcp_tools, _) = build_registry(
        loaded,
        &sandbox,
        memory.clone(),
        todo_store,
        task_registry,
        mcp_catalog,
    );
    let initial_profile = Profile::coding()
        .with_allowed_commands(loaded.allowed_commands.clone())
        .with_extra_tools(&mcp_tools);
    let policy = PolicyEngine::new(
        initial_profile.policy_rules.clone(),
        sandbox,
        AuditLog::new(&loaded.paths.audit_log),
    )
    .with_bypass_permissions(bypass_permissions);

    let events = EventLog::new(replay_path).read_all()?;
    let mut replay_provider = ReplayProvider::from_events(&events);
    let replay_out = EventLog::new(loaded.paths.repo_dir.join("replay_verify.jsonl"));
    let mut agent = AgentLoop::new(replay_out, policy, registry, memory, initial_profile);
    let mut prompter = HeadlessDenier;

    for ev in &events {
        if let SessionEvent::MessageRecorded { message } = ev
            && message.role == Role::User
            && message.content != "[tool_result_continuation]"
        {
            let reply =
                agent.run_user_prompt(&message.content, &mut replay_provider, &mut prompter)?;
            println!("[replay] user -> assistant: {reply}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_key_value_roundtrips_all_categories() -> Result<()> {
        let mut cfg = ReinConfig::default();
        apply_config_key_value(&mut cfg, "provider", "antigravity")?;
        assert_eq!(read_config_key(&cfg, "provider")?, "antigravity");

        apply_config_key_value(&mut cfg, "model", "gemini-2.5-pro")?;
        assert_eq!(read_config_key(&cfg, "model")?, "gemini-2.5-pro");

        apply_config_key_value(&mut cfg, "max_tokens", "8192")?;
        assert_eq!(read_config_key(&cfg, "max_tokens")?, "8192");

        apply_config_key_value(&mut cfg, "settings.thinking_mode", "false")?;
        assert_eq!(read_config_key(&cfg, "settings.thinking_mode")?, "false");

        apply_config_key_value(&mut cfg, "settings.default_permission_mode", "auto-bypass")?;
        assert_eq!(
            read_config_key(&cfg, "settings.default_permission_mode")?,
            "auto-bypass"
        );

        apply_config_key_value(&mut cfg, "dream.enabled", "false")?;
        apply_config_key_value(&mut cfg, "dream.min_hours", "12")?;
        assert_eq!(read_config_key(&cfg, "dream.enabled")?, "false");
        assert_eq!(read_config_key(&cfg, "dream.min_hours")?, "12");
        Ok(())
    }
}
