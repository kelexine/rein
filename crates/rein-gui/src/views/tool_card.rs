// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Claude Desktop-style inline disclosure tool cards with inspector & diff support

use iced::widget::{Space, button, column, container, row, text};
use iced::{Alignment, Element, Font, Length};
use serde_json::Value;

use crate::app::{Message, ToolItemState, format_size_and_lines};
use crate::theme::palette::{
    STATUS_GREEN, STATUS_RED, STATUS_YELLOW, TEXT_DIM, TEXT_MUTED, TEXT_PRIMARY,
};
use crate::theme::styles::{disclosure_card, ghost_button};

/// Renders a collapsible tool card with muted inline header when collapsed and code card when expanded.
#[must_use]
pub fn render_tool_card<'a>(
    idx: usize,
    name: &'a str,
    arguments: &'a Value,
    state: ToolItemState,
    bg_id: Option<&'a str>,
    output: &'a str,
    expanded: bool,
) -> Element<'a, Message> {
    let (dot_color, status_badge) = match state {
        ToolItemState::Running => (
            STATUS_YELLOW,
            bg_id.map_or_else(|| "running…".to_owned(), |id| format!("background ({id})…")),
        ),
        ToolItemState::Done => (STATUS_GREEN, summarize_done(name, output)),
        ToolItemState::Error => (STATUS_RED, "error".to_owned()),
    };

    let (title, subtitle) = format_tool_title(name, arguments);
    let arrow = if expanded { "⌄" } else { "›" };

    let mut header_row = row![
        text("●").size(9).color(dot_color),
        text(title).size(13).color(TEXT_MUTED),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    if !subtitle.is_empty() {
        header_row = header_row.push(
            text(subtitle.clone())
                .size(12)
                .font(Font::MONOSPACE)
                .color(TEXT_DIM),
        );
    }

    header_row = header_row
        .push(Space::new().width(Length::Fill))
        .push(text(status_badge).size(11).color(TEXT_DIM))
        .push(text(arrow).size(13).color(TEXT_DIM));

    let header = button(header_row)
        .width(Length::Fill)
        .padding([4, 8])
        .style(ghost_button)
        .on_press(Message::ToggleExpand(idx));

    if !expanded {
        return header.into();
    }

    let body = format_tool_body(name, arguments, output);
    let line_count = output.lines().count().max(1);
    let inspect_btn = button(text("Open in inspector ›").size(11).color(TEXT_DIM))
        .padding([2, 6])
        .style(ghost_button)
        .on_press(Message::OpenInspector {
            title: format!("{name} output"),
            subtitle: format_size_and_lines(output.len(), line_count),
            content: output.to_owned(),
        });

    let body_card = container(
        column![
            body,
            row![Space::new().width(Length::Fill), inspect_btn],
        ]
        .spacing(6),
    )
    .padding(12)
    .width(Length::Fill)
    .style(disclosure_card);

    column![header, body_card].spacing(4).into()
}

fn format_tool_title(name: &str, args: &Value) -> (String, String) {
    let str_arg = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_owned()
    };
    match name {
        "bash" => {
            let cmd = str_arg("command");
            let label = if cmd.is_empty() {
                str_arg("program")
            } else {
                cmd
            };
            ("Run".to_owned(), truncate_chars(&label, 68))
        }
        "read" => ("Read".to_owned(), str_arg("path")),
        "write" => ("Write".to_owned(), str_arg("path")),
        "edit" => ("Edit".to_owned(), str_arg("path")),
        "glob" => ("Glob".to_owned(), str_arg("pattern")),
        "grep" => ("Search".to_owned(), str_arg("pattern")),
        "web_search" => ("Web Search".to_owned(), str_arg("query")),
        "web_fetch" => {
            let method = args.get("method").and_then(Value::as_str).unwrap_or("GET");
            (
                format!("Fetch {method}"),
                truncate_chars(&str_arg("url"), 60),
            )
        }
        "invoke_subagent" => ("Subagent".to_owned(), str_arg("role")),
        "manage_task" => ("Task".to_owned(), str_arg("action")),
        "schedule" => ("Schedule".to_owned(), str_arg("prompt")),
        _ => (name.to_owned(), String::new()),
    }
}

fn summarize_done(name: &str, output: &str) -> String {
    if output.trim().is_empty() {
        return "done".to_owned();
    }
    if let Ok(val) = serde_json::from_str::<Value>(output) {
        if name == "bash"
            && let Some(code) = val
                .get("status_code")
                .or_else(|| val.get("exit_code"))
                .and_then(Value::as_i64)
        {
            return if code == 0 {
                "exit 0".to_owned()
            } else {
                format!("exit {code}")
            };
        }
        if let Some(count) = val.get("matches").and_then(Value::as_array).map(Vec::len) {
            return format!("{count} matches");
        }
        if let Some(count) = val.get("files").and_then(Value::as_array).map(Vec::len) {
            return format!("{count} files");
        }
    }
    let lines = output.lines().count();
    if lines > 1 {
        format!("{lines} lines")
    } else {
        "done".to_owned()
    }
}

