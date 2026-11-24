// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Ratatui bottom status bar and mode/quota/sandbox color helpers

use super::layout::*;

pub(super) fn render_status_bar(state: &AppState, frame: &mut Frame<'_>, area: Rect) {
    let mode = state.active_profile().mode;
    let mode_label = mode_name(mode);
    let toggle_label = if state.expand_outputs() {
        "ctrl+o: collapse"
    } else {
        "ctrl+o: expand"
    };
    let (right, right_style) = if state.ctrl_c_exit_armed() {
        (
            "press ctrl+c again within 2s to exit".to_owned(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else if state.strip_selected().is_some() {
        (
            "↑/↓: select task · enter: view · esc: prompt".to_owned(),
            Style::default().fg(Color::LightBlue),
        )
    } else if state.is_busy() && !state.queued_messages().is_empty() {
        (
            "↑: edit queued · alt+m: send now · esc: stop".to_owned(),
            Style::default().fg(Color::DarkGray),
        )
    } else if state.is_busy() {
        (
            format!("{toggle_label} · esc: stop · ctrl+c x2: exit"),
            Style::default().fg(Color::DarkGray),
        )
    } else {
        (
            format!("{toggle_label} · shift+tab: mode · /: commands · /exit"),
            Style::default().fg(Color::DarkGray),
        )
    };
    let right_width = u16::try_from(right.chars().count()).unwrap_or(52);
    let bar_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(10),
            Constraint::Length(2),
            Constraint::Length(right_width),
        ])
        .split(area);

    let mut left_spans = vec![Span::styled(
        mode_label,
        Style::default().fg(mode_color(mode)),
    )];
    if state.bypass_permissions() {
        left_spans.push(Span::styled(
            " [AUTO·BYPASS]",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    }
    left_spans.extend([
        Span::styled(" · ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            state.sandbox_summary(),
            Style::default().fg(sandbox_color(state.sandbox_summary())),
        ),
        Span::styled(" · ", Style::default().fg(Color::DarkGray)),
        Span::styled(state.model_name(), Style::default().fg(Color::LightBlue)),
    ]);
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
    let subagent_count = state.active_subagents().len();
    if subagent_count > 0 {
        let noun = if subagent_count == 1 {
            "subagent"
        } else {
            "subagents"
        };
        left_spans.push(Span::styled(" · ", Style::default().fg(Color::DarkGray)));
        left_spans.push(Span::styled(
            format!("{subagent_count} {noun}"),
            Style::default().fg(Color::Yellow),
        ));
    }
    let running_tasks = state
        .task_registry()
        .list_tasks()
        .into_iter()
        .filter(|t| !t.status.is_done())
        .count();
    if running_tasks > 0 {
        let noun = if running_tasks == 1 { "task" } else { "tasks" };
        left_spans.push(Span::styled(" · ", Style::default().fg(Color::DarkGray)));
        left_spans.push(Span::styled(
            format!("{running_tasks} {noun}"),
            Style::default().fg(Color::Yellow),
        ));
    }
    if subagent_count > 0 || !state.all_task_entries().is_empty() {
        left_spans.push(Span::styled(" · ", Style::default().fg(Color::DarkGray)));
        left_spans.push(Span::styled("/tasks", Style::default().fg(Color::DarkGray)));
    }

    frame.render_widget(Paragraph::new(Line::from(left_spans)), bar_cols[0]);
    frame.render_widget(Paragraph::new(right).style(right_style), bar_cols[2]);
}

pub(super) fn sandbox_color(summary: &str) -> Color {
    if summary.contains("off") {
        Color::Yellow
    } else {
        Color::Green
    }
}

pub(super) fn quota_color(pct: u8) -> Color {
    if pct >= 50 {
        Color::Green
    } else if pct >= 20 {
        Color::Yellow
    } else {
        Color::LightRed
    }
}

pub(super) fn format_tokens(total: u64) -> String {
    if total >= 1_000 {
        let tenths = total / 100;
        format!("{}.{}k tok", tenths / 10, tenths % 10)
    } else {
        format!("{total} tok")
    }
}

pub(super) fn mode_name(mode: ModeId) -> &'static str {
    match mode {
        ModeId::Coding => "coding",
        ModeId::Plan => "plan",
        ModeId::Personal => "personal",
    }
}

pub(super) fn mode_color(mode: ModeId) -> Color {
    match mode {
        ModeId::Coding => Color::Cyan,
        ModeId::Plan => Color::LightMagenta,
        ModeId::Personal => Color::Rgb(115, 210, 125),
    }
}
