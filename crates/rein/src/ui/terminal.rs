// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Crossterm raw-mode guard, slash command dispatcher, and non-blocking Ratatui loop

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
use rein_core::{MemoryScope, ModeId, Provider};

use crate::agent::{AgentLoop, ApprovalPrompter};
use crate::commands::{BuiltinAction, SlashKind, SlashResolution};
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
    Note(String),
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

/// Runs the minimalist Ratatui loop in-process until the user exits with `Esc`, `Ctrl-C`, or `/quit`.
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
    let slash_open = !app.slash_matches().is_empty();
    match key.code {
        KeyCode::Tab if slash_open => {
            if let Some(sel) = app.selected_slash() {
                let completed = format!("/{} ", sel.name);
                app.set_input(completed);
            }
        }
        KeyCode::Tab => {
            let _ = agent.toggle_mode()?;
            app.set_active_profile(agent.active_profile().clone());
        }
        KeyCode::Up if slash_open => app.move_slash_selection(-1),
        KeyCode::Down if slash_open => app.move_slash_selection(1),
        KeyCode::Up => scroll_app(app, guard, -1)?,
        KeyCode::Down => scroll_app(app, guard, 1)?,
        KeyCode::PageUp => scroll_app(app, guard, -8)?,
        KeyCode::PageDown => scroll_app(app, guard, 8)?,
        KeyCode::Backspace => app.pop_input_char(),
        KeyCode::Char(c) => app.push_input_char(c),
        KeyCode::Enter => return handle_enter(agent, provider, app, guard),
        _ => {}
    }
    Ok(false)
}

fn handle_enter(
    agent: &mut AgentLoop,
    provider: &mut dyn Provider,
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
    if trimmed.starts_with('/') {
        return dispatch_slash(trimmed, agent, provider, app, guard);
    }
    stream_prompt_background(
        trimmed.to_owned(),
        trimmed.to_owned(),
        agent,
        provider,
        app,
        guard,
    )
}

fn dispatch_slash(
    raw: &str,
    agent: &mut AgentLoop,
    provider: &mut dyn Provider,
    app: &mut AppState,
    guard: &mut TerminalGuard,
) -> Result<bool> {
    let registry = app.slash_registry().clone();
    match registry.resolve(raw) {
        SlashResolution::Unknown(name) => {
            app.push_line(format!("! unknown command: /{name} (try /help)"));
            Ok(false)
        }
        SlashResolution::Matched { command, args } => match &command.kind {
            SlashKind::Builtin(action) => {
                run_builtin_action(*action, &args, agent, provider, app, guard)
            }
            SlashKind::Skill { .. } => {
                let prompt = command
                    .expand_skill_prompt(&args)
                    .unwrap_or_else(|| raw.to_owned());
                stream_prompt_background(raw.to_owned(), prompt, agent, provider, app, guard)
            }
        },
    }
}

fn run_builtin_action(
    action: BuiltinAction,
    args: &str,
    agent: &mut AgentLoop,
    provider: &mut dyn Provider,
    app: &mut AppState,
    guard: &mut TerminalGuard,
) -> Result<bool> {
    match action {
        BuiltinAction::Quit => return Ok(true),
        BuiltinAction::Clear => {
            agent.clear_history();
            app.clear_transcript();
        }
        BuiltinAction::Help => {
            let lines: Vec<String> = app
                .slash_registry()
                .all()
                .iter()
                .map(|c| match c.source.badge() {
                    Some(b) => format!("· /{:<14} {} ({b})", c.name, c.description),
                    None => format!("· /{:<14} {}", c.name, c.description),
                })
                .collect();
            for line in lines {
                app.push_line(line);
            }
        }
        BuiltinAction::Mode => run_mode_command(args, agent, app)?,
        BuiltinAction::Compact => return compact_background(args, agent, provider, app, guard),
        BuiltinAction::Memory => run_memory_command(args, agent, app)?,
        BuiltinAction::Commands => {
            if app.configured_commands().is_empty() {
                app.push_line("· commands.json allowlist: (none)");
            } else {
                app.push_line(format!(
                    "· commands.json allowlist: {}",
                    app.configured_commands().join(", ")
                ));
            }
        }
        BuiltinAction::Mcp => run_mcp_command(agent, app),
        BuiltinAction::Resume => {
            let lines = agent.hydrate_from_log()?;
            app.clear_transcript();
            app.extend_transcript(lines);
            app.set_active_profile(agent.active_profile().clone());
        }
    }
    Ok(false)
}

fn run_mode_command(args: &str, agent: &mut AgentLoop, app: &mut AppState) -> Result<()> {
    let mode = match args.trim().to_ascii_lowercase().as_str() {
        "" => agent.toggle_mode()?,
        "coding" | "code" => agent.set_mode(ModeId::Coding)?,
        "personal" => agent.set_mode(ModeId::Personal)?,
        other => {
            app.push_line(format!("! unknown mode '{other}' (use coding or personal)"));
            return Ok(());
        }
    };
    app.set_active_profile(agent.active_profile().clone());
    app.push_line(format!("· mode: {mode:?}"));
    Ok(())
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
            app.push_line(format!("· [{:?}] {}: {}", rec.scope, rec.key, rec.value));
        }
    }
    if found == 0 {
        app.push_line("· memory: (no matching records)");
    }
    Ok(())
}

fn run_mcp_command(agent: &AgentLoop, app: &mut AppState) {
    if app.mcp_servers().is_empty() {
        app.push_line("· mcp servers: (none configured)");
    } else {
        let server_lines: Vec<String> = app
            .mcp_servers()
            .iter()
            .map(|srv| format!("· mcp {}: {} {}", srv.name, srv.command, srv.args.join(" ")))
            .collect();
        for line in server_lines {
            app.push_line(line);
        }
    }
    app.push_line(format!(
        "· active tools: {}",
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
    app.push_line("· compacting conversation via provider…");
    app.set_busy(true);
    guard.terminal.draw(|f| render_frame(app, f))?;

    let (ev_tx, ev_rx) = mpsc::channel::<WorkerEvent>();
    let (_ans_tx, ans_rx) = mpsc::channel::<bool>();
    let mut should_quit = false;

    std::thread::scope(|scope| {
        scope.spawn(move || {
            let outcome = match agent.compact_with_provider(focus.as_deref(), provider) {
                Ok(msg) => {
                    let _ = ev_tx.send(WorkerEvent::Note(format!("· {msg}")));
                    Ok(())
                }
                Err(err) => Err(err),
            };
            let _ = ev_tx.send(WorkerEvent::Finished(outcome));
        });

        while app.is_busy() {
            drain_worker_events(&ev_rx, app);
            let _ = guard.terminal.draw(|f| render_frame(app, f));
            if !app.is_busy() {
                break;
            }
            if event::poll(Duration::from_millis(16)).unwrap_or(false)
                && let Ok(Event::Key(key)) = event::read()
                && is_quit_key(key)
            {
                should_quit = true;
                break;
            }
        }
        drop(ans_rx);
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
    app.push_line(format!("❯ {display_label}"));
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
                .run_user_prompt(&prompt_for_model, provider, &mut prompter)
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
            WorkerEvent::Note(note) => app.push_line(note),
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
    let transcript_height = size.height.saturating_sub(4);
    let max_scroll = compute_max_scroll(app.transcript(), size.width, transcript_height);
    app.scroll_by(delta, max_scroll);
    Ok(())
}

fn is_quit_key(key: KeyEvent) -> bool {
    key.code == KeyCode::Esc
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
}
