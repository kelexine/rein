// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Deterministic ReplayProvider driven by recorded JSONL SessionEvents

use std::collections::VecDeque;

use crate::event::{CoreError, SessionEvent};
use crate::message::{Message, Role};
use crate::provider::{Provider, ProviderStreamEvent, ToolSpec};

/// Deterministic replay provider driven by recorded [`SessionEvent`] logs with zero network calls.
#[derive(Debug, Clone)]
pub struct ReplayProvider {
    queued_turns: VecDeque<Vec<ProviderStreamEvent>>,
    cursor: u64,
}

impl ReplayProvider {
    /// Reconstructs replayable model turns from a recorded [`SessionEvent`] slice.
    #[must_use]
    pub fn from_events(events: &[SessionEvent]) -> Self {
        let mut queued_turns = VecDeque::new();
        let mut current = Vec::new();
        for event in events {
            match event {
                SessionEvent::StreamDelta { delta } => {
                    current.push(ProviderStreamEvent::TextDelta(delta.clone()));
                }
                SessionEvent::ToolRequested { call } => {
                    current.push(ProviderStreamEvent::ToolUse(call.clone()));
                    current.push(ProviderStreamEvent::Done);
                    queued_turns.push_back(std::mem::take(&mut current));
                }
                SessionEvent::MessageRecorded { message } if message.role == Role::Assistant => {
                    current.push(ProviderStreamEvent::Done);
                    queued_turns.push_back(std::mem::take(&mut current));
                }
                _ => {}
            }
        }
        Self {
            queued_turns,
            cursor: 0,
        }
    }
}

impl Provider for ReplayProvider {
    fn name(&self) -> &str {
        "replay"
    }

    fn stream_turn_with(
        &mut self,
        _system_prompt: &str,
        _messages: &[Message],
        _tools: &[ToolSpec],
        on_event: &mut dyn FnMut(&ProviderStreamEvent),
    ) -> Result<Vec<ProviderStreamEvent>, CoreError> {
        let seq = self.cursor;
        self.cursor += 1;
        let turn = self
            .queued_turns
            .pop_front()
            .ok_or(CoreError::ReplayExhausted { seq })?;
        for ev in &turn {
            on_event(ev);
        }
        Ok(turn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventLog;
    use crate::message::{ToolCall, ToolOutput};
    use crate::state::Turn;

    #[test]
    fn typestate_and_replay_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let temp_path = std::env::temp_dir().join("rein_core_replay_test.jsonl");
        let _ = std::fs::remove_file(&temp_path);

        let log = EventLog::new(&temp_path);
        let turn = Turn::new(&log).begin_user_turn("list files")?;
        let awaiting = turn.request_tool(ToolCall {
            id: "call_1".to_owned(),
            name: "read".to_owned(),
            arguments: serde_json::json!({"path": "Cargo.toml"}),
        })?;
        let executing = awaiting.approve("workspace read allowed")?;
        let _idle = executing.complete(ToolOutput {
            call_id: "call_1".to_owned(),
            content: "[workspace]".to_owned(),
            is_error: false,
        })?;

        let events = log.read_all()?;
        let mut replay = ReplayProvider::from_events(&events);
        let replayed = replay.stream_turn("system", &[], &[])?;
        assert_eq!(replayed.len(), 2);
        let _ = std::fs::remove_file(&temp_path);
        Ok(())
    }
}
