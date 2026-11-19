// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Crossterm raw-mode guard, slash & /model dispatcher, Ctrl+O toggle, and non-blocking Ratatui loop

use std::io::{self, Stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use rein_core::{
    CoreError, EventLog, MemoryScope, Provider, ToolCall, ToolOutput, UsageStats,
    login_with_browser_until, logout_rein_token,
};
use rein_tools::TaskNotification;

use crate::agent::{
    AgentLoop, ApprovalAction, ApprovalDecision, ApprovalPrompter, ApprovalRequest,
};
use crate::commands::{BuiltinAction, SlashKind, SlashResolution};
use crate::config::{
    ProviderKind, ReinConfig, ReinPaths, apply_provider_switch, build_provider,
    generate_session_uid, list_sessions, resolve_model_target, save_provider_selection,
    write_latest_session_uid,
};
use crate::memory::AutoDreamCoordinator;
use crate::modes::Profile;
use crate::ui::view::{compute_max_scroll, transcript_viewport};
use crate::ui::{AppState, render_frame};

type TuiTerminal = Terminal<CrosstermBackend<Stdout>>;

struct TerminalGuard {
    terminal: TuiTerminal,
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
    }
}

enum WorkerEvent {
    ThinkingDelta(String),
    Delta(String),
    ToolStart(ToolCall),
    ToolFinish(ToolCall, ToolOutput),
    Usage(UsageStats),
    Quota(u8),
    ModeSwitched(Box<Profile>),
    AskConfirm(ApprovalRequest),
    Note(String),
    Finished(Result<()>),
}

struct ChannelPrompter {
    tx: Sender<WorkerEvent>,
    confirm_rx: Receiver<ApprovalDecision>,
}

impl ApprovalPrompter for ChannelPrompter {
    fn confirm(&mut self, request: &ApprovalRequest) -> Result<ApprovalDecision> {
        let _ = self.tx.send(WorkerEvent::AskConfirm(request.clone()));
        Ok(self.confirm_rx.recv().unwrap_or(ApprovalDecision {
            action: ApprovalAction::Deny,
            feedback: None,
        }))
    }

    fn on_thinking_delta(&mut self, delta: &str) {
        let _ = self.tx.send(WorkerEvent::ThinkingDelta(delta.to_owned()));
    }

    fn on_delta(&mut self, delta: &str) {
        let _ = self.tx.send(WorkerEvent::Delta(delta.to_owned()));
    }

    fn on_tool_start(&mut self, call: &ToolCall) {
        let _ = self.tx.send(WorkerEvent::ToolStart(call.clone()));
    }

    fn on_tool_finish(&mut self, call: &ToolCall, output: &ToolOutput) {
        let _ = self
            .tx
            .send(WorkerEvent::ToolFinish(call.clone(), output.clone()));
    }

    fn on_usage(&mut self, usage: UsageStats) {
        let _ = self.tx.send(WorkerEvent::Usage(usage));
    }

    fn on_mode_switched(&mut self, profile: &Profile) {
        let _ = self
            .tx
            .send(WorkerEvent::ModeSwitched(Box::new(profile.clone())));
    }
}

/// Runs the minimalist Ratatui loop in-process until the user exits with `/exit`, `/quit`, or consecutive `Ctrl+C` twice within 2s.
pub fn run_tui(
    agent: &mut AgentLoop,
    provider: &mut Box<dyn Provider>,
    config: &mut ReinConfig,
    paths: &ReinPaths,
    app: &mut AppState,
) -> Result<()> {
    let mut guard = TerminalGuard::enter()?;
    let auto_dream = AutoDreamCoordinator::new();
    loop {
        auto_dream.maybe_spawn(config, paths, agent.memory_store());
        if app.pending_approval().is_none()
            && app.pending_model_picker().is_none()
            && app.pending_session_picker().is_none()
            && app.pending_rewind_picker().is_none()
            && let Some((display, prompt)) = collect_wakeup_batch(app)
            && stream_prompt_background(display, prompt, agent, provider.as_mut(), app, &mut guard)?
        {
            break;
        }
        guard.terminal.draw(|f| render_frame(app, f))?;
        if !event::poll(Duration::from_millis(50))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) => {
                if handle_idle_key(key, agent, provider, config, paths, app, &mut guard)? {
                    break;
                }
            }
            Event::Paste(text) => {
                app.clear_ctrl_c_arm();
                if app.pending_model_picker().is_none()
                    && app.pending_session_picker().is_none()
                    && app.pending_task_manager().is_none()
                    && app.pending_task_details().is_none()
                    && app.pending_approval().is_none()
                {
                    app.clear_strip_selection();
                    app.push_input_str(&text);
                }
            }
            _ => {}
        }
    }
    app.subagent_tracker().cancel_all();
    app.task_registry().shutdown_all();
    Ok(())
}

