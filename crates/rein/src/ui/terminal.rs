// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Crossterm raw-mode guard and non-blocking scoped-thread Ratatui event loop

use std::io::{self, Stdout};
use std::sync::mpsc::{self, Receiver, Sender};
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
use crate::ui::view::compute_max_scroll;
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

enum WorkerEvent {
    Delta(String),
    ToolStatus(String),
    AskConfirm(String),
    Finished(Result<()>),
}

struct ChannelPrompter {
    tx: Sender<WorkerEvent>,
    confirm_rx: Receiver<bool>,
}

impl ApprovalPrompter for ChannelPrompter {
    fn confirm(&mut self, prompt: &str) -> Result<bool> {
        let _ = self.tx.send(WorkerEvent::AskConfirm(prompt.to_owned()));
        Ok(self.confirm_rx.recv().unwrap_or(false))
    }

    fn on_delta(&mut self, delta: &str) {
        let _ = self.tx.send(WorkerEvent::Delta(delta.to_owned()));
    }

    fn on_tool_status(&mut self, status: &str) {
        let _ = self.tx.send(WorkerEvent::ToolStatus(status.to_owned()));
    }
}

/// Runs the minimalist Ratatui loop in-process until the user exits with `Esc` or `Ctrl-C`.
pub fn run_tui(
    agent: &mut AgentLoop,
    provider: &mut dyn Provider,
    app: &mut AppState,
) -> Result<()> {
    let mut guard = TerminalGuard::enter()?;
    loop {
        guard.terminal.draw(|f| render_frame(app, f))?;
        if !event::poll(Duration::from_millis(50))? {
            continue;
        }
        if let Event::Key(key) = event::read()?
            && handle_idle_key(key, agent, provider, app, &mut guard)?
        {
            break;
        }
    }
    Ok(())
}

fn handle_idle_key(
    key: KeyEvent,
    agent: &mut AgentLoop,
    provider: &mut dyn Provider,
    app: &mut AppState,
    guard: &mut TerminalGuard,
) -> Result<bool> {
    if is_quit_key(key) {
        return Ok(true);
    }
    match key.code {
        KeyCode::Tab => {
            let _ = agent.toggle_mode()?;
            app.set_active_profile(agent.active_profile().clone());
        }
        KeyCode::Up => scroll_app(app, guard, -1)?,
        KeyCode::Down => scroll_app(app, guard, 1)?,
        KeyCode::PageUp => scroll_app(app, guard, -8)?,
        KeyCode::PageDown => scroll_app(app, guard, 8)?,
        KeyCode::Backspace => app.pop_input_char(),
        KeyCode::Char(c) => app.push_input_char(c),
        KeyCode::Enter => {
            let prompt = app.take_input();
            if !prompt.trim().is_empty() {
                return stream_prompt_background(prompt, agent, provider, app, guard);
            }
        }
        _ => {}
    }
    Ok(false)
}

fn stream_prompt_background(
    prompt: String,
    agent: &mut AgentLoop,
    provider: &mut dyn Provider,
    app: &mut AppState,
    guard: &mut TerminalGuard,
) -> Result<bool> {
    app.push_line(format!("❯ {prompt}"));
    app.set_busy(true);
    guard.terminal.draw(|f| render_frame(app, f))?;

    let (ev_tx, ev_rx) = mpsc::channel::<WorkerEvent>();
    let (ans_tx, ans_rx) = mpsc::channel::<bool>();
    let mut should_quit = false;

    std::thread::scope(|scope| {
        scope.spawn(move || {
            let mut prompter = ChannelPrompter {
                tx: ev_tx.clone(),
                confirm_rx: ans_rx,
            };
            let res = agent
                .run_user_prompt(&prompt, provider, &mut prompter)
                .map(|_| ());
            let _ = ev_tx.send(WorkerEvent::Finished(res));
        });

        while app.is_busy() {
            drain_worker_events(&ev_rx, app);
            let _ = guard.terminal.draw(|f| render_frame(app, f));
            if !app.is_busy() {
                break;
            }
            if event::poll(Duration::from_millis(16)).unwrap_or(false)
                && let Ok(Event::Key(key)) = event::read()
                && handle_streaming_key(key, app, guard, &ans_tx)
            {
                should_quit = true;
                let _ = ans_tx.send(false);
                break;
            }
        }
    });

    app.set_pending_prompt(None);
    app.set_busy(false);
    Ok(should_quit)
}

fn drain_worker_events(ev_rx: &Receiver<WorkerEvent>, app: &mut AppState) {
    while let Ok(ev) = ev_rx.try_recv() {
        match ev {
            WorkerEvent::Delta(delta) => app.append_stream_delta(&delta),
            WorkerEvent::ToolStatus(status) => app.push_line(status),
            WorkerEvent::AskConfirm(prompt) => app.set_pending_prompt(Some(prompt)),
            WorkerEvent::Finished(res) => {
                if let Err(err) = res {
                    app.push_line(format!("! {err}"));
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
    ans_tx: &Sender<bool>,
) -> bool {
    if is_quit_key(key) {
        return true;
    }
    if app.pending_prompt().is_some() {
        match key.code {
            KeyCode::Char('y' | 'Y') => {
                app.set_pending_prompt(None);
                let _ = ans_tx.send(true);
            }
            KeyCode::Char('n' | 'N') => {
                app.set_pending_prompt(None);
                let _ = ans_tx.send(false);
            }
            _ => {}
        }
        return false;
    }
    match key.code {
        KeyCode::Up => {
            let _ = scroll_app(app, guard, -1);
        }
        KeyCode::Down => {
            let _ = scroll_app(app, guard, 1);
        }
        KeyCode::Backspace => app.pop_input_char(),
        KeyCode::Char(c) => app.push_input_char(c),
        _ => {}
    }
    false
}

fn scroll_app(app: &mut AppState, guard: &TerminalGuard, delta: i16) -> Result<()> {
    let size = guard.terminal.size()?;
    let transcript_height = size.height.saturating_sub(3);
    let max_scroll = compute_max_scroll(app.transcript(), size.width, transcript_height);
    app.scroll_by(delta, max_scroll);
    Ok(())
}

fn is_quit_key(key: KeyEvent) -> bool {
    key.code == KeyCode::Esc
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
}
