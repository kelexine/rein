// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-15
// Purpose: Structured transcript entries for reasoning traces, hidden-by-default Ctrl+O tool outputs, diffs, and todos

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use rein_core::{ToolCall, ToolOutput};
use serde_json::Value;

use crate::ui::markdown::render_assistant_markdown;

/// Live execution state of a rendered tool invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRunState {
    /// Policy approved / awaiting execution.
    Running,
    /// Completed without error.
    Done,
    /// Completed with error or denied by policy/user.
    Error,
}

/// Structured tool invocation entry carrying raw call metadata and captured output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolTranscriptEntry {
    /// Unique provider tool call identifier.
    pub call_id: String,
    /// Raw tool name (`bash`, `read`, `edit`, `todo_write`, etc.).
    pub name: String,
    /// Structured tool arguments.
    pub arguments: Value,
    /// Current execution state.
    pub state: ToolRunState,
    /// Captured full output or denial reason.
    pub output: String,
}

/// Typed transcript entry replacing flat raw-string buffers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptEntry {
    /// Prompt submitted by the user.
    UserTurn(String),
    /// Model reasoning / thought trace displayed as collapsible disclosure.
    Thinking {
        /// Accumulated reasoning text.
        content: String,
        /// Elapsed seconds while reasoning was streamed.
        elapsed_secs: u64,
        /// Whether tokens are still streaming into this thought block.
        active: bool,
    },
    /// Markdown response streamed by the assistant.
    AssistantTurn(String),
    /// Structured tool invocation + `Ctrl+O` expandable `└` output tree.
    Tool(ToolTranscriptEntry),
    /// Muted status/system feedback (`· ...`).
    SystemNote(String),
    /// Error or warning notice (`! ...`).
    ErrorNote(String),
}

impl TranscriptEntry {
    /// Creates a running tool entry from a [`ToolCall`].
    #[must_use]
    pub fn tool_running(call: &ToolCall) -> Self {
        Self::Tool(ToolTranscriptEntry {
            call_id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
            state: ToolRunState::Running,
            output: String::new(),
        })
    }

    /// Creates a completed tool entry from a [`ToolCall`] and [`ToolOutput`].
    #[must_use]
    pub fn tool_completed(call: &ToolCall, output: &ToolOutput) -> Self {
        Self::Tool(ToolTranscriptEntry {
            call_id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
            state: if output.is_error {
                ToolRunState::Error
            } else {
                ToolRunState::Done
            },
            output: output.content.clone(),
        })
    }
}

/// Renders the structured transcript into styled Ratatui [`Line`]s with separators and conditional `Ctrl+O` output hiding.
#[must_use]
pub fn render_transcript_lines(
    entries: &[TranscriptEntry],
    width: u16,
    expand_outputs: bool,
) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut prev_kind = EntryKind::Other;

    for entry in entries {
        let kind = EntryKind::from(entry);
        let skip_blank = (prev_kind == EntryKind::Note && kind == EntryKind::Note)
            || (!expand_outputs && prev_kind == EntryKind::Tool && kind == EntryKind::Tool);
        if !out.is_empty() && !skip_blank {
            out.push(Line::default());
        }
        match entry {
            TranscriptEntry::UserTurn(text) => render_user_turn(text, width, &mut out),
            TranscriptEntry::Thinking {
                content,
                elapsed_secs,
                active,
            } => render_thinking_entry(content, *elapsed_secs, *active, expand_outputs, &mut out),
            TranscriptEntry::AssistantTurn(text) => {
                let mut md_lines = render_assistant_markdown(text, width);
                if md_lines.is_empty() {
                    md_lines.push(Line::from(Span::styled(
                        "● ",
                        Style::default().fg(Color::White),
                    )));
                }
                out.extend(md_lines);
            }
            TranscriptEntry::Tool(tool) => render_tool_entry(tool, expand_outputs, &mut out),
            TranscriptEntry::SystemNote(note) => render_system_note(note, &mut out),
            TranscriptEntry::ErrorNote(err) => render_error_note(err, &mut out),
        }
        prev_kind = kind;
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Tool,
    Note,
    Other,
}