fn handle_idle_key(
    key: KeyEvent,
    agent: &mut AgentLoop,
    provider: &mut Box<dyn Provider>,
    config: &mut ReinConfig,
    paths: &ReinPaths,
    app: &mut AppState,
    guard: &mut TerminalGuard,
) -> Result<bool> {
    if is_ctrl_c(key) {
        return Ok(app.arm_or_confirm_ctrl_c(Instant::now()));
    }
    app.clear_ctrl_c_arm();
    if is_ctrl_o(key) {
        let _ = app.toggle_expand_outputs();
        return Ok(false);
    }
    if app.pending_task_details().is_some() {
        return handle_task_details_key(key, app, guard);
    }
    if app.pending_task_manager().is_some() {
        return handle_task_manager_key(key, app);
    }
    if is_ctrl_t(key) {
        app.open_task_manager();
        return Ok(false);
    }
    if app.pending_model_picker().is_some() {
        return handle_model_picker_key(key, provider, config, paths, app);
    }
    if app.pending_session_picker().is_some() {
        return handle_session_picker_key(key, agent, paths, app);
    }
    if app.pending_rewind_picker().is_some() {
        return handle_rewind_picker_key(key, agent, app);
    }
    if app.strip_selected().is_some() {
        match key.code {
            KeyCode::Up => {
                let _ = app.move_strip_selection(-1);
                return Ok(false);
            }
            KeyCode::Down => {
                let _ = app.move_strip_selection(1);
                return Ok(false);
            }
            KeyCode::Enter => {
                let _ = app.open_selected_strip_details();
                return Ok(false);
            }
            KeyCode::Esc => {
                app.clear_strip_selection();
                return Ok(false);
            }
            _ => {
                app.clear_strip_selection();
            }
        }
    }
    if key.code == KeyCode::Esc {
        return Ok(false);
    }

    let slash_open = !app.slash_matches().is_empty();
    if is_newline_key(key) {
        app.push_input_char('\n');
        return Ok(false);
    }
    match key.code {
        KeyCode::Tab if slash_open => {
            if let Some(sel) = app.selected_slash() {
                let completed = format!("/{} ", sel.name);
                app.set_input(completed);
            }
        }
        KeyCode::BackTab => {
            let _ = agent.toggle_mode()?;
            app.set_active_profile(agent.active_profile().clone());
        }
        KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
            let _ = agent.toggle_mode()?;
            app.set_active_profile(agent.active_profile().clone());
        }
        KeyCode::Up if slash_open => app.move_slash_selection(-1),
        KeyCode::Down if slash_open => app.move_slash_selection(1),
        KeyCode::Up => {
            if !app.move_cursor_vertical(-1) {
                app.navigate_history(-1);
            }
        }
        KeyCode::Down => {
            if !app.move_cursor_vertical(1) {
                if app.history_cursor_active() {
                    app.navigate_history(1);
                } else if !app.running_strip_items().is_empty() {
                    let _ = app.move_strip_selection(1);
                } else {
                    app.navigate_history(1);
                }
            }
        }
        KeyCode::Left => app.move_cursor_left(),
        KeyCode::Right => app.move_cursor_right(),
        KeyCode::Home => app.move_cursor_home(),
        KeyCode::End => app.move_cursor_end(),
        KeyCode::PageUp => scroll_page(app, guard, -1)?,
        KeyCode::PageDown => scroll_page(app, guard, 1)?,
        KeyCode::Backspace => app.pop_input_char(),
        KeyCode::Delete => app.delete_input_char(),
        KeyCode::Char(c) => app.push_input_char(c),
        KeyCode::Enter => return handle_enter(agent, provider, config, paths, app, guard),
        _ => {}
    }
    Ok(false)
}

fn handle_model_picker_key(
    key: KeyEvent,
    provider: &mut Box<dyn Provider>,
    config: &mut ReinConfig,
    paths: &ReinPaths,
    app: &mut AppState,
) -> Result<bool> {
    match key.code {
        KeyCode::Esc => app.clear_model_picker(),
        KeyCode::Up => app.move_model_selection(-1),
        KeyCode::Down => app.move_model_selection(1),
        KeyCode::Enter => {
            if let Some((kind, chosen)) = app.take_selected_model() {
                switch_and_persist_provider(
                    provider,
                    config,
                    paths,
                    app,
                    kind,
                    Some(&chosen.id),
                    chosen.quota_percent,
                );
            }
        }
        _ => {}
    }
    Ok(false)
}

fn handle_session_picker_key(
    key: KeyEvent,
    agent: &mut AgentLoop,
    paths: &ReinPaths,
    app: &mut AppState,
) -> Result<bool> {
    match key.code {
        KeyCode::Esc => app.clear_session_picker(),
        KeyCode::Up => app.move_session_selection(-1),
        KeyCode::Down => app.move_session_selection(1),
        KeyCode::Enter => {
            if let Some(chosen) = app.take_selected_session() {
                agent.switch_session(EventLog::new(&chosen.event_log), chosen.full_transcript);
                let _ = write_latest_session_uid(paths, &chosen.uid);
                let items = agent.hydrate_structured()?;
                app.clear_transcript();
                app.set_session_id(&chosen.uid);
                app.extend_hydrated(items);
                app.sync_todo_session(&chosen.uid);
                app.set_active_profile(agent.active_profile().clone());
                app.push_system_note(format!("resumed session {}", chosen.uid));
            }
        }
        _ => {}
    }
    Ok(false)
}