fn format_tool_body<'a>(name: &'a str, args: &'a Value, output: &'a str) -> Element<'a, Message> {
    if name == "edit" {
        return render_edit_diff(args, output);
    }
    if name == "bash" {
        return render_bash_body(args, output);
    }
    if let Ok(val) = serde_json::from_str::<Value>(output) {
        return render_json_human(&val);
    }
    if output.trim().is_empty() {
        return text("(no output)").size(12).color(TEXT_DIM).into();
    }
    text(truncate_chars(output, 3500))
        .size(12)
        .font(Font::MONOSPACE)
        .color(TEXT_MUTED)
        .into()
}

fn render_bash_body<'a>(args: &'a Value, output: &'a str) -> Element<'a, Message> {
    let mut col = column![].spacing(6);
    if let Some(cmd) = args.get("command").and_then(Value::as_str) {
        col = col.push(
            text(format!("$ {cmd}"))
                .size(12)
                .font(Font::MONOSPACE)
                .color(TEXT_PRIMARY),
        );
    }
    if let Ok(val) = serde_json::from_str::<Value>(output) {
        if let Some(msg) = val.get("message").and_then(Value::as_str) {
            col = col.push(text(msg.to_owned()).size(12).color(STATUS_YELLOW));
        }
        let stdout = val
            .get("stdout")
            .or_else(|| val.get("partial_output"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if !stdout.is_empty() {
            col = col.push(
                text(truncate_chars(stdout, 3500))
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(TEXT_MUTED),
            );
        }
        let stderr = val
            .get("stderr")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if !stderr.is_empty() {
            col = col.push(
                text(truncate_chars(stderr, 1500))
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(STATUS_RED),
            );
        }
        if stdout.is_empty() && stderr.is_empty() && val.get("message").is_none() {
            col = col.push(text("(no output)").size(12).color(TEXT_DIM));
        }
        return col.into();
    }
    if !output.trim().is_empty() {
        col = col.push(
            text(truncate_chars(output, 3500))
                .size(12)
                .font(Font::MONOSPACE)
                .color(TEXT_MUTED),
        );
    }
    col.into()
}

fn render_edit_diff<'a>(args: &'a Value, output: &'a str) -> Element<'a, Message> {
    let mut col = column![].spacing(3);
    if let Some(old_str) = args.get("old_string").and_then(Value::as_str) {
        for line in old_str.lines().take(20) {
            col = col.push(
                text(format!("- {line}"))
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(STATUS_RED),
            );
        }
    }
    if let Some(new_str) = args.get("new_string").and_then(Value::as_str) {
        for line in new_str.lines().take(20) {
            col = col.push(
                text(format!("+ {line}"))
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(STATUS_GREEN),
            );
        }
    }
    if args.get("old_string").is_none() && !output.is_empty() {
        col = col.push(
            text(truncate_chars(output, 1500))
                .size(12)
                .color(TEXT_MUTED),
        );
    }
    col.into()
}

fn render_json_human<'a>(val: &Value) -> Element<'a, Message> {
    if let Some(content) = val.get("content").and_then(Value::as_str) {
        return text(truncate_chars(content, 3500))
            .size(12)
            .font(Font::MONOSPACE)
            .color(TEXT_MUTED)
            .into();
    }
    if let Some(files) = val.get("files").and_then(Value::as_array) {
        let joined: String = files
            .iter()
            .filter_map(Value::as_str)
            .take(50)
            .collect::<Vec<_>>()
            .join("\n");
        let display = if joined.is_empty() {
            "(no matching files)".to_owned()
        } else {
            joined
        };
        return text(display)
            .size(12)
            .font(Font::MONOSPACE)
            .color(TEXT_MUTED)
            .into();
    }
    if let Some(matches) = val.get("matches").and_then(Value::as_array) {
        let mut col = column![].spacing(2);
        for m in matches.iter().take(40) {
            let file = m.get("file").and_then(Value::as_str).unwrap_or("");
            let line = m.get("line").and_then(Value::as_u64).unwrap_or(0);
            let txt = m.get("text").and_then(Value::as_str).unwrap_or("").trim();
            col = col.push(
                text(format!("{file}:{line}: {txt}"))
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(TEXT_MUTED),
            );
        }
        if matches.is_empty() {
            col = col.push(text("(no matches)").size(12).color(TEXT_DIM));
        }
        return col.into();
    }
    if let Some(obj) = val.as_object() {
        let mut col = column![].spacing(3);
        for (k, v) in obj {
            let rendered = match v {
                Value::String(s) => truncate_chars(s, 300),
                other => other.to_string(),
            };
            col = col.push(
                row![
                    text(format!("{k}:")).size(12).color(TEXT_DIM),
                    text(rendered).size(12).color(TEXT_MUTED),
                ]
                .spacing(6),
            );
        }
        return col.into();
    }
    text(val.to_string()).size(12).color(TEXT_MUTED).into()
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    let mut iter = input.chars();
    let head: String = iter.by_ref().take(max_chars).collect();
    if iter.next().is_some() {
        format!("{head}\n… (truncated)")
    } else {
        head
    }
}