impl From<&TranscriptEntry> for EntryKind {
    fn from(entry: &TranscriptEntry) -> Self {
        match entry {
            TranscriptEntry::Tool(_) => Self::Tool,
            TranscriptEntry::SystemNote(_) => Self::Note,
            _ => Self::Other,
        }
    }
}

fn render_user_turn(text: &str, width: u16, out: &mut Vec<Line<'static>>) {
    let bar_bg = Color::Rgb(34, 38, 46);
    let cols = usize::from(width.max(10));
    let inner_cols = cols.saturating_sub(2).max(1);
    let mut is_first_row = true;
    for raw_line in text.split('\n') {
        let mut remaining = raw_line;
        loop {
            let prefix = if is_first_row { "❯ " } else { "  " };
            is_first_row = false;
            let chunk: String = remaining.chars().take(inner_cols).collect();
            let consumed = chunk.len();
            let used = 2 + chunk.chars().count();
            let pad = cols.saturating_sub(used);
            out.push(Line::from(vec![
                Span::styled(
                    prefix,
                    Style::default()
                        .fg(Color::Cyan)
                        .bg(bar_bg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    chunk,
                    Style::default()
                        .fg(Color::White)
                        .bg(bar_bg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" ".repeat(pad), Style::default().bg(bar_bg)),
            ]));
            remaining = &remaining[consumed..];
            if remaining.is_empty() {
                break;
            }
        }
    }
}

fn render_thinking_entry(
    content: &str,
    elapsed_secs: u64,
    active: bool,
    expand_outputs: bool,
    out: &mut Vec<Line<'static>>,
) {
    let arrow = if expand_outputs { "▾ " } else { "▸ " };
    let header_label = if active {
        "Thinking…".to_owned()
    } else if elapsed_secs > 0 {
        format!("Thought for {elapsed_secs}s")
    } else {
        "Thought process".to_owned()
    };
    let toggle_hint = if expand_outputs {
        " (ctrl+o to collapse)"
    } else {
        " (ctrl+o to expand)"
    };
    out.push(Line::from(vec![
        Span::styled(arrow, Style::default().fg(Color::DarkGray)),
        Span::styled(
            header_label,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(toggle_hint, Style::default().fg(Color::DarkGray)),
    ]));

    let trimmed = content.trim();
    if trimmed.is_empty() {
        return;
    }

    let style = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::ITALIC);
    if expand_outputs {
        for (idx, line) in trimmed.lines().enumerate() {
            let tree_prefix = if idx == 0 { "  └  " } else { "     " };
            out.push(Line::from(vec![
                Span::styled(tree_prefix, Style::default().fg(Color::DarkGray)),
                Span::styled(line.to_owned(), style),
            ]));
        }
    } else {
        let preview = clip_single_line(trimmed, 110);
        out.push(Line::from(vec![
            Span::styled("  └  ", Style::default().fg(Color::DarkGray)),
            Span::styled(preview, style),
        ]));
    }
}

fn render_system_note(note: &str, out: &mut Vec<Line<'static>>) {
    let clean = note.strip_prefix("· ").unwrap_or(note);
    let text_color = if clean.starts_with("signed in") || clean.starts_with("active provider") {
        Color::Green
    } else if clean.starts_with("switched") {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    out.push(Line::from(vec![
        Span::styled("· ", Style::default().fg(Color::DarkGray)),
        Span::styled(clean.to_owned(), Style::default().fg(text_color)),
    ]));
}

fn render_error_note(err: &str, out: &mut Vec<Line<'static>>) {
    let clean = err.strip_prefix("! ").unwrap_or(err);
    out.push(Line::from(vec![
        Span::styled(
            "! ",
            Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(clean.to_owned(), Style::default().fg(Color::LightRed)),
    ]));
}

fn render_tool_entry(
    tool: &ToolTranscriptEntry,
    expand_outputs: bool,
    out: &mut Vec<Line<'static>>,
) {
    let dot_color = match tool.state {
        ToolRunState::Running => Color::Yellow,
        ToolRunState::Done => Color::Rgb(115, 210, 125),
        ToolRunState::Error => Color::LightRed,
    };
    let (title, arg_label) = format_tool_header(&tool.name, &tool.arguments);
    let mut header_spans = vec![
        Span::styled("● ", Style::default().fg(dot_color)),
        Span::styled(
            title,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(arg) = arg_label {
        header_spans.push(Span::styled("(", Style::default().fg(Color::DarkGray)));
        header_spans.push(Span::styled(arg, Style::default().fg(Color::Gray)));
        header_spans.push(Span::styled(")", Style::default().fg(Color::DarkGray)));
    }

    if tool.state == ToolRunState::Running {
        header_spans.push(Span::styled(
            "  running…",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::ITALIC),
        ));
        out.push(Line::from(header_spans));
        return;
    }

    if !expand_outputs {
        let suffix = match tool.state {
            ToolRunState::Error => " (error · ctrl+o to expand)",
            ToolRunState::Done => " (ctrl+o to expand)",
            ToolRunState::Running => "",
        };
        let suffix_color = if tool.state == ToolRunState::Error {
            Color::LightRed
        } else {
            Color::DarkGray
        };
        header_spans.push(Span::styled(suffix, Style::default().fg(suffix_color)));
        out.push(Line::from(header_spans));
        return;
    }

    out.push(Line::from(header_spans));
    let body_lines = build_tool_output_lines(tool);
    for (idx, body) in body_lines.into_iter().enumerate() {
        let tree_prefix = if idx == 0 { "  └  " } else { "     " };
        let mut spans = vec![Span::styled(
            tree_prefix,
            Style::default().fg(Color::DarkGray),
        )];
        spans.extend(body.spans);
        out.push(Line::from(spans));
    }
    out.push(Line::from(Span::styled(
        "     (ctrl+o to collapse)",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    )));
}

fn format_tool_header(name: &str, args: &Value) -> (String, Option<String>) {
    match name {
        "todo_write" => ("Update Todos".to_owned(), None),
        "todo_read" => ("Read Todos".to_owned(), None),
        "switch_mode" => ("Switch Mode".to_owned(), arg_str(args, "mode", 32)),
        "bash" => {
            let cmd = args
                .get("command")
                .or_else(|| args.get("program"))
                .and_then(Value::as_str)
                .map(|s| clip_single_line(s, 72));
            ("Bash".to_owned(), cmd)
        }
        "read" => ("Read".to_owned(), arg_str(args, "path", 68)),
        "write" => ("Write".to_owned(), arg_str(args, "path", 68)),
        "edit" => ("Edit".to_owned(), arg_str(args, "path", 68)),
        "glob" => ("Glob".to_owned(), arg_str(args, "pattern", 68)),
        "grep" => ("Grep".to_owned(), arg_str(args, "pattern", 68)),
        "memory" => {
            let action = args.get("action").and_then(Value::as_str).unwrap_or("");
            let key = args
                .get("key")
                .or_else(|| args.get("query"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let arg = if key.is_empty() {
                action.to_owned()
            } else {
                format!("{action} {key}")
            };
            ("Memory".to_owned(), (!arg.is_empty()).then_some(arg))
        }
        other => {
            let first = args
                .get("path")
                .or_else(|| args.get("action"))
                .or_else(|| args.get("query"))
                .or_else(|| args.get("title"))
                .and_then(Value::as_str)
                .map(|s| clip_single_line(s, 60));
            (title_case(other), first)
        }
    }
}

fn build_tool_output_lines(tool: &ToolTranscriptEntry) -> Vec<Line<'static>> {
    if tool.state == ToolRunState::Error {
        return split_plain_lines(&tool.output, Color::LightRed);
    }

    match tool.name.as_str() {
        "todo_write" | "todo_read" => render_todo_lines(&tool.output)
            .unwrap_or_else(|| split_plain_lines(&tool.output, Color::Gray)),
        "edit" => render_edit_diff_lines(tool),
        "write" => render_write_lines(tool),
        "read" => render_read_lines(&tool.output),
        _ => split_plain_lines(&tool.output, Color::Gray),
    }
}

fn render_todo_lines(raw_output: &str) -> Option<Vec<Line<'static>>> {
    let parsed: Value = serde_json::from_str(raw_output).ok()?;
    let items = parsed.get("todos")?.as_array()?;
    if items.is_empty() {
        return Some(vec![Line::from(Span::styled(
            "(no todos)",
            Style::default().fg(Color::DarkGray),
        ))]);
    }
    let mut lines = Vec::with_capacity(items.len());
    for item in items {
        let content = item
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("(untitled)");
        let status = item
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending");
        let line = match status {
            "completed" => Line::from(vec![
                Span::styled("☑ ", Style::default().fg(Color::Rgb(115, 210, 125))),
                Span::styled(
                    content.to_owned(),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::CROSSED_OUT),
                ),
            ]),
            "in_progress" => Line::from(vec![
                Span::styled(
                    "☐ ",
                    Style::default()
                        .fg(Color::Rgb(178, 165, 255))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    content.to_owned(),
                    Style::default()
                        .fg(Color::Rgb(178, 165, 255))
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            _ => Line::from(vec![
                Span::styled("☐ ", Style::default().fg(Color::Gray)),
                Span::styled(content.to_owned(), Style::default().fg(Color::White)),
            ]),
        };
        lines.push(line);
    }
    Some(lines)
}

fn render_edit_diff_lines(tool: &ToolTranscriptEntry) -> Vec<Line<'static>> {
    let old = tool
        .arguments
        .get("old")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let new = tool
        .arguments
        .get("new")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if old.is_empty() && new.is_empty() {
        return split_plain_lines(&tool.output, Color::Gray);
    }
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let mut out = vec![Line::from(vec![
        Span::styled(
            format!("+{} ", new_lines.len()),
            Style::default().fg(Color::Rgb(115, 210, 125)),
        ),
        Span::styled("/ ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("-{} lines", old_lines.len()),
            Style::default().fg(Color::LightRed),
        ),
    ])];
    for line in old_lines {
        out.push(Line::from(Span::styled(
            format!("- {line}"),
            Style::default()
                .fg(Color::Rgb(245, 165, 165))
                .bg(Color::Rgb(56, 24, 24)),
        )));
    }
    for line in new_lines {
        out.push(Line::from(Span::styled(
            format!("+ {line}"),
            Style::default()
                .fg(Color::Rgb(165, 235, 175))
                .bg(Color::Rgb(24, 52, 30)),
        )));
    }
    out
}

fn render_write_lines(tool: &ToolTranscriptEntry) -> Vec<Line<'static>> {
    let content = tool
        .arguments
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let path = tool
        .arguments
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or("file");
    let count = content.lines().count();
    let mut out = vec![Line::from(Span::styled(
        format!("Wrote {count} lines to {path}"),
        Style::default().fg(Color::White),
    ))];
    for line in content.lines() {
        out.push(Line::from(Span::styled(
            line.to_owned(),
            Style::default().fg(Color::Gray),
        )));
    }
    out
}

fn render_read_lines(raw_output: &str) -> Vec<Line<'static>> {
    if raw_output.trim().is_empty() || raw_output.trim() == "(no output)" {
        return vec![Line::from(Span::styled(
            "(empty file)",
            Style::default().fg(Color::DarkGray),
        ))];
    }
    let lines: Vec<&str> = raw_output.lines().collect();
    let mut out = vec![Line::from(Span::styled(
        format!("Read {} lines", lines.len()),
        Style::default().fg(Color::White),
    ))];
    for line in lines {
        out.push(Line::from(Span::styled(
            line.to_owned(),
            Style::default().fg(Color::Gray),
        )));
    }
    out
}

fn split_plain_lines(raw: &str, color: Color) -> Vec<Line<'static>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return vec![Line::from(Span::styled(
            "(no output)",
            Style::default().fg(Color::DarkGray),
        ))];
    }
    trimmed
        .lines()
        .map(|l| Line::from(Span::styled(l.to_owned(), Style::default().fg(color))))
        .collect()
}

