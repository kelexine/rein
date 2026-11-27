// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Single-binary CLI/TUI entrypoint delegating session bootstrap to rein::runtime

//! Rein single binary entrypoint.
#![forbid(unsafe_code)]

use std::io::IsTerminal;

use anyhow::{Context, Result};

use rein::cli::{execute_cli_command, parse_cli_args};
use rein::config::{build_provider, ensure_dirs_and_load, list_sessions, save_cached_models};
use rein::runtime::{ReinSession, parse_resume_flag};
use rein::ui::{AppState, run_tui};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    rein_policy::harden_process_dumpable();

    let workspace_root = std::env::current_dir().context("failed to resolve workspace root")?;
    let loaded = ensure_dirs_and_load(&workspace_root)?;

    let cli_cmd = parse_cli_args(&args)?;
    if execute_cli_command(cli_cmd, &workspace_root, &loaded)? {
        return Ok(());
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
    let app = AppState::new(
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
    .with_plugin_manager(session.plugin_manager)
    .with_cached_models(session.cached_models);

    let (refresh_tx, refresh_rx) = std::sync::mpsc::channel();
    let bg_kind = session.provider_kind;
    let bg_cfg = session.loaded.config.clone();
    let bg_global_dir = session.loaded.paths.global_dir.clone();
    std::thread::spawn(move || {
        let (mut probe, _) = build_provider(bg_kind, &bg_cfg);
        if let Ok(models) = probe.list_models()
            && !models.is_empty()
        {
            save_cached_models(&bg_global_dir, bg_kind, &models);
            let _ = refresh_tx.send(models);
        }
    });
    let mut app = app.with_model_refresh_rx(refresh_rx);

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
