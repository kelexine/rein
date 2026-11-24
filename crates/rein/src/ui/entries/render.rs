// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Tool output line formatting (bash, todo, edit diffs, write, read) and entry tests

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::model::{ToolRunState, ToolTranscriptEntry};

pub(super) fn build_tool_output_lines(tool: &ToolTranscriptEntry) -> Vec<Line<'static>> {
    if tool.name == "bash" {
        return render_bash_lines(tool);
    }
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

fn render_bash_lines(tool: &ToolTranscriptEntry) -> Vec<Line<'static>> {
    let fallback_color = if tool.state == ToolRunState::Error {
        Color::LightRed
    } else {
        Color::Gray
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&tool.output) else {
        return split_plain_lines(&tool.output, fallback_color);
    };
    let Some(obj) = parsed.as_object() else {
        return split_plain_lines(&tool.output, fallback_color);
    };
    if obj.get("status").and_then(Value::as_str) == Some("backgrounded") {
        let task_id = obj.get("task_id").and_then(Value::as_str).unwrap_or("task");
        let is_daemon = obj
            .get("is_daemon")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let label = if is_daemon {
            format!("daemon started in background ({task_id})")
        } else {
            format!("running in background ({task_id})")
        };
        let mut out = vec![Line::from(Span::styled(
            label,
            Style::default().fg(Color::Yellow),
        ))];
        if let Some(partial) = obj
            .get("partial_output")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            for line in partial.lines() {
                out.push(Line::from(Span::styled(
                    line.to_owned(),
                    Style::default().fg(Color::Gray),
                )));
            }
        }
        if let Some(output_file) = obj.get("output_file").and_then(Value::as_str) {
            out.push(Line::from(Span::styled(
                format!("log: {}", relative_display_path(output_file)),
                Style::default().fg(Color::DarkGray),
            )));
        }
        return out;
    }
    if !obj.contains_key("stdout")
        && !obj.contains_key("stderr")
        && !obj.contains_key("status_code")
    {
        return split_plain_lines(&tool.output, fallback_color);
    }

    let status_code = obj.get("status_code").and_then(Value::as_i64).unwrap_or(0);
    let stdout = obj
        .get("stdout")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_matches(|c| c == '\n' || c == '\r');
    let stderr = obj
        .get("stderr")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_matches(|c| c == '\n' || c == '\r');

    let mut out = Vec::new();
    for line in stdout.lines() {
        out.push(Line::from(Span::styled(
            line.to_owned(),
            Style::default().fg(Color::Gray),
        )));
    }
    let stderr_color = if status_code != 0 || tool.state == ToolRunState::Error {
        Color::LightRed
    } else {
        Color::Gray
    };
    for line in stderr.lines() {
        out.push(Line::from(Span::styled(
            line.to_owned(),
            Style::default().fg(stderr_color),
        )));
    }
    if out.is_empty() {
        if status_code == 0 {
            out.push(Line::from(Span::styled(
                "(no output)",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            out.push(Line::from(Span::styled(
                format!("exit code {status_code}"),
                Style::default().fg(Color::LightRed),
            )));
        }
    } else if status_code != 0 {
        out.push(Line::from(Span::styled(
            format!("exit code {status_code}"),
            Style::default().fg(Color::LightRed),
        )));
    }
    if let Some(output_file) = obj.get("output_file").and_then(Value::as_str)
        && obj.get("truncated").and_then(Value::as_bool) == Some(true)
    {
        out.push(Line::from(Span::styled(
            format!("full output: {}", relative_display_path(output_file)),
            Style::default().fg(Color::DarkGray),
        )));
    }
    out
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
    if tool.state == ToolRunState::Error {
        return split_plain_lines(&tool.output, Color::LightRed);
    }
    let old = tool
        .arguments
        .get("old_text")
        .or_else(|| tool.arguments.get("old"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let new = tool
        .arguments
        .get("new_text")
        .or_else(|| tool.arguments.get("new"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if old.is_empty() && new.is_empty() {
        return split_plain_lines(&tool.output, Color::Gray);
    }

    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let prefix_len = old_lines
        .iter()
        .zip(new_lines.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let suffix_len = old_lines[prefix_len..]
        .iter()
        .rev()
        .zip(new_lines[prefix_len..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();

    let old_changed = &old_lines[prefix_len..old_lines.len() - suffix_len];
    let new_changed = &new_lines[prefix_len..new_lines.len() - suffix_len];
    let added = new_changed.len();
    let removed = old_changed.len();

    let mut out = vec![edit_diff_summary_line(added, removed)];

    let file_ctx = read_edit_file_context(tool, new, new_lines.len());
    let base_line = file_ctx.as_ref().map_or(1, |ctx| ctx.start_line);

    if let Some(ctx) = &file_ctx {
        for (idx, line) in ctx.before.iter().enumerate() {
            let line_no = base_line - ctx.before.len() + idx;
            out.push(diff_context_line(line_no, line));
        }
    }

    for (idx, line) in new_lines[..prefix_len].iter().enumerate() {
        out.push(diff_context_line(base_line + idx, line));
    }

    let change_start = base_line + prefix_len;
    for (idx, line) in old_changed.iter().enumerate() {
        out.push(Line::from(Span::styled(
            format!("{:>4} - {line}", change_start + idx),
            Style::default()
                .fg(Color::Rgb(245, 165, 165))
                .bg(Color::Rgb(56, 24, 24)),
        )));
    }
    for (idx, line) in new_changed.iter().enumerate() {
        out.push(Line::from(Span::styled(
            format!("{:>4} + {line}", change_start + idx),
            Style::default()
                .fg(Color::Rgb(165, 235, 175))
                .bg(Color::Rgb(24, 52, 30)),
        )));
    }

    let suffix_start = change_start + added;
    for (idx, line) in new_lines[new_lines.len() - suffix_len..].iter().enumerate() {
        out.push(diff_context_line(suffix_start + idx, line));
    }
    if let Some(ctx) = &file_ctx {
        let after_start = base_line + new_lines.len();
        for (idx, line) in ctx.after.iter().enumerate() {
            out.push(diff_context_line(after_start + idx, line));
        }
    }

    out
}

struct EditFileContext {
    start_line: usize,
    before: Vec<String>,
    after: Vec<String>,
}

fn read_edit_file_context(
    tool: &ToolTranscriptEntry,
    new_text: &str,
    new_line_count: usize,
) -> Option<EditFileContext> {
    if new_text.is_empty() {
        return None;
    }
    let path_str = tool.arguments.get("path").and_then(Value::as_str)?;
    let raw = std::fs::read_to_string(path_str).ok()?;
    let byte_pos = raw.find(new_text)?;
    let start_line = raw[..byte_pos].bytes().filter(|&b| b == b'\n').count() + 1;
    let all_lines: Vec<&str> = raw.lines().collect();
    let zero_idx = start_line.saturating_sub(1);
    let before_start = zero_idx.saturating_sub(2);
    let before = all_lines[before_start..zero_idx]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let after_start = (zero_idx + new_line_count).min(all_lines.len());
    let after_end = (after_start + 2).min(all_lines.len());
    let after = all_lines[after_start..after_end]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    Some(EditFileContext {
        start_line,
        before,
        after,
    })
}

fn edit_diff_summary_line(added: usize, removed: usize) -> Line<'static> {
    let plural = |n: usize| if n == 1 { "line" } else { "lines" };
    if added > 0 && removed == 0 {
        Line::from(Span::styled(
            format!("Added {added} {}", plural(added)),
            Style::default().fg(Color::Rgb(115, 210, 125)),
        ))
    } else if removed > 0 && added == 0 {
        Line::from(Span::styled(
            format!("Removed {removed} {}", plural(removed)),
            Style::default().fg(Color::LightRed),
        ))
    } else {
        Line::from(vec![
            Span::styled(
                format!("+{added} "),
                Style::default().fg(Color::Rgb(115, 210, 125)),
            ),
            Span::styled("/ ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("-{removed} {}", plural(removed)),
                Style::default().fg(Color::LightRed),
            ),
        ])
    }
}

fn diff_context_line(line_no: usize, line: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("{line_no:>4}   {line}"),
        Style::default().fg(Color::Gray),
    ))
}

fn render_write_lines(tool: &ToolTranscriptEntry) -> Vec<Line<'static>> {
    if tool.state == ToolRunState::Error {
        return split_plain_lines(&tool.output, Color::LightRed);
    }
    let content = tool
        .arguments
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let path = tool
        .arguments
        .get("path")
        .and_then(Value::as_str)
        .map(relative_display_path)
        .unwrap_or_else(|| "file".to_owned());
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

pub(super) fn format_read_arg(path: &str, args: &Value) -> String {
    let start = args.get("start_line").and_then(Value::as_u64);
    let end = args.get("end_line").and_then(Value::as_u64);
    match (start, end) {
        (Some(s), Some(e)) => format!("{path}:{s}-{e}"),
        (Some(s), None) => format!("{path}:{s}-"),
        (None, Some(e)) => format!("{path}:1-{e}"),
        (None, None) => path.to_owned(),
    }
}

pub(super) fn relative_display_path(raw: &str) -> String {
    let path = std::path::Path::new(raw);
    if path.is_absolute() {
        if let Ok(cwd) = std::env::current_dir()
            && let Ok(rel) = path.strip_prefix(&cwd)
        {
            let s = rel.to_string_lossy();
            return if s.is_empty() {
                ".".to_owned()
            } else {
                s.into_owned()
            };
        }
        if let Some(home) = std::env::var_os("HOME")
            && let Ok(rel) = path.strip_prefix(std::path::Path::new(&home))
        {
            return format!("~/{}", rel.to_string_lossy());
        }
    }
    raw.to_owned()
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

pub(super) fn arg_str(args: &Value, key: &str, max_chars: usize) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(|s| clip_single_line(s, max_chars))
}

pub(super) fn clip_single_line(raw: &str, max_chars: usize) -> String {
    let single = raw.lines().next().unwrap_or_default().trim();
    if single.chars().count() <= max_chars {
        single.to_owned()
    } else {
        let prefix: String = single.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{prefix}…")
    }
}

pub(super) fn title_case(raw: &str) -> String {
    let mut chars = raw.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => raw.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::*;
    use super::*;
    use rein_core::{ToolCall, ToolOutput};
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

    #[test]
    fn summarizes_file_and_search_tool_headers_and_marks_errors() {
        let cases = [
            ("read", json!({"path": "Cargo.toml"}), "● Read(Cargo.toml)"),
            (
                "write",
                json!({"path": "src/lib.rs", "content": "x"}),
                "● Write(src/lib.rs)",
            ),
            (
                "edit",
                json!({"path": "src/main.rs", "old_text": "a", "new_text": "b"}),
                "● Edit(src/main.rs)",
            ),
            (
                "grep",
                json!({"query": "PolicyEngine"}),
                "● Grep(PolicyEngine)",
            ),
            ("glob", json!({"pattern": "*.rs"}), "● Glob(*.rs)"),
        ];
        for (name, arguments, expected_header) in cases {
            let call = ToolCall {
                id: format!("id_{name}"),
                name: name.to_owned(),
                arguments,
            };
            let entry = TranscriptEntry::tool_completed(
                &call,
                &ToolOutput {
                    call_id: call.id.clone(),
                    content: "ok".to_owned(),
                    is_error: false,
                },
            );
            let lines = render_transcript_lines(&[entry], 90, false);
            let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
            assert_eq!(header, format!("{expected_header} (ctrl+o to expand)"));
        }

        let err_call = ToolCall {
            id: "err_1".to_owned(),
            name: "read".to_owned(),
            arguments: json!({"path": "missing.rs"}),
        };
        let err_entry = TranscriptEntry::tool_completed(
            &err_call,
            &ToolOutput {
                call_id: "err_1".to_owned(),
                content: "file not found".to_owned(),
                is_error: true,
            },
        );
        let err_lines = render_transcript_lines(&[err_entry], 90, true);
        let err_text: Vec<String> = err_lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(err_text.iter().any(|l| l.contains("file not found")));
        assert_eq!(err_lines[0].spans[0].style.fg, Some(Color::LightRed));
    }

    #[test]
    fn expanded_tool_output_truncates_to_ten_lines_and_write_relativizes_path()
    -> Result<(), Box<dyn std::error::Error>> {
        let many_lines: String = (1..=25)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let bash_call = ToolCall {
            id: "bash_1".to_owned(),
            name: "bash".to_owned(),
            arguments: json!({"command": "seq 1 25"}),
        };
        let bash_entry = TranscriptEntry::tool_completed(
            &bash_call,
            &ToolOutput {
                call_id: "bash_1".to_owned(),
                content: many_lines,
                is_error: false,
            },
        );
        let rendered = render_transcript_lines(&[bash_entry], 90, true);
        let text: Vec<String> = rendered
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(text.iter().any(|l| l.contains("line 10")));
        assert!(!text.iter().any(|l| l.contains("line 11")));
        assert!(text.iter().any(|l| l == "     + 15 more lines"));

        let abs_path = std::env::current_dir()?.join("src/new_file.rs");
        let write_call = ToolCall {
            id: "write_1".to_owned(),
            name: "write".to_owned(),
            arguments: json!({
                "path": abs_path.to_string_lossy().to_string(),
                "content": "fn a() {}\nfn b() {}\nfn c() {}"
            }),
        };
        let write_entry = TranscriptEntry::tool_completed(
            &write_call,
            &ToolOutput {
                call_id: "write_1".to_owned(),
                content: "wrote 30 bytes".to_owned(),
                is_error: false,
            },
        );
        let write_rendered = render_transcript_lines(&[write_entry], 90, true);
        let write_text: Vec<String> = write_rendered
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(write_text[0], "● Write(src/new_file.rs)");
        assert_eq!(write_text[1], "  └  Wrote 3 lines to src/new_file.rs");
        Ok(())
    }

    #[test]
    fn edit_diff_renders_added_removed_summaries_with_line_numbers_and_context()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join(format!("rein_edit_diff_ui_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let file_path = dir.join("sample.rs");
        std::fs::write(&file_path, "line 1\nline 2\nadded line\nline 3\nline 4\n")?;

        let add_call = ToolCall {
            id: "edit_add".to_owned(),
            name: "edit".to_owned(),
            arguments: json!({
                "path": file_path.to_string_lossy().to_string(),
                "old_text": "line 2\nline 3",
                "new_text": "line 2\nadded line\nline 3"
            }),
        };
        let add_entry = TranscriptEntry::tool_completed(
            &add_call,
            &ToolOutput {
                call_id: "edit_add".to_owned(),
                content: "edited".to_owned(),
                is_error: false,
            },
        );
        let add_rendered = render_transcript_lines(&[add_entry], 100, true);
        let add_text: Vec<String> = add_rendered
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(add_text.iter().any(|l| l.contains("Added 1 line")));
        assert!(add_text.iter().any(|l| l.contains("   3 + added line")));
        assert!(add_text.iter().any(|l| l.contains("   2   line 2")));

        let del_call = ToolCall {
            id: "edit_del".to_owned(),
            name: "edit".to_owned(),
            arguments: json!({
                "path": "missing.rs",
                "old_text": "keep\ndrop 1\ndrop 2",
                "new_text": "keep"
            }),
        };
        let del_entry = TranscriptEntry::tool_completed(
            &del_call,
            &ToolOutput {
                call_id: "edit_del".to_owned(),
                content: "edited".to_owned(),
                is_error: false,
            },
        );
        let del_rendered = render_transcript_lines(&[del_entry], 100, true);
        let del_text: Vec<String> = del_rendered
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(del_text.iter().any(|l| l.contains("Removed 2 lines")));
        assert!(del_text.iter().any(|l| l.contains("   2 - drop 1")));

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn bash_output_parses_json_stdout_stderr_into_organized_lines() {
        let lines_out: String = (1..=14)
            .map(|i| format!("test case_{i} ... ok"))
            .collect::<Vec<_>>()
            .join("\n");
        let payload = json!({
            "status_code": 0,
            "stdout": format!("\n{lines_out}\n"),
            "stderr": ""
        })
        .to_string();

        let call = ToolCall {
            id: "bash_json".to_owned(),
            name: "bash".to_owned(),
            arguments: json!({"command": "cargo test"}),
        };
        let entry = TranscriptEntry::tool_completed(
            &call,
            &ToolOutput {
                call_id: "bash_json".to_owned(),
                content: payload,
                is_error: false,
            },
        );
        let rendered = render_transcript_lines(&[entry], 100, true);
        let text: Vec<String> = rendered
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(!text.iter().any(|l| l.contains("status_code")));
        assert!(text.iter().any(|l| l.contains("test case_1 ... ok")));
        assert!(text.iter().any(|l| l.contains("test case_10 ... ok")));
        assert!(!text.iter().any(|l| l.contains("test case_11 ... ok")));
        assert!(text.iter().any(|l| l == "     + 4 more lines"));
    }

    #[test]
    fn agent_tool_renders_typed_role_and_prompt_header() {
        let call = ToolCall {
            id: "agent_1".to_owned(),
            name: "agent".to_owned(),
            arguments: json!({
                "type": "research",
                "role": "Vendor Build Auditor",
                "prompt": "Perform a strictly READ-ONLY audit"
            }),
        };
        let entry = TranscriptEntry::tool_completed(
            &call,
            &ToolOutput {
                call_id: "agent_1".to_owned(),
                content: "Audit complete".to_owned(),
                is_error: false,
            },
        );
        let lines = render_transcript_lines(&[entry], 120, false);
        let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(
            header,
            "● Agent(research: Vendor Build Auditor)(Perform a strictly READ-ONLY audit) (ctrl+o to expand)"
        );
    }
}
