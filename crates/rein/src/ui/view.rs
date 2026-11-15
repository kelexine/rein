// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Borderless Ratatui renderer with Citadel Owl header, Markdown transcript, /model picker, and status bar

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use rein_core::ModeId;

use crate::commands::SlashCommand;
use crate::ui::AppState;
use crate::ui::app::{PendingApproval, PendingModelPicker};
use crate::ui::entries::render_transcript_lines;

const MAX_MENU_ROWS: usize = 6;
const MIN_HEIGHT_FOR_HEADER: u16 = 14;
const CITADEL_OWL: [&str; 3] = ["▜▙████▟▛", "▐▛▄██▄▜▌", " ▝▜▀▀▛▘ "];
const SPINNER_FRAMES: [&str; 6] = ["✶", "✢", "✽", "✻", "✳", "·"];

/// Computes the number of top rows occupied by the Citadel Owl header at `total_height`.
#[must_use]
pub fn header_height(state: &AppState, total_height: u16) -> u16 {
    if total_height < MIN_HEIGHT_FOR_HEADER {
        return 0;
    }
    let notice_rows = u16::try_from(state.startup_notices().len().min(2)).unwrap_or(0);
    if notice_rows == 0 { 4 } else { 5 + notice_rows }
}

/// Renders the Citadel Owl header, styled transcript, optional popup/spinner, hairline prompt, and status bar.
pub fn render_frame(state: &AppState, frame: &mut Frame<'_>) {
    let area = frame.area();
    let hdr_rows = header_height(state, area.height);
    let matches = state.slash_matches();
    let popup_rows = compute_popup_rows(state, matches.len());
    let spinner_rows = u16::from(state.is_busy() && state.pending_approval().is_none());

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(hdr_rows),
            Constraint::Min(3),
            Constraint::Length(spinner_rows),
            Constraint::Length(popup_rows),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    if hdr_rows > 0 {
        render_header(state, frame, chunks[0]);
    }

    let styled_lines =
        render_transcript_lines(state.entries(), chunks[1].width, state.expand_outputs());
    let max_scroll = compute_lines_max_scroll(&styled_lines, chunks[1].width, chunks[1].height);
    let scroll = state.effective_scroll(max_scroll);
    let transcript = Paragraph::new(styled_lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(transcript, chunks[1]);

    if spinner_rows > 0 {
        render_busy_spinner(state, frame, chunks[2]);
    }

    if let Some(pending) = state.pending_approval() {
        render_approval_menu(pending, frame, chunks[3]);
    } else if let Some(picker) = state.pending_model_picker() {
        render_model_picker(picker, state.model_name(), frame, chunks[3]);
    } else if popup_rows > 0 {
        render_slash_menu(&matches, state.slash_selected(), frame, chunks[3]);
    }

    render_rule(frame, chunks[4]);
    render_prompt_line(state, frame, chunks[5]);
    render_rule(frame, chunks[6]);
    render_status_bar(state, frame, chunks[7]);
}

/// Computes the vertical max scroll offset for the rendered transcript inside `(width, height)`.
#[must_use]
pub fn compute_max_scroll(state: &AppState, width: u16, area_height: u16) -> u16 {
    let lines = render_transcript_lines(state.entries(), width, state.expand_outputs());
    compute_lines_max_scroll(&lines, width, area_height)
}

fn compute_popup_rows(state: &AppState, slash_count: usize) -> u16 {
    if let Some(pending) = state.pending_approval() {
        if pending.editing_feedback || !pending.feedback.is_empty() {
            6
        } else {
            5
        }
    } else if let Some(picker) = state.pending_model_picker() {
        let count = picker.items.len().min(MAX_MENU_ROWS);
        u16::try_from(count + 1).unwrap_or(2)
    } else {
        u16::try_from(slash_count.min(MAX_MENU_ROWS)).unwrap_or(0)
    }
}

fn compute_lines_max_scroll(lines: &[Line<'_>], width: u16, area_height: u16) -> u16 {
    let cols = usize::from(width.max(1));
    let total_rows: usize = lines
        .iter()
        .map(|l| {
            let len: usize = l.spans.iter().map(|s| s.content.chars().count()).sum();
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
        Line::from(vec![
            Span::styled(state.model_name(), Style::default().fg(Color::LightBlue)),
            Span::styled(" · ", Style::default().fg(Color::DarkGray)),
            Span::styled(mode_label, Style::default().fg(Color::Cyan)),
            Span::styled(" · ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                state.sandbox_summary(),
                Style::default().fg(sandbox_color(state.sandbox_summary())),
            ),
        ]),
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

fn render_busy_spinner(state: &AppState, frame: &mut Frame<'_>, area: Rect) {
    let glyph = SPINNER_FRAMES[state.spinner_tick() % SPINNER_FRAMES.len()];
    let coral = Color::Rgb(224, 130, 96);
    let line = Line::from(vec![
        Span::styled(
            format!("{glyph} "),
            Style::default().fg(coral).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            state.busy_status(),
            Style::default().fg(coral).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" (esc to interrupt)", Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_rule(frame: &mut Frame<'_>, area: Rect) {
    let rule = "─".repeat(area.width as usize);
    frame.render_widget(
        Paragraph::new(rule).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn render_model_picker(
    picker: &PendingModelPicker,
    current_model: &str,
    frame: &mut Frame<'_>,
    area: Rect,
) {
    let mut rows = Vec::with_capacity(MAX_MENU_ROWS + 1);
    rows.push(Line::from(vec![
        Span::styled(
            format!("? Models ({}) ", picker.provider_kind.as_str()),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "[↑/↓ · Enter: switch · Esc: cancel]",
            Style::default().fg(Color::DarkGray),
        ),
    ]));

    let total = picker.items.len();
    if total == 0 {
        rows.push(Line::from(Span::styled(
            "  (no models returned by server)",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(Paragraph::new(rows), area);
        return;
    }

    let clamped = picker.selected.min(total.saturating_sub(1));
    let window_start = clamped.saturating_sub(MAX_MENU_ROWS - 1);
    let window_end = (window_start + MAX_MENU_ROWS).min(total);
    let col_width = usize::from(area.width);
    let id_col = 28usize.min(col_width / 2);

    for (offset, item) in picker.items[window_start..window_end].iter().enumerate() {
        let is_selected = window_start + offset == clamped;
        let is_active = current_model.ends_with(&item.id);
        let marker = if is_selected { "› " } else { "  " };
        let padded_id = format!("{marker}{:<width$}", item.id, width = id_col);

        let id_style = if is_selected {
            Style::default()
                .fg(Color::LightBlue)
                .add_modifier(Modifier::BOLD)
        } else if is_active {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        let desc_style = if is_selected {
            Style::default().fg(Color::LightBlue)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let mut spans = vec![
            Span::styled(padded_id, id_style),
            Span::styled(format!(" {}", item.display_name), desc_style),
        ];
        if let Some(pct) = item.quota_percent {
            spans.push(Span::styled(
                format!(" · {pct}%"),
                Style::default().fg(quota_color(pct)),
            ));
        }
        if is_active {
            spans.push(Span::styled(" (active)", Style::default().fg(Color::Green)));
        }
        rows.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(rows), area);
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

fn render_approval_menu(pending: &PendingApproval, frame: &mut Frame<'_>, area: Rect) {
    let options = [
        format!("1. Allow: {}", pending.request.excerpt),
        format!("2. Allow in CWD: {}", pending.request.pattern),
        format!("3. Allow Global: {}", pending.request.pattern),
        "4. Deny".to_owned(),
    ];
    let mut rows = Vec::with_capacity(6);
    rows.push(Line::from(Span::styled(
        format!("? {}", pending.request.title),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));

    let hint = if pending.editing_feedback {
        "  [Tab: close]"
    } else {
        "  [Tab: feedback]"
    };
    for (idx, label) in options.iter().enumerate() {
        let is_selected = idx == pending.selected;
        let marker = if is_selected { "› " } else { "  " };
        let label_style = if is_selected {
            let color = if idx == 3 {
                Color::LightRed
            } else {
                Color::LightBlue
            };
            Style::default().fg(color).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        let mut spans = vec![Span::styled(format!("{marker}{label}"), label_style)];
        if is_selected {
            spans.push(Span::styled(hint, Style::default().fg(Color::DarkGray)));
        }
        rows.push(Line::from(spans));
    }

    let show_feedback = pending.editing_feedback || !pending.feedback.is_empty();
    if show_feedback {
        let label = "  feedback › ";
        rows.push(Line::from(vec![
            Span::styled(
                label,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(&pending.feedback),
        ]));
        if pending.editing_feedback && area.height >= 6 {
            let prefix_len = u16::try_from(label.chars().count()).unwrap_or(13);
            let input_len = u16::try_from(pending.feedback.chars().count()).unwrap_or(u16::MAX);
            let cx = area
                .x
                .saturating_add(prefix_len)
                .saturating_add(input_len)
                .min(area.right().saturating_sub(1));
            frame.set_cursor_position(Position::new(cx, area.y.saturating_add(5)));
        }
    }

    frame.render_widget(Paragraph::new(rows), area);
}

fn render_prompt_line(state: &AppState, frame: &mut Frame<'_>, area: Rect) {
    if let Some(pending) = state.pending_approval() {
        let (label, color) = if pending.editing_feedback {
            ("feedback [Enter: submit · Tab/Esc: back]: ", Color::Cyan)
        } else {
            ("choose [1-4 · ↑/↓ · Tab feedback · Enter]: ", Color::Yellow)
        };
        let line = Line::from(vec![
            Span::styled(
                label,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                &pending.request.excerpt,
                Style::default().fg(Color::DarkGray),
            ),
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
    let toggle_label = if state.expand_outputs() {
        "ctrl+o: collapse"
    } else {
        "ctrl+o: expand"
    };
    let right = format!("{toggle_label} · tab: mode · /: commands · esc: quit");
    let right_width = u16::try_from(right.chars().count()).unwrap_or(52);
    let bar_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(10),
            Constraint::Length(2),
            Constraint::Length(right_width),
        ])
        .split(area);

    let mut left_spans = vec![
        Span::styled(mode_label, Style::default().fg(Color::Cyan)),
        Span::styled(" · ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            state.sandbox_summary(),
            Style::default().fg(sandbox_color(state.sandbox_summary())),
        ),
        Span::styled(" · ", Style::default().fg(Color::DarkGray)),
        Span::styled(state.model_name(), Style::default().fg(Color::LightBlue)),
    ];
    let usage = state.usage_totals();
    if usage.total_tokens > 0 {
        left_spans.push(Span::styled(" · ", Style::default().fg(Color::DarkGray)));
        left_spans.push(Span::styled(
            format_tokens(usage.total_tokens),
            Style::default().fg(Color::Gray),
        ));
    }
    if let Some(pct) = state.model_quota() {
        left_spans.push(Span::styled(" · ", Style::default().fg(Color::DarkGray)));
        left_spans.push(Span::styled(
            format!("{pct}% quota"),
            Style::default().fg(quota_color(pct)),
        ));
    }

    frame.render_widget(Paragraph::new(Line::from(left_spans)), bar_cols[0]);
    frame.render_widget(
        Paragraph::new(right).style(Style::default().fg(Color::DarkGray)),
        bar_cols[2],
    );
}

fn sandbox_color(summary: &str) -> Color {
    if summary.contains("off") {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn quota_color(pct: u8) -> Color {
    if pct >= 50 {
        Color::Green
    } else if pct >= 20 {
        Color::Yellow
    } else {
        Color::LightRed
    }
}

fn format_tokens(total: u64) -> String {
    if total >= 1_000 {
        let tenths = total / 100;
        format!("{}.{}k tok", tenths / 10, tenths % 10)
    } else {
        format!("{total} tok")
    }
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
    use rein_core::AvailableModel;

    #[test]
    fn renders_citadel_owl_header_and_slash_menu() -> std::io::Result<()> {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend)?;
        let mut state = AppState::new(Profile::coding(), "landlock v11", "claude-sonnet-5-5")
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
        assert!(all_text.contains("ctrl+o: expand"));
        Ok(())
    }

    #[test]
    fn renders_four_option_approval_menu_with_tab_feedback() -> std::io::Result<()> {
        use crate::agent::ApprovalRequest;

        let backend = TestBackend::new(90, 22);
        let mut terminal = Terminal::new(backend)?;
        let mut state = AppState::new(Profile::coding(), "landlock v11", "claude-sonnet-5-5");
        state.set_pending_approval(Some(ApprovalRequest {
            title: "[Exec | Workspace] Allow 'cargo test --lib'?".to_owned(),
            excerpt: "cargo test --lib".to_owned(),
            pattern: "cargo *".to_owned(),
        }));
        state.move_approval_selection(3);
        state.toggle_approval_feedback();
        for ch in "use cargo check".chars() {
            state.push_approval_feedback_char(ch);
        }

        terminal.draw(|frame| render_frame(&state, frame))?;
        let buffer = terminal.backend().buffer();
        let all_text: String = (0..22)
            .map(|y| (0..90).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(all_text.contains("1. Allow: cargo test --lib"));
        assert!(all_text.contains("2. Allow in CWD: cargo *"));
        assert!(all_text.contains("3. Allow Global: cargo *"));
        assert!(all_text.contains("› 4. Deny"));
        assert!(all_text.contains("feedback › use cargo check"));
        Ok(())
    }

    #[test]
    fn renders_model_picker_with_server_quota_and_busy_spinner() -> std::io::Result<()> {
        let backend = TestBackend::new(100, 22);
        let mut terminal = Terminal::new(backend)?;
        let mut state = AppState::new(
            Profile::coding(),
            "landlock v11",
            "antigravity/claude-sonnet-5-5-medium",
        );
        state.set_busy(true);
        state.open_model_picker(
            vec![
                AvailableModel {
                    id: "claude-sonnet-5-5-medium".to_owned(),
                    display_name: "Claude Sonnet 5.5 (Medium)".to_owned(),
                    quota_percent: Some(98),
                    recommended: true,
                },
                AvailableModel {
                    id: "gemini-4-argon-medium".to_owned(),
                    display_name: "Gemini 4 Argon (Medium)".to_owned(),
                    quota_percent: Some(45),
                    recommended: false,
                },
            ],
            crate::config::ProviderKind::Antigravity,
            "antigravity/claude-sonnet-5-5-medium",
        );

        terminal.draw(|frame| render_frame(&state, frame))?;
        let buffer = terminal.backend().buffer();
        let all_text: String = (0..22)
            .map(|y| {
                (0..100)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(all_text.contains("Thinking… (esc to interrupt)"));
        assert!(all_text.contains("Models (antigravity)"));
        assert!(all_text.contains("Claude Sonnet 5.5 (Medium) · 98% (active)"));
        assert!(all_text.contains("Gemini 4 Argon (Medium) · 45%"));
        Ok(())
    }
}
