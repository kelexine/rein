// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Anthropic Messages API streaming provider configuration and HTTP/SSE backend

use std::io::BufReader;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::event::CoreError;
use crate::message::{Message, Role};
use crate::provider::sse::SseEventAccumulator;
use crate::provider::{Provider, ProviderStreamEvent, ToolSpec};

/// Default Anthropic Messages API endpoint.
pub const DEFAULT_ANTHROPIC_ENDPOINT: &str = "https://api.anthropic.com/v1/messages";

/// Default Claude model identifier for v1 cloud inference.
pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-sonnet-4-20250514";

/// Required Anthropic API version header value.
pub const ANTHROPIC_API_VERSION: &str = "2023-06-01";

/// Default maximum output tokens per turn.
pub const DEFAULT_MAX_TOKENS: u32 = 8192;

/// Configuration for the Anthropic Messages streaming backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnthropicConfig {
    /// API endpoint URL.
    pub endpoint: String,
    /// Model name.
    pub model: String,
    /// Maximum output tokens.
    pub max_tokens: u32,
    /// Name of the environment variable holding the API key (never stored in plaintext config).
    pub api_key_env: String,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_ANTHROPIC_ENDPOINT.to_owned(),
            model: DEFAULT_ANTHROPIC_MODEL.to_owned(),
            max_tokens: DEFAULT_MAX_TOKENS,
            api_key_env: "ANTHROPIC_API_KEY".to_owned(),
        }
    }
}

/// Cloud provider backed by the Anthropic Messages SSE API via `ureq`.
#[derive(Debug, Clone)]
pub struct AnthropicProvider {
    config: AnthropicConfig,
    agent: ureq::Agent,
}

impl AnthropicProvider {
    /// Creates an [`AnthropicProvider`] with the given configuration.
    #[must_use]
    pub fn new(config: AnthropicConfig) -> Self {
        Self {
            config,
            agent: ureq::Agent::new(),
        }
    }

    /// Returns the provider configuration.
    #[must_use]
    pub fn config(&self) -> &AnthropicConfig {
        &self.config
    }

    /// Builds the Anthropic Messages API JSON request body.
    #[must_use]
    pub fn build_request_body(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Value {
        let api_messages: Vec<Value> = messages.iter().filter_map(to_anthropic_message).collect();
        let api_tools: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.input_schema,
                })
            })
            .collect();

        let mut body = json!({
            "model": self.config.model,
            "max_tokens": self.config.max_tokens,
            "system": system_prompt,
            "messages": api_messages,
            "stream": true,
        });
        if !api_tools.is_empty()
            && let Some(obj) = body.as_object_mut()
        {
            obj.insert("tools".to_owned(), Value::Array(api_tools));
        }
        body
    }
}

impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn stream_turn_with(
        &mut self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolSpec],
        on_event: &mut dyn FnMut(&ProviderStreamEvent),
    ) -> Result<Vec<ProviderStreamEvent>, CoreError> {
        let api_key = std::env::var(&self.config.api_key_env).map_err(|_| {
            CoreError::Provider(format!(
                "missing environment variable '{}'",
                self.config.api_key_env
            ))
        })?;
        let body = self.build_request_body(system_prompt, messages, tools);
        let payload = serde_json::to_string(&body)?;
        let response = self
            .agent
            .post(&self.config.endpoint)
            .set("x-api-key", &api_key)
            .set("anthropic-version", ANTHROPIC_API_VERSION)
            .set("content-type", "application/json")
            .send_string(&payload)
            .map_err(|err| CoreError::Provider(format!("Anthropic HTTP request failed: {err}")))?;

        let reader = BufReader::new(response.into_reader());
        let mut acc = SseEventAccumulator::new();
        acc.read_stream(reader, on_event)
    }
}

fn to_anthropic_message(msg: &Message) -> Option<Value> {
    match msg.role {
        Role::System => None,
        Role::User => Some(json!({
            "role": "user",
            "content": msg.content,
        })),
        Role::Assistant => {
            let mut blocks = Vec::new();
            if !msg.content.is_empty() {
                blocks.push(json!({ "type": "text", "text": msg.content }));
            }
            for call in &msg.tool_calls {
                blocks.push(json!({
                    "type": "tool_use",
                    "id": call.id,
                    "name": call.name,
                    "input": call.arguments,
                }));
            }
            Some(json!({ "role": "assistant", "content": blocks }))
        }
        Role::Tool => {
            let tool_use_id = msg.tool_calls.first().map_or("unknown", |c| c.id.as_str());
            Some(json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": tool_use_id,
                    "content": msg.content,
                }]
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ToolCall;

    #[test]
    fn builds_valid_anthropic_payload_with_tool_calls() {
        let provider = AnthropicProvider::new(AnthropicConfig::default());
        let mut assistant = Message::new(Role::Assistant, "Reading Cargo.toml");
        assistant.tool_calls.push(ToolCall {
            id: "call_1".to_owned(),
            name: "read".to_owned(),
            arguments: json!({"path": "Cargo.toml"}),
        });
        let history = vec![Message::new(Role::User, "check Cargo.toml"), assistant];
        let tools = vec![ToolSpec {
            name: "read".to_owned(),
            description: "Read file".to_owned(),
            input_schema: json!({"type": "object"}),
        }];

        let body = provider.build_request_body("system prompt", &history, &tools);
        assert_eq!(body["stream"], true);
        assert_eq!(body["tools"][0]["name"], "read");
        assert_eq!(body["messages"][1]["content"][1]["type"], "tool_use");
    }
}
