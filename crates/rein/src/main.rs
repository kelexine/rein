// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Single-binary CLI/TUI entrypoint delegating session bootstrap to rein::runtime

//! Rein single binary entrypoint.
#![forbid(unsafe_code)]

use std::io::IsTerminal;
use std::path::Path;

use anyhow::{Context, Result};
use rein_core::{EventLog, ReplayProvider, Role, SessionEvent, login_with_browser};
use rein_policy::{AuditLog, PolicyEngine, WorkspaceSandbox};
use rein_tools::{JsonlMemoryStore, McpCatalog, TaskRegistry, TodoStore};

use rein::agent::{AgentLoop, ApprovalAction, ApprovalDecision, ApprovalPrompter, ApprovalRequest};
use rein::config::{
    LoadedReinState, ProviderKind, ensure_dirs_and_load, list_sessions, save_provider_selection,
};
use rein::modes::Profile;
use rein::runtime::{ReinSession, build_registry, has_bypass_permissions_flag, parse_resume_flag};
use rein::ui::{AppState, run_tui};

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
    rein_policy::harden_process_dumpable();

    let workspace_root = std::env::current_dir().context("failed to resolve workspace root")?;
    let loaded = ensure_dirs_and_load(&workspace_root)?;

    if args.get(1).map(String::as_str) == Some("login") {
        return run_login_cli(&loaded);
    }

    if args.get(1).map(String::as_str) == Some("replay")
        && let Some(replay_path) = args.get(2)
    {
        return run_replay_cli(
            Path::new(replay_path),
            &workspace_root,
            &loaded,
            has_bypass_permissions_flag(&args),
        );
    }

    if let Err(msg) = parse_resume_flag(&args) {
        print_resume_usage_help(&msg, &loaded.paths);
        std::process::exit(2);
    }

    let session = ReinSession::bootstrap(&workspace_root, &args)?;
    run_interactive_tui(session)
}

fn run_interactive_tui(mut session: ReinSession) -> Result<()> {
    let bypass_permissions = session.agent.policy().is_bypass_permissions();
    let mut app = AppState::new(
        session.agent.active_profile().clone(),
        session.sandbox_label,
        session.model_label,
    )
    .with_bypass_permissions(bypass_permissions)
    .with_subagent_tracker(session.subagent_tracker)
    .with_subagent_manager(session.subagent_manager)
    .with_swarm_manager(session.swarm_manager)
    .with_question_relay(session.question_relay)
    .with_task_registry(session.task_registry)
    .with_task_notifications(session.task_rx)
    .with_workspace_label(session.workspace_label)
    .with_session_id(&session.session_uid)
    .with_startup_notices(session.notices)
    .with_slash_registry(session.slash_registry)
    .with_configured_commands(session.loaded.allowed_commands.clone())
    .with_mcp_servers(session.loaded.config.mcp_servers.clone())
    .with_todo_store(session.todo_store)
    .with_ui_settings(
        session.loaded.config.settings.clone(),
        session.repo_overrides,
    )
    .with_plugin_manager(session.plugin_manager);

    if session.should_hydrate {
        app.extend_hydrated(session.agent.hydrate_structured()?);
        app.sync_todo_session(&session.session_uid);
        app.set_active_profile(session.agent.active_profile().clone());
        app.push_system_note(format!("resumed session {}", session.session_uid));
    }

    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        run_tui(
            &mut session.agent,
            &mut session.provider,
            &mut session.loaded.config,
            &session.loaded.paths,
            &mut app,
        )?;
        let active_uid = app.session_id();
        if !active_uid.is_empty()
            && session
                .loaded
                .paths
                .session_paths_for(active_uid)
                .0
                .exists()
        {
            println!("To continue this session run rein --resume {active_uid}");
        }
    } else {
        println!(
            "Rein initialized ({} tools active in {:?} mode; provider={}; global={}, repo={}).",
            session.agent.active_profile().allowed_tools.len(),
            session.agent.active_profile().mode,
            session.provider_kind.as_str(),
            session.loaded.paths.global_dir.display(),
            session.loaded.paths.repo_dir.display(),
        );
    }
    Ok(())
}

fn print_resume_usage_help(msg: &anyhow::Error, paths: &rein::config::ReinPaths) {
    eprintln!("warning: {msg}");
    eprintln!("usage: rein --resume <uid>");
    if let Ok(sessions) = list_sessions(paths) {
        if sessions.is_empty() {
            eprintln!(
                "no saved sessions in {}; run `rein` to start a new session.",
                paths.workspace_dir.display()
            );
        } else {
            eprintln!("saved sessions in {}:", paths.workspace_dir.display());
            for s in sessions.iter().take(5) {
                let preview = s.preview.as_deref().unwrap_or("(empty session)");
                eprintln!("  {:<10} {} ({} turns)", s.uid, preview, s.turn_count);
            }
        }
    }
}

fn run_login_cli(loaded: &LoadedReinState) -> Result<()> {
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
    Ok(())
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
