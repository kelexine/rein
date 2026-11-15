// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Borderless Ratatui renderer with Citadel Owl header, Markdown transcript, /model picker, and status bar

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use rein_core::ModeId;

use crate::commands::SlashCommand;
use crate::ui::AppState;
use crate::ui::app::{
    PendingApproval, PendingModelPicker, PendingSessionPicker, model_vendor_group,
};
use crate::ui::entries::render_transcript_lines;

const MAX_MENU_ROWS: usize = 6;
const MAX_MODEL_MENU_ROWS: usize = 10;
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

fn split_layout(state: &AppState, area: Rect) -> std::rc::Rc<[Rect]> {
    let hdr_rows = header_height(state, area.height);
    let matches = state.slash_matches();
    let popup_rows = compute_popup_rows(state, matches.len());
    let spinner_rows = u16::from(state.is_busy() && state.pending_approval().is_none());

    Layout::default()
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
        .split(area)
}

/// Returns the exact transcript viewport [`Rect`] computed identically to [`render_frame`].
#[must_use]
pub fn transcript_viewport(state: &AppState, area: Rect) -> Rect {
    split_layout(state, area)[1]
}

/// Renders the Citadel Owl header, styled transcript, optional popup/spinner, hairline prompt, and status bar.
pub fn render_frame(state: &AppState, frame: &mut Frame<'_>) {
    let area = frame.area();
    let hdr_rows = header_height(state, area.height);
    let matches = state.slash_matches();
    let chunks = split_layout(state, area);

    if hdr_rows > 0 {
        render_header(state, frame, chunks[0]);
    }

    let styled_lines =
        render_transcript_lines(state.entries(), chunks[1].width, state.expand_outputs());
    let wrapped_lines = wrap_styled_lines(styled_lines, chunks[1].width);
    let max_scroll = compute_wrapped_max_scroll(wrapped_lines.len(), chunks[1].height);
    let scroll = state.effective_scroll(max_scroll);
    frame.render_widget(Paragraph::new(wrapped_lines).scroll((scroll, 0)), chunks[1]);

    if chunks[2].height > 0 {
        render_busy_spinner(state, frame, chunks[2]);
    }

    if let Some(pending) = state.pending_approval() {
        render_approval_menu(pending, frame, chunks[3]);
    } else if let Some(picker) = state.pending_model_picker() {
        render_model_picker(picker, state.model_name(), frame, chunks[3]);
    } else if let Some(picker) = state.pending_session_picker() {
        render_session_picker(
            picker,
            state.workspace_label(),
            state.session_id(),
            frame,
            chunks[3],
        );
    } else if chunks[3].height > 0 {
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
    let wrapped = wrap_styled_lines(lines, width);
    compute_wrapped_max_scroll(wrapped.len(), area_height)
}

fn compute_popup_rows(state: &AppState, slash_count: usize) -> u16 {
    if let Some(pending) = state.pending_approval() {
        if pending.editing_feedback || !pending.feedback.is_empty() {
            6
        } else {
            5
        }
    } else if let Some(picker) = state.pending_model_picker() {
        let group_headers = count_model_groups(picker);
        let body_rows = (picker.items.len() + group_headers).clamp(1, MAX_MODEL_MENU_ROWS);
        u16::try_from(body_rows + 1).unwrap_or(2)
    } else if let Some(picker) = state.pending_session_picker() {
        let count = picker.items.len().clamp(1, MAX_MENU_ROWS);
        u16::try_from(count + 1).unwrap_or(2)
    } else {
        u16::try_from(slash_count.min(MAX_MENU_ROWS)).unwrap_or(0)
    }
}

fn count_model_groups(picker: &PendingModelPicker) -> usize {
    let mut last_group: Option<&'static str> = None;
    let mut count = 0usize;
    for item in &picker.items {
        let group = model_vendor_group(&item.id).0;
        if last_group != Some(group) {
            count += 1;
            last_group = Some(group);
        }
    }
    count
}

fn compute_wrapped_max_scroll(total_rows: usize, area_height: u16) -> u16 {
    let overflow = total_rows.saturating_sub(usize::from(area_height));
    u16::try_from(overflow).unwrap_or(u16::MAX)
}

fn wrap_styled_lines(lines: Vec<Line<'_>>, width: u16) -> Vec<Line<'_>> {
    let cols = usize::from(width.max(1));
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        if line.width() <= cols {
            out.push(line);
            continue;
        }
        wrap_single_line(line, cols, &mut out);
    }
    out
}

fn wrap_single_line<'a>(line: Line<'a>, cols: usize, out: &mut Vec<Line<'a>>) {
    let mut row: Vec<Span<'a>> = Vec::new();
    let mut used = 0usize;

    for span in line.spans {
        let style = span.style;
        for token in split_tokens(span.content.as_ref()) {
            let tok_width = Span::raw(token).width();
            let is_ws = token.chars().all(char::is_whitespace);

            if is_ws {
                if used > 0 && used + tok_width <= cols {
                    row.push(Span::styled(token.to_owned(), style));
                    used += tok_width;
                }
                continue;
            }

            if used > 0 && used + tok_width > cols {
                out.push(Line::from(std::mem::take(&mut row)));
                used = 0;
            }

            if tok_width <= cols {
                row.push(Span::styled(token.to_owned(), style));
                used += tok_width;
                continue;
            }

            push_long_token(token, style, cols, &mut row, &mut used, out);
        }
    }

    if !row.is_empty() || out.is_empty() {
        out.push(Line::from(row));
    }
}

