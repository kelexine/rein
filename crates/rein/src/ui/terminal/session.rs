// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Crossterm raw-mode guard, slash & /model dispatcher, Ctrl+O toggle, and non-blocking Ratatui loop

use super::commands::*;
use super::idle_keys::handle_idle_key;
use super::streaming_keys::collect_wakeup_batch;

pub(super) use std::io::{self, Stdout};
pub(super) use std::sync::atomic::{AtomicBool, Ordering};
pub(super) use std::sync::mpsc::{self, Receiver, Sender};
pub(super) use std::time::{Duration, Instant};

pub(super) use anyhow::Result;
pub(super) use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyModifiers,
};
pub(super) use crossterm::execute;
pub(super) use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
pub(super) use ratatui::Terminal;
pub(super) use ratatui::backend::CrosstermBackend;
pub(super) use ratatui::layout::Rect;
pub(super) use rein_core::{
    CoreError, EventLog, MemoryScope, Provider, ToolCall, ToolOutput, UsageStats,
    login_with_browser_until, logout_rein_token,
};
pub(super) use rein_tools::TaskNotification;

pub(super) use crate::agent::{
    AgentLoop, ApprovalAction, ApprovalDecision, ApprovalPrompter, ApprovalRequest,
};
pub(super) use crate::commands::{
    BuiltinAction, CommandSource, SlashCommand, SlashKind, SlashResolution,
};
pub(super) use crate::config::{
    ProviderKind, ReinConfig, ReinPaths, apply_provider_switch, build_provider,
    generate_session_uid, list_sessions, resolve_model_target, save_global_settings,
    save_provider_selection, write_latest_session_uid,
};
pub(super) use crate::memory::AutoDreamCoordinator;
pub(super) use crate::modes::Profile;
pub(super) use crate::ui::app::{ConfigTab, PluginsTab, QuestionPromptRequest};
pub(super) use crate::ui::mentions::resolve_mentions_for_prompt;
pub(super) use crate::ui::view::{compute_max_scroll, transcript_viewport};
pub(super) use crate::ui::{AppState, render_frame};

type TuiTerminal = Terminal<CrosstermBackend<Stdout>>;

pub(super) struct TerminalGuard {
    pub(super) terminal: TuiTerminal,
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

pub(super) enum WorkerEvent {
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

pub(super) struct ChannelPrompter {
    pub(super) tx: Sender<WorkerEvent>,
    pub(super) confirm_rx: Receiver<ApprovalDecision>,
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
        app.poll_model_refresh();
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

pub(super) fn tick_spinner(last_spin: &mut Instant, app: &mut AppState) {
    if last_spin.elapsed() >= Duration::from_millis(180) {
        app.advance_spinner();
        *last_spin = Instant::now();
    }
}

pub(super) fn drain_worker_events(ev_rx: &Receiver<WorkerEvent>, app: &mut AppState) {
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
