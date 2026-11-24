// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Tool batch execution, parallel subagent dispatch, and policy approval handling

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use rein_core::{EventLog, Message, ModeId, Role, ToolCall, ToolOutput, Turn};
use rein_policy::{PolicyDecision, PolicyEngine};
use rein_tools::{ToolRegistry, parse_mode_id};

use crate::agent::snapshot::now_unix_secs;
use crate::config::append_allowed_command;
use crate::modes::{ModeManager, Profile};

use super::format::{build_approval_request, format_tool_status, record_full_message};
use super::{ApprovalAction, ApprovalPrompter, HydratedEvent};

pub(super) fn build_mode_profile(
    mode: ModeId,
    allowed_commands: &[String],
    extra_tools: &[String],
) -> Profile {
    Profile::for_mode(mode)
        .with_allowed_commands(allowed_commands.to_vec())
        .with_extra_tools(extra_tools)
}

pub(super) fn finalize_cancelled_stream(
    streaming: Turn<'_, rein_core::Streaming>,
    full_transcript_path: Option<&Path>,
    history: &mut Vec<Message>,
) -> Result<()> {
    let (_idle, mut msg) = streaming.finish_response()?;
    if msg.content.trim().is_empty() {
        msg.content = "[stopped by user]".to_owned();
    }
    record_full_message(full_transcript_path, &msg)?;
    history.push(msg);
    Ok(())
}

