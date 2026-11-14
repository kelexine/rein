// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Minimalist borderless Ratatui renderer with slash autocomplete popup, prompt rules, and muted status bar

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use rein_core::ModeId;

use crate::commands::SlashCommand;
use crate::ui::AppState;

const MAX_MENU_ROWS: usize = 6;

/// Renders the borderless [`AppState`] and optional slash autocomplete popup into `frame`.
pub fn render_frame(state: &AppState, frame: &mut Frame<'_>) {
    let matches = state.slash_matches();
    let menu_rows = u16::try_from(matches.len().min(MAX_MENU_ROWS)).unwrap_or(0);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(menu_rows),
            Constraint::Length(1),
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

    if menu_rows > 0 {
        render_slash_menu(&matches, state.slash_selected(), frame, chunks[1]);
    }

    render_rule(frame, chunks[2]);
    render_prompt_line(state, frame, chunks[3]);
    render_rule(frame, chunks[4]);
    render_status_bar(state, frame, chunks[5]);
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

fn render_rule(frame: &mut Frame<'_>, area: Rect) {
    let rule = "─".repeat(area.width as usize);
    frame.render_widget(
        Paragraph::new(rule).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn render_slash_menu(
    matches: &[&SlashCommand],
    selected: usize,
    frame: &mut Frame<'_>,
    area: Rect,
) {
    let total = matches.len();
    let clamped = selected.min(total.saturating_sub(1));
    let window_start = clamped.saturating_sub(MAX_MENU_ROWS - 1);
    let window_end = (window_start + MAX_MENU_ROWS).min(total);
    let col_width = usize::from(area.width);
    let name_col = 22usize.min(col_width / 2);

    let mut rows = Vec::with_capacity(window_end - window_start);
    for (offset, cmd) in matches[window_start..window_end].iter().enumerate() {
        let is_selected = window_start + offset == clamped;
        let marker = if is_selected { "› " } else { "  " };
        let token = format!("{marker}/{}", cmd.name);
        let padded_name = format!("{token:<width$}", width = name_col);

        let desc_budget = col_width.saturating_sub(name_col + 1);
        let badge = cmd
            .source
            .badge()
            .map(|b| format!(" ({b})"))
            .unwrap_or_default();
        let desc = format_menu_desc(&cmd.description, &badge, desc_budget);

        let (name_style, desc_style) = if is_selected {
            (
                Style::default()
                    .fg(Color::LightBlue)
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(Color::LightBlue),
            )
        } else {
            (
                Style::default().fg(Color::Gray),
                Style::default().fg(Color::DarkGray),
            )
        };
        rows.push(Line::from(vec![
            Span::styled(padded_name, name_style),
            Span::styled(desc, desc_style),
        ]));
    }
    frame.render_widget(Paragraph::new(rows), area);
}

fn format_menu_desc(description: &str, badge: &str, max_width: usize) -> String {
    let combined = format!("{description}{badge}");
    if combined.chars().count() <= max_width {
        return combined;
    }
    let keep = max_width.saturating_sub(badge.chars().count() + 1);
    let clipped: String = description.chars().take(keep).collect();
    format!("{clipped}…{badge}")
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

fn render_prompt_line(state: &AppState, frame: &mut Frame<'_>, area: Rect) {
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

fn render_status_bar(state: &AppState, frame: &mut Frame<'_>, area: Rect) {
    let mode_label = match state.active_profile().mode {
        ModeId::Coding => "coding",
        ModeId::Personal => "personal",
    };
    let left = format!(
        "{mode_label} · {} · {}",
        state.sandbox_summary(),
        state.model_name()
    );
    let right = "tab: mode · /: commands · esc: quit";
    let right_width = u16::try_from(right.chars().count()).unwrap_or(36);
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
