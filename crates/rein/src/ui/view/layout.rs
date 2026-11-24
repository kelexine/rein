// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Borderless Ratatui renderer with Citadel Owl header, Markdown transcript, /model picker, and status bar

use super::modals::*;
use super::prompt::*;
pub(super) use super::statusbar::*;

pub(super) use ratatui::Frame;
pub(super) use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
pub(super) use ratatui::style::{Color, Modifier, Style};
pub(super) use ratatui::text::{Line, Span};
pub(super) use ratatui::widgets::Paragraph;
pub(super) use rein_core::ModeId;
pub(super) use rein_tools::{TodoItem, TodoStatus};

pub(super) use crate::agent::{format_relative_age, now_unix_secs};
pub(super) use crate::commands::SlashCommand;
pub(super) use crate::ui::AppState;
pub(super) use crate::ui::app::{
    ConfigTab, PendingApproval, PendingConfigModal, PendingModelPicker, PendingPluginsModal,
    PendingQuestion, PendingRewindPicker, PendingSessionPicker, PendingTaskManager, PluginsTab,
    RewindStep, TaskDetailsSnapshot, TaskDetailsView, TaskListEntry, TaskTargetKind,
    is_model_active, model_vendor_group,
};
pub(super) use crate::ui::entries::render_transcript_lines;
pub(super) use crate::ui::mentions::{MentionCandidate, MentionKind};

pub(super) const MAX_MENU_ROWS: usize = 6;
pub(super) const MAX_MODEL_MENU_ROWS: usize = 10;
pub(super) const MAX_PROMPT_ROWS: usize = 6;
pub(super) const MAX_TODO_BAR_ROWS: usize = 5;
const MIN_HEIGHT_FOR_HEADER: u16 = 14;
const CITADEL_OWL: [&str; 3] = ["▜▙████▟▛", "▐▛▄██▄▜▌", " ▝▜▀▀▛▘ "];
const SPINNER_FRAMES: [&str; 10] = ["✶", "✸", "✹", "✺", "✹", "✸", "✶", "✢", "✽", "✻"];

/// Computes the number of top rows occupied by the Citadel Owl header at `total_height`.
#[must_use]
pub fn header_height(state: &AppState, total_height: u16) -> u16 {
    if total_height < MIN_HEIGHT_FOR_HEADER {
        return 0;
    }
    let notice_rows = u16::try_from(state.startup_notices().len().min(2)).unwrap_or(0);
    if notice_rows == 0 { 4 } else { 5 + notice_rows }
}

fn prompt_rows(state: &AppState, total_width: u16, total_height: u16) -> u16 {
    if state.pending_approval().is_some() {
        return 1;
    }
    let (lines, _, _) = build_wrapped_prompt(state.input(), state.input_cursor_byte(), total_width);
    let max_rows = MAX_PROMPT_ROWS.min(usize::from(total_height.saturating_sub(8).max(1)));
    u16::try_from(lines.len().clamp(1, max_rows.max(1))).unwrap_or(1)
}

fn todo_bar_rows(state: &AppState) -> u16 {
    let count = state.active_todos().len();
    if count == 0 {
        0
    } else {
        u16::try_from(1 + count.min(MAX_TODO_BAR_ROWS)).unwrap_or(0)
    }
}

fn task_strip_rows(state: &AppState) -> u16 {
    u16::try_from(state.running_strip_items().len().min(5)).unwrap_or(0)
}

fn split_layout(state: &AppState, area: Rect) -> std::rc::Rc<[Rect]> {
    let hdr_rows = header_height(state, area.height);
    let matches = state.slash_matches();
    let popup_rows = compute_popup_rows(state, matches.len());
    let spinner_rows = u16::from(state.is_busy() && state.pending_approval().is_none());
    let todo_rows = todo_bar_rows(state);
    let strip_rows = task_strip_rows(state);
    let input_rows = prompt_rows(state, area.width, area.height);

    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(hdr_rows),
            Constraint::Min(3),
            Constraint::Length(spinner_rows),
            Constraint::Length(popup_rows),
            Constraint::Length(todo_rows),
            Constraint::Length(1),
            Constraint::Length(input_rows),
            Constraint::Length(1),
            Constraint::Length(strip_rows),
            Constraint::Length(1),
        ])
        .split(area)
}

