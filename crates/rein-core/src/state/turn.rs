// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Compile-time typestate Turn machine implementation

use crate::event::{CoreError, EventLog, SessionEvent};
use crate::message::{Message, Role, ToolCall, ToolOutput};
use crate::state::markers::{AwaitingPolicy, Executing, Idle, Streaming};

/// Compile-time typestate turn ensuring no tool executes without a policy check.
#[derive(Debug)]
pub struct Turn<'a, S> {
    log: &'a EventLog,
    state: S,
}

impl<'a> Turn<'a, Idle> {
    /// Creates a new idle turn bound to the session [`EventLog`].
    #[must_use]
    pub fn new(log: &'a EventLog) -> Self {
        Self { log, state: Idle }
    }

    /// Records the user prompt and transitions to [`Streaming`].
    pub fn begin_user_turn(
        self,
        prompt: impl Into<String>,
    ) -> Result<Turn<'a, Streaming>, CoreError> {
        let message = Message::new(Role::User, prompt);
        self.log
            .append(&SessionEvent::MessageRecorded { message })?;
        Ok(Turn {
            log: self.log,
            state: Streaming {
                accumulated_text: String::new(),
            },
        })
    }

    /// Transitions directly from [`Idle`] to [`AwaitingPolicy`] for follow-up calls in a multi-tool batch.
    pub fn request_tool(self, call: ToolCall) -> Result<Turn<'a, AwaitingPolicy>, CoreError> {
        self.log
            .append(&SessionEvent::ToolRequested { call: call.clone() })?;
        Ok(Turn {
            log: self.log,
            state: AwaitingPolicy { pending_call: call },
        })
    }
}

impl<'a> Turn<'a, Streaming> {
    /// Appends a streamed text chunk to the event log and accumulator.
    pub fn push_delta(&mut self, delta: &str) -> Result<(), CoreError> {
        self.state.accumulated_text.push_str(delta);
        self.log.append(&SessionEvent::StreamDelta {
            delta: delta.to_owned(),
        })
    }

    /// Completes the assistant turn when no tool call is requested.
    pub fn finish_response(self) -> Result<(Turn<'a, Idle>, Message), CoreError> {
        let message = Message::new(Role::Assistant, self.state.accumulated_text);
        self.log.append(&SessionEvent::MessageRecorded {
            message: message.clone(),
        })?;
        Ok((Turn::new(self.log), message))
    }

    /// Transitions to [`AwaitingPolicy`] when the model emits a [`ToolCall`].
    pub fn request_tool(self, call: ToolCall) -> Result<Turn<'a, AwaitingPolicy>, CoreError> {
        self.log
            .append(&SessionEvent::ToolRequested { call: call.clone() })?;
        Ok(Turn {
            log: self.log,
            state: AwaitingPolicy { pending_call: call },
        })
    }
}

impl<'a> Turn<'a, AwaitingPolicy> {
    /// Returns a reference to the tool call awaiting policy verification.
    #[must_use]
    pub fn pending_call(&self) -> &ToolCall {
        &self.state.pending_call
    }

    /// Approves the tool call and transitions to [`Executing`].
    pub fn approve(self, reason: impl Into<String>) -> Result<Turn<'a, Executing>, CoreError> {
        self.log.append(&SessionEvent::PolicyEvaluated {
            call_id: self.state.pending_call.id.clone(),
            allowed: true,
            reason: reason.into(),
        })?;
        Ok(Turn {
            log: self.log,
            state: Executing {
                approved_call: self.state.pending_call,
            },
        })
    }

    /// Denies the tool call, records a synthetic error output, and returns to [`Idle`].
    pub fn deny(
        self,
        reason: impl Into<String>,
    ) -> Result<(Turn<'a, Idle>, ToolOutput), CoreError> {
        let reason_str = reason.into();
        self.log.append(&SessionEvent::PolicyEvaluated {
            call_id: self.state.pending_call.id.clone(),
            allowed: false,
            reason: reason_str.clone(),
        })?;
        let output = ToolOutput {
            call_id: self.state.pending_call.id,
            content: format!("policy denied tool execution: {reason_str}"),
            is_error: true,
        };
        self.log.append(&SessionEvent::ToolCompleted {
            output: output.clone(),
        })?;
        Ok((Turn::new(self.log), output))
    }
}