fn handle_rewind_picker_key(
    key: KeyEvent,
    agent: &mut AgentLoop,
    app: &mut AppState,
) -> Result<bool> {
    match key.code {
        KeyCode::Esc => app.cancel_rewind_step(),
        KeyCode::Up => app.move_rewind_selection(-1),
        KeyCode::Down => app.move_rewind_selection(1),
        KeyCode::Enter => {
            if let Some((turn_index, restore_code)) = app.confirm_rewind_step() {
                match agent.rewind_to(turn_index, restore_code) {
                    Ok(cp) => {
                        app.truncate_entries_to_user_turn(turn_index);
                        let scope = if restore_code {
                            "messages and code"
                        } else {
                            "messages only"
                        };
                        app.push_system_note(format!(
                            "rewound {scope} to turn {}: {}",
                            turn_index + 1,
                            cp.prompt_preview
                        ));
                    }
                    Err(err) => app.push_error_note(format!("rewind failed: {err}")),
                }
            }
        }
        _ => {}
    }
    Ok(false)
}

fn handle_enter(
    agent: &mut AgentLoop,
    provider: &mut Box<dyn Provider>,
    config: &mut ReinConfig,
    paths: &ReinPaths,
    app: &mut AppState,
    guard: &mut TerminalGuard,
) -> Result<bool> {
    let raw = if let Some(sel) = app.selected_slash() {
        let sel_name = sel.name.clone();
        let typed = app.take_input();
        let token = typed.trim().trim_start_matches('/');
        if token.is_empty()
            || matches!(
                app.slash_registry().resolve(&typed),
                SlashResolution::Unknown(_)
            )
        {
            format!("/{sel_name}")
        } else {
            typed
        }
    } else {
        app.take_input()
    };

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(false);
    }
    if trimmed.starts_with('/') && !trimmed.contains('\n') {
        return dispatch_slash(trimmed, agent, provider, config, paths, app, guard);
    }
    app.task_registry().notify_external_event("user");
    stream_prompt_background(
        trimmed.to_owned(),
        trimmed.to_owned(),
        agent,
        provider.as_mut(),
        app,
        guard,
    )
}

