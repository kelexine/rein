// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-15
// Purpose: Structured transcript entries for reasoning traces, hidden-by-default Ctrl+O tool outputs, diffs, and todos

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use rein_core::{ToolCall, ToolOutput};
use serde_json::Value;

use super::render::{
    arg_str, build_tool_output_lines, clip_single_line, format_read_arg, relative_display_path,
    title_case,
};
use crate::ui::markdown::render_assistant_markdown;

/// Maximum number of body lines shown when expanding a tool output with `Ctrl+O`.
const EXPANDED_TOOL_MAX_LINES: usize = 10;

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
    /// Optional background task/subagent ID when this tool continues asynchronously.
    pub background_task_id: Option<String>,
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
            background_task_id: None,
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
            background_task_id: None,
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
    let mut header_spans = if tool.name == "agent" {
        build_agent_header_spans(dot_color, &tool.arguments)
    } else {
        let (title, arg_label) = format_tool_header(&tool.name, &tool.arguments);
        let mut spans = vec![
            Span::styled("● ", Style::default().fg(dot_color)),
            Span::styled(
                title,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ];
        if let Some(arg) = arg_label {
            spans.push(Span::styled("(", Style::default().fg(Color::DarkGray)));
            spans.push(Span::styled(arg, Style::default().fg(Color::Gray)));
            spans.push(Span::styled(")", Style::default().fg(Color::DarkGray)));
        }
        spans
    };

    if tool.state == ToolRunState::Running {
        let running_label = match &tool.background_task_id {
            Some(id) => format!("  running in background ({id})…"),
            None => "  running…".to_owned(),
        };
        header_spans.push(Span::styled(
            running_label,
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
    let total_lines = body_lines.len();
    let hidden_lines = total_lines.saturating_sub(EXPANDED_TOOL_MAX_LINES);
    for (idx, body) in body_lines
        .into_iter()
        .take(EXPANDED_TOOL_MAX_LINES)
        .enumerate()
    {
        let tree_prefix = if idx == 0 { "  └  " } else { "     " };
        let mut spans = vec![Span::styled(
            tree_prefix,
            Style::default().fg(Color::DarkGray),
        )];
        spans.extend(body.spans);
        out.push(Line::from(spans));
    }
    if hidden_lines > 0 {
        out.push(Line::from(Span::styled(
            format!("     + {hidden_lines} more lines"),
            Style::default().fg(Color::DarkGray),
        )));
    }
    out.push(Line::from(Span::styled(
        "     (ctrl+o to collapse)",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    )));
}

fn build_agent_header_spans(dot_color: Color, args: &Value) -> Vec<Span<'static>> {
    let kind = args
        .get("type")
        .or_else(|| args.get("subagent_type"))
        .and_then(Value::as_str)
        .unwrap_or("research");
    let role = args
        .get("role")
        .or_else(|| args.get("description"))
        .and_then(Value::as_str)
        .map(|s| clip_single_line(s, 48))
        .unwrap_or_else(|| "Subagent".to_owned());
    let mut spans = vec![
        Span::styled("● ", Style::default().fg(dot_color)),
        Span::styled(
            format!("Agent({kind}: {role})"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(prompt) = arg_str(args, "prompt", 110) {
        spans.push(Span::styled("(", Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled(prompt, Style::default().fg(Color::Gray)));
        spans.push(Span::styled(")", Style::default().fg(Color::DarkGray)));
    }
    spans
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
        "read" => (
            "Read".to_owned(),
            arg_str(args, "path", 68).map(|p| format_read_arg(&relative_display_path(&p), args)),
        ),
        "write" => (
            "Write".to_owned(),
            arg_str(args, "path", 68).map(|p| relative_display_path(&p)),
        ),
        "edit" => (
            "Edit".to_owned(),
            arg_str(args, "path", 68).map(|p| relative_display_path(&p)),
        ),
        "glob" => (
            "Glob".to_owned(),
            arg_str(args, "pattern", 68).map(|pat| match arg_str(args, "path", 24) {
                Some(path) if path != "." => format!("{pat} in {}", relative_display_path(&path)),
                _ => pat,
            }),
        ),
        "grep" => (
            "Grep".to_owned(),
            arg_str(args, "query", 52)
                .or_else(|| arg_str(args, "pattern", 52))
                .map(|q| {
                    let filter = arg_str(args, "glob", 20)
                        .or_else(|| arg_str(args, "include", 20))
                        .or_else(|| arg_str(args, "type", 16));
                    match filter {
                        Some(f) => format!("{q} · {f}"),
                        None => q,
                    }
                }),
        ),
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
