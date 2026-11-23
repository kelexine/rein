// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Background thread owning ReinSession's AgentLoop and Provider

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use rein::agent::ApprovalDecision;
use rein::config::{
    build_provider, generate_session_uid, resolve_resume_session, write_latest_session_uid,
};
use rein::runtime::ReinSession;
use rein_core::{CoreError, EventLog, ModeId};

use super::events::{GuiAgentBridge, WorkerCommand, WorkerEvent};

/// Handle to the background agent thread, cancellation flag, and approval channel.
pub struct WorkerHandle {
    cmd_tx: mpsc::Sender<WorkerCommand>,
    ev_rx: Arc<Mutex<mpsc::Receiver<WorkerEvent>>>,
    approval_tx: mpsc::Sender<ApprovalDecision>,
    cancel_flag: Arc<AtomicBool>,
    wake_flag: Arc<AtomicBool>,
}

impl WorkerHandle {
    /// Spawns the background agent thread owning `session.agent` and `session.provider`.
    #[must_use]
    pub fn spawn(mut session: ReinSession) -> (Self, Arc<Mutex<mpsc::Receiver<WorkerEvent>>>) {
        let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCommand>();
        let (ev_tx, ev_rx) = mpsc::channel::<WorkerEvent>();
        let (approval_tx, approval_rx) = mpsc::channel::<ApprovalDecision>();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let cancel_worker = Arc::clone(&cancel_flag);
        let wake_flag = Arc::new(AtomicBool::new(false));
        let wake_worker = Arc::clone(&wake_flag);
        let wake_q = Arc::clone(&wake_flag);
        let ev_rx_shared = Arc::new(Mutex::new(ev_rx));
        let ev_rx_ret = Arc::clone(&ev_rx_shared);

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
                handle_command(cmd, &mut session, &mut bridge, &cancel_worker);
            }
        });

        (
            Self {
                cmd_tx,
                ev_rx: ev_rx_shared,
                approval_tx,
                cancel_flag,
                wake_flag,
            },
            ev_rx_ret,
        )
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
) {
    match cmd {
        WorkerCommand::RunPrompt { prompt } => {
            let res = session.agent.run_user_prompt_cancellable(
                &prompt,
                session.provider.as_mut(),
                bridge,
                cancel,
            );
            if let Err(err) = res {
                if matches!(err.downcast_ref::<CoreError>(), Some(CoreError::Cancelled)) {
                    let _ = bridge
                        .ev_tx
                        .send(WorkerEvent::Note("stopped response".to_owned()));
                } else {
                    let _ = bridge.ev_tx.send(WorkerEvent::Error(err.to_string()));
                }
            }
            let _ = bridge.ev_tx.send(WorkerEvent::TurnFinished);
        }
        WorkerCommand::SwitchMode(mode) => match session.agent.set_mode(mode) {
            Ok(_) => {
                let active = session.agent.active_profile().clone();
                let _ = bridge
                    .ev_tx
                    .send(WorkerEvent::ModeSwitched(Box::new(active)));
            }
            Err(err) => {
                let _ = bridge.ev_tx.send(WorkerEvent::Error(err.to_string()));
            }
        },
        WorkerCommand::FetchModels => match session.provider.list_models() {
            Ok(models) => {
                let _ = bridge.ev_tx.send(WorkerEvent::ModelsLoaded(models));
            }
            Err(err) => {
                let _ = bridge.ev_tx.send(WorkerEvent::Error(err.to_string()));
            }
        },
        WorkerCommand::SelectModel(model_id) => {
            session.loaded.config.model = Some(model_id.clone());
            let (new_provider, model_label) =
                build_provider(session.provider_kind, &session.loaded.config);
            session.provider = new_provider;
            session.model_label = model_label.clone();
            let _ = bridge.ev_tx.send(WorkerEvent::ModelSelected(model_label));
        }
        WorkerCommand::Compact => {
            match session
                .agent
                .compact_with_provider(None, session.provider.as_mut())
            {
                Ok(msg) => {
                    let _ = bridge.ev_tx.send(WorkerEvent::Note(msg));
                }
                Err(err) => {
                    let _ = bridge.ev_tx.send(WorkerEvent::Error(err.to_string()));
                }
            }
            let _ = bridge.ev_tx.send(WorkerEvent::TurnFinished);
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
            let _ = bridge.ev_tx.send(WorkerEvent::SessionReset {
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
                            let _ = bridge.ev_tx.send(WorkerEvent::SessionHydrated {
                                session_uid: summary.uid,
                                events,
                                profile: Box::new(active),
                            });
                        }
                        Err(err) => {
                            let _ = bridge.ev_tx.send(WorkerEvent::Error(err.to_string()));
                        }
                    }
                }
                Err(err) => {
                    let _ = bridge.ev_tx.send(WorkerEvent::Error(err.to_string()));
                }
            }
        }
    }
}
