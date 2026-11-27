// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Background thread owning ReinSession's AgentLoop and Provider

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use rein::agent::ApprovalDecision;
use rein::config::{
    ProviderKind, apply_provider_switch, build_provider, generate_session_uid, load_cached_models,
    resolve_resume_session, save_cached_models, save_config_file, save_provider_selection,
    write_latest_session_uid,
};
use rein::runtime::ReinSession;
use rein_core::{CoreError, EventLog, ModeId, login_with_browser};

use super::events::{GuiAgentBridge, WorkerCommand, WorkerEvent};

type SteerSlot = Arc<Mutex<Option<mpsc::Sender<(String, String)>>>>;

/// Handle to the background agent thread, cancellation flag, steering channel, and approval channel.
pub struct WorkerHandle {
    cmd_tx: mpsc::Sender<WorkerCommand>,
    ev_rx: Arc<Mutex<mpsc::Receiver<WorkerEvent>>>,
    approval_tx: mpsc::Sender<ApprovalDecision>,
    steer_slot: SteerSlot,
    cancel_flag: Arc<AtomicBool>,
    wake_flag: Arc<AtomicBool>,
}

impl WorkerHandle {
    /// Spawns the background agent thread owning `session.agent` and `session.provider`.
    #[must_use]
    pub fn spawn(mut session: ReinSession) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCommand>();
        let (ev_tx, ev_rx) = mpsc::channel::<WorkerEvent>();
        let (approval_tx, approval_rx) = mpsc::channel::<ApprovalDecision>();
        let steer_slot: SteerSlot = Arc::new(Mutex::new(None));
        let steer_worker = Arc::clone(&steer_slot);
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let cancel_worker = Arc::clone(&cancel_flag);
        let wake_flag = Arc::new(AtomicBool::new(false));
        let wake_worker = Arc::clone(&wake_flag);
        let wake_q = Arc::clone(&wake_flag);
        let ev_rx_shared = Arc::new(Mutex::new(ev_rx));

        let (q_tx, q_rx) = mpsc::channel();
        if let Ok(mut guard) = session.question_relay.lock() {
            *guard = Some(q_tx);
        }
        let q_ev_tx = ev_tx.clone();
        std::thread::spawn(move || {
            while let Ok((questions, reply_tx)) = q_rx.recv() {
                let sent = q_ev_tx
                    .send(WorkerEvent::QuestionRequested {
                        questions,
                        reply_tx,
                    })
                    .is_ok();
                if sent {
                    wake_q.store(true, Ordering::Release);
                } else {
                    break;
                }
            }
        });

        std::thread::spawn(move || {
            let mut bridge = GuiAgentBridge {
                ev_tx,
                approval_rx,
                wake_flag: wake_worker,
            };
            while let Ok(cmd) = cmd_rx.recv() {
                cancel_worker.store(false, Ordering::Release);
                handle_command(
                    cmd,
                    &mut session,
                    &mut bridge,
                    &cancel_worker,
                    &steer_worker,
                );
            }
        });

        Self {
            cmd_tx,
            ev_rx: ev_rx_shared,
            approval_tx,
            steer_slot,
            cancel_flag,
            wake_flag,
        }
    }

    /// Returns the shared wake flag set whenever the worker emits a UI event.
    #[must_use]
    pub fn wake_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.wake_flag)
    }

    /// Sends a [`WorkerCommand`] to the background thread.
    pub fn send(&self, cmd: WorkerCommand) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// Queues a mid-turn steering message `(display, prompt)` into the active turn if running.
    pub fn send_steer(&self, display: String, prompt: String) -> bool {
        if let Ok(guard) = self.steer_slot.lock()
            && let Some(tx) = guard.as_ref()
        {
            return tx.send((display, prompt)).is_ok();
        }
        false
    }

    /// Sends a policy [`ApprovalDecision`] to the waiting `GuiAgentBridge`.
    pub fn send_approval(&self, decision: ApprovalDecision) {
        let _ = self.approval_tx.send(decision);
    }

    /// Signals cancellation of the active turn.
    pub fn cancel_turn(&self) {
        self.cancel_flag.store(true, Ordering::Release);
    }

    /// Drains all currently queued [`WorkerEvent`]s without blocking.
    #[must_use]
    pub fn drain_events(&self) -> Vec<WorkerEvent> {
        let mut out = Vec::new();
        if let Ok(rx) = self.ev_rx.lock() {
            while let Ok(ev) = rx.try_recv() {
                out.push(ev);
            }
        }
        out
    }
}

