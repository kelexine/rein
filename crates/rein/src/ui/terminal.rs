// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Crossterm raw-mode guard and interactive Ratatui event loop

use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use rein_core::Provider;

use crate::agent::{AgentLoop, ApprovalPrompter};
use crate::ui::{AppState, render_frame};

type TuiTerminal = Terminal<CrosstermBackend<Stdout>>;

struct TerminalGuard {
    terminal: TuiTerminal,
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

struct TuiPrompter<'a> {
    guard: &'a mut TerminalGuard,
    app: &'a mut AppState,
}

impl ApprovalPrompter for TuiPrompter<'_> {
    fn confirm(&mut self, prompt: &str) -> Result<bool> {
        self.app.set_pending_prompt(Some(prompt.to_owned()));
        let decision = loop {
            self.guard.terminal.draw(|f| render_frame(self.app, f))?;
            if event::poll(Duration::from_millis(50))?
                && let Event::Key(key) = event::read()?
            {
                match key.code {
                    KeyCode::Char('y' | 'Y') => break true,
                    KeyCode::Char('n' | 'N') | KeyCode::Esc => break false,
                    _ => {}
                }
            }
        };
        self.app.set_pending_prompt(None);
        Ok(decision)
    }

    fn on_delta(&mut self, delta: &str) {
        self.app.append_stream_delta(delta);
        let _ = self.guard.terminal.draw(|f| render_frame(self.app, f));
    }
}

/// Runs the interactive Ratatui loop in-process until the user exits with `Esc` or `Ctrl-C`.
pub fn run_tui(
    agent: &mut AgentLoop,
    provider: &mut dyn Provider,
    app: &mut AppState,
) -> Result<()> {
    let mut guard = TerminalGuard::enter()?;
    loop {
        guard.terminal.draw(|f| render_frame(app, f))?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        if let Event::Key(key) = event::read()?
            && handle_key_event(key, agent, provider, app, &mut guard)?
        {
            break;
        }
    }
    Ok(())
}

fn handle_key_event(
    key: KeyEvent,
    agent: &mut AgentLoop,
    provider: &mut dyn Provider,
    app: &mut AppState,
    guard: &mut TerminalGuard,
) -> Result<bool> {
    if key.code == KeyCode::Esc
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
    {
        return Ok(true);
    }
    match key.code {
        KeyCode::Tab => {
            let new_mode = agent.toggle_mode()?;
            app.set_active_profile(agent.active_profile().clone());
            app.push_line(format!("[mode switched -> {new_mode:?}]"));
        }
        KeyCode::Up => app.scroll_by(-1),
        KeyCode::Down => app.scroll_by(1),
        KeyCode::Backspace => app.pop_input_char(),
        KeyCode::Char(c) => app.push_input_char(c),
        KeyCode::Enter => submit_prompt(agent, provider, app, guard)?,
        _ => {}
    }
    Ok(false)
}

fn submit_prompt(
    agent: &mut AgentLoop,
    provider: &mut dyn Provider,
    app: &mut AppState,
    guard: &mut TerminalGuard,
) -> Result<()> {
    let prompt = app.take_input();
    if prompt.trim().is_empty() {
        return Ok(());
    }
    app.push_line(format!("You: {prompt}"));
    guard.terminal.draw(|f| render_frame(app, f))?;

    let mut prompter = TuiPrompter { guard, app };
    let res = agent.run_user_prompt(&prompt, provider, &mut prompter);
    if let Err(err) = res {
        app.push_line(format!("[error: {err}]"));
    }
    Ok(())
}
