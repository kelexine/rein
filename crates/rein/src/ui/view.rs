// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Minimalist borderless Ratatui renderer with styled transcript, inline prompt, and muted status line

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Position};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use rein_core::ModeId;

use crate::ui::AppState;

/// Renders the borderless [`AppState`] into a Ratatui [`Frame`].
pub fn render_frame(state: &AppState, frame: &mut Frame<'_>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let max_scroll = compute_max_scroll(state.transcript(), chunks[0].width, chunks[0].height);
    let scroll = state.effective_scroll(max_scroll);

    let styled_lines: Vec<Line<'_>> = state.transcript().iter().map(|l| style_line(l)).collect();
    let transcript = Paragraph::new(styled_lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(transcript, chunks[0]);

    let rule = "─".repeat(chunks[1].width as usize);
    frame.render_widget(
        Paragraph::new(rule).style(Style::default().fg(Color::DarkGray)),
        chunks[1],
    );

    render_prompt_line(state, frame, chunks[2]);
    render_status_bar(state, frame, chunks[3]);
}

/// Estimates the maximum scroll offset for wrapped transcript lines in `area_height`.
#[must_use]
pub fn compute_max_scroll(lines: &[String], width: u16, area_height: u16) -> u16 {
    let cols = usize::from(width.max(1));
    let total_rows: usize = lines
        .iter()
        .map(|l| {
            let len = l.chars().count();
            if len == 0 { 1 } else { len.div_ceil(cols) }
        })
        .sum();
    let overflow = total_rows.saturating_sub(usize::from(area_height));
    u16::try_from(overflow).unwrap_or(u16::MAX)
}

fn style_line(line: &str) -> Line<'_> {
    if let Some(rest) = line.strip_prefix("❯ ") {
        Line::from(vec![
            Span::styled(
                "❯ ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(rest, Style::default().add_modifier(Modifier::BOLD)),
        ])
    } else if line.starts_with("· ") {
        Line::from(Span::styled(line, Style::default().fg(Color::DarkGray)))
    } else if line.starts_with("! ") {
        Line::from(Span::styled(line, Style::default().fg(Color::Red)))
    } else {
        Line::from(line)
    }
}

fn render_prompt_line(state: &AppState, frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
    if let Some(prompt) = state.pending_prompt() {
        let line = Line::from(vec![
            Span::styled(
                "allow [y/n]: ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(prompt),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    let (glyph, glyph_style) = if state.is_busy() {
        ("⋯ ", Style::default().fg(Color::DarkGray))
    } else {
        (
            "❯ ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    };
    let line = Line::from(vec![
        Span::styled(glyph, glyph_style),
        Span::raw(state.input()),
    ]);
    frame.render_widget(Paragraph::new(line), area);

    let cursor_offset = u16::try_from(state.input().chars().count()).unwrap_or(u16::MAX);
    let cursor_x = area
        .x
        .saturating_add(2)
        .saturating_add(cursor_offset)
        .min(area.right().saturating_sub(1));
    frame.set_cursor_position(Position::new(cursor_x, area.y));
}

fn render_status_bar(state: &AppState, frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
    let mode_label = match state.active_profile().mode {
        ModeId::Coding => "coding",
        ModeId::Personal => "personal",
    };
    let left = format!(
        "{mode_label} · {} · {}",
        state.sandbox_summary(),
        state.model_name()
    );
    let right = "tab: mode · ↑↓: scroll · esc: quit";
    let right_width = u16::try_from(right.chars().count()).unwrap_or(34);
    let bar_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(10),
            Constraint::Length(2),
            Constraint::Length(right_width),
        ])
        .split(area);

    let muted = Style::default().fg(Color::DarkGray);
    frame.render_widget(Paragraph::new(left).style(muted), bar_cols[0]);
    frame.render_widget(Paragraph::new(right).style(muted), bar_cols[2]);
}
