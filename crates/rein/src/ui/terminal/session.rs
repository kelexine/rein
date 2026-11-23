// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Crossterm raw-mode guard, slash & /model dispatcher, Ctrl+O toggle, and non-blocking Ratatui loop

use super::commands::*;

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
pub(super) use crate::commands::{BuiltinAction, CommandSource, SlashCommand, SlashKind, SlashResolution};
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

pub(super) fn handle_model_picker_key(
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

pub(super) fn handle_session_picker_key(
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

pub(super) fn handle_rewind_picker_key(
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

pub(super) fn handle_streaming_key(
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

pub(super) fn scroll_page(app: &mut AppState, guard: &TerminalGuard, dir: i16) -> Result<()> {
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

pub(super) fn is_ctrl_c(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

pub(super) fn is_ctrl_o(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('o') && key.modifiers.contains(KeyModifiers::CONTROL)
}

fn is_ctrl_t(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('t') && key.modifiers.contains(KeyModifiers::CONTROL)
}

pub(super) fn handle_task_manager_key(key: KeyEvent, app: &mut AppState) -> Result<bool> {
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

pub(super) fn handle_task_details_key(
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

pub(super) fn handle_config_modal_key(
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

pub(super) fn handle_plugins_modal_key(
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
