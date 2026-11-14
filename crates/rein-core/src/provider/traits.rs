// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Provider trait, tool spec envelope, and streaming event definitions

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::event::CoreError;
use crate::message::{Message, ToolCall};

/// Provider-facing tool schema descriptor (keeps `rein-core` decoupled from `rein-tools`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Registered tool name.
    pub name: String,
    /// Human-readable description of what the tool does.
    pub description: String,
    /// JSON Schema object describing input parameters.
    pub input_schema: Value,
}

/// Incremental event yielded by a [`Provider`] during a turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderStreamEvent {
    /// Text token delta.
    TextDelta(String),
    /// Structured tool invocation emitted by the model.
    ToolUse(ToolCall),
    /// End of model turn stream.
    Done,
}

/// Clean model provider abstraction supporting cloud streaming now and local backends later.
pub trait Provider: Send {
    /// Human-readable provider identifier.
    #[must_use]
    fn name(&self) -> &str;

    /// Streams response events into `on_event` for the given prompt, transcript, and active tools.
    fn stream_turn_with(
        &mut self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolSpec],
        on_event: &mut dyn FnMut(&ProviderStreamEvent),
    ) -> Result<Vec<ProviderStreamEvent>, CoreError>;

    /// Convenience helper collecting all streamed events for a turn.
    fn stream_turn(
        &mut self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Result<Vec<ProviderStreamEvent>, CoreError> {
        let mut noop = |_: &ProviderStreamEvent| {};
        self.stream_turn_with(system_prompt, messages, tools, &mut noop)
    }
}