/// Returns the exact transcript viewport [`Rect`] computed identically to [`render_frame`].
#[must_use]
pub fn transcript_viewport(state: &AppState, area: Rect) -> Rect {
    split_layout(state, area)[1]
}

/// Renders the Citadel Owl header, styled transcript, optional popup/spinner, hairline prompt, live strip, or Task Details.
pub fn render_frame(state: &AppState, frame: &mut Frame<'_>) {
    let area = frame.area();
    if let Some(details) = state.pending_task_details() {
        render_task_details_frame(state, details, frame, area);
        return;
    }

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
    } else if let Some(pq) = state.pending_question() {
        render_question_picker(pq, frame, chunks[3]);
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
    } else if let Some(picker) = state.pending_rewind_picker() {
        render_rewind_picker(picker, frame, chunks[3]);
    } else if let Some(mgr) = state.pending_task_manager() {
        render_task_manager(mgr, &state.all_task_entries(), frame, chunks[3]);
    } else if let Some(cfg_modal) = state.pending_config_modal() {
        render_config_modal(state, cfg_modal, frame, chunks[3]);
    } else if let Some(plug_modal) = state.pending_plugins_modal() {
        render_plugins_modal(state, plug_modal, frame, chunks[3]);
    } else if let Some((_, _, mention_matches)) = state.active_mention_matches()
        && chunks[3].height > 0
    {
        render_mention_menu(&mention_matches, state.mention_selected(), frame, chunks[3]);
    } else if (!state.queued_messages().is_empty() || !state.steering_pending().is_empty())
        && chunks[3].height > 0
    {
        render_queued_messages(state, frame, chunks[3]);
    } else if chunks[3].height > 0 {
        render_slash_menu(&matches, state.slash_selected(), frame, chunks[3]);
    }

    if chunks[4].height > 0 {
        render_todo_bar(state, frame, chunks[4]);
    }
    render_rule(frame, chunks[5]);
    render_prompt_line(state, frame, chunks[6]);
    render_rule(frame, chunks[7]);
    if chunks[8].height > 0 {
        render_task_strip(state, frame, chunks[8]);
    }
    render_status_bar(state, frame, chunks[9]);
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
    } else if let Some(pq) = state.pending_question() {
        let opt_count = pq
            .questions
            .get(pq.current)
            .map_or(1, |q| q.options.len() + 1);
        u16::try_from(opt_count.clamp(1, MAX_MODEL_MENU_ROWS) + 1).unwrap_or(3)
    } else if let Some(picker) = state.pending_model_picker() {
        let group_headers = count_model_groups(picker);
        let body_rows = (picker.items.len() + group_headers).clamp(1, MAX_MODEL_MENU_ROWS);
        u16::try_from(body_rows + 1).unwrap_or(2)
    } else if let Some(picker) = state.pending_session_picker() {
        let count = picker.items.len().clamp(1, MAX_MENU_ROWS);
        u16::try_from(count + 1).unwrap_or(2)
    } else if let Some(picker) = state.pending_rewind_picker() {
        match picker.step {
            RewindStep::SelectTurn => {
                let count = (picker.items.len() + 1).clamp(1, MAX_MENU_ROWS);
                u16::try_from(count + 2).unwrap_or(3)
            }
            RewindStep::SelectAction { .. } => 4,
        }
    } else if state.pending_task_manager().is_some() {
        let count = state.all_task_entries().len().clamp(1, MAX_MENU_ROWS);
        u16::try_from(count + 1).unwrap_or(2)
    } else if state.pending_config_modal().is_some() {
        15
    } else if state.pending_plugins_modal().is_some() {
        16
    } else if let Some((_, _, mention_matches)) = state.active_mention_matches() {
        let vis = mention_matches.len().min(MAX_MODEL_MENU_ROWS);
        let extra = usize::from(mention_matches.len() > MAX_MODEL_MENU_ROWS);
        u16::try_from(vis + extra).unwrap_or(0)
    } else if !state.queued_messages().is_empty() || !state.steering_pending().is_empty() {
        let rows = state.queued_messages().len() + state.steering_pending().len() * 2;
        u16::try_from(rows.min(MAX_MENU_ROWS)).unwrap_or(0)
    } else {
        u16::try_from(slash_count.min(MAX_MENU_ROWS)).unwrap_or(0)
    }
}

