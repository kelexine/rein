// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Ratatui modal overlays (model/session/rewind pickers, approval, task manager, config, plugins)

use super::layout::*;

pub(super) fn render_model_picker(
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
        let is_active = is_model_active(current_model, &item.id);
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

pub(super) fn render_session_picker(
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

pub(super) fn render_rewind_picker(
    picker: &PendingRewindPicker,
    frame: &mut Frame<'_>,
    area: Rect,
) {
    let mut rows = Vec::with_capacity(MAX_MENU_ROWS + 2);
    match picker.step {
        RewindStep::SelectTurn => {
            rows.push(Line::from(vec![
                Span::styled(
                    "Rewind Conversation ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "[↑/↓ Navigate · Enter Select · Esc Cancel]",
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
            let total = picker.items.len() + 1;
            let clamped = picker.selected.min(total.saturating_sub(1));
            let window_start = clamped.saturating_sub(MAX_MENU_ROWS - 1);
            let window_end = (window_start + MAX_MENU_ROWS).min(total);
            let now = now_unix_secs();

            for idx in window_start..window_end {
                let is_selected = idx == clamped;
                let marker = if is_selected { "› " } else { "  " };
                let style = if is_selected {
                    Style::default()
                        .fg(Color::LightBlue)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                };
                if idx < picker.items.len() {
                    let cp = &picker.items[idx];
                    let age = if cp.timestamp_secs == 0 {
                        "resumed".to_owned()
                    } else {
                        format_relative_age(cp.timestamp_secs, now)
                    };
                    let snap_tag = if cp.snapshot.is_some() {
                        ""
                    } else {
                        " · no code snapshot"
                    };
                    rows.push(Line::from(vec![
                        Span::styled(format!("{marker}{}", cp.prompt_preview), style),
                        Span::styled(
                            format!(" ({age}{snap_tag})"),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]));
                } else {
                    rows.push(Line::from(Span::styled(
                        format!("{marker}(current)"),
                        if is_selected {
                            Style::default()
                                .fg(Color::LightBlue)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        },
                    )));
                }
            }
        }
        RewindStep::SelectAction { turn_index } => {
            let preview = picker
                .items
                .get(turn_index)
                .map_or("", |c| c.prompt_preview.as_str());
            let has_snapshot = picker
                .items
                .get(turn_index)
                .is_some_and(|c| c.snapshot.is_some());
            rows.push(Line::from(vec![
                Span::styled(
                    "Rewind to: ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(preview.to_owned(), Style::default().fg(Color::White)),
                Span::styled(
                    " [↑/↓ · Enter · Esc Back]",
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
            let mut options = vec!["1. Rewind messages only"];
            if has_snapshot {
                options.push("2. Rewind messages and code changes");
            }
            for (idx, label) in options.iter().enumerate() {
                let is_selected = idx == picker.selected;
                let marker = if is_selected { "› " } else { "  " };
                rows.push(Line::from(Span::styled(
                    format!("{marker}{label}"),
                    if is_selected {
                        Style::default()
                            .fg(Color::LightBlue)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Gray)
                    },
                )));
            }
        }
    }
    frame.render_widget(Paragraph::new(rows), area);
}

pub(super) fn render_approval_menu(pending: &PendingApproval, frame: &mut Frame<'_>, area: Rect) {
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

pub(super) fn render_task_manager(
    mgr: &PendingTaskManager,
    items: &[TaskListEntry],
    frame: &mut Frame<'_>,
    area: Rect,
) {
    let mut rows = Vec::with_capacity(MAX_MENU_ROWS + 1);
    rows.push(Line::from(vec![
        Span::styled(
            "? Tasks ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "[↑/↓ · Enter: details · k: kill · x: delete done · Esc: close]",
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    if items.is_empty() {
        rows.push(Line::from(Span::styled(
            "  (no background tasks or subagents)",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(Paragraph::new(rows), area);
        return;
    }

    let total = items.len();
    let clamped = mgr.selected.min(total.saturating_sub(1));
    let window_start = clamped.saturating_sub(MAX_MENU_ROWS - 1);
    let window_end = (window_start + MAX_MENU_ROWS).min(total);
    for (offset, item) in items[window_start..window_end].iter().enumerate() {
        let is_selected = window_start + offset == clamped;
        let marker = if is_selected { "› " } else { "  " };
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
        rows.push(Line::from(vec![
            Span::styled(marker, title_style),
            Span::styled(
                format!("[{}] ", item.started_hms),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(format!("{} ", item.title), title_style),
            Span::styled(
                format!("({}) ", item.kind.display_type()),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(item.status_label.clone(), Style::default().fg(status_color)),
        ]));
    }
    frame.render_widget(Paragraph::new(rows), area);
}

pub(super) fn render_question_picker(pq: &PendingQuestion, frame: &mut Frame<'_>, area: Rect) {
    let Some(q) = pq.questions.get(pq.current) else {
        return;
    };
    let hint = if q.is_multi_select {
        "[↑/↓ · Space toggle · Enter confirm · Esc cancel]"
    } else {
        "[↑/↓ · Enter select · Esc cancel]"
    };
    let mut rows = Vec::with_capacity(q.options.len() + 2);
    rows.push(Line::from(vec![
        Span::styled(
            format!(
                "? ({}/{}) {} ",
                pq.current + 1,
                pq.questions.len(),
                q.question
            ),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(hint, Style::default().fg(Color::DarkGray)),
    ]));
    for (idx, opt) in q.options.iter().enumerate() {
        let focused = pq.cursor == idx;
        let chosen = pq.selections[pq.current].contains(&idx);
        let prefix = match (q.is_multi_select, focused, chosen) {
            (true, true, true) => "› [x] ",
            (true, true, false) => "› [ ] ",
            (true, false, true) => "  [x] ",
            (true, false, false) => "  [ ] ",
            (false, true, _) => "› ● ",
            (false, false, true) => "  ● ",
            (false, false, false) => "  ○ ",
        };
        let style = if focused {
            Style::default()
                .fg(Color::LightBlue)
                .add_modifier(Modifier::BOLD)
        } else if chosen {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Gray)
        };
        rows.push(Line::from(Span::styled(format!("{prefix}{opt}"), style)));
    }
    let other_focused = pq.cursor == q.options.len();
    let other_text = &pq.custom_inputs[pq.current];
    let other_label = if other_text.is_empty() {
        "Other (type custom answer)...".to_owned()
    } else {
        format!("Other: {other_text}")
    };
    rows.push(Line::from(Span::styled(
        format!("{} {other_label}", if other_focused { "›" } else { " " }),
        if other_focused {
            Style::default().fg(Color::LightBlue)
        } else {
            Style::default().fg(Color::DarkGray)
        },
    )));
    frame.render_widget(Paragraph::new(rows), area);
}

pub(super) fn render_task_details_frame(
    state: &AppState,
    details: &TaskDetailsView,
    frame: &mut Frame<'_>,
    area: Rect,
) {
    let snap = state
        .task_details_snapshot()
        .unwrap_or_else(|| TaskDetailsSnapshot {
            command: details.target_id.clone(),
            status_label: "removed".to_owned(),
            type_label: details.kind.display_type().to_owned(),
            started_full: "-".to_owned(),
            is_running: false,
            is_error: false,
            output_lines: Vec::new(),
        });

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(4),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Task Details",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ))),
        chunks[0],
    );

    let status_color = if snap.is_running {
        Color::Yellow
    } else if snap.is_error {
        Color::LightRed
    } else {
        Color::Green
    };
    let meta = vec![
        Line::from(vec![
            Span::styled("Command: ", Style::default().fg(Color::LightBlue)),
            Span::styled(snap.command, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("Status:  ", Style::default().fg(Color::LightBlue)),
            Span::styled(
                format!("• {}", snap.status_label),
                Style::default().fg(status_color),
            ),
        ]),
        Line::from(vec![
            Span::styled("Type:    ", Style::default().fg(Color::LightBlue)),
            Span::styled(snap.type_label, Style::default().fg(Color::Gray)),
        ]),
        Line::from(vec![
            Span::styled("Started: ", Style::default().fg(Color::LightBlue)),
            Span::styled(snap.started_full, Style::default().fg(Color::Gray)),
        ]),
    ];
    frame.render_widget(Paragraph::new(meta), chunks[2]);
    render_rule(frame, chunks[3]);

    frame.render_widget(
        Paragraph::new(vec![Line::from(Span::styled(
            "Output",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ))]),
        chunks[4],
    );

    let out_lines: Vec<Line<'_>> = if snap.output_lines.is_empty() {
        vec![Line::from(Span::styled(
            "(no output)",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        snap.output_lines
            .into_iter()
            .map(|l| Line::from(Span::styled(l, Style::default().fg(Color::Gray))))
            .collect()
    };
    let view_height = usize::from(chunks[5].height.max(1));
    let max_scroll = out_lines.len().saturating_sub(view_height);
    let scroll = if details.follow_bottom {
        max_scroll
    } else {
        details.scroll.min(max_scroll)
    };
    frame.render_widget(
        Paragraph::new(out_lines).scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0)),
        chunks[5],
    );
    render_rule(frame, chunks[6]);

    let hint = Line::from(Span::styled(
        "↑/↓ scroll · pgup/pgdown page · ctrl+end bottom · ctrl+home top · esc back",
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(Paragraph::new(hint), chunks[7]);
}

pub(super) fn render_config_modal(
    state: &AppState,
    modal: &PendingConfigModal,
    frame: &mut Frame<'_>,
    area: Rect,
) {
    if area.height == 0 {
        return;
    }
    let mut tab_spans = vec![Span::styled(
        "Config  ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )];
    for tab in ConfigTab::ALL {
        let active = tab == modal.tab;
        let style = if active {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        tab_spans.push(Span::styled(format!(" {} ", tab.label()), style));
        tab_spans.push(Span::raw(" "));
    }
    let mut lines = vec![Line::from(tab_spans)];

    match modal.tab {
        ConfigTab::Settings | ConfigTab::Config => {
            let search_label = if modal.searching {
                format!("> Search settings: {}_", modal.search)
            } else if modal.search.is_empty() {
                "  / Search settings...".to_owned()
            } else {
                format!("  Filter: {} (/ to edit)", modal.search)
            };
            lines.push(Line::from(Span::styled(
                search_label,
                Style::default().fg(if modal.searching {
                    Color::Cyan
                } else {
                    Color::DarkGray
                }),
            )));
            let rows = state.config_setting_rows();
            let visible_cap = usize::from(area.height).saturating_sub(3).max(1);
            let start = if modal.selected >= visible_cap {
                modal.selected + 1 - visible_cap
            } else {
                0
            };
            for (idx, row) in rows.iter().skip(start).take(visible_cap).enumerate() {
                let abs = start + idx;
                let is_sel = abs == modal.selected;
                let marker = if is_sel { "> " } else { "  " };
                let key_style = if is_sel {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                let mut spans = vec![
                    Span::styled(marker, key_style),
                    Span::styled(format!("{:<38}", row.label), key_style),
                    Span::styled(row.value.clone(), Style::default().fg(Color::Yellow)),
                ];
                if row.repo_overridden {
                    spans.push(Span::styled(
                        "  [repo override]",
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                lines.push(Line::from(spans));
            }
            if rows.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  (no matching settings)",
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
        ConfigTab::Status => {
            let overrides = if state.repo_override_keys().is_empty() {
                "none".to_owned()
            } else {
                state.repo_override_keys().join(", ")
            };
            for (k, v) in [
                ("Session", state.session_id().to_owned()),
                ("Workspace", state.workspace_label().to_owned()),
                ("Model", state.model_name().to_owned()),
                ("Mode", mode_name(state.active_profile().mode).to_owned()),
                ("Sandbox", state.sandbox_summary().to_owned()),
                (
                    "Global config",
                    "~/.rein/config.json (default write target)".to_owned(),
                ),
                ("Repo overrides", overrides),
            ] {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {:<18}", k), Style::default().fg(Color::Gray)),
                    Span::styled(v, Style::default().fg(Color::White)),
                ]));
            }
        }
        ConfigTab::Usage => {
            let u = state.usage_totals();
            let quota = state
                .model_quota()
                .map_or_else(|| "n/a".to_owned(), |q| format!("{q}% remaining"));
            for (k, v) in [
                ("Input tokens", u.input_tokens.to_string()),
                ("Output tokens", u.output_tokens.to_string()),
                (
                    "Total tokens",
                    (u.input_tokens + u.output_tokens).to_string(),
                ),
                ("Model quota", quota),
                ("Active tasks", state.all_task_entries().len().to_string()),
            ] {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {:<18}", k), Style::default().fg(Color::Gray)),
                    Span::styled(v, Style::default().fg(Color::Cyan)),
                ]));
            }
        }
    }

    lines.push(Line::from(Span::styled(
        "  [↑/↓ navigate · ←/→ tabs · Space/Enter toggle · / search · Esc close]",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(Paragraph::new(lines), area);
}

pub(super) fn render_plugins_modal(
    state: &AppState,
    modal: &PendingPluginsModal,
    frame: &mut Frame<'_>,
    area: Rect,
) {
    if area.height == 0 {
        return;
    }
    let mut tab_spans = vec![Span::styled(
        "Plugins  ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )];
    for tab in PluginsTab::ALL {
        let active = tab == modal.tab;
        let style = if active {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        tab_spans.push(Span::styled(format!(" {} ", tab.label()), style));
        tab_spans.push(Span::raw(" "));
    }
    let mut lines = vec![Line::from(tab_spans)];

    if modal.adding_marketplace {
        lines.push(Line::from(Span::styled(
            format!(
                "> Add marketplace (owner/repo or git URL): {}_",
                modal.marketplace_input
            ),
            Style::default().fg(Color::Yellow),
        )));
    } else {
        let search_line = if modal.searching {
            format!("> Search plugins: {}_", modal.search)
        } else if modal.search.is_empty() {
            "  / Search plugins...".to_owned()
        } else {
            format!("  Filter: {} (/ to edit)", modal.search)
        };
        lines.push(Line::from(Span::styled(
            search_line,
            Style::default().fg(if modal.searching {
                Color::Cyan
            } else {
                Color::DarkGray
            }),
        )));
    }

    if let Some(notice) = &modal.status_notice {
        lines.push(Line::from(Span::styled(
            format!("  {notice}"),
            Style::default().fg(Color::Yellow),
        )));
    }

    let visible_cap = usize::from(area.height)
        .saturating_sub(lines.len() + 1)
        .max(1);
    let q = modal.search.trim().to_ascii_lowercase();

    if let Some(pm) = state.plugin_manager() {
        match modal.tab {
            PluginsTab::Discover => {
                let mut catalog = pm.discover_catalog(modal.marketplace_filter.as_deref());
                if !q.is_empty() {
                    catalog.retain(|p| {
                        p.name.to_ascii_lowercase().contains(&q)
                            || p.description.to_ascii_lowercase().contains(&q)
                    });
                }
                if catalog.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "  No cached marketplace plugins yet — press 'r' to git-clone/sync marketplace.",
                        Style::default().fg(Color::DarkGray),
                    )));
                } else {
                    let start = if modal.selected >= visible_cap {
                        modal.selected + 1 - visible_cap
                    } else {
                        0
                    };
                    for (idx, p) in catalog.iter().skip(start).take(visible_cap).enumerate() {
                        let abs = start + idx;
                        let is_sel = abs == modal.selected;
                        let check = if p.installed {
                            if p.enabled { "[x]" } else { "[-]" }
                        } else {
                            "[ ]"
                        };
                        let marker = if is_sel { "> " } else { "  " };
                        let style = if is_sel {
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::White)
                        };
                        lines.push(Line::from(vec![
                            Span::styled(format!("{marker}{check} "), style),
                            Span::styled(format!("{:<24}", p.name), style),
                            Span::styled(
                                format!(" {}  ", p.marketplace),
                                Style::default().fg(Color::DarkGray),
                            ),
                            Span::styled(p.description.clone(), Style::default().fg(Color::Gray)),
                        ]));
                    }
                }
            }
            PluginsTab::Installed => {
                let mut inst = pm.installed_plugins();
                if !q.is_empty() {
                    inst.retain(|p| {
                        p.name.to_ascii_lowercase().contains(&q)
                            || p.description.to_ascii_lowercase().contains(&q)
                    });
                }
                if inst.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "  No plugins installed. Switch to Discover and press Enter/Space.",
                        Style::default().fg(Color::DarkGray),
                    )));
                } else {
                    let start = if modal.selected >= visible_cap {
                        modal.selected + 1 - visible_cap
                    } else {
                        0
                    };
                    for (idx, p) in inst.iter().skip(start).take(visible_cap).enumerate() {
                        let abs = start + idx;
                        let is_sel = abs == modal.selected;
                        let check = if p.enabled { "[on]" } else { "[off]" };
                        let marker = if is_sel { "> " } else { "  " };
                        let style = if is_sel {
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::White)
                        };
                        lines.push(Line::from(vec![
                            Span::styled(format!("{marker}{check} "), style),
                            Span::styled(format!("{:<24}", p.name), style),
                            Span::styled(
                                format!(" {}  ", p.marketplace),
                                Style::default().fg(Color::DarkGray),
                            ),
                            Span::styled(p.description.clone(), Style::default().fg(Color::Gray)),
                        ]));
                    }
                }
            }
            PluginsTab::Marketplaces => {
                let mps = pm.list_marketplaces();
                for (idx, mp) in mps.iter().take(visible_cap).enumerate() {
                    let is_sel = idx == modal.selected;
                    let marker = if is_sel { "> " } else { "  " };
                    let style = if is_sel {
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    let synced = if mp.last_synced_secs == 0 {
                        "not synced".to_owned()
                    } else {
                        format!(
                            "synced {}",
                            format_relative_age(mp.last_synced_secs, now_unix_secs())
                        )
                    };
                    lines.push(Line::from(vec![
                        Span::styled(marker, style),
                        Span::styled(format!("{:<26}", mp.name), style),
                        Span::styled(format!("{synced:<16}"), Style::default().fg(Color::Yellow)),
                        Span::styled(mp.url.clone(), Style::default().fg(Color::DarkGray)),
                    ]));
                }
            }
            PluginsTab::Errors => {
                let bundle = pm.collect_enabled_components();
                if bundle.errors.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "  No plugin load errors.",
                        Style::default().fg(Color::Green),
                    )));
                } else {
                    for err in bundle.errors.iter().take(visible_cap) {
                        lines.push(Line::from(Span::styled(
                            format!("  ! {err}"),
                            Style::default().fg(Color::Red),
                        )));
                    }
                }
            }
        }
    }

    lines.push(Line::from(Span::styled(
        "  [↑/↓ nav · ←/→ tabs · Space/Enter install/toggle · r git sync · a add repo · x remove · / search · Esc close]",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(Paragraph::new(lines), area);
}
