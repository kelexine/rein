// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Borderless Ratatui renderer with Citadel Owl header, slash autocomplete popup, and muted footer

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use rein_core::ModeId;

use crate::commands::SlashCommand;
use crate::ui::AppState;

const MAX_MENU_ROWS: usize = 6;
const MIN_HEIGHT_FOR_HEADER: u16 = 14;
const CITADEL_OWL: [&str; 3] = ["▜▙████▟▛", "▐▛▄██▄▜▌", " ▝▜▀▀▛▘ "];

/// Computes the number of top rows occupied by the Citadel Owl header at `total_height`.
#[must_use]
pub fn header_height(state: &AppState, total_height: u16) -> u16 {
    if total_height < MIN_HEIGHT_FOR_HEADER {
        return 0;
    }
    let notice_rows = u16::try_from(state.startup_notices().len().min(2)).unwrap_or(0);
    if notice_rows == 0 { 4 } else { 5 + notice_rows }
}

/// Renders the Citadel Owl header, transcript, optional slash popup, hairline prompt, and status bar.
pub fn render_frame(state: &AppState, frame: &mut Frame<'_>) {
    let area = frame.area();
    let hdr_rows = header_height(state, area.height);
    let matches = state.slash_matches();
    let menu_rows = u16::try_from(matches.len().min(MAX_MENU_ROWS)).unwrap_or(0);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(hdr_rows),
            Constraint::Min(3),
            Constraint::Length(menu_rows),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    if hdr_rows > 0 {
        render_header(state, frame, chunks[0]);
    }

    let max_scroll = compute_max_scroll(state.transcript(), chunks[1].width, chunks[1].height);
    let scroll = state.effective_scroll(max_scroll);
    let styled_lines: Vec<Line<'_>> = state.transcript().iter().map(|l| style_line(l)).collect();
    let transcript = Paragraph::new(styled_lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(transcript, chunks[1]);

    if menu_rows > 0 {
        render_slash_menu(&matches, state.slash_selected(), frame, chunks[2]);
    }

    render_rule(frame, chunks[3]);
    render_prompt_line(state, frame, chunks[4]);
    render_rule(frame, chunks[5]);
    render_status_bar(state, frame, chunks[6]);
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

fn render_header(state: &AppState, frame: &mut Frame<'_>, area: Rect) {
    let notice_rows = u16::try_from(state.startup_notices().len().min(2)).unwrap_or(0);
    let blocks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(notice_rows),
            Constraint::Min(0),
        ])
        .split(area);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(8),
            Constraint::Length(2),
            Constraint::Min(10),
        ])
        .split(blocks[0]);

    let owl_lines: Vec<Line<'_>> = CITADEL_OWL
        .iter()
        .map(|row| {
            Line::from(Span::styled(
                *row,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ))
        })
        .collect();
    frame.render_widget(Paragraph::new(owl_lines), cols[1]);

    let mode_label = mode_name(state.active_profile().mode);
    let info_lines = vec![
        Line::from(vec![
            Span::styled("Rein", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                concat!(" v", env!("CARGO_PKG_VERSION")),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(Span::styled(
            format!(
                "{} · {mode_label} · {}",
                state.model_name(),
                state.sandbox_summary()
            ),
            Style::default().fg(Color::Gray),
        )),
        Line::from(Span::styled(
            state.workspace_label(),
            Style::default().fg(Color::DarkGray),
        )),
    ];
    frame.render_widget(Paragraph::new(info_lines), cols[3]);

    if notice_rows > 0 {
        let notices: Vec<Line<'_>> = state
            .startup_notices()
            .iter()
            .take(2)
            .map(|msg| {
                Line::from(vec![
                    Span::styled(" ⚠ ", Style::default().fg(Color::Yellow)),
                    Span::styled(msg.as_str(), Style::default().fg(Color::Yellow)),
                ])
            })
            .collect();
        frame.render_widget(Paragraph::new(notices), blocks[2]);
    }
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
    let mode_label = mode_name(state.active_profile().mode);
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

fn mode_name(mode: ModeId) -> &'static str {
    match mode {
        ModeId::Coding => "coding",
        ModeId::Personal => "personal",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modes::Profile;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn renders_citadel_owl_header_and_slash_menu() -> std::io::Result<()> {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend)?;
        let mut state = AppState::new(Profile::coding(), "landlock v11", "claude-sonnet-4")
            .with_workspace_label("~/dev/rein")
            .with_startup_notices(vec!["ANTHROPIC_API_KEY not set".to_owned()]);
        state.set_input("/m");

        terminal.draw(|frame| render_frame(&state, frame))?;
        let buffer = terminal.backend().buffer();
        let all_text: String = (0..20)
            .map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(all_text.contains("▜▙████▟▛"));
        assert!(all_text.contains("Rein v"));
        assert!(all_text.contains("~/dev/rein"));
        assert!(all_text.contains("⚠ ANTHROPIC_API_KEY not set"));
        assert!(all_text.contains("› /mode"));
        Ok(())
    }
}
