// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Single-binary entrypoint wiring ~/.rein + .rein dual scopes, MCP discovery, --resume, and borderless TUI

//! Rein single binary entrypoint.
#![forbid(unsafe_code)]

/// In-process event-sourced agent loop.
pub mod agent;
/// Built-in slash commands and `~/.rein/skills` + `<workspace>/.rein/skills` loader.
pub mod commands;
/// Dual-scope (`~/.rein` and `<workspace>/.rein`) config and command allowlist loader.
pub mod config;
/// Mode profiles, context strategy, and runtime switcher.
pub mod modes;
/// In-process Ratatui terminal interface.
pub mod ui;

use std::io::IsTerminal;
use std::path::Path;

use anyhow::{Context, Result};
use rein_core::{AnthropicProvider, EventLog, ReplayProvider, Role, SessionEvent};
use rein_policy::{
    AuditLog, PolicyEngine, SANDBOX_CHILD_FLAG, SandboxStatus, WorkspaceSandbox,
    execute_sandbox_child,
};
use rein_tools::{
    BashTool, CalendarTool, EditTool, EmailTool, GlobTool, GrepTool, JsonlMemoryStore, McpClient,
    McpToolAdapter, MemoryTool, NotesTool, ReadTool, ToolRegistry, WriteTool,
};

use crate::agent::{
    AgentLoop, ApprovalAction, ApprovalDecision, ApprovalPrompter, ApprovalRequest,
};
use crate::config::{LoadedReinState, ensure_dirs_and_load};
use crate::modes::Profile;
use crate::ui::{AppState, run_tui};

