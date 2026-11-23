// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Ratatui prompt composer, todo bar, task strip, slash menu, and mention menu

use super::layout::*;

pub(super) fn render_slash_menu(
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

pub(super) fn render_todo_bar(state: &AppState, frame: &mut Frame<'_>, area: Rect) {
    let todos = state.active_todos();
    if todos.is_empty() || area.height == 0 {
        return;
    }
    let total = todos.len();
    let done = todos
        .iter()
        .filter(|item| item.status == TodoStatus::Completed)
        .count();
    let title = state.active_todo_title().unwrap_or("Tasks");
    let start = todos
        .iter()
        .position(|item| item.status != TodoStatus::Completed)
        .unwrap_or(0)
        .min(total.saturating_sub(MAX_TODO_BAR_ROWS));
    let end = (start + MAX_TODO_BAR_ROWS).min(total);
    let hidden_after = total.saturating_sub(end);

    let mut header_spans = vec![
        Span::styled("  ▸ ", Style::default().fg(Color::Cyan)),
        Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" ({done}/{total})"),
            Style::default().fg(Color::DarkGray),
        ),
    ];
    if hidden_after > 0 {
        header_spans.push(Span::styled(
            format!(" · +{hidden_after} more"),
            Style::default().fg(Color::DarkGray),
        ));
    }

    let mut lines = Vec::with_capacity(usize::from(area.height));
    lines.push(Line::from(header_spans));
    for item in &todos[start..end] {
        lines.push(format_todo_bar_item(item));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn format_todo_bar_item(item: &TodoItem) -> Line<'static> {
    match item.status {
        TodoStatus::Completed => Line::from(vec![
            Span::styled("    ☑ ", Style::default().fg(Color::Rgb(115, 210, 125))),
            Span::styled(
                item.content.clone(),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::CROSSED_OUT),
            ),
        ]),
        TodoStatus::InProgress => Line::from(vec![
            Span::styled(
                "    ▸ ",
                Style::default()
                    .fg(Color::Rgb(178, 165, 255))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                item.content.clone(),
                Style::default()
                    .fg(Color::Rgb(178, 165, 255))
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        TodoStatus::Pending => Line::from(vec![
            Span::styled("    ☐ ", Style::default().fg(Color::Gray)),
            Span::styled(item.content.clone(), Style::default().fg(Color::White)),
        ]),
    }
}

pub(super) fn render_prompt_line(state: &AppState, frame: &mut Frame<'_>, area: Rect) {
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
    let (visual_lines, cursor_row, cursor_col) =
        build_wrapped_prompt(state.input(), state.input_cursor_byte(), area.width);
    let visible_rows = usize::from(area.height.max(1));
    let scroll_top = cursor_row.saturating_sub(visible_rows.saturating_sub(1));
    let rendered: Vec<Line<'_>> = visual_lines
        .iter()
        .enumerate()
        .skip(scroll_top)
        .take(visible_rows)
        .map(|(idx, text)| {
            if idx == 0 {
                Line::from(vec![
                    Span::styled(glyph, glyph_style),
                    Span::raw(text.as_str()),
                ])
            } else {
                Line::from(vec![Span::raw("  "), Span::raw(text.as_str())])
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(rendered), area);

    if state.strip_selected().is_some()
        || state.pending_task_manager().is_some()
        || state.pending_question().is_some()
    {
        return;
    }
    let cursor_offset_x = u16::try_from(cursor_col).unwrap_or(u16::MAX);
    let cursor_offset_y = u16::try_from(cursor_row.saturating_sub(scroll_top)).unwrap_or(0);
    let cursor_x = area
        .x
        .saturating_add(2)
        .saturating_add(cursor_offset_x)
        .min(area.right().saturating_sub(1));
    let cursor_y = area
        .y
        .saturating_add(cursor_offset_y)
        .min(area.bottom().saturating_sub(1));
    frame.set_cursor_position(Position::new(cursor_x, cursor_y));
}

pub(super) fn build_wrapped_prompt(
    input: &str,
    cursor_byte: usize,
    total_width: u16,
) -> (Vec<String>, usize, usize) {
    let inner_cols = usize::from(total_width.saturating_sub(2).max(1));
    let mut rows = Vec::new();
    let mut cursor_pos: Option<(usize, usize)> = None;
    let mut byte_pos = 0usize;

    for raw_line in input.split('\n') {
        if raw_line.is_empty() {
            let row_idx = rows.len();
            rows.push(String::new());
            if cursor_pos.is_none() && cursor_byte <= byte_pos {
                cursor_pos = Some((row_idx, 0));
            }
            byte_pos = byte_pos.saturating_add(1);
            continue;
        }
        let mut remaining = raw_line;
        let mut seg_offset = byte_pos;
        while !remaining.is_empty() {
            let chunk: String = remaining.chars().take(inner_cols).collect();
            let chunk_bytes = chunk.len();
            let chunk_end = seg_offset + chunk_bytes;
            let row_idx = rows.len();
            let is_line_end = chunk_bytes == remaining.len();
            if cursor_pos.is_none()
                && cursor_byte >= seg_offset
                && (cursor_byte < chunk_end || (is_line_end && cursor_byte == chunk_end))
            {
                let col = remaining[..cursor_byte - seg_offset].chars().count();
                cursor_pos = Some((row_idx, col));
            }
            rows.push(chunk);
            remaining = &remaining[chunk_bytes..];
            seg_offset = chunk_end;
        }
        byte_pos = seg_offset.saturating_add(1);
    }

    if rows.is_empty() {
        rows.push(String::new());
    }
    let (c_row, c_col) =
        cursor_pos.unwrap_or((rows.len() - 1, rows.last().map_or(0, |r| r.chars().count())));
    (rows, c_row, c_col)
}

pub(super) fn render_task_strip(state: &AppState, frame: &mut Frame<'_>, area: Rect) {
    let items = state.running_strip_items();
    if items.is_empty() || area.height == 0 {
        return;
    }
    let selected = state.strip_selected();
    let lines: Vec<Line<'_>> = items
        .iter()
        .enumerate()
        .take(usize::from(area.height))
        .map(|(idx, item)| format_strip_entry(item, selected == Some(idx)))
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn format_strip_entry(item: &TaskListEntry, is_selected: bool) -> Line<'static> {
    let prefix = if is_selected { "▸ • " } else { "  • " };
    let marker_style = if is_selected {
        Style::default()
            .fg(Color::LightBlue)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Yellow)
    };
    if item.kind == TaskTargetKind::Subagent {
        let title_style = if is_selected {
            Style::default()
                .fg(Color::LightBlue)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        };
        return Line::from(vec![
            Span::styled(prefix, marker_style),
            Span::styled(item.title.clone(), title_style),
            Span::styled(" · ", Style::default().fg(Color::DarkGray)),
            Span::styled(item.detail.clone(), Style::default().fg(Color::Gray)),
        ]);
    }
    let title_style = if is_selected {
        Style::default()
            .fg(Color::LightBlue)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };
    let status_color = if item.is_running {
        Color::Yellow
    } else if item.is_error {
        Color::LightRed
    } else {
        Color::Green
    };
    Line::from(vec![
        Span::styled(prefix, marker_style),
        Span::styled(
            format!("[{}] ", item.started_hms),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(item.title.clone(), title_style),
        Span::styled(
            format!(" {}", item.status_label),
            Style::default().fg(status_color),
        ),
    ])
}

pub(super) fn render_queued_messages(state: &AppState, frame: &mut Frame<'_>, area: Rect) {
    if area.height == 0 {
        return;
    }
    let mut lines = Vec::new();
    for msg in state.steering_pending() {
        lines.push(Line::from(vec![
            Span::styled("▸ ", Style::default().fg(Color::Cyan)),
            Span::styled(msg.clone(), Style::default().fg(Color::White)),
        ]));
        lines.push(Line::from(Span::styled(
            "  └ Sending to agent…",
            Style::default().fg(Color::Yellow),
        )));
    }
    for (display, _) in state.queued_messages() {
        lines.push(Line::from(vec![
            Span::styled("▸ ", Style::default().fg(Color::Gray)),
            Span::styled(display.clone(), Style::default().fg(Color::White)),
            Span::styled(
                " (alt+m to send immediately)",
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

pub(super) fn render_mention_menu(
    matches: &[&MentionCandidate],
    selected: usize,
    frame: &mut Frame<'_>,
    area: Rect,
) {
    if area.height == 0 || matches.is_empty() {
        return;
    }
    let max_rows = usize::from(area.height).min(MAX_MODEL_MENU_ROWS);
    let total = matches.len();
    let has_more = total > max_rows;
    let body_rows = if has_more && max_rows > 1 {
        max_rows - 1
    } else {
        max_rows
    };
    let start = if selected >= body_rows {
        (selected + 1 - body_rows).min(total.saturating_sub(body_rows))
    } else {
        0
    };
    let end = (start + body_rows).min(total);
    let mut lines = Vec::with_capacity(body_rows + 1);

    for (idx, item) in matches[start..end].iter().enumerate() {
        let abs = start + idx;
        let is_sel = abs == selected;
        let prefix = if is_sel { "> " } else { "  " };
        let icon = match item.kind {
            MentionKind::Directory => "+ ",
            MentionKind::File => "  ",
            MentionKind::Agent => "* ",
            MentionKind::Plugin | MentionKind::Mcp => "+ ",
        };
        let label_style = if is_sel {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if item.kind == MentionKind::Agent {
            Style::default().fg(Color::LightMagenta)
        } else {
            Style::default().fg(Color::White)
        };
        let mut spans = vec![
            Span::styled(prefix, label_style),
            Span::styled(icon, Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{:<32}", item.label), label_style),
        ];
        if !item.description.is_empty() && item.kind != MentionKind::File {
            spans.push(Span::styled(
                format!(" {}", item.description),
                Style::default().fg(Color::DarkGray),
            ));
        }
        lines.push(Line::from(spans));
    }
    if has_more && total > end {
        lines.push(Line::from(Span::styled(
            format!("  ↓ {} more", total - end),
            Style::default().fg(Color::DarkGray),
        )));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

