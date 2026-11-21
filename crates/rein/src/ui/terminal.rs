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
use crate::commands::{BuiltinAction, CommandSource, SlashCommand, SlashKind, SlashResolution};
use crate::config::{
    ProviderKind, ReinConfig, ReinPaths, apply_provider_switch, build_provider,
    generate_session_uid, list_sessions, resolve_model_target, save_global_settings,
    save_provider_selection, write_latest_session_uid,
};
use crate::memory::AutoDreamCoordinator;
use crate::modes::Profile;
use crate::ui::app::{ConfigTab, PluginsTab, QuestionPromptRequest};
use crate::ui::mentions::resolve_mentions_for_prompt;
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
    SteerDelivered(String),
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

    fn on_steer_delivered(&mut self, display: &str) {
        let _ = self
            .tx
            .send(WorkerEvent::SteerDelivered(display.to_owned()));
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
            && stream_prompt_background(
                display,
                prompt,
                true,
                agent,
                provider.as_mut(),
                app,
                &mut guard,
            )?
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
    if let Some(sub_mgr) = app.subagent_manager() {
        let _ = sub_mgr.kill_all();
    }
    if let Some(sw_mgr) = app.swarm_manager() {
        sw_mgr.kill_all();
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
    if app.pending_config_modal().is_some() {
        return handle_config_modal_key(key, provider, config, paths, app, guard);
    }
    if app.pending_plugins_modal().is_some() {
        return handle_plugins_modal_key(key, app, guard);
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
    let mention_open = app.active_mention_matches().is_some();
    if key.code == KeyCode::Esc {
        if mention_open {
            app.dismiss_mention_popup();
        }
        return Ok(false);
    }

    let slash_open = !app.slash_matches().is_empty();
    if is_newline_key(key) {
        app.push_input_char('\n');
        return Ok(false);
    }
    match key.code {
        KeyCode::Tab if mention_open => {
            let _ = app.accept_selected_mention();
        }
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
        KeyCode::Up if mention_open => app.move_mention_selection(-1),
        KeyCode::Down if mention_open => app.move_mention_selection(1),
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
        KeyCode::Left => {
            app.move_cursor_left();
            app.sync_mention_dismissal();
        }
        KeyCode::Right => {
            app.move_cursor_right();
            app.sync_mention_dismissal();
        }
        KeyCode::Home => app.move_cursor_home(),
        KeyCode::End => app.move_cursor_end(),
        KeyCode::PageUp => scroll_page(app, guard, -1)?,
        KeyCode::PageDown => scroll_page(app, guard, 1)?,
        KeyCode::Backspace => {
            app.pop_input_char();
            app.sync_mention_dismissal();
        }
        KeyCode::Delete => {
            app.delete_input_char();
            app.sync_mention_dismissal();
        }
        KeyCode::Char(c) => {
            app.push_input_char(c);
            if c == '@' {
                app.refresh_mention_candidates(agent.policy().sandbox().root());
            }
            app.sync_mention_dismissal();
        }
        KeyCode::Enter if mention_open => {
            let _ = app.accept_selected_mention();
        }
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
    let prompt_for_model = resolve_mentions_for_prompt(trimmed, agent.policy().sandbox().root());
    stream_prompt_background(
        trimmed.to_owned(),
        prompt_for_model,
        false,
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
            SlashKind::Skill { .. } | SlashKind::PluginCommand { .. } => {
                let prompt = command
                    .expand_skill_prompt(&args)
                    .unwrap_or_else(|| raw.to_owned());
                stream_prompt_background(
                    raw.to_owned(),
                    prompt,
                    false,
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
        BuiltinAction::Config => app.open_config_modal(),
        BuiltinAction::Plugins => app.open_plugins_modal(),
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
    mut display_label: String,
    mut prompt_for_model: String,
    mut is_wakeup: bool,
    agent: &mut AgentLoop,
    provider: &mut dyn Provider,
    app: &mut AppState,
    guard: &mut TerminalGuard,
) -> Result<bool> {
    let workspace_root = agent.policy().sandbox().root().to_path_buf();
    let mut should_quit = false;

    loop {
        if is_wakeup {
            app.push_system_note(display_label);
        } else {
            app.push_user_turn(display_label);
        }
        app.set_busy_status("Thinking…");
        app.set_busy(true);
        guard.terminal.draw(|f| render_frame(app, f))?;

        let (ev_tx, ev_rx) = mpsc::channel::<WorkerEvent>();
        let (ans_tx, ans_rx) = mpsc::channel::<ApprovalDecision>();
        let (q_tx, q_rx) = mpsc::channel::<QuestionPromptRequest>();
        let (steer_tx, steer_rx) = mpsc::channel::<(String, String)>();
        agent.set_steer_receiver(steer_rx);
        if let Some(relay) = app.question_relay()
            && let Ok(mut guard_relay) = relay.lock()
        {
            *guard_relay = Some(q_tx);
        }
        let cancel = AtomicBool::new(false);
        let mut last_spin = Instant::now();
        let agent_ref: &mut AgentLoop = agent;
        let provider_ref: &mut dyn Provider = provider;

        std::thread::scope(|scope| {
            let cancel_ref = &cancel;
            scope.spawn(move || {
                let mut prompter = ChannelPrompter {
                    tx: ev_tx.clone(),
                    confirm_rx: ans_rx,
                };
                let res = agent_ref
                    .run_user_prompt_cancellable(
                        &prompt_for_model,
                        provider_ref,
                        &mut prompter,
                        cancel_ref,
                    )
                    .map(|_| ());
                if let Some(quota) = provider_ref.active_model_quota() {
                    let _ = ev_tx.send(WorkerEvent::Quota(quota));
                }
                let _ = ev_tx.send(WorkerEvent::Finished(res));
            });

            while app.is_busy() {
                drain_worker_events(&ev_rx, app);
                while let Ok((questions, reply_tx)) = q_rx.try_recv() {
                    app.open_question_picker(questions, reply_tx);
                }
                tick_spinner(&mut last_spin, app);
                let _ = guard.terminal.draw(|f| render_frame(app, f));
                if !app.is_busy() {
                    break;
                }
                if event::poll(Duration::from_millis(16)).unwrap_or(false) {
                    match event::read() {
                        Ok(Event::Key(key)) => {
                            if handle_streaming_key(
                                key,
                                app,
                                guard,
                                &ans_tx,
                                &cancel,
                                &steer_tx,
                                &workspace_root,
                            ) {
                                should_quit = true;
                                app.cancel_question_picker();
                                let _ = ans_tx.send(ApprovalDecision {
                                    action: ApprovalAction::Deny,
                                    feedback: None,
                                });
                                break;
                            }
                        }
                        Ok(Event::Paste(text)) => {
                            app.clear_ctrl_c_arm();
                            if app.pending_approval().is_none() && app.pending_question().is_none()
                            {
                                app.push_input_str(&text);
                            }
                        }
                        _ => {}
                    }
                }
            }
        });

        agent.clear_steer_receiver();
        app.clear_steering_pending();
        if let Some(relay) = app.question_relay()
            && let Ok(mut guard_relay) = relay.lock()
        {
            *guard_relay = None;
        }
        app.cancel_question_picker();
        app.set_pending_approval(None);
        app.set_busy(false);

        if should_quit || cancel.load(Ordering::Acquire) {
            break;
        }
        if let Some((next_display, next_prompt)) = app.take_next_queued_message() {
            display_label = next_display;
            prompt_for_model = next_prompt;
            is_wakeup = false;
            continue;
        }
        break;
    }
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
            WorkerEvent::SteerDelivered(display) => app.mark_steering_delivered(&display),
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
    steer_tx: &Sender<(String, String)>,
    workspace_root: &std::path::Path,
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

    if app.pending_question().is_some() {
        match key.code {
            KeyCode::Esc => app.cancel_question_picker(),
            KeyCode::Up => app.move_question_cursor(-1),
            KeyCode::Down => app.move_question_cursor(1),
            KeyCode::Char(' ') => app.toggle_question_option(),
            KeyCode::Backspace => app.pop_question_other_char(),
            KeyCode::Enter => app.confirm_question_step(),
            KeyCode::Char(c) => app.push_question_other_char(c),
            _ => {}
        }
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
        cancel.store(true, Ordering::Release);
        app.set_busy_status("Stopping…");
        return false;
    }
    if is_newline_key(key) {
        app.push_input_char('\n');
        return false;
    }
    if key.code == KeyCode::Char('m') && key.modifiers.contains(KeyModifiers::ALT) {
        if let Some((display, resolved)) = app.take_next_queued_message() {
            app.push_steering_pending(display.clone());
            if steer_tx.send((display.clone(), resolved.clone())).is_err() {
                app.enqueue_queued_message(display, resolved);
            }
        }
        return false;
    }
    match key.code {
        KeyCode::Up => {
            if app.input().is_empty()
                && let Some(queued) = app.pop_last_queued_for_edit()
            {
                app.set_input(queued);
            } else if !app.move_cursor_vertical(-1) {
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
        KeyCode::Enter => {
            let raw = app.take_input();
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                let display = trimmed.to_owned();
                let resolved = resolve_mentions_for_prompt(trimmed, workspace_root);
                if app.ui_settings().queued_messages == "queue" {
                    app.enqueue_queued_message(display, resolved);
                } else {
                    app.push_steering_pending(display.clone());
                    if steer_tx.send((display.clone(), resolved.clone())).is_err() {
                        app.enqueue_queued_message(display, resolved);
                    }
                }
            }
        }
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
                    .as_ref()
                    .map(|p| format!("\nLog file: {}", p.display()))
                    .unwrap_or_default();
                let excerpt = if tail.trim().is_empty() {
                    "(no output)".to_owned()
                } else {
                    tail
                };
                let failed = exit_code.is_some_and(|c| c != 0)
                    || matches!(
                        status,
                        rein_tools::TaskStatus::Failed | rein_tools::TaskStatus::Killed
                    );
                let final_json = serde_json::json!({
                    "task_id": task_id,
                    "status": status.label(),
                    "exit_code": exit_code,
                    "output_file": log_path,
                    "stdout": excerpt,
                })
                .to_string();
                app.complete_background_tool(&task_id, failed, final_json);
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
            TaskNotification::SubagentMessage {
                conversation_id,
                role,
                agent_type,
                content,
                finished,
            } => {
                app.task_registry().notify_external_event(&conversation_id);
                if finished {
                    app.complete_background_tool(&conversation_id, false, content.clone());
                }
                let state = if finished { "finished" } else { "waiting" };
                sections.push(format!(
                    "- Subagent `{conversation_id}` (`{agent_type}: {role}`, state: {state}) sent:\n{content}"
                ));
                labels.push(role);
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

fn handle_config_modal_key(
    key: KeyEvent,
    provider: &mut Box<dyn Provider>,
    config: &mut ReinConfig,
    paths: &ReinPaths,
    app: &mut AppState,
    guard: &mut TerminalGuard,
) -> Result<bool> {
    let searching = app.pending_config_modal().is_some_and(|m| m.searching);
    if searching {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                if let Some(m) = app.pending_config_modal_mut() {
                    m.searching = false;
                }
            }
            KeyCode::Backspace => {
                if let Some(m) = app.pending_config_modal_mut() {
                    m.search.pop();
                    m.selected = 0;
                }
            }
            KeyCode::Char(c) => {
                if let Some(m) = app.pending_config_modal_mut() {
                    m.search.push(c);
                    m.selected = 0;
                }
            }
            _ => {}
        }
        return Ok(false);
    }

    match key.code {
        KeyCode::Esc => app.close_config_modal(),
        KeyCode::Char('/') => {
            if let Some(m) = app.pending_config_modal_mut() {
                m.searching = true;
            }
        }
        KeyCode::Left | KeyCode::BackTab => {
            if let Some(m) = app.pending_config_modal_mut() {
                let idx = ConfigTab::ALL.iter().position(|&t| t == m.tab).unwrap_or(0);
                let prev = (idx + ConfigTab::ALL.len() - 1) % ConfigTab::ALL.len();
                m.tab = ConfigTab::ALL[prev];
                m.selected = 0;
            }
        }
        KeyCode::Right | KeyCode::Tab => {
            if let Some(m) = app.pending_config_modal_mut() {
                let idx = ConfigTab::ALL.iter().position(|&t| t == m.tab).unwrap_or(0);
                let next = (idx + 1) % ConfigTab::ALL.len();
                m.tab = ConfigTab::ALL[next];
                m.selected = 0;
            }
        }
        KeyCode::Up => {
            let count = app.config_setting_rows().len();
            if let Some(m) = app.pending_config_modal_mut()
                && count > 0
            {
                m.selected = (m.selected + count - 1) % count;
            }
        }
        KeyCode::Down => {
            let count = app.config_setting_rows().len();
            if let Some(m) = app.pending_config_modal_mut()
                && count > 0
            {
                m.selected = (m.selected + 1) % count;
            }
        }
        KeyCode::Char(' ') | KeyCode::Enter => {
            if let Some(m) = app.pending_config_modal()
                && matches!(m.tab, ConfigTab::Settings | ConfigTab::Config)
                && let Some((_, open_model)) = app.activate_selected_config_setting()
            {
                if open_model {
                    app.close_config_modal();
                    return run_model_command("", provider, config, paths, app, guard);
                }
                config.settings = app.ui_settings().clone();
                if let Err(err) = save_global_settings(&paths.global_config, &config.settings) {
                    app.push_error_note(format!("failed to save global settings: {err}"));
                }
            }
        }
        _ => {}
    }
    Ok(false)
}

fn handle_plugins_modal_key(
    key: KeyEvent,
    app: &mut AppState,
    guard: &mut TerminalGuard,
) -> Result<bool> {
    let (searching, adding) = app
        .pending_plugins_modal()
        .map_or((false, false), |m| (m.searching, m.adding_marketplace));

    if adding {
        match key.code {
            KeyCode::Esc => {
                if let Some(m) = app.pending_plugins_modal_mut() {
                    m.adding_marketplace = false;
                    m.marketplace_input.clear();
                }
            }
            KeyCode::Backspace => {
                if let Some(m) = app.pending_plugins_modal_mut() {
                    m.marketplace_input.pop();
                }
            }
            KeyCode::Char(c) => {
                if let Some(m) = app.pending_plugins_modal_mut() {
                    m.marketplace_input.push(c);
                }
            }
            KeyCode::Enter => {
                let spec = app
                    .pending_plugins_modal()
                    .map(|m| m.marketplace_input.trim().to_owned())
                    .unwrap_or_default();
                if let Some(m) = app.pending_plugins_modal_mut() {
                    m.adding_marketplace = false;
                    m.marketplace_input.clear();
                }
                if !spec.is_empty()
                    && let Some(pm) = app.plugin_manager().cloned()
                {
                    let notice = match pm.add_marketplace(&spec) {
                        Ok(mp) => format!("added marketplace '{}' ({})", mp.name, mp.url),
                        Err(e) => format!("add failed: {e}"),
                    };
                    if let Some(m) = app.pending_plugins_modal_mut() {
                        m.status_notice = Some(notice);
                    }
                }
            }
            _ => {}
        }
        return Ok(false);
    }

    if searching {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                if let Some(m) = app.pending_plugins_modal_mut() {
                    m.searching = false;
                }
            }
            KeyCode::Backspace => {
                if let Some(m) = app.pending_plugins_modal_mut() {
                    m.search.pop();
                    m.selected = 0;
                }
            }
            KeyCode::Char(c) => {
                if let Some(m) = app.pending_plugins_modal_mut() {
                    m.search.push(c);
                    m.selected = 0;
                }
            }
            _ => {}
        }
        return Ok(false);
    }

    if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
        if let Some(pm) = app.plugin_manager().cloned() {
            let names: Vec<String> = pm.list_marketplaces().into_iter().map(|m| m.name).collect();
            if let Some(m) = app.pending_plugins_modal_mut() {
                m.marketplace_filter = match &m.marketplace_filter {
                    None => names.first().cloned(),
                    Some(cur) => {
                        let idx = names.iter().position(|n| n == cur).unwrap_or(0);
                        names.get(idx + 1).cloned()
                    }
                };
                m.selected = 0;
            }
        }
        return Ok(false);
    }

    let Some(pm) = app.plugin_manager().cloned() else {
        if key.code == KeyCode::Esc {
            app.close_plugins_modal();
        }
        return Ok(false);
    };

    let tab = app
        .pending_plugins_modal()
        .map_or(PluginsTab::Discover, |m| m.tab);
    let q = app
        .pending_plugins_modal()
        .map(|m| m.search.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let mp_filter = app
        .pending_plugins_modal()
        .and_then(|m| m.marketplace_filter.clone());

    let item_count = match tab {
        PluginsTab::Discover => pm
            .discover_catalog(mp_filter.as_deref())
            .into_iter()
            .filter(|p| {
                q.is_empty()
                    || p.name.to_ascii_lowercase().contains(&q)
                    || p.description.to_ascii_lowercase().contains(&q)
            })
            .count(),
        PluginsTab::Installed => pm
            .installed_plugins()
            .into_iter()
            .filter(|p| {
                q.is_empty()
                    || p.name.to_ascii_lowercase().contains(&q)
                    || p.description.to_ascii_lowercase().contains(&q)
            })
            .count(),
        PluginsTab::Marketplaces => pm.list_marketplaces().len(),
        PluginsTab::Errors => pm.collect_enabled_components().errors.len(),
    };

    match key.code {
        KeyCode::Esc => app.close_plugins_modal(),
        KeyCode::Char('/') => {
            if let Some(m) = app.pending_plugins_modal_mut() {
                m.searching = true;
            }
        }
        KeyCode::Char('a') => {
            if let Some(m) = app.pending_plugins_modal_mut() {
                m.adding_marketplace = true;
                m.marketplace_input.clear();
            }
        }
        KeyCode::Left | KeyCode::BackTab => {
            if let Some(m) = app.pending_plugins_modal_mut() {
                let idx = PluginsTab::ALL
                    .iter()
                    .position(|&t| t == m.tab)
                    .unwrap_or(0);
                m.tab = PluginsTab::ALL[(idx + PluginsTab::ALL.len() - 1) % PluginsTab::ALL.len()];
                m.selected = 0;
            }
        }
        KeyCode::Right | KeyCode::Tab => {
            if let Some(m) = app.pending_plugins_modal_mut() {
                let idx = PluginsTab::ALL
                    .iter()
                    .position(|&t| t == m.tab)
                    .unwrap_or(0);
                m.tab = PluginsTab::ALL[(idx + 1) % PluginsTab::ALL.len()];
                m.selected = 0;
            }
        }
        KeyCode::Up if item_count > 0 => {
            if let Some(m) = app.pending_plugins_modal_mut() {
                m.selected = (m.selected + item_count - 1) % item_count;
            }
        }
        KeyCode::Down if item_count > 0 => {
            if let Some(m) = app.pending_plugins_modal_mut() {
                m.selected = (m.selected + 1) % item_count;
            }
        }
        KeyCode::Char('r') => {
            let target_mp = if tab == PluginsTab::Marketplaces {
                let sel = app.pending_plugins_modal().map_or(0, |m| m.selected);
                pm.list_marketplaces().get(sel).map(|m| m.name.clone())
            } else {
                mp_filter.or_else(|| pm.list_marketplaces().first().map(|m| m.name.clone()))
            };
            if let Some(mp_name) = target_mp {
                app.set_busy_status(format!("git syncing {mp_name}…"));
                app.set_busy(true);
                guard.terminal.draw(|f| render_frame(app, f))?;
                let notice = match pm.refresh_marketplace(&mp_name) {
                    Ok(n) => format!("synced '{mp_name}' ({n} plugins indexed)"),
                    Err(e) => format!("git sync failed: {e}"),
                };
                app.set_busy(false);
                if let Some(m) = app.pending_plugins_modal_mut() {
                    m.status_notice = Some(notice);
                }
            }
        }
        KeyCode::Char(' ') | KeyCode::Enter => {
            let sel = app.pending_plugins_modal().map_or(0, |m| m.selected);
            let mut reload_cmds = false;
            let notice = match tab {
                PluginsTab::Discover => {
                    let catalog: Vec<_> = pm
                        .discover_catalog(mp_filter.as_deref())
                        .into_iter()
                        .filter(|p| {
                            q.is_empty()
                                || p.name.to_ascii_lowercase().contains(&q)
                                || p.description.to_ascii_lowercase().contains(&q)
                        })
                        .collect();
                    if let Some(entry) = catalog.get(sel) {
                        if entry.installed {
                            match pm.toggle_plugin_enabled(&entry.name) {
                                Ok(en) => {
                                    reload_cmds = true;
                                    format!(
                                        "{} {}",
                                        entry.name,
                                        if en { "enabled" } else { "disabled" }
                                    )
                                }
                                Err(e) => e,
                            }
                        } else {
                            app.set_busy_status(format!("Installing {} via git…", entry.name));
                            app.set_busy(true);
                            guard.terminal.draw(|f| render_frame(app, f))?;
                            let res = pm.install_plugin(entry);
                            app.set_busy(false);
                            match res {
                                Ok(rec) => {
                                    reload_cmds = true;
                                    format!(
                                        "installed '{}' (restart or use commands/agents)",
                                        rec.name
                                    )
                                }
                                Err(e) => format!("install failed: {e}"),
                            }
                        }
                    } else {
                        "no plugin selected".to_owned()
                    }
                }
                PluginsTab::Installed => {
                    let inst: Vec<_> = pm
                        .installed_plugins()
                        .into_iter()
                        .filter(|p| {
                            q.is_empty()
                                || p.name.to_ascii_lowercase().contains(&q)
                                || p.description.to_ascii_lowercase().contains(&q)
                        })
                        .collect();
                    if let Some(rec) = inst.get(sel) {
                        match pm.toggle_plugin_enabled(&rec.name) {
                            Ok(en) => {
                                reload_cmds = true;
                                format!("{} {}", rec.name, if en { "enabled" } else { "disabled" })
                            }
                            Err(e) => e,
                        }
                    } else {
                        "no plugin selected".to_owned()
                    }
                }
                PluginsTab::Marketplaces => {
                    if let Some(mp) = pm.list_marketplaces().get(sel) {
                        app.set_busy_status(format!("git syncing {}…", mp.name));
                        app.set_busy(true);
                        guard.terminal.draw(|f| render_frame(app, f))?;
                        let res = pm.refresh_marketplace(&mp.name);
                        app.set_busy(false);
                        match res {
                            Ok(n) => format!("synced '{}' ({n} plugins)", mp.name),
                            Err(e) => format!("git sync failed: {e}"),
                        }
                    } else {
                        "no marketplace selected".to_owned()
                    }
                }
                PluginsTab::Errors => String::new(),
            };
            if reload_cmds {
                let bundle = pm.collect_enabled_components();
                let mut reg = app.slash_registry().clone();
                for cmd in bundle.commands {
                    reg.register(SlashCommand {
                        name: cmd.name,
                        description: cmd.description,
                        source: CommandSource::Plugin,
                        kind: SlashKind::PluginCommand {
                            template: cmd.template,
                        },
                    });
                }
                app.set_slash_registry(reg);
            }
            if !notice.is_empty()
                && let Some(m) = app.pending_plugins_modal_mut()
            {
                m.status_notice = Some(notice);
            }
        }
        KeyCode::Char('x') => {
            let sel = app.pending_plugins_modal().map_or(0, |m| m.selected);
            let notice = match tab {
                PluginsTab::Installed => {
                    let inst: Vec<_> = pm
                        .installed_plugins()
                        .into_iter()
                        .filter(|p| {
                            q.is_empty()
                                || p.name.to_ascii_lowercase().contains(&q)
                                || p.description.to_ascii_lowercase().contains(&q)
                        })
                        .collect();
                    if let Some(rec) = inst.get(sel) {
                        match pm.uninstall_plugin(&rec.name) {
                            Ok(()) => format!("uninstalled '{}'", rec.name),
                            Err(e) => e,
                        }
                    } else {
                        String::new()
                    }
                }
                PluginsTab::Marketplaces => {
                    if let Some(mp) = pm.list_marketplaces().get(sel) {
                        match pm.remove_marketplace(&mp.name) {
                            Ok(()) => format!("removed marketplace '{}'", mp.name),
                            Err(e) => e,
                        }
                    } else {
                        String::new()
                    }
                }
                _ => String::new(),
            };
            if !notice.is_empty()
                && let Some(m) = app.pending_plugins_modal_mut()
            {
                m.status_notice = Some(notice);
                m.selected = 0;
            }
        }
        _ => {}
    }
    Ok(false)
}