pub(super) fn flush_tool_round(
    history: &mut Vec<Message>,
    items: &mut Vec<HydratedEvent>,
    round_text: &mut String,
    pending_calls: &mut Vec<ToolCall>,
    pending_tools: &mut Vec<Message>,
) {
    let text = std::mem::take(round_text);
    let calls = std::mem::take(pending_calls);
    let tools = std::mem::take(pending_tools);
    if calls.is_empty() && tools.is_empty() {
        return;
    }
    let completed_calls: Vec<ToolCall> = calls
        .into_iter()
        .filter(|call| {
            tools
                .iter()
                .any(|msg| msg.tool_calls.first().is_some_and(|c| c.id == call.id))
        })
        .collect();
    if completed_calls.is_empty() {
        return;
    }
    if !text.trim().is_empty() {
        items.push(HydratedEvent::Assistant(text.clone()));
    }
    let mut assistant_msg = Message::new(Role::Assistant, text);
    assistant_msg.tool_calls = completed_calls;
    history.push(assistant_msg);
    history.extend(tools);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_tool_batch(
    streaming: Turn<'_, rein_core::Streaming>,
    event_log: &EventLog,
    full_transcript_path: Option<&Path>,
    repo_commands_path: Option<&Path>,
    global_commands_path: Option<&Path>,
    policy: &mut PolicyEngine,
    allowed_commands: &mut Vec<String>,
    extra_tools: &[String],
    registry: &ToolRegistry,
    modes: &mut ModeManager,
    history: &mut Vec<Message>,
    streamed_text: &str,
    calls: Vec<ToolCall>,
    prompter: &mut dyn ApprovalPrompter,
    cancel: &AtomicBool,
) -> Result<()> {
    let mut assistant_msg = Message::new(Role::Assistant, streamed_text);
    assistant_msg.tool_calls = calls.clone();
    record_full_message(full_transcript_path, &assistant_msg)?;
    history.push(assistant_msg);

    if calls.len() > 1 && calls.iter().all(|c| c.name == "agent") {
        return execute_parallel_agent_batch(
            streaming,
            full_transcript_path,
            policy,
            registry,
            modes,
            history,
            calls,
            prompter,
            cancel,
        );
    }

    let mut iter = calls.into_iter();
    let Some(first) = iter.next() else {
        return Ok(());
    };
    let mut idle = execute_awaiting(
        streaming.request_tool(first)?,
        event_log,
        full_transcript_path,
        repo_commands_path,
        global_commands_path,
        policy,
        allowed_commands,
        extra_tools,
        registry,
        modes,
        history,
        prompter,
        cancel,
    )?;
    for call in iter {
        idle = execute_awaiting(
            idle.request_tool(call)?,
            event_log,
            full_transcript_path,
            repo_commands_path,
            global_commands_path,
            policy,
            allowed_commands,
            extra_tools,
            registry,
            modes,
            history,
            prompter,
            cancel,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_parallel_agent_batch(
    streaming: Turn<'_, rein_core::Streaming>,
    full_transcript_path: Option<&Path>,
    policy: &PolicyEngine,
    registry: &ToolRegistry,
    modes: &ModeManager,
    history: &mut Vec<Message>,
    calls: Vec<ToolCall>,
    prompter: &mut dyn ApprovalPrompter,
    cancel: &AtomicBool,
) -> Result<()> {
    for call in &calls {
        prompter.on_tool_status(&format_tool_status(call));
        prompter.on_tool_start(call);
    }

    let mut decisions = Vec::with_capacity(calls.len());
    let temp_dir = std::env::temp_dir().join(format!(
        "rein_parallel_eval_{}_{}",
        std::process::id(),
        now_unix_secs()
    ));
    let _ = std::fs::create_dir_all(&temp_dir);
    let eval_log = EventLog::new(temp_dir.join("eval.jsonl"));
    for call in &calls {
        let awaiting = Turn::new(&eval_log).request_tool(call.clone())?;
        let decision =
            registry.evaluate_turn_for_profile(&awaiting, policy, &modes.active().allowed_tools)?;
        decisions.push(decision);
    }
    let _ = std::fs::remove_dir_all(&temp_dir);

    let mut raw_outputs: Vec<Option<ToolOutput>> = vec![None; calls.len()];
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for (idx, (call, decision)) in calls.iter().zip(decisions.iter()).enumerate() {
            if cancel.load(Ordering::Relaxed) {
                continue;
            }
            if matches!(decision, PolicyDecision::Allow { .. }) {
                handles.push((idx, scope.spawn(move || registry.invoke_raw(call))));
            }
        }
        for (idx, handle) in handles {
            if let Ok(Ok(out)) = handle.join() {
                raw_outputs[idx] = Some(out);
            }
        }
    });

    let mut iter = calls.into_iter().zip(decisions).zip(raw_outputs);
    let Some(((first_call, first_decision), first_out)) = iter.next() else {
        return Ok(());
    };
    let mut idle = commit_parallel_agent_turn(
        streaming.request_tool(first_call)?,
        first_decision,
        first_out,
        full_transcript_path,
        history,
        prompter,
        cancel,
    )?;
    for ((call, decision), out) in iter {
        idle = commit_parallel_agent_turn(
            idle.request_tool(call)?,
            decision,
            out,
            full_transcript_path,
            history,
            prompter,
            cancel,
        )?;
    }
    Ok(())
}

pub(super) fn commit_parallel_agent_turn<'a>(
    awaiting: Turn<'a, rein_core::AwaitingPolicy>,
    decision: PolicyDecision,
    precomputed: Option<ToolOutput>,
    full_transcript_path: Option<&Path>,
    history: &mut Vec<Message>,
    prompter: &mut dyn ApprovalPrompter,
    cancel: &AtomicBool,
) -> Result<Turn<'a, rein_core::Idle>> {
    let call = awaiting.pending_call().clone();
    let (idle, output) = if cancel.load(Ordering::Relaxed) {
        awaiting.deny("cancelled by user")?
    } else {
        match (decision, precomputed) {
            (PolicyDecision::Allow { reason }, Some(out)) => {
                let executing = awaiting.approve(reason)?;
                let done = executing.complete(out.clone())?;
                (done, out)
            }
            (PolicyDecision::Allow { reason }, None) => {
                awaiting.deny(format!("parallel sub-agent execution failed ({reason})"))?
            }
            (PolicyDecision::Deny { reason }, _) => awaiting.deny(reason)?,
            (PolicyDecision::Ask { prompt }, _) => awaiting.deny(format!(
                "parallel sub-agent batch requires policy-allowed role: {prompt}"
            ))?,
        }
    };
    prompter.on_tool_finish(&call, &output);
    let mut tool_msg = Message::new(Role::Tool, output.content);
    tool_msg.tool_calls.push(call);
    record_full_message(full_transcript_path, &tool_msg)?;
    history.push(tool_msg);
    Ok(idle)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_awaiting<'a>(
    awaiting: Turn<'a, rein_core::AwaitingPolicy>,
    event_log: &EventLog,
    full_transcript_path: Option<&Path>,
    repo_commands_path: Option<&Path>,
    global_commands_path: Option<&Path>,
    policy: &mut PolicyEngine,
    allowed_commands: &mut Vec<String>,
    extra_tools: &[String],
    registry: &ToolRegistry,
    modes: &mut ModeManager,
    history: &mut Vec<Message>,
    prompter: &mut dyn ApprovalPrompter,
    cancel: &AtomicBool,
) -> Result<Turn<'a, rein_core::Idle>> {
    let call = awaiting.pending_call().clone();
    prompter.on_tool_status(&format_tool_status(&call));
    prompter.on_tool_start(&call);

    if cancel.load(Ordering::Relaxed) {
        let (idle, mut output) = awaiting.deny("cancelled by user")?;
        if output.content.is_empty() {
            output.content = "cancelled by user".to_owned();
        }
        prompter.on_tool_finish(&call, &output);
        let mut tool_msg = Message::new(Role::Tool, output.content);
        tool_msg.tool_calls.push(call);
        record_full_message(full_transcript_path, &tool_msg)?;
        history.push(tool_msg);
        return Ok(idle);
    }

    let decision =
        registry.evaluate_turn_for_profile(&awaiting, policy, &modes.active().allowed_tools)?;
    let (idle, mut output) = match decision {
        PolicyDecision::Allow { reason } => {
            let executing = awaiting.approve(reason)?;
            registry.execute_approved(executing)?
        }
        PolicyDecision::Ask { prompt: ask_prompt } => {
            let req = build_approval_request(&call, &ask_prompt, policy.sandbox());
            let choice = prompter.confirm(&req)?;
            let feedback = choice
                .feedback
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            match choice.action {
                ApprovalAction::AllowOnce => {
                    let executing = awaiting.approve("user approved once in TUI")?;
                    let (done, mut res) = registry.execute_approved(executing)?;
                    if let Some(fb) = feedback {
                        append_user_feedback(&mut res.content, &fb);
                    }
                    (done, res)
                }
                ApprovalAction::AllowCwd(pattern) => {
                    persist_allow_pattern(repo_commands_path, policy, allowed_commands, &pattern)?;
                    let executing = awaiting.approve(format!("user allowed '{pattern}' in CWD"))?;
                    let (done, mut res) = registry.execute_approved(executing)?;
                    if let Some(fb) = feedback {
                        append_user_feedback(&mut res.content, &fb);
                    }
                    (done, res)
                }
                ApprovalAction::AllowGlobal(pattern) => {
                    persist_allow_pattern(
                        global_commands_path,
                        policy,
                        allowed_commands,
                        &pattern,
                    )?;
                    let executing =
                        awaiting.approve(format!("user allowed '{pattern}' globally"))?;
                    let (done, mut res) = registry.execute_approved(executing)?;
                    if let Some(fb) = feedback {
                        append_user_feedback(&mut res.content, &fb);
                    }
                    (done, res)
                }
                ApprovalAction::Deny => {
                    let reason = match feedback {
                        Some(fb) => format!("user denied in TUI — User feedback: {fb}"),
                        None => "user denied in TUI".to_owned(),
                    };
                    awaiting.deny(reason)?
                }
            }
        }
        PolicyDecision::Deny { reason } => awaiting.deny(reason)?,
    };

    if call.name == "switch_mode"
        && !output.is_error
        && let Some(raw_mode) = call
            .arguments
            .get("mode")
            .and_then(serde_json::Value::as_str)
        && let Ok(target_mode) = parse_mode_id(raw_mode)
        && modes.active().mode != target_mode
    {
        let next_profile = build_mode_profile(target_mode, allowed_commands, extra_tools);
        let _ = modes.switch_to(next_profile, policy, event_log)?;
        prompter.on_mode_switched(modes.active());
    }

    if output.content.is_empty() {
        output.content = "(no output)".to_owned();
    }
    prompter.on_tool_finish(&call, &output);
    let mut tool_msg = Message::new(Role::Tool, output.content);
    tool_msg.tool_calls.push(call);
    record_full_message(full_transcript_path, &tool_msg)?;
    history.push(tool_msg);
    Ok(idle)
}

pub(super) fn persist_allow_pattern(
    persist_path: Option<&Path>,
    policy: &mut PolicyEngine,
    allowed_commands: &mut Vec<String>,
    pattern: &str,
) -> Result<()> {
    if let Some(path) = persist_path {
        append_allowed_command(path, pattern)?;
    }
    policy.allow_command_pattern(pattern);
    if !allowed_commands.iter().any(|p| p == pattern) {
        allowed_commands.push(pattern.to_owned());
    }
    Ok(())
}

pub(super) fn append_user_feedback(content: &mut String, feedback: &str) {
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str("[User feedback: ");
    content.push_str(feedback);
    content.push(']');
}
