// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Approval request formatting, tool status summaries, and history serialization helpers

use std::path::Path;

use anyhow::{Context, Result};
use rein_core::{EventLog, Message, Role, SessionEvent, ToolCall, ToolSpec};
use rein_policy::{WorkspaceSandbox, suggest_command_allow_pattern};
use rein_tools::ToolSchema;

use super::ApprovalRequest;

/// Builds a numbered 4-option [`ApprovalRequest`] for any `PolicyDecision::Ask`.
#[must_use]
pub fn build_approval_request(
    call: &ToolCall,
    base_prompt: &str,
    sandbox: &WorkspaceSandbox,
) -> ApprovalRequest {
    if call.name == "bash" {
        let (program, args) = if let Some(cmd) = call
            .arguments
            .get("command")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            ("bash".to_owned(), vec!["-c".to_owned(), cmd.to_owned()])
        } else {
            let prog = call
                .arguments
                .get("program")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("bash")
                .to_owned();
            let argv = call
                .arguments
                .get("args")
                .and_then(serde_json::Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            (prog, argv)
        };
        let (excerpt, pattern) = suggest_command_allow_pattern(&program, &args, sandbox);
        return ApprovalRequest {
            title: base_prompt.to_owned(),
            excerpt,
            pattern,
        };
    }

    ApprovalRequest {
        title: base_prompt.to_owned(),
        excerpt: format_confirmation_prompt(call, base_prompt),
        pattern: format!("tool:{}", call.name),
    }
}

pub(super) const COMPACT_SYSTEM_PROMPT: &str = "Summarize the conversation faithfully for context continuity. Preserve file paths, symbols, decisions, constraints, tool outcomes, and open tasks without inventing facts.";

pub(super) fn record_full_message(path: Option<&Path>, message: &Message) -> Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    EventLog::new(path)
        .append(&SessionEvent::MessageRecorded {
            message: message.clone(),
        })
        .context("failed to append to session full transcript")
}

pub(super) fn format_history_for_summary(history: &[Message], focus: Option<&str>) -> String {
    let mut out = String::from("Summarize every important detail from this conversation:\n\n");
    if let Some(focus_text) = focus.filter(|f| !f.trim().is_empty()) {
        out.push_str("Focus especially on: ");
        out.push_str(focus_text.trim());
        out.push_str("\n\n");
    }
    for msg in history {
        let role_label = match msg.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::Tool => "Tool",
            Role::System => "System",
        };
        out.push_str(role_label);
        if !msg.tool_calls.is_empty() {
            let names: Vec<_> = msg.tool_calls.iter().map(|c| c.name.as_str()).collect();
            out.push_str(" [tools: ");
            out.push_str(&names.join(", "));
            out.push(']');
        }
        out.push_str(": ");
        out.push_str(&msg.content);
        out.push('\n');
    }
    out
}

/// Formats a compact 1-line tool indicator shown inline in the TUI transcript.
#[must_use]
pub fn format_tool_status(call: &ToolCall) -> String {
    if call.name == "agent" {
        let kind = call
            .arguments
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("research");
        let role = call
            .arguments
            .get("role")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Subagent");
        return format!("· Agent({kind}: {})", truncate_inline(role, 48));
    }
    let target = call
        .arguments
        .get("path")
        .or_else(|| call.arguments.get("pattern"))
        .or_else(|| call.arguments.get("command"))
        .or_else(|| call.arguments.get("program"))
        .or_else(|| call.arguments.get("action"))
        .or_else(|| call.arguments.get("mode"))
        .or_else(|| call.arguments.get("key"))
        .and_then(serde_json::Value::as_str);
    match target {
        Some(arg) => format!("· {} {}", call.name, truncate_inline(arg, 60)),
        None => format!("· {}", call.name),
    }
}

/// Enhances `PolicyDecision::Ask` prompts with concise inline diff/summary for `write` and `edit`.
#[must_use]
pub fn format_confirmation_prompt(call: &ToolCall, base_prompt: &str) -> String {
    let args = &call.arguments;
    let path = args
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<unknown>");
    match call.name.as_str() {
        "edit" => {
            let old = args
                .get("old")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let new = args
                .get("new")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            format!(
                "edit {path} (-{} / +{})",
                truncate_inline(old, 32),
                truncate_inline(new, 32)
            )
        }
        "write" => {
            let content = args
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            format!(
                "write {path} ({} lines, {} bytes)",
                content.lines().count(),
                content.len()
            )
        }
        _ => base_prompt.to_owned(),
    }
}

fn truncate_inline(raw: &str, max_chars: usize) -> String {
    let single = raw.lines().next().unwrap_or_default().trim();
    if single.chars().count() <= max_chars {
        single.to_owned()
    } else {
        let prefix: String = single.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{prefix}…")
    }
}

pub(super) fn schema_to_spec(schema: &ToolSchema) -> ToolSpec {
    ToolSpec {
        name: schema.name.clone(),
        description: schema.description.clone(),
        input_schema: schema.input_schema.clone(),
    }
}

