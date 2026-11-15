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

/// Token consumption reported by a provider turn or chunk.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageStats {
    /// Prompt/input tokens consumed.
    pub input_tokens: u64,
    /// Completion/output tokens generated.
    pub output_tokens: u64,
    /// Combined token count (input + output or provider-reported total).
    pub total_tokens: u64,
}

impl UsageStats {
    /// Accumulates another turn's usage cleanly.
    pub fn add_assign_turn(&mut self, other: Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        let combined = other
            .total_tokens
            .max(other.input_tokens.saturating_add(other.output_tokens));
        self.total_tokens = self.total_tokens.saturating_add(combined);
    }
}

/// Server-reported model metadata and optional remaining quota percentage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableModel {
    /// Canonical model ID passed back in requests.
    pub id: String,
    /// Human-readable display label.
    pub display_name: String,
    /// Remaining account quota percentage (`0..=100`) when reported by the backend.
    pub quota_percent: Option<u8>,
    /// Whether the backend marks this model as recommended for agentic coding.
    pub recommended: bool,
}

/// Incremental event yielded by a [`Provider`] during a turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderStreamEvent {
    /// Model reasoning / thought trace token delta.
    ThinkingDelta(String),
    /// Text token delta.
    TextDelta(String),
    /// Structured tool invocation emitted by the model.
    ToolUse(ToolCall),
    /// Token usage snapshot reported by the provider.
    Usage(UsageStats),
    /// End of model turn stream.
    Done,
}

/// Clean model provider abstraction supporting cloud streaming now and local backends later.
pub trait Provider: Send {
    /// Human-readable provider identifier.
    #[must_use]
    fn name(&self) -> &str;

    /// Fetches available models (and quota info where supported) dynamically from the provider backend.
    fn list_models(&mut self) -> Result<Vec<AvailableModel>, CoreError> {
        Ok(Vec::new())
    }

    /// Returns cached remaining quota percentage (`0..=100`) for the active model if known.
    #[must_use]
    fn active_model_quota(&self) -> Option<u8> {
        None
    }

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
