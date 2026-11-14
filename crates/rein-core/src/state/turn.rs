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