fn handle_command(
    cmd: WorkerCommand,
    session: &mut ReinSession,
    bridge: &mut GuiAgentBridge,
    cancel: &AtomicBool,
    steer_slot: &SteerSlot,
) {
    match cmd {
        WorkerCommand::RunPrompt { prompt } => {
            let (steer_tx, steer_rx) = mpsc::channel();
            if let Ok(mut guard) = steer_slot.lock() {
                *guard = Some(steer_tx);
            }
            session.agent.set_steer_receiver(steer_rx);
            let res = session.agent.run_user_prompt_cancellable(
                &prompt,
                session.provider.as_mut(),
                bridge,
                cancel,
            );
            session.agent.clear_steer_receiver();
            if let Ok(mut guard) = steer_slot.lock() {
                *guard = None;
            }
            if let Err(err) = res {
                if matches!(err.downcast_ref::<CoreError>(), Some(CoreError::Cancelled)) {
                    bridge.emit(WorkerEvent::Note("stopped response".to_owned()));
                } else {
                    bridge.emit(WorkerEvent::Error(err.to_string()));
                }
            }
            bridge.emit(WorkerEvent::TurnFinished);
        }
        WorkerCommand::SwitchMode(mode) => match session.agent.set_mode(mode) {
            Ok(_) => {
                let active = session.agent.active_profile().clone();
                bridge.emit(WorkerEvent::ModeSwitched(Box::new(active)));
            }
            Err(err) => {
                bridge.emit(WorkerEvent::Error(err.to_string()));
            }
        },
        WorkerCommand::FetchModels => {
            if let Ok(models) = session.provider.list_models() {
                save_cached_models(
                    &session.loaded.paths.global_dir,
                    session.provider_kind,
                    &models,
                );
                bridge.emit(WorkerEvent::ModelsLoaded(models));
            }
        }
        WorkerCommand::SelectModel(model_id) => {
            session.loaded.config.model = Some(model_id.clone());
            let model_label = if session.provider.set_model(&model_id) {
                format!("{}/{}", session.provider_kind.as_str(), model_id)
            } else {
                let (new_provider, label) =
                    build_provider(session.provider_kind, &session.loaded.config);
                session.provider = new_provider;
                label
            };
            session.model_label = model_label.clone();
            let _ = save_provider_selection(
                &session.loaded.paths.global_config,
                session.provider_kind,
                Some(&model_id),
            );
            bridge.emit(WorkerEvent::ModelSelected(model_label));
        }
        WorkerCommand::SwitchProvider(kind) => {
            apply_provider_switch(&mut session.loaded.config, kind, None);
            session.provider_kind = kind;
            let (new_provider, label) = build_provider(kind, &session.loaded.config);
            session.provider = new_provider;
            session.model_label = label.clone();
            let _ = save_provider_selection(
                &session.loaded.paths.global_config,
                kind,
                session.loaded.config.model.as_deref(),
            );
            let mut models = load_cached_models(&session.loaded.paths.global_dir, kind);
            if models.is_empty()
                && let Ok(live) = session.provider.list_models()
            {
                save_cached_models(&session.loaded.paths.global_dir, kind, &live);
                models = live;
            }
            bridge.emit(WorkerEvent::ProviderUpdated {
                provider_kind: kind,
                model_label: label,
                models,
            });
        }
        WorkerCommand::ApplyConfig(new_cfg) => {
            let kind = new_cfg.resolved_provider();
            session.loaded.config = *new_cfg;
            session.provider_kind = kind;
            let (new_provider, label) = build_provider(kind, &session.loaded.config);
            session.provider = new_provider;
            session.model_label = label.clone();
            let _ = save_config_file(&session.loaded.paths.global_config, &session.loaded.config);
            let bypass = session.loaded.config.settings.default_permission_mode == "auto-bypass";
            session.agent.set_bypass_permissions(bypass);
            let models = load_cached_models(&session.loaded.paths.global_dir, kind);
            bridge.emit(WorkerEvent::ProviderUpdated {
                provider_kind: kind,
                model_label: label,
                models,
            });
        }
        WorkerCommand::SyncPolicy {
            allowed_commands,
            bypass_permissions,
        } => {
            session
                .loaded
                .allowed_commands
                .clone_from(&allowed_commands);
            session.agent.set_allowed_commands(allowed_commands);
            session.agent.set_bypass_permissions(bypass_permissions);
        }
        WorkerCommand::StartOAuthLogin => {
            let token_path = session
                .loaded
                .config
                .antigravity_token_path
                .clone()
                .unwrap_or_else(|| {
                    session
                        .loaded
                        .paths
                        .global_dir
                        .join("antigravity-oauth.json")
                });
            match login_with_browser(&token_path, |_| {}) {
                Ok(res) => {
                    apply_provider_switch(
                        &mut session.loaded.config,
                        ProviderKind::Antigravity,
                        None,
                    );
                    session.provider_kind = ProviderKind::Antigravity;
                    let (new_provider, label) =
                        build_provider(ProviderKind::Antigravity, &session.loaded.config);
                    session.provider = new_provider;
                    session.model_label = label.clone();
                    let _ = save_provider_selection(
                        &session.loaded.paths.global_config,
                        ProviderKind::Antigravity,
                        None,
                    );
                    let models = session.provider.list_models().unwrap_or_default();
                    if !models.is_empty() {
                        save_cached_models(
                            &session.loaded.paths.global_dir,
                            ProviderKind::Antigravity,
                            &models,
                        );
                    }
                    let who = res.email.unwrap_or_else(|| "Google account".to_owned());
                    bridge.emit(WorkerEvent::ProviderUpdated {
                        provider_kind: ProviderKind::Antigravity,
                        model_label: label,
                        models,
                    });
                    bridge.emit(WorkerEvent::Note(format!(
                        "Authenticated Antigravity ({who})"
                    )));
                }
                Err(err) => {
                    bridge.emit(WorkerEvent::Error(format!("OAuth sign-in failed: {err}")));
                }
            }
        }
        WorkerCommand::Compact => {
            match session
                .agent
                .compact_with_provider(None, session.provider.as_mut())
            {
                Ok(msg) => {
                    bridge.emit(WorkerEvent::Note(msg));
                }
                Err(err) => {
                    bridge.emit(WorkerEvent::Error(err.to_string()));
                }
            }
            bridge.emit(WorkerEvent::TurnFinished);
        }
        WorkerCommand::NewSession => {
            let uid = generate_session_uid(&session.loaded.paths);
            let (log_path, full_path) = session.loaded.paths.session_paths_for(&uid);
            let _ = write_latest_session_uid(&session.loaded.paths, &uid);
            session.todo_store.set_session_uid(&uid);
            session
                .agent
                .switch_session(EventLog::new(&log_path), full_path);
            let _ = session.agent.set_mode(ModeId::Coding);
            let active = session.agent.active_profile().clone();
            bridge.emit(WorkerEvent::SessionReset {
                session_uid: uid,
                profile: Box::new(active),
            });
        }
        WorkerCommand::ResumeSession(uid) => {
            match resolve_resume_session(&session.loaded.paths, Some(&uid)) {
                Ok((summary, _)) => {
                    let _ = write_latest_session_uid(&session.loaded.paths, &summary.uid);
                    session.todo_store.set_session_uid(&summary.uid);
                    session
                        .agent
                        .switch_session(EventLog::new(&summary.event_log), summary.full_transcript);
                    match session.agent.hydrate_structured() {
                        Ok(events) => {
                            let active = session.agent.active_profile().clone();
                            bridge.emit(WorkerEvent::SessionHydrated {
                                session_uid: summary.uid,
                                events,
                                profile: Box::new(active),
                            });
                        }
                        Err(err) => {
                            bridge.emit(WorkerEvent::Error(err.to_string()));
                        }
                    }
                }
                Err(err) => {
                    bridge.emit(WorkerEvent::Error(err.to_string()));
                }
            }
        }
    }
}
