// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: TUI slash command dispatch, background streaming, login, and compaction handlers

use super::session::*;

pub(super) fn handle_enter(
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

pub(super) fn dispatch_slash(
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

pub(super) fn run_model_command(
    args: &str,
    provider: &mut Box<dyn Provider>,
    config: &mut ReinConfig,
    paths: &ReinPaths,
    app: &mut AppState,
    guard: &mut TerminalGuard,
) -> Result<bool> {
    let trimmed = args.trim();
    let force_refresh = trimmed.eq_ignore_ascii_case("refresh");
    let target_kind = if trimmed.is_empty() || force_refresh {
        Some(config.resolved_provider())
    } else {
        ProviderKind::parse(trimmed)
    };

    if let Some(kind) = target_kind {
        let is_active_provider = kind == config.resolved_provider();
        let models_res = if is_active_provider && !force_refresh && !app.cached_models().is_empty() {
            Ok(app.cached_models().to_vec())
        } else {
            app.set_busy_status("Fetching models…");
            app.set_busy(true);
            guard.terminal.draw(|f| render_frame(app, f))?;
            let fetched = if is_active_provider {
                provider.list_models()
            } else {
                let (mut probe, _) = build_provider(kind, config);
                probe.list_models()
            };
            app.set_busy(false);
            if let Ok(ref models) = fetched
                && !models.is_empty()
            {
                crate::config::save_cached_models(&paths.global_dir, kind, models);
                if is_active_provider {
                    app.set_cached_models(models.clone());
                }
            }
            fetched
        };

        match models_res {
            Ok(models) if !models.is_empty() => {
                if is_active_provider {
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
    if let Some(matched) = app.cached_models().iter().find(|m| m.id == model_id) {
        app.set_model_quota(matched.quota_percent);
    }
    Ok(false)
}

pub(super) fn run_provider_command(
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

pub(super) fn switch_and_persist_provider(
    provider: &mut Box<dyn Provider>,
    config: &mut ReinConfig,
    paths: &ReinPaths,
    app: &mut AppState,
    kind: ProviderKind,
    explicit_model: Option<&str>,
    quota: Option<u8>,
) {
    let same_provider = kind == config.resolved_provider();
    apply_provider_switch(config, kind, explicit_model);
    let label = if same_provider
        && let Some(model_id) = config.model.as_deref()
        && provider.set_model(model_id)
    {
        format!("{}/{model_id}", kind.as_str())
    } else {
        let (next_provider, built_label) = build_provider(kind, config);
        *provider = next_provider;
        let cached = crate::config::load_cached_models(&paths.global_dir, kind);
        app.set_cached_models(cached);
        built_label
    };
    app.set_model_name(&label);
    app.set_model_quota(quota.or_else(|| provider.active_model_quota()));
    let _ = save_provider_selection(&paths.global_config, kind, config.model.as_deref());
    app.push_system_note(format!("switched model: {label}"));
}

pub(super) fn run_login_background(
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

pub(super) fn compact_background(
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

pub(super) fn stream_prompt_background(
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