pub(super) fn count_model_groups(picker: &PendingModelPicker) -> usize {
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

    let mode = state.active_profile().mode;
    let mode_label = mode_name(mode);
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
            Span::styled(mode_label, Style::default().fg(mode_color(mode))),
            if state.bypass_permissions() {
                Span::styled(
                    " [AUTO·BYPASS]",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                )
            } else {
                Span::raw("")
            },
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
    let tick = state.spinner_tick();
    let glyph_step = tick / 2;
    let glyph = SPINNER_FRAMES[glyph_step % SPINNER_FRAMES.len()];
    let glyph_color = match glyph_step % 4 {
        0 => Color::Rgb(255, 210, 175),
        1 | 3 => Color::Rgb(240, 162, 124),
        _ => Color::Rgb(224, 130, 96),
    };
    let mut spans = vec![Span::styled(
        format!("{glyph} "),
        Style::default()
            .fg(glyph_color)
            .add_modifier(Modifier::BOLD),
    )];

    let status = state.busy_status();
    let chars: Vec<char> = status.chars().collect();
    let sweep_len = chars.len() + 10;
    let peak = tick % sweep_len.max(1);
    for (idx, ch) in chars.into_iter().enumerate() {
        let dist = idx.abs_diff(peak);
        let color = match dist {
            0 => Color::Rgb(255, 222, 192),
            1 => Color::Rgb(244, 176, 138),
            2 => Color::Rgb(234, 148, 112),
            _ => Color::Rgb(224, 130, 96),
        };
        spans.push(Span::styled(
            ch.to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }

    spans.push(Span::styled(
        " (esc to interrupt)",
        Style::default().fg(Color::DarkGray),
    ));

    let elapsed = state.busy_elapsed_secs();
    if elapsed >= 1 {
        let formatted = if elapsed >= 60 {
            format!(" · {}m {:02}s", elapsed / 60, elapsed % 60)
        } else {
            format!(" · {elapsed}s")
        };
        spans.push(Span::styled(formatted, Style::default().fg(Color::Gray)));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub(super) fn render_rule(frame: &mut Frame<'_>, area: Rect) {
    let rule = "─".repeat(area.width as usize);
    frame.render_widget(
        Paragraph::new(rule).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modes::Profile;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use rein_core::AvailableModel;

    #[test]
    fn renders_citadel_owl_header_and_slash_menu() -> Result<(), Box<dyn std::error::Error>> {
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
        assert!(all_text.contains("› /memory"));
        assert!(all_text.contains("ctrl+o: expand"));
        assert!(all_text.contains("shift+tab: mode"));
        Ok(())
    }

    #[test]
    fn renders_four_option_approval_menu_with_tab_feedback()
    -> Result<(), Box<dyn std::error::Error>> {
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
    fn renders_model_picker_with_server_quota_and_busy_spinner()
    -> Result<(), Box<dyn std::error::Error>> {
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
    fn compute_max_scroll_matches_word_wrapped_paragraph_bottom()
    -> Result<(), Box<dyn std::error::Error>> {
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
    fn renders_interactive_resume_session_picker() -> Result<(), Box<dyn std::error::Error>> {
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

    #[test]
    fn renders_plan_mode_and_multiline_prompt_box() -> Result<(), Box<dyn std::error::Error>> {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend)?;
        let mut state = AppState::new(Profile::plan(), "landlock v11", "antigravity/gemini-4");
        state.push_input_str("first line\nsecond line\nthird line");

        terminal.draw(|frame| render_frame(&state, frame))?;
        let buffer = terminal.backend().buffer();
        let all_text: String = (0..20)
            .map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(all_text.contains("plan"));
        assert!(all_text.contains("first line"));
        assert!(all_text.contains("second line"));
        assert!(all_text.contains("third line"));
        assert!(all_text.contains("/exit"));
        assert!(!all_text.contains("esc: quit"));
        Ok(())
    }

    #[test]
    fn renders_dynamic_todo_bar_above_prompt_and_collapses_when_cleared()
    -> Result<(), Box<dyn std::error::Error>> {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend)?;
        let mut state = AppState::new(Profile::coding(), "landlock v11", "claude-sonnet-5-5");
        state.set_active_todos(
            Some("Refactor auth".to_owned()),
            vec![
                TodoItem {
                    id: "1".to_owned(),
                    content: "Split token module".to_owned(),
                    status: TodoStatus::Completed,
                },
                TodoItem {
                    id: "2".to_owned(),
                    content: "Verify session hooks".to_owned(),
                    status: TodoStatus::InProgress,
                },
                TodoItem {
                    id: "3".to_owned(),
                    content: "Update integration tests".to_owned(),
                    status: TodoStatus::Pending,
                },
            ],
        );

        terminal.draw(|frame| render_frame(&state, frame))?;
        let buffer = terminal.backend().buffer();
        let active_text: String = (0..20)
            .map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(active_text.contains("Refactor auth (1/3)"));
        assert!(active_text.contains("☑ Split token module"));
        assert!(active_text.contains("▸ Verify session hooks"));
        assert!(active_text.contains("☐ Update integration tests"));

        state.clear_active_todos();
        assert_eq!(todo_bar_rows(&state), 0);
        terminal.draw(|frame| render_frame(&state, frame))?;
        let buffer = terminal.backend().buffer();
        let cleared_text: String = (0..20)
            .map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!cleared_text.contains("Refactor auth"));
        assert!(!cleared_text.contains("Verify session hooks"));
        Ok(())
    }

    #[test]
    fn renders_live_subagent_bar_and_status_count_when_active()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::agent::{LiveSubagentStatus, SubagentRunState, SubagentTracker, now_unix_secs};
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;

        let backend = TestBackend::new(120, 22);
        let mut terminal = Terminal::new(backend)?;
        let tracker = SubagentTracker::new();
        tracker.start(LiveSubagentStatus {
            id: "sub_1".to_owned(),
            agent_type: "research".to_owned(),
            role: "Vendor ELF Auditor".to_owned(),
            model_label: "gemini-4-argon".to_owned(),
            status: "Reading file".to_owned(),
            run_state: SubagentRunState::Running,
            started_unix_secs: now_unix_secs(),
            started_hms: String::new(),
            started_full: String::new(),
            finished_unix_secs: None,
            output_lines: vec!["line 1".to_owned()],
            cancel_flag: Arc::new(AtomicBool::new(false)),
        });
        tracker.start(LiveSubagentStatus {
            id: "sub_2".to_owned(),
            agent_type: "research".to_owned(),
            role: "HAL Config Auditor".to_owned(),
            model_label: "gemini-4-argon".to_owned(),
            status: "Searching files".to_owned(),
            run_state: SubagentRunState::Running,
            started_unix_secs: now_unix_secs(),
            started_hms: String::new(),
            started_full: String::new(),
            finished_unix_secs: None,
            output_lines: Vec::new(),
            cancel_flag: Arc::new(AtomicBool::new(false)),
        });

        let mut state = AppState::new(Profile::coding(), "landlock v11", "gemini-4-argon-medium")
            .with_subagent_tracker(tracker);
        terminal.draw(|frame| render_frame(&state, frame))?;
        let buffer = terminal.backend().buffer();
        let rendered: String = (0..22)
            .map(|y| {
                (0..120)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Agent(research: Vendor ELF Auditor)"));
        assert!(rendered.contains("Reading file"));
        assert!(rendered.contains("2 subagents"));

        assert!(state.move_strip_selection(1));
        assert!(state.open_selected_strip_details());
        terminal.draw(|frame| render_frame(&state, frame))?;
        let details_buf = terminal.backend().buffer();
        let details_str: String = (0..22)
            .map(|y| {
                (0..120)
                    .map(|x| details_buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(details_str.contains("Task Details"));
        assert!(details_str.contains("Agent(research: Vendor ELF Auditor)"));
        assert!(details_str.contains("line 1"));
        Ok(())
    }

    #[test]
    fn task_manager_lists_kills_and_deletes_done_tasks() -> Result<(), Box<dyn std::error::Error>> {
        use rein_tools::{TaskRegistry, TimerCondition};

        let reg = TaskRegistry::new();
        let summary = reg.schedule_timer(
            60,
            "check status".to_owned(),
            TimerCondition::Never,
            Some("1m timer".to_owned()),
        )?;

        let mut state = AppState::new(Profile::coding(), "landlock v11", "gemini-4-argon-medium")
            .with_task_registry(reg);
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend)?;

        terminal.draw(|frame| render_frame(&state, frame))?;
        let strip_buf = terminal.backend().buffer();
        let strip_str: String = (0..24)
            .map(|y| {
                (0..120)
                    .map(|x| strip_buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(strip_str.contains("1m timer"));
        assert!(strip_str.contains("1 task"));
        assert!(strip_str.contains("/tasks"));

        state.open_task_manager();
        // Cannot delete a still-running task with 'x' before killing.
        let del_running = state
            .delete_selected_done_task()
            .ok_or("expected delete outcome")?;
        assert!(del_running.is_err());

        let killed = state
            .kill_selected_managed_task()
            .ok_or("expected kill outcome")?;
        assert!(killed.is_ok());

        let deleted = state
            .delete_selected_done_task()
            .ok_or("expected delete outcome")?;
        assert!(deleted.is_ok());
        assert!(state.all_task_entries().is_empty());
        let _ = summary;
        Ok(())
    }

    #[test]
    fn renders_and_collects_multi_select_ask_question_dialog()
    -> Result<(), Box<dyn std::error::Error>> {
        use rein_tools::QuestionSpec;
        use std::sync::mpsc;

        let (tx, rx) = mpsc::channel();
        let mut state = AppState::new(Profile::coding(), "policy-gated", "gemini-4-argon-medium");
        state.open_question_picker(
            vec![QuestionSpec {
                question: "Pick target checks".to_owned(),
                options: vec!["clippy".to_owned(), "cargo test".to_owned()],
                is_multi_select: true,
            }],
            tx,
        );

        let backend = TestBackend::new(100, 20);
        let mut terminal = Terminal::new(backend)?;
        terminal.draw(|frame| render_frame(&state, frame))?;
        let buf = terminal.backend().buffer();
        let rendered: String = (0..20)
            .map(|y| (0..100).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Pick target checks"));
        assert!(rendered.contains("clippy"));
        assert!(rendered.contains("cargo test"));

        state.toggle_question_option();
        state.move_question_cursor(1);
        state.toggle_question_option();
        state.confirm_question_step();
        let answers = rx.recv()?.ok_or("expected question answers")?;
        assert_eq!(answers[0].selected, vec!["clippy", "cargo test"]);
        Ok(())
    }
}