fn push_long_token<'a>(
    token: &str,
    style: Style,
    cols: usize,
    row: &mut Vec<Span<'a>>,
    used: &mut usize,
    out: &mut Vec<Line<'a>>,
) {
    let mut chunk = String::new();
    let mut chunk_w = 0usize;
    for ch in token.chars() {
        let mut buf = [0u8; 4];
        let ch_w = Span::raw(&*ch.encode_utf8(&mut buf)).width();
        if *used + chunk_w + ch_w > cols && (*used > 0 || chunk_w > 0) {
            if !chunk.is_empty() {
                row.push(Span::styled(std::mem::take(&mut chunk), style));
                chunk_w = 0;
            }
            out.push(Line::from(std::mem::take(row)));
            *used = 0;
        }
        chunk.push(ch);
        chunk_w += ch_w;
    }
    if !chunk.is_empty() {
        *used += chunk_w;
        row.push(Span::styled(chunk, style));
    }
}

fn split_tokens(text: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = 0usize;
    let mut last_ws: Option<bool> = None;
    for (idx, ch) in text.char_indices() {
        let ws = ch.is_whitespace();
        if let Some(prev_ws) = last_ws
            && prev_ws != ws
        {
            tokens.push(&text[start..idx]);
            start = idx;
        }
        last_ws = Some(ws);
    }
    if start < text.len() {
        tokens.push(&text[start..]);
    }
    tokens
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
        Line::from(if state.session_id().is_empty() {
            vec![Span::styled(
                state.workspace_label(),
                Style::default().fg(Color::DarkGray),
            )]
        } else {
            vec![
                Span::styled(
                    state.workspace_label(),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(" · ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("session {}", state.session_id()),
                    Style::default().fg(Color::Gray),
                ),
            ]
        }),
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
    let title = Line::from(vec![
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
    ]);

    let total = picker.items.len();
    if total == 0 {
        frame.render_widget(
            Paragraph::new(vec![
                title,
                Line::from(Span::styled(
                    "  (no models returned by server)",
                    Style::default().fg(Color::DarkGray),
                )),
            ]),
            area,
        );
        return;
    }

    let clamped = picker.selected.min(total.saturating_sub(1));
    let col_width = usize::from(area.width);
    let id_col = 28usize.min(col_width / 2);

    let mut body_rows = Vec::with_capacity(total + 4);
    let mut selected_row = 0usize;
    let mut last_group: Option<&'static str> = None;

    for (idx, item) in picker.items.iter().enumerate() {
        let group = model_vendor_group(&item.id).0;
        if last_group != Some(group) {
            body_rows.push(Line::from(Span::styled(
                format!("  {group}"),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));
            last_group = Some(group);
        }

        let is_selected = idx == clamped;
        if is_selected {
            selected_row = body_rows.len();
        }
        let is_active = current_model.ends_with(&item.id);
        let marker = if is_selected { "  › " } else { "    " };
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
        body_rows.push(Line::from(spans));
    }

    let body_cap = usize::from(area.height.saturating_sub(1)).min(MAX_MODEL_MENU_ROWS);
    let window_start = if body_cap == 0 {
        0
    } else {
        selected_row
            .saturating_sub(body_cap.saturating_sub(1))
            .min(body_rows.len().saturating_sub(body_cap))
    };
    let window_end = (window_start + body_cap).min(body_rows.len());

    let mut rows = Vec::with_capacity(body_cap + 1);
    rows.push(title);
    rows.extend(body_rows.drain(window_start..window_end));
    frame.render_widget(Paragraph::new(rows), area);
}

fn render_session_picker(
    picker: &PendingSessionPicker,
    workspace_label: &str,
    current_session_id: &str,
    frame: &mut Frame<'_>,
    area: Rect,
) {
    let mut rows = Vec::with_capacity(MAX_MENU_ROWS + 1);
    rows.push(Line::from(vec![
        Span::styled(
            format!("? Sessions ({workspace_label}) "),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "[↑/↓ · Enter: resume · Esc: cancel]",
            Style::default().fg(Color::DarkGray),
        ),
    ]));

    let total = picker.items.len();
    if total == 0 {
        rows.push(Line::from(Span::styled(
            "  (no saved sessions in this directory)",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(Paragraph::new(rows), area);
        return;
    }

    let clamped = picker.selected.min(total.saturating_sub(1));
    let window_start = clamped.saturating_sub(MAX_MENU_ROWS - 1);
    let window_end = (window_start + MAX_MENU_ROWS).min(total);

    for (offset, item) in picker.items[window_start..window_end].iter().enumerate() {
        let is_selected = window_start + offset == clamped;
        let is_active = item.uid == current_session_id;
        let marker = if is_selected { "› " } else { "  " };
        let uid_col = format!("{marker}{:<10}", item.uid);

        let uid_style = if is_selected {
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
        let preview = item.preview.as_deref().unwrap_or("(empty session)");
        let turns_label = if item.turn_count == 1 {
            "1 turn".to_owned()
        } else {
            format!("{} turns", item.turn_count)
        };
        let mut spans = vec![
            Span::styled(uid_col, uid_style),
            Span::styled(
                format!(" {preview}"),
                if is_selected {
                    Style::default().fg(Color::LightBlue)
                } else {
                    Style::default().fg(Color::DarkGray)
                },
            ),
            Span::styled(
                format!(" · {turns_label}"),
                Style::default().fg(Color::DarkGray),
            ),
        ];
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
        assert!(all_text.contains("Google Gemini"));
        assert!(all_text.contains("Anthropic Claude"));
        assert!(all_text.contains("Claude Sonnet 5.5 (Medium) · 98% (active)"));
        assert!(all_text.contains("Gemini 4 Argon (Medium) · 45%"));
        Ok(())
    }

    #[test]
    fn compute_max_scroll_matches_word_wrapped_paragraph_bottom() -> std::io::Result<()> {
        let backend = TestBackend::new(40, 14);
        let mut terminal = Terminal::new(backend)?;
        let mut state = AppState::new(Profile::coding(), "off", "antigravity/claude-sonnet-5-5");
        state.push_user_turn("explain wrapping behavior across narrow viewports");
        state.append_stream_delta(
            "wordone wordtwo wordthree wordfour wordfive wordsix wordseven wordeight wordnine \
             wordten wordeleven wordtwelve wordthirteen wordfourteen wordfifteen wordsixteen \
             wordseventeen wordeighteen wordnineteen wordtwenty tail_end_marker",
        );

        let viewport = transcript_viewport(&state, Rect::new(0, 0, 40, 14));
        let max_scroll = compute_max_scroll(&state, viewport.width, viewport.height);
        assert!(max_scroll > 0);
        assert_eq!(state.effective_scroll(max_scroll), max_scroll);

        terminal.draw(|frame| render_frame(&state, frame))?;
        let buffer = terminal.backend().buffer();
        let all_text: String = (0..14)
            .map(|y| (0..40).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all_text.contains("tail_end_marker"));
        Ok(())
    }

    #[test]
    fn renders_interactive_resume_session_picker() -> std::io::Result<()> {
        let backend = TestBackend::new(90, 20);
        let mut terminal = Terminal::new(backend)?;
        let mut state = AppState::new(
            Profile::coding(),
            "landlock v11",
            "antigravity/claude-sonnet-5-5",
        )
        .with_workspace_label("~/dev/rein")
        .with_session_id("a1b2c3d4");
        state.open_session_picker(vec![
            crate::config::SessionSummary {
                uid: "a1b2c3d4".to_owned(),
                event_log: std::path::PathBuf::from("/tmp/a1b2c3d4.jsonl"),
                full_transcript: std::path::PathBuf::from("/tmp/a1b2c3d4.full.jsonl"),
                preview: Some("refactor session management".to_owned()),
                turn_count: 3,
            },
            crate::config::SessionSummary {
                uid: "deadbeef".to_owned(),
                event_log: std::path::PathBuf::from("/tmp/deadbeef.jsonl"),
                full_transcript: std::path::PathBuf::from("/tmp/deadbeef.full.jsonl"),
                preview: Some("audit policy rules".to_owned()),
                turn_count: 1,
            },
        ]);

        terminal.draw(|frame| render_frame(&state, frame))?;
        let buffer = terminal.backend().buffer();
        let all_text: String = (0..20)
            .map(|y| (0..90).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(all_text.contains("session a1b2c3d4"));
        assert!(all_text.contains("Sessions (~/dev/rein)"));
        assert!(all_text.contains("a1b2c3d4"));
        assert!(all_text.contains("refactor session management"));
        assert!(all_text.contains("3 turns"));
        assert!(all_text.contains("(active)"));
        assert!(all_text.contains("deadbeef"));
        Ok(())
    }
}