struct HeadlessDenier;
impl ApprovalPrompter for HeadlessDenier {
    fn confirm(&mut self, _request: &ApprovalRequest) -> Result<ApprovalDecision> {
        Ok(ApprovalDecision {
            action: ApprovalAction::Deny,
            feedback: None,
        })
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some(SANDBOX_CHILD_FLAG)
        && let Some(raw_spec) = args.get(2)
    {
        let code = execute_sandbox_child(raw_spec)?;
        std::process::exit(code);
    }

    let workspace_root = std::env::current_dir().context("failed to resolve workspace root")?;
    let loaded = ensure_dirs_and_load(&workspace_root)?;
    let sandbox = WorkspaceSandbox::new(&workspace_root);
    let memory = JsonlMemoryStore::dual(&loaded.paths.repo_memory, &loaded.paths.global_memory);
    let (registry, mcp_tools, mcp_warnings) = build_registry(&loaded, &sandbox, memory.clone());

    let initial_profile = Profile::coding()
        .with_allowed_commands(loaded.allowed_commands.clone())
        .with_extra_tools(&mcp_tools);
    let policy = PolicyEngine::new(
        initial_profile.policy_rules.clone(),
        sandbox,
        AuditLog::new(&loaded.paths.audit_log),
    );

    if args.get(1).map(String::as_str) == Some("replay")
        && let Some(replay_path) = args.get(2)
    {
        return run_replay_cli(
            Path::new(replay_path),
            &loaded.paths.repo_dir,
            policy,
            registry,
            memory,
        );
    }

    let resume = args.iter().any(|arg| arg == "--resume");
    run_interactive(
        &workspace_root,
        loaded,
        policy,
        registry,
        memory,
        initial_profile,
        mcp_tools,
        mcp_warnings,
        resume,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_interactive(
    workspace_root: &Path,
    loaded: LoadedReinState,
    policy: PolicyEngine,
    registry: ToolRegistry,
    memory: JsonlMemoryStore,
    initial_profile: Profile,
    mcp_tools: Vec<String>,
    mcp_warnings: Vec<String>,
    resume: bool,
) -> Result<()> {
    let sandbox_label = format_sandbox_label(policy.sandbox_status());
    let anthropic_cfg = loaded.config.to_anthropic_config();
    let model_label = anthropic_cfg.model.clone();
    let mut notices = Vec::new();
    if std::env::var_os(&anthropic_cfg.api_key_env).is_none() {
        notices.push(format!(
            "{} not set · export it before sending prompts",
            anthropic_cfg.api_key_env
        ));
    }
    notices.extend(mcp_warnings);

    let slash_registry = crate::commands::load_slash_registry(&loaded.paths)?;
    let event_log = EventLog::new(&loaded.paths.session_log);

    let mut agent = AgentLoop::new(event_log, policy, registry, memory, initial_profile)
        .with_full_transcript_path(&loaded.paths.full_transcript)
        .with_command_allowlist_paths(&loaded.paths.repo_commands, &loaded.paths.global_commands)
        .with_allowed_commands(loaded.allowed_commands.clone())
        .with_extra_tools(mcp_tools);
    let mut provider = AnthropicProvider::new(anthropic_cfg);
    let mut app = AppState::new(agent.active_profile().clone(), sandbox_label, model_label)
        .with_workspace_label(collapse_home_path(workspace_root))
        .with_startup_notices(notices)
        .with_slash_registry(slash_registry)
        .with_configured_commands(loaded.allowed_commands)
        .with_mcp_servers(loaded.config.mcp_servers);

    if resume {
        app.extend_transcript(agent.hydrate_from_log()?);
        app.set_active_profile(agent.active_profile().clone());
    }

    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        run_tui(&mut agent, &mut provider, &mut app)?;
    } else {
        println!(
            "Rein initialized ({} tools active in {:?} mode; global={}, repo={}).",
            agent.active_profile().allowed_tools.len(),
            agent.active_profile().mode,
            loaded.paths.global_dir.display(),
            loaded.paths.repo_dir.display(),
        );
    }
    Ok(())
}

fn run_replay_cli(
    replay_path: &Path,
    repo_dir: &Path,
    policy: PolicyEngine,
    registry: ToolRegistry,
    memory: JsonlMemoryStore,
) -> Result<()> {
    let events = EventLog::new(replay_path).read_all()?;
    let mut replay_provider = ReplayProvider::from_events(&events);
    let replay_out = EventLog::new(repo_dir.join("replay_verify.jsonl"));
    let mut agent = AgentLoop::new(replay_out, policy, registry, memory, Profile::coding());
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

fn build_registry(
    loaded: &LoadedReinState,
    sandbox: &WorkspaceSandbox,
    memory: JsonlMemoryStore,
) -> (ToolRegistry, Vec<String>, Vec<String>) {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadTool::new(sandbox.clone())));
    registry.register(Box::new(WriteTool::new(sandbox.clone())));
    registry.register(Box::new(EditTool::new(sandbox.clone())));
    registry.register(Box::new(GlobTool::new(sandbox.clone())));
    registry.register(Box::new(GrepTool::new(sandbox.clone())));
    registry.register(Box::new(
        BashTool::new(sandbox.clone()).with_output_dir(loaded.paths.repo_dir.join("bash-outputs")),
    ));
    registry.register(Box::new(MemoryTool::new(memory)));
    registry.register(Box::new(NotesTool::new(&loaded.paths.notes)));

    let mut calendar = CalendarTool::new(&loaded.paths.calendar);
    if let Some(ics_dir) = &loaded.config.ics_dir {
        calendar = calendar.with_ics_dir(ics_dir);
    }
    registry.register(Box::new(calendar));

    let mut email = EmailTool::new(&loaded.paths.email);
    if let Some(maildir) = &loaded.config.maildir {
        email = email.with_maildir(maildir);
    }
    registry.register(Box::new(email));

    let mut mcp_names = Vec::new();
    let mut warnings = Vec::new();
    for server in &loaded.config.mcp_servers {
        let client = McpClient::new(server.clone());
        match client.list_tools() {
            Ok(schemas) => {
                for schema in schemas {
                    mcp_names.push(schema.name.clone());
                    registry.register(Box::new(McpToolAdapter::new(client.clone(), schema)));
                }
            }
            Err(err) => warnings.push(format!("mcp '{}': {err}", server.name)),
        }
    }
    (registry, mcp_names, warnings)
}

fn format_sandbox_label(status: &SandboxStatus) -> String {
    match status {
        SandboxStatus::Enforced { abi_version } => format!("landlock v{abi_version}"),
        SandboxStatus::Unavailable { .. } => "sandbox off".to_owned(),
    }
}

fn collapse_home_path(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME")
        && let Ok(rel) = path.strip_prefix(Path::new(&home))
    {
        return if rel.as_os_str().is_empty() {
            "~".to_owned()
        } else {
            format!("~/{}", rel.display())
        };
    }
    path.display().to_string()
}