fn dispatch_slash(
    raw: &str,
    agent: &mut AgentLoop,
    provider: &mut Box<dyn Provider>,
    config: &mut ReinConfig,
    paths: &ReinPaths,
    app: &mut AppState,
    guard: &mut TerminalGuard,
) -> Result<bool> {
    let registry = app.slash_registry().clone();
    match registry.resolve(raw) {
        SlashResolution::Unknown(name) => {
            app.push_error_note(format!("unknown command: /{name} (try /help)"));
            Ok(false)
        }
        SlashResolution::Matched { command, args } => match &command.kind {
            SlashKind::Builtin(action) => {
                run_builtin_action(*action, &args, agent, provider, config, paths, app, guard)
            }
            SlashKind::Skill { .. } => {
                let prompt = command
                    .expand_skill_prompt(&args)
                    .unwrap_or_else(|| raw.to_owned());
                stream_prompt_background(
                    raw.to_owned(),
                    prompt,
                    agent,
                    provider.as_mut(),
                    app,
                    guard,
                )
            }
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn run_builtin_action(
    action: BuiltinAction,
    args: &str,
    agent: &mut AgentLoop,
    provider: &mut Box<dyn Provider>,
    config: &mut ReinConfig,
    paths: &ReinPaths,
    app: &mut AppState,
    guard: &mut TerminalGuard,
) -> Result<bool> {
    match action {
        BuiltinAction::Quit => return Ok(true),
        BuiltinAction::Clear => {
            let new_uid = generate_session_uid(paths);
            let (log_path, full_path) = paths.session_paths_for(&new_uid);
            agent.switch_session(EventLog::new(log_path), full_path);
            let _ = write_latest_session_uid(paths, &new_uid);
            app.clear_transcript();
            app.set_session_id(&new_uid);
            app.sync_todo_session(&new_uid);
            app.push_system_note(format!("started new session {new_uid}"));
        }
        BuiltinAction::Help => {
            let lines: Vec<String> = app
                .slash_registry()
                .all()
                .iter()
                .map(|c| match c.source.badge() {
                    Some(b) => format!("/{:<14} {} ({b})", c.name, c.description),
                    None => format!("/{:<14} {}", c.name, c.description),
                })
                .collect();
            for line in lines {
                app.push_system_note(line);
            }
            app.push_system_note(
                "shortcuts: ctrl+o expand/collapse · shift+tab mode · shift+enter newline · esc stop · ctrl+c x2 exit",
            );
        }
        BuiltinAction::Compact => {
            return compact_background(args, agent, provider.as_mut(), app, guard);
        }
        BuiltinAction::Memory => run_memory_command(args, agent, app)?,
        BuiltinAction::Commands => {
            if app.configured_commands().is_empty() {
                app.push_system_note("commands.json allowlist: (none)");
            } else {
                app.push_system_note(format!(
                    "commands.json allowlist: {}",
                    app.configured_commands().join(", ")
                ));
            }
        }
        BuiltinAction::Mcp => run_mcp_command(agent, app),
        BuiltinAction::Login => return run_login_background(provider, config, paths, app, guard),
        BuiltinAction::Logout => {
            match logout_rein_token() {
                Ok(Some(removed)) => {
                    app.push_system_note(format!("signed out (removed {})", removed.display()));
                }
                Ok(None) => {
                    app.push_system_note("signed out (no saved Rein OAuth token found)");
                }
                Err(err) => {
                    app.push_system_note(format!("logout error: {err}"));
                }
            }
            let (fresh_provider, label) = build_provider(config.resolved_provider(), config);
            *provider = fresh_provider;
            app.set_model_name(label);
            app.set_model_quota(None);
        }
        BuiltinAction::Model => {
            return run_model_command(args, provider, config, paths, app, guard);
        }
        BuiltinAction::Provider => run_provider_command(args, provider, config, paths, app)?,
        BuiltinAction::Resume => {
            let sessions = list_sessions(paths)?;
            if sessions.is_empty() {
                app.push_system_note("no saved sessions in this directory yet");
            } else {
                app.open_session_picker(sessions);
            }
        }
        BuiltinAction::Rewind => {
            let checkpoints = agent.checkpoints().to_vec();
            if checkpoints.is_empty() {
                app.push_system_note("no turns to rewind in this session yet");
            } else {
                app.open_rewind_picker(checkpoints);
            }
        }
        BuiltinAction::Tasks => app.open_task_manager(),
    }
    Ok(false)
}

fn run_model_command(
    args: &str,
    provider: &mut Box<dyn Provider>,
    config: &mut ReinConfig,
    paths: &ReinPaths,
    app: &mut AppState,
    guard: &mut TerminalGuard,
) -> Result<bool> {
    let trimmed = args.trim();
    let target_kind = if trimmed.is_empty() {
        Some(config.resolved_provider())
    } else {
        ProviderKind::parse(trimmed)
    };

    if let Some(kind) = target_kind {
        app.set_busy_status("Fetching models…");
        app.set_busy(true);
        guard.terminal.draw(|f| render_frame(app, f))?;
        let fetched = if kind == config.resolved_provider() {
            provider.list_models()
        } else {
            let (mut probe, _) = build_provider(kind, config);
            probe.list_models()
        };
        app.set_busy(false);

        match fetched {
            Ok(models) if !models.is_empty() => {
                if kind == config.resolved_provider() {
                    app.set_model_quota(provider.active_model_quota());
                }
                let current_model = app.model_name().to_owned();
                app.open_model_picker(models, kind, &current_model);
            }
            Ok(_) => {
                app.push_system_note("server returned no models; pass `/model <id>` directly");
            }
            Err(err) => {
                app.push_error_note(format!("failed to fetch models from server: {err}"));
            }
        }
        return Ok(false);
    }

    let (kind, model_id) = resolve_model_target(config, trimmed);
    switch_and_persist_provider(provider, config, paths, app, kind, Some(&model_id), None);
    if let Ok(models) = provider.list_models()
        && let Some(matched) = models.iter().find(|m| m.id == model_id)
    {
        app.set_model_quota(matched.quota_percent);
    }
    Ok(false)
}

fn run_provider_command(
    args: &str,
    provider: &mut Box<dyn Provider>,
    config: &mut ReinConfig,
    paths: &ReinPaths,
    app: &mut AppState,
) -> Result<()> {
    let mut parts = args.split_whitespace();
    let Some(first) = parts.next() else {
        app.push_system_note(format!("provider: {}", app.model_name()));
        return Ok(());
    };
    let explicit_model = parts.next();
    let Some(kind) = ProviderKind::parse(first) else {
        app.push_error_note("usage: /provider [anthropic|antigravity] [model]");
        return Ok(());
    };
    switch_and_persist_provider(provider, config, paths, app, kind, explicit_model, None);
    Ok(())
}

fn switch_and_persist_provider(
    provider: &mut Box<dyn Provider>,
    config: &mut ReinConfig,
    paths: &ReinPaths,
    app: &mut AppState,
    kind: ProviderKind,
    explicit_model: Option<&str>,
    quota: Option<u8>,
) {
    apply_provider_switch(config, kind, explicit_model);
    let (next_provider, label) = build_provider(kind, config);
    *provider = next_provider;
    app.set_model_name(&label);
    app.set_model_quota(quota);
    let _ = save_provider_selection(&paths.global_config, kind, config.model.as_deref());
    app.push_system_note(format!("switched model: {label}"));
}

fn run_login_background(
    provider: &mut Box<dyn Provider>,
    config: &mut ReinConfig,
    paths: &ReinPaths,
    app: &mut AppState,
    guard: &mut TerminalGuard,
) -> Result<bool> {
    let token_path = config
        .antigravity_token_path
        .clone()
        .unwrap_or_else(|| paths.global_dir.join("antigravity-oauth.json"));
    app.push_system_note("launching Google Antigravity OAuth login in browser…");
    app.set_busy_status("Waiting for OAuth login…");
    app.set_busy(true);
    guard.terminal.draw(|f| render_frame(app, f))?;

    let (ev_tx, ev_rx) = mpsc::channel::<WorkerEvent>();
    let cancel = AtomicBool::new(false);
    let login_ok = AtomicBool::new(false);
    let mut should_quit = false;
    let mut last_spin = Instant::now();

    std::thread::scope(|scope| {
        let cancel_ref = &cancel;
        let login_ok_ref = &login_ok;
        let token_path_ref = &token_path;
        let ev_tx_worker = ev_tx.clone();
        scope.spawn(move || {
            let res = login_with_browser_until(
                token_path_ref,
                |url| {
                    let _ = ev_tx_worker.send(WorkerEvent::Note(format!(
                        "open URL if browser did not open: {url}"
                    )));
                },
                cancel_ref,
            );
            let status = match res {
                Ok(info) => {
                    let who = info.email.as_deref().unwrap_or("authenticated account");
                    let _ = ev_tx_worker.send(WorkerEvent::Note(format!(
                        "signed in as {who} (saved {})",
                        info.token_path.display()
                    )));
                    Ok(true)
                }
                Err(err) => Err(err.into()),
            };
            match status {
                Ok(ok) => {
                    login_ok_ref.store(ok, Ordering::Relaxed);
                    let _ = ev_tx_worker.send(WorkerEvent::Finished(Ok(())));
                }
                Err(err) => {
                    let _ = ev_tx_worker.send(WorkerEvent::Finished(Err(err)));
                }
            }
        });

        while app.is_busy() {
            drain_worker_events(&ev_rx, app);
            tick_spinner(&mut last_spin, app);
            let _ = guard.terminal.draw(|f| render_frame(app, f));
            if !app.is_busy() {
                break;
            }
            if event::poll(Duration::from_millis(25)).unwrap_or(false)
                && let Ok(Event::Key(key)) = event::read()
            {
                if is_ctrl_c(key) {
                    if app.arm_or_confirm_ctrl_c(Instant::now()) {
                        cancel.store(true, Ordering::Relaxed);
                        should_quit = true;
                        break;
                    }
                } else {
                    app.clear_ctrl_c_arm();
                    if is_ctrl_o(key) {
                        let _ = app.toggle_expand_outputs();
                    } else if key.code == KeyCode::PageUp {
                        let _ = scroll_page(app, guard, -1);
                    } else if key.code == KeyCode::PageDown {
                        let _ = scroll_page(app, guard, 1);
                    } else if key.code == KeyCode::Esc {
                        cancel.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            }
        }
    });

    app.set_busy(false);
    if login_ok.load(Ordering::Relaxed) {
        apply_provider_switch(config, ProviderKind::Antigravity, None);
        let (mut next_provider, label) = build_provider(ProviderKind::Antigravity, config);
        let _ = next_provider.list_models();
        let quota = next_provider.active_model_quota();
        *provider = next_provider;
        app.set_model_name(&label);
        app.set_model_quota(quota);
        let _ = save_provider_selection(&paths.global_config, ProviderKind::Antigravity, None);
        app.push_system_note(format!("active provider: {label}"));
    }
    Ok(should_quit)
}

fn run_memory_command(args: &str, agent: &AgentLoop, app: &mut AppState) -> Result<()> {
    let mut parts = args.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or_default().trim();
    let rest = parts.next().unwrap_or_default().trim();
    let (scopes, query) = match first {
        "project" => (vec![MemoryScope::Project], rest),
        "personal" => (vec![MemoryScope::Personal], rest),
        _ => (
            vec![MemoryScope::Project, MemoryScope::Personal],
            args.trim(),
        ),
    };
    let mut found = 0usize;
    for scope in scopes {
        for rec in agent.query_memory(scope, query)? {
            found += 1;
            app.push_system_note(format!("[{:?}] {}: {}", rec.scope, rec.key, rec.value));
        }
    }
    if found == 0 {
        app.push_system_note("memory: (no matching records)");
    }
    Ok(())
}

fn run_mcp_command(agent: &AgentLoop, app: &mut AppState) {
    if app.mcp_servers().is_empty() {
        app.push_system_note("mcp servers: (none configured)");
    } else {
        let server_lines: Vec<String> = app
            .mcp_servers()
            .iter()
            .map(|srv| format!("mcp {}: {} {}", srv.name, srv.command, srv.args.join(" ")))
            .collect();
        for line in server_lines {
            app.push_system_note(line);
        }
    }
    app.push_system_note(format!(
        "active tools: {}",
        agent.active_profile().allowed_tools.join(", ")
    ));
}

fn compact_background(
    args: &str,
    agent: &mut AgentLoop,
    provider: &mut dyn Provider,
    app: &mut AppState,
    guard: &mut TerminalGuard,
) -> Result<bool> {
    let focus = (!args.trim().is_empty()).then(|| args.trim().to_owned());
    app.push_system_note("compacting conversation via provider…");
    app.set_busy_status("Compacting…");
    app.set_busy(true);
    guard.terminal.draw(|f| render_frame(app, f))?;

    let (ev_tx, ev_rx) = mpsc::channel::<WorkerEvent>();
    let mut should_quit = false;
    let mut last_spin = Instant::now();

    std::thread::scope(|scope| {
        scope.spawn(move || {
            let outcome = match agent.compact_with_provider(focus.as_deref(), provider) {
                Ok(msg) => {
                    let _ = ev_tx.send(WorkerEvent::Note(msg));
                    Ok(())
                }
                Err(err) => Err(err),
            };
            let _ = ev_tx.send(WorkerEvent::Finished(outcome));
        });

        while app.is_busy() {
            drain_worker_events(&ev_rx, app);
            tick_spinner(&mut last_spin, app);
            let _ = guard.terminal.draw(|f| render_frame(app, f));
            if !app.is_busy() {
                break;
            }
            if event::poll(Duration::from_millis(16)).unwrap_or(false)
                && let Ok(Event::Key(key)) = event::read()
            {
                if is_ctrl_c(key) {
                    if app.arm_or_confirm_ctrl_c(Instant::now()) {
                        should_quit = true;
                        break;
                    }
                } else {
                    app.clear_ctrl_c_arm();
                    if is_ctrl_o(key) {
                        let _ = app.toggle_expand_outputs();
                    } else if key.code == KeyCode::PageUp {
                        let _ = scroll_page(app, guard, -1);
                    } else if key.code == KeyCode::PageDown {
                        let _ = scroll_page(app, guard, 1);
                    }
                }
            }
        }
    });

    app.set_busy(false);
    Ok(should_quit)
}

fn stream_prompt_background(
    display_label: String,
    prompt_for_model: String,
    agent: &mut AgentLoop,
    provider: &mut dyn Provider,
    app: &mut AppState,
    guard: &mut TerminalGuard,
) -> Result<bool> {
    app.push_user_turn(display_label);
    app.set_busy_status("Thinking…");
    app.set_busy(true);
    guard.terminal.draw(|f| render_frame(app, f))?;

    let (ev_tx, ev_rx) = mpsc::channel::<WorkerEvent>();
    let (ans_tx, ans_rx) = mpsc::channel::<ApprovalDecision>();
    let cancel = AtomicBool::new(false);
    let mut should_quit = false;
    let mut last_spin = Instant::now();

    std::thread::scope(|scope| {
        let cancel_ref = &cancel;
        scope.spawn(move || {
            let mut prompter = ChannelPrompter {
                tx: ev_tx.clone(),
                confirm_rx: ans_rx,
            };
            let res = agent
                .run_user_prompt_cancellable(&prompt_for_model, provider, &mut prompter, cancel_ref)
                .map(|_| ());
            if let Some(quota) = provider.active_model_quota() {
                let _ = ev_tx.send(WorkerEvent::Quota(quota));
            }
            let _ = ev_tx.send(WorkerEvent::Finished(res));
        });

        while app.is_busy() {
            drain_worker_events(&ev_rx, app);
            tick_spinner(&mut last_spin, app);
            let _ = guard.terminal.draw(|f| render_frame(app, f));
            if !app.is_busy() {
                break;
            }
            if event::poll(Duration::from_millis(16)).unwrap_or(false) {
                match event::read() {
                    Ok(Event::Key(key)) => {
                        if handle_streaming_key(key, app, guard, &ans_tx, &cancel) {
                            should_quit = true;
                            let _ = ans_tx.send(ApprovalDecision {
                                action: ApprovalAction::Deny,
                                feedback: None,
                            });
                            break;
                        }
                    }
                    Ok(Event::Paste(text)) => {
                        app.clear_ctrl_c_arm();
                        if app.pending_approval().is_none() {
                            app.push_input_str(&text);
                        }
                    }
                    _ => {}
                }
            }
        }
    });

    app.set_pending_approval(None);
    app.set_busy(false);
    Ok(should_quit)
}

fn tick_spinner(last_spin: &mut Instant, app: &mut AppState) {
    if last_spin.elapsed() >= Duration::from_millis(110) {
        app.advance_spinner();
        *last_spin = Instant::now();
    }
}

fn drain_worker_events(ev_rx: &Receiver<WorkerEvent>, app: &mut AppState) {
    while let Ok(ev) = ev_rx.try_recv() {
        match ev {
            WorkerEvent::ThinkingDelta(delta) => app.append_thinking_delta(&delta),
            WorkerEvent::Delta(delta) => app.append_stream_delta(&delta),
            WorkerEvent::ToolStart(call) => app.push_tool_running(&call),
            WorkerEvent::ToolFinish(call, output) => app.finish_tool_entry(&call, &output),
            WorkerEvent::Usage(usage) => app.record_usage(usage),
            WorkerEvent::Quota(quota) => app.set_model_quota(Some(quota)),
            WorkerEvent::ModeSwitched(profile) => {
                let mode = profile.mode;
                app.set_active_profile(*profile);
                app.push_system_note(format!("mode: {mode:?}"));
            }
            WorkerEvent::AskConfirm(req) => app.set_pending_approval(Some(req)),
            WorkerEvent::Note(note) => app.push_system_note(note),
            WorkerEvent::Finished(res) => {
                if let Err(err) = res {
                    if matches!(err.downcast_ref::<CoreError>(), Some(CoreError::Cancelled)) {
                        app.push_system_note("stopped response");
                    } else {
                        app.push_error_note(err.to_string());
                    }
                }
                app.set_busy(false);
            }
        }
    }
}

fn handle_streaming_key(
    key: KeyEvent,
    app: &mut AppState,
    guard: &mut TerminalGuard,
    ans_tx: &Sender<ApprovalDecision>,
    cancel: &AtomicBool,
) -> bool {
    if is_ctrl_c(key) {
        if app.arm_or_confirm_ctrl_c(Instant::now()) {
            cancel.store(true, Ordering::Relaxed);
            return true;
        }
        return false;
    }
    app.clear_ctrl_c_arm();
    if is_ctrl_o(key) {
        let _ = app.toggle_expand_outputs();
        return false;
    }

    if let Some(pending) = app.pending_approval() {
        let editing = pending.editing_feedback;
        if editing {
            match key.code {
                KeyCode::Esc | KeyCode::Tab => app.toggle_approval_feedback(),
                KeyCode::Up => app.move_approval_selection(-1),
                KeyCode::Down => app.move_approval_selection(1),
                KeyCode::Backspace => app.pop_approval_feedback_char(),
                KeyCode::Enter => {
                    if let Some(decision) = app.take_approval_decision(None) {
                        let _ = ans_tx.send(decision);
                    }
                }
                KeyCode::Char(c) => app.push_approval_feedback_char(c),
                _ => {}
            }
            return false;
        }

        match key.code {
            KeyCode::Esc => {
                cancel.store(true, Ordering::Relaxed);
                app.set_busy_status("Stopping…");
                if app.take_approval_decision(Some(3)).is_some() {
                    let _ = ans_tx.send(ApprovalDecision {
                        action: ApprovalAction::Deny,
                        feedback: Some("stopped by user via Esc".to_owned()),
                    });
                }
            }
            KeyCode::Up => app.move_approval_selection(-1),
            KeyCode::Down => app.move_approval_selection(1),
            KeyCode::Tab => app.toggle_approval_feedback(),
            KeyCode::Enter => {
                if let Some(decision) = app.take_approval_decision(None) {
                    let _ = ans_tx.send(decision);
                }
            }
            KeyCode::Char('1' | 'y' | 'Y') => {
                if let Some(decision) = app.take_approval_decision(Some(0)) {
                    let _ = ans_tx.send(decision);
                }
            }
            KeyCode::Char('2') => {
                if let Some(decision) = app.take_approval_decision(Some(1)) {
                    let _ = ans_tx.send(decision);
                }
            }
            KeyCode::Char('3') => {
                if let Some(decision) = app.take_approval_decision(Some(2)) {
                    let _ = ans_tx.send(decision);
                }
            }
            KeyCode::Char('4' | 'n' | 'N') => {
                if let Some(decision) = app.take_approval_decision(Some(3)) {
                    let _ = ans_tx.send(decision);
                }
            }
            _ => {}
        }
        return false;
    }

    if app.pending_task_details().is_some() {
        let _ = handle_task_details_key(key, app, guard);
        return false;
    }
    if app.pending_task_manager().is_some() {
        let _ = handle_task_manager_key(key, app);
        return false;
    }
    if is_ctrl_t(key) {
        app.open_task_manager();
        return false;
    }
    if app.strip_selected().is_some() {
        match key.code {
            KeyCode::Up => {
                let _ = app.move_strip_selection(-1);
                return false;
            }
            KeyCode::Down => {
                let _ = app.move_strip_selection(1);
                return false;
            }
            KeyCode::Enter => {
                let _ = app.open_selected_strip_details();
                return false;
            }
            KeyCode::Esc => {
                app.clear_strip_selection();
                return false;
            }
            _ => {
                app.clear_strip_selection();
            }
        }
    }

    if key.code == KeyCode::Esc {
        cancel.store(true, Ordering::Relaxed);
        app.subagent_tracker().cancel_all();
        app.set_busy_status("Stopping…");
        return false;
    }
    if is_newline_key(key) {
        app.push_input_char('\n');
        return false;
    }
    match key.code {
        KeyCode::Up => {
            if !app.move_cursor_vertical(-1) {
                app.navigate_history(-1);
            }
        }
        KeyCode::Down => {
            if !app.move_cursor_vertical(1) {
                if app.history_cursor_active() {
                    app.navigate_history(1);
                } else if !app.running_strip_items().is_empty() {
                    let _ = app.move_strip_selection(1);
                } else {
                    app.navigate_history(1);
                }
            }
        }
        KeyCode::Left => app.move_cursor_left(),
        KeyCode::Right => app.move_cursor_right(),
        KeyCode::Home => app.move_cursor_home(),
        KeyCode::End => app.move_cursor_end(),
        KeyCode::PageUp => {
            let _ = scroll_page(app, guard, -1);
        }
        KeyCode::PageDown => {
            let _ = scroll_page(app, guard, 1);
        }
        KeyCode::Backspace => app.pop_input_char(),
        KeyCode::Delete => app.delete_input_char(),
        KeyCode::Char(c) => app.push_input_char(c),
        _ => {}
    }
    false
}

fn viewport_rect(app: &AppState, guard: &TerminalGuard) -> Result<Rect> {
    let size = guard.terminal.size()?;
    Ok(transcript_viewport(
        app,
        Rect::new(0, 0, size.width, size.height),
    ))
}

fn scroll_page(app: &mut AppState, guard: &TerminalGuard, dir: i16) -> Result<()> {
    let rect = viewport_rect(app, guard)?;
    let step = i16::try_from(rect.height.saturating_sub(2).max(1)).unwrap_or(10);
    let max_scroll = compute_max_scroll(app, rect.width, rect.height);
    app.scroll_by(dir.saturating_mul(step), max_scroll);
    Ok(())
}

fn is_newline_key(key: KeyEvent) -> bool {
    (key.code == KeyCode::Enter
        && (key.modifiers.contains(KeyModifiers::SHIFT)
            || key.modifiers.contains(KeyModifiers::ALT)))
        || (key.code == KeyCode::Char('j') && key.modifiers.contains(KeyModifiers::CONTROL))
        || matches!(key.code, KeyCode::Char('\n' | '\r'))
}

fn is_ctrl_c(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

fn is_ctrl_o(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('o') && key.modifiers.contains(KeyModifiers::CONTROL)
}

fn is_ctrl_t(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('t') && key.modifiers.contains(KeyModifiers::CONTROL)
}

fn handle_task_manager_key(key: KeyEvent, app: &mut AppState) -> Result<bool> {
    match key.code {
        KeyCode::Esc => app.close_task_manager(),
        KeyCode::Up => app.move_task_manager_selection(-1),
        KeyCode::Down => app.move_task_manager_selection(1),
        KeyCode::Enter => {
            let _ = app.open_selected_manager_details();
        }
        KeyCode::Char('k' | 'K') => {
            if let Some(outcome) = app.kill_selected_managed_task() {
                match outcome {
                    Ok(msg) => app.push_system_note(msg),
                    Err(err) => app.push_error_note(err),
                }
            }
        }
        KeyCode::Char('x' | 'X') => {
            if let Some(outcome) = app.delete_selected_done_task() {
                match outcome {
                    Ok(msg) => app.push_system_note(msg),
                    Err(err) => app.push_error_note(err),
                }
            }
        }
        _ => {}
    }
    Ok(false)
}

fn handle_task_details_key(
    key: KeyEvent,
    app: &mut AppState,
    guard: &TerminalGuard,
) -> Result<bool> {
    let total_lines = app
        .task_details_snapshot()
        .map_or(0, |s| s.output_lines.len());
    let size = guard.terminal.size()?;
    let view_height = usize::from(size.height.saturating_sub(10).max(3));
    let page_step = isize::try_from(view_height.saturating_sub(2).max(1)).unwrap_or(10);

    match key.code {
        KeyCode::Esc => app.close_task_details(),
        KeyCode::Up => app.scroll_task_details(-1, total_lines, view_height),
        KeyCode::Down => app.scroll_task_details(1, total_lines, view_height),
        KeyCode::PageUp => app.scroll_task_details(-page_step, total_lines, view_height),
        KeyCode::PageDown => app.scroll_task_details(page_step, total_lines, view_height),
        KeyCode::Home if key.modifiers.contains(KeyModifiers::CONTROL) => app.task_details_home(),
        KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => app.task_details_end(),
        _ => {}
    }
    Ok(false)
}

fn collect_wakeup_batch(app: &mut AppState) -> Option<(String, String)> {
    let mut events = app.drain_task_notifications();
    if events.is_empty() {
        return None;
    }
    std::thread::sleep(Duration::from_millis(35));
    events.extend(app.drain_task_notifications());

    let mut labels = Vec::new();
    let mut sections = Vec::new();
    for ev in events {
        match ev {
            TaskNotification::TaskExited {
                task_id,
                command,
                status,
                exit_code,
                tail,
                log_path,
            } => {
                app.task_registry().notify_external_event(&task_id);
                labels.push(format!("{task_id} ({})", status.label()));
                let code_str = exit_code.map_or_else(|| "none".to_owned(), |c| c.to_string());
                let log_hint = log_path
                    .map(|p| format!("\nLog file: {}", p.display()))
                    .unwrap_or_default();
                let excerpt = if tail.trim().is_empty() {
                    "(no output)".to_owned()
                } else {
                    tail
                };
                sections.push(format!(
                    "- Background task `{task_id}` (`{command}`) finished with status `{}` (exit code: {code_str}).{log_hint}\nOutput tail:\n```\n{excerpt}\n```",
                    status.label()
                ));
            }
            TaskNotification::ScheduleFired {
                task_id,
                label,
                prompt,
                iteration,
                finished,
            } => {
                app.task_registry().notify_external_event(&task_id);
                labels.push(format!("{task_id} ({label})"));
                let state = if finished { "completed" } else { "recurring" };
                sections.push(format!(
                    "- Scheduled task `{task_id}` (`{label}`, trigger #{iteration}, {state}) fired with prompt:\n{prompt}"
                ));
            }
        }
    }

    let display = format!("[task wakeup] {}", labels.join(", "));
    let prompt_for_model = format!(
        "[Background Task / Schedule Wakeup Notification]\n{}\n\nRespond to these completed task/schedule events and continue any pending work.",
        sections.join("\n\n")
    );
    Some((display, prompt_for_model))
}