fn arg_str(args: &Value, key: &str, max_chars: usize) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(|s| clip_single_line(s, max_chars))
}

fn clip_single_line(raw: &str, max_chars: usize) -> String {
    let single = raw.lines().next().unwrap_or_default().trim();
    if single.chars().count() <= max_chars {
        single.to_owned()
    } else {
        let prefix: String = single.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{prefix}…")
    }
}

fn title_case(raw: &str) -> String {
    let mut chars = raw.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => raw.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hides_tool_outputs_until_ctrl_o_then_renders_tree_connector() {
        let many_lines = (1..=10)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let call = ToolCall {
            id: "c1".to_owned(),
            name: "bash".to_owned(),
            arguments: json!({"command": "cargo test"}),
        };
        let entry = TranscriptEntry::tool_completed(
            &call,
            &ToolOutput {
                call_id: "c1".to_owned(),
                content: many_lines,
                is_error: false,
            },
        );

        let collapsed = render_transcript_lines(std::slice::from_ref(&entry), 90, false);
        let collapsed_text: Vec<String> = collapsed
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert_eq!(
            collapsed_text,
            vec!["● Bash(cargo test) (ctrl+o to expand)"]
        );

        let expanded = render_transcript_lines(&[entry], 90, true);
        let expanded_text: Vec<String> = expanded
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert_eq!(expanded_text[0], "● Bash(cargo test)");
        assert_eq!(expanded_text[1], "  └  line 1");
        assert!(expanded_text.iter().any(|l| l.contains("line 10")));
        assert!(
            expanded_text
                .iter()
                .any(|l| l.contains("(ctrl+o to collapse)"))
        );
    }

    #[test]
    fn renders_todo_and_thinking_with_ctrl_o_expansion() {
        let call = ToolCall {
            id: "t1".to_owned(),
            name: "todo_write".to_owned(),
            arguments: json!({"todos": []}),
        };
        let todo_entry = TranscriptEntry::tool_completed(
            &call,
            &ToolOutput {
                call_id: "t1".to_owned(),
                content: json!({
                    "todos": [
                        {"id": "1", "content": "Done step", "status": "completed"},
                        {"id": "2", "content": "Active step", "status": "in_progress"},
                        {"id": "3", "content": "Next step", "status": "pending"}
                    ]
                })
                .to_string(),
                is_error: false,
            },
        );
        let thought = TranscriptEntry::Thinking {
            content: "first line\nsecond line".to_owned(),
            elapsed_secs: 4,
            active: false,
        };

        let collapsed = render_transcript_lines(&[thought.clone(), todo_entry.clone()], 90, false);
        let collapsed_text: Vec<String> = collapsed
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert!(collapsed_text[0].starts_with("▸ Thought for 4s"));
        assert_eq!(collapsed_text[1], "  └  first line");
        assert!(!collapsed_text.iter().any(|l| l.contains("second line")));
        assert!(
            collapsed_text
                .iter()
                .any(|l| l == "● Update Todos (ctrl+o to expand)")
        );

        let expanded = render_transcript_lines(&[thought, todo_entry], 90, true);
        let expanded_text: Vec<String> = expanded
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert!(expanded_text[0].starts_with("▾ Thought for 4s"));
        assert!(expanded_text.iter().any(|l| l == "     second line"));
        assert!(expanded_text.iter().any(|l| l == "● Update Todos"));
        assert!(expanded_text.iter().any(|l| l == "  └  ☑ Done step"));
        assert!(expanded_text.iter().any(|l| l == "     ☐ Active step"));
        assert!(expanded_text.iter().any(|l| l == "     ☐ Next step"));
    }
}
