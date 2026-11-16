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
        let mut saw_tool = false;

        for event in events {
            match event {
                SessionEvent::StreamDelta { delta } => {
                    if saw_tool {
                        current.push(ProviderStreamEvent::Done);
                        queued_turns.push_back(std::mem::take(&mut current));
                        saw_tool = false;
                    }
                    current.push(ProviderStreamEvent::TextDelta(delta.clone()));
                }
                SessionEvent::ToolRequested { call } => {
                    saw_tool = true;
                    current.push(ProviderStreamEvent::ToolUse(call.clone()));
                }
                SessionEvent::PolicyEvaluated { .. } | SessionEvent::ToolCompleted { .. } => {
                    if saw_tool {
                        current.push(ProviderStreamEvent::Done);
                        queued_turns.push_back(std::mem::take(&mut current));
                        saw_tool = false;
                    }
                }
                SessionEvent::MessageRecorded { message } if message.role == Role::Assistant => {
                    if saw_tool || !current.is_empty() {
                        current.push(ProviderStreamEvent::Done);
                        queued_turns.push_back(std::mem::take(&mut current));
                        saw_tool = false;
                    } else if !message.content.is_empty() {
                        queued_turns.push_back(vec![
                            ProviderStreamEvent::TextDelta(message.content.clone()),
                            ProviderStreamEvent::Done,
                        ]);
                    }
                }
                _ => {}
            }
        }
        if !current.is_empty() {
            current.push(ProviderStreamEvent::Done);
            queued_turns.push_back(current);
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
        assert_eq!(replay.name(), "replay");
        let replayed = replay.stream_turn("system", &[], &[])?;
        assert_eq!(replayed.len(), 2);
        let _ = std::fs::remove_file(&temp_path);
        Ok(())
    }

    #[test]
    fn replays_multi_turn_stream_deltas_and_errors_when_exhausted() {
        let events = vec![
            SessionEvent::StreamDelta {
                delta: "Hel".to_owned(),
            },
            SessionEvent::StreamDelta {
                delta: "lo".to_owned(),
            },
            SessionEvent::MessageRecorded {
                message: Message::new(Role::Assistant, "Hello"),
            },
            SessionEvent::MessageRecorded {
                message: Message::new(Role::Assistant, "Second turn"),
            },
        ];
        let mut replay = ReplayProvider::from_events(&events);

        let first = replay
            .stream_turn("sys", &[], &[])
            .expect("first turn available");
        assert_eq!(
            first,
            vec![
                ProviderStreamEvent::TextDelta("Hel".to_owned()),
                ProviderStreamEvent::TextDelta("lo".to_owned()),
                ProviderStreamEvent::Done,
            ]
        );

        let second = replay
            .stream_turn("sys", &[], &[])
            .expect("second turn available");
        assert_eq!(
            second,
            vec![
                ProviderStreamEvent::TextDelta("Second turn".to_owned()),
                ProviderStreamEvent::Done,
            ]
        );

        let err = replay
            .stream_turn("sys", &[], &[])
            .expect_err("no more turns");
        assert!(matches!(err, CoreError::ReplayExhausted { seq: 2 }));
    }

    #[test]
    fn empty_event_log_immediately_exhausts_replay() {
        let mut replay = ReplayProvider::from_events(&[]);
        let err = replay
            .stream_turn("sys", &[], &[])
            .expect_err("empty replay");
        assert!(matches!(err, CoreError::ReplayExhausted { seq: 0 }));
    }
}
