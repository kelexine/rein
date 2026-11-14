// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Single-binary entrypoint wiring re-exec child sandbox, headless replay, and Ratatui UI

//! Rein single binary entrypoint.
#![forbid(unsafe_code)]

/// In-process event-sourced agent loop.
pub mod agent;
/// Mode profiles, context strategy, and runtime switcher.
pub mod modes;
/// In-process Ratatui terminal interface.
pub mod ui;

use std::io::IsTerminal;
use std::path::Path;

use anyhow::{Context, Result};
use rein_core::{AnthropicConfig, AnthropicProvider, EventLog, ReplayProvider, Role, SessionEvent};
use rein_policy::{
    AuditLog, PolicyEngine, SANDBOX_CHILD_FLAG, WorkspaceSandbox, execute_sandbox_child,
};
use rein_tools::{
    CalendarTool, EditTool, EmailTool, GlobTool, GrepTool, JsonlMemoryStore, MemoryTool, NotesTool,
    ReadTool, ShellTool, ToolRegistry, WriteTool,
};

use crate::agent::{AgentLoop, ApprovalPrompter};
use crate::modes::Profile;
use crate::ui::{AppState, run_tui};

struct HeadlessDenier;
impl ApprovalPrompter for HeadlessDenier {
    fn confirm(&mut self, _prompt: &str) -> Result<bool> {
        Ok(false)
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
    let data_dir = workspace_root.join(".rein");
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("failed to create data dir '{}'", data_dir.display()))?;

    let sandbox = WorkspaceSandbox::new(&workspace_root);
    let initial_profile = Profile::coding();
    let policy = PolicyEngine::new(
        initial_profile.policy_rules.clone(),
        sandbox.clone(),
        AuditLog::new(data_dir.join("audit.jsonl")),
    );
    let sandbox_summary = format!("{:?}", policy.sandbox_status());
    let memory = JsonlMemoryStore::new(data_dir.join("memory.jsonl"));
    let registry = build_registry(&data_dir, &sandbox, memory.clone());

    if args.get(1).map(String::as_str) == Some("replay")
        && let Some(replay_path) = args.get(2)
    {
        return run_replay_cli(Path::new(replay_path), &data_dir, policy, registry, memory);
    }

    let event_log = EventLog::new(data_dir.join("session.jsonl"));
    let mut agent = AgentLoop::new(event_log, policy, registry, memory, initial_profile.clone());
    let mut provider = AnthropicProvider::new(AnthropicConfig::default());
    let mut app = AppState::new(initial_profile, sandbox_summary);
    app.push_line("Rein ready. Type a prompt or press Tab to switch Coding/Personal mode.");

    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        run_tui(&mut agent, &mut provider, &mut app)?;
    } else {
        println!(
            "Rein initialized ({} tools active in {:?} mode; run in a TTY for TUI or `rein replay <log.jsonl>`).",
            agent.active_profile().allowed_tools.len(),
            agent.active_profile().mode
        );
    }
    Ok(())
}

fn run_replay_cli(
    replay_path: &Path,
    data_dir: &Path,
    policy: PolicyEngine,
    registry: ToolRegistry,
    memory: JsonlMemoryStore,
) -> Result<()> {
    let events = EventLog::new(replay_path).read_all()?;
    let mut replay_provider = ReplayProvider::from_events(&events);
    let replay_out = EventLog::new(data_dir.join("replay_verify.jsonl"));
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
    data_dir: &Path,
    sandbox: &WorkspaceSandbox,
    memory: JsonlMemoryStore,
) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadTool::new(sandbox.clone())));
    registry.register(Box::new(WriteTool::new(sandbox.clone())));
    registry.register(Box::new(EditTool::new(sandbox.clone())));
    registry.register(Box::new(GlobTool::new(sandbox.clone())));
    registry.register(Box::new(GrepTool::new(sandbox.clone())));
    registry.register(Box::new(ShellTool::new(sandbox.clone())));
    registry.register(Box::new(MemoryTool::new(memory)));
    registry.register(Box::new(NotesTool::new(data_dir.join("notes.jsonl"))));
    registry.register(Box::new(CalendarTool::new(data_dir.join("calendar.jsonl"))));
    registry.register(Box::new(EmailTool::new(data_dir.join("email.jsonl"))));
    registry
}
