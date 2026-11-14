// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Ratatui frame rendering for mode header, transcript pane, input box, and policy gate

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::ui::AppState;

/// Renders the current [`AppState`] into a Ratatui [`Frame`].
pub fn render_frame(state: &AppState, frame: &mut Frame<'_>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .split(frame.area());

    let profile = state.active_profile();
    let header_text = format!(
        "Mode: {:?} | Tools: {} | Memory: {:?}",
        profile.mode,
        profile.allowed_tools.join(", "),
        profile.readable_memory_scopes
    );
    let header = Paragraph::new(header_text)
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .block(Block::default().borders(Borders::ALL).title("Rein"));
    frame.render_widget(header, chunks[0]);

    let body_text = state.transcript().join("\n");
    let transcript = Paragraph::new(body_text)
        .wrap(Wrap { trim: false })
        .scroll((state.scroll_offset(), 0))
        .block(Block::default().borders(Borders::ALL).title("Session"));
    frame.render_widget(transcript, chunks[1]);

    let input_box = Paragraph::new(format!("> {}", state.input()))
        .block(Block::default().borders(Borders::ALL).title("Prompt"));
    frame.render_widget(input_box, chunks[2]);

    let (footer_text, footer_color) = if let Some(prompt) = state.pending_prompt() {
        (format!("CONFIRM [y/n]: {prompt}"), Color::Yellow)
    } else {
        (
            format!(
                "{} | Enter: send | Tab: mode | Up/Down: scroll | Esc: quit",
                state.sandbox_summary()
            ),
            Color::Green,
        )
    };
    let footer = Paragraph::new(footer_text)
        .style(Style::default().fg(footer_color))
        .block(Block::default().borders(Borders::ALL).title("Policy Gate"));
    frame.render_widget(footer, chunks[3]);
}