impl<'a> Turn<'a, Executing> {
    /// Returns the policy-approved [`ToolCall`] ready for execution.
    #[must_use]
    pub fn approved_call(&self) -> &ToolCall {
        &self.state.approved_call
    }

    /// Records the completed [`ToolOutput`] and returns the turn to [`Idle`].
    pub fn complete(self, output: ToolOutput) -> Result<Turn<'a, Idle>, CoreError> {
        self.log.append(&SessionEvent::ToolCompleted { output })?;
        Ok(Turn::new(self.log))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::message::Role;

    #[test]
    fn turn_text_flow_records_user_deltas_and_assistant_message()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("rein_core_turn_text_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let log_path = dir.join("session.jsonl");
        let log = EventLog::new(&log_path);

        let mut streaming = Turn::new(&log).begin_user_turn("hello rein")?;
        streaming.push_delta("hi ")?;
        streaming.push_delta("there")?;
        let (_idle, reply) = streaming.finish_response()?;

        assert_eq!(reply.role, Role::Assistant);
        assert_eq!(reply.content, "hi there");

        let events = log.read_all()?;
        assert_eq!(events.len(), 4);
        assert!(matches!(
            &events[0],
            SessionEvent::MessageRecorded { message }
                if message.role == Role::User && message.content == "hello rein"
        ));
        assert!(matches!(
            &events[1],
            SessionEvent::StreamDelta { delta } if delta == "hi "
        ));
        assert!(matches!(
            &events[2],
            SessionEvent::StreamDelta { delta } if delta == "there"
        ));
        assert!(matches!(
            &events[3],
            SessionEvent::MessageRecorded { message }
                if message.role == Role::Assistant && message.content == "hi there"
        ));

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn turn_tool_approve_and_deny_paths_record_policy_and_output()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("rein_core_turn_tool_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let log_path = dir.join("session.jsonl");
        let log = EventLog::new(&log_path);

        let streaming = Turn::new(&log).begin_user_turn("run check")?;
        let call_allow = ToolCall {
            id: "call_1".to_owned(),
            name: "read".to_owned(),
            arguments: json!({"path": "Cargo.toml"}),
        };
        let awaiting = streaming.request_tool(call_allow)?;
        assert_eq!(awaiting.pending_call().id, "call_1");

        let executing = awaiting.approve("read-only workspace file")?;
        assert_eq!(executing.approved_call().name, "read");
        let idle = executing.complete(ToolOutput {
            call_id: "call_1".to_owned(),
            content: "ok".to_owned(),
            is_error: false,
        })?;

        let call_deny = ToolCall {
            id: "call_2".to_owned(),
            name: "bash".to_owned(),
            arguments: json!({"command": "rm -rf /"}),
        };
        let (_idle2, denied_out) = idle
            .request_tool(call_deny)?
            .deny("protected mutation blocked")?;
        assert!(denied_out.is_error);
        assert_eq!(denied_out.call_id, "call_2");
        assert!(denied_out.content.contains("protected mutation blocked"));

        let events = log.read_all()?;
        assert!(events.iter().any(|ev| matches!(
            ev,
            SessionEvent::PolicyEvaluated { call_id, allowed: true, .. } if call_id == "call_1"
        )));
        assert!(events.iter().any(|ev| matches!(
            ev,
            SessionEvent::PolicyEvaluated { call_id, allowed: false, .. } if call_id == "call_2"
        )));
        assert!(events.iter().any(|ev| matches!(
            ev,
            SessionEvent::ToolCompleted { output } if output.call_id == "call_2" && output.is_error
        )));

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
