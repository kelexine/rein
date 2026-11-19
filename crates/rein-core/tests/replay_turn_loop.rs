// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-18
// Purpose: Integration tests for EventLog, Turn typestate transitions, and ReplayProvider

//! Integration tests for `rein-core` event log, typestate turns, and replay provider.

use std::path::PathBuf;

use rein_core::{
    CoreError, EventLog, Provider, ProviderStreamEvent, ReplayProvider, Role, SessionEvent,
    ToolCall, ToolOutput, Turn,
};
use serde_json::json;

fn fresh_temp_dir(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!("rein_core_it_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[test]
fn turn_typestate_and_replay_provider_round_trip() -> Result<(), Box<dyn std::error::Error>> {
    let dir = fresh_temp_dir("round_trip")?;
    let log_path = dir.join("session.jsonl");
    let log = EventLog::new(&log_path);

    let call = ToolCall {
        id: "call_1".to_owned(),
        name: "read".to_owned(),
        arguments: json!({ "path": "Cargo.toml" }),
    };

    // Drive Turn: Idle -> Streaming -> AwaitingPolicy -> Executing -> Idle -> Streaming -> Idle
    let streaming = Turn::new(&log).begin_user_turn("inspect Cargo.toml")?;
    let awaiting = streaming.request_tool(call.clone())?;
    assert_eq!(awaiting.pending_call(), &call);
    let executing = awaiting.approve("allowlisted read")?;
    let idle = executing.complete(ToolOutput {
        call_id: "call_1".to_owned(),
        content: "[workspace]".to_owned(),
        is_error: false,
    })?;

    let mut reply_turn = idle.begin_user_turn("summarize")?;
    reply_turn.push_delta("Workspace ")?;
    reply_turn.push_delta("ready.")?;
    let (_idle, assistant_msg) = reply_turn.finish_response()?;
    assert_eq!(assistant_msg.role, Role::Assistant);
    assert_eq!(assistant_msg.content, "Workspace ready.");

    // Reload from disk and replay deterministically.
    let recorded = EventLog::new(&log_path).read_all()?;
    assert!(recorded.len() >= 6);

    let mut replay = ReplayProvider::from_events(&recorded);
    let mut first_turn = Vec::new();
    replay.stream_turn_with("sys", &[], &[], &mut |ev| first_turn.push(ev.clone()))?;
    assert!(
        first_turn
            .iter()
            .any(|ev| matches!(ev, ProviderStreamEvent::ToolUse(c) if c.id == "call_1"))
    );

    let mut second_turn = Vec::new();
    replay.stream_turn_with("sys", &[], &[], &mut |ev| second_turn.push(ev.clone()))?;
    let text: String = second_turn
        .iter()
        .filter_map(|ev| match ev {
            ProviderStreamEvent::TextDelta(d) => Some(d.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "Workspace ready.");

    let exhausted = replay.stream_turn_with("sys", &[], &[], &mut |_| {});
    assert!(matches!(exhausted, Err(CoreError::ReplayExhausted { .. })));

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn turn_deny_records_synthetic_error_output() -> Result<(), Box<dyn std::error::Error>> {
    let dir = fresh_temp_dir("deny")?;
    let log = EventLog::new(dir.join("deny.jsonl"));

    let call = ToolCall {
        id: "call_deny".to_owned(),
        name: "bash".to_owned(),
        arguments: json!({ "command": "rm -rf /" }),
    };

    let streaming = Turn::new(&log).begin_user_turn("danger")?;
    let awaiting = streaming.request_tool(call)?;
    let (_idle, output) = awaiting.deny("destructive command blocked")?;

    assert!(output.is_error);
    assert!(output.content.contains("destructive command blocked"));

    let events = log.read_all()?;
    assert!(
        events
            .iter()
            .any(|ev| matches!(ev, SessionEvent::PolicyEvaluated { allowed: false, .. }))
    );

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
