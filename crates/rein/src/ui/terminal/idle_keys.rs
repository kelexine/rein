// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Idle-mode key handling and picker navigation for Ratatui TUI

use super::commands::{handle_enter, switch_and_persist_provider};
use super::session::*;
use super::streaming_keys::*;

pub(super) fn handle_idle_key(
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
