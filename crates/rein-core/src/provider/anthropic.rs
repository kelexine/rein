// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Anthropic Messages API streaming provider configuration and HTTP/SSE backend

use std::io::BufReader;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::event::CoreError;
use crate::message::{Message, Role};
use crate::provider::sse::SseEventAccumulator;
use crate::provider::{AvailableModel, Provider, ProviderStreamEvent, ToolSpec};

/// Default Anthropic Messages API endpoint.
pub const DEFAULT_ANTHROPIC_ENDPOINT: &str = "https://api.anthropic.com/v1/messages";

/// Default Claude model identifier for v1 cloud inference.
pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-sonnet-5-5";

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
            endpoint: std::env::var("ANTHROPIC_BASE_URL")
                .unwrap_or_else(|_| DEFAULT_ANTHROPIC_ENDPOINT.to_owned()),
            model: std::env::var("ANTHROPIC_MODEL")
                .unwrap_or_else(|_| DEFAULT_ANTHROPIC_MODEL.to_owned()),
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
        let cfg = ureq::config::Config::builder()
            .http_status_as_error(false)
            .build();
        Self {
            config,
            agent: ureq::Agent::new_with_config(cfg),
        }
    }

    /// Returns the provider configuration.
    #[must_use]
    pub fn config(&self) -> &AnthropicConfig {
        &self.config
    }

    /// Builds the Anthropic Messages API JSON request body, batching consecutive tool results.
    #[must_use]
    pub fn build_request_body(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Value {
        let api_messages = encode_anthropic_messages(messages);
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
            "messages": api_messages,
            "stream": true,
        });
        if let Some(obj) = body.as_object_mut() {
            if !system_prompt.trim().is_empty() {
                obj.insert("system".to_owned(), Value::String(system_prompt.to_owned()));
            }
            if !api_tools.is_empty() {
                obj.insert("tools".to_owned(), Value::Array(api_tools));
            }
        }
        body
    }
}

impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn list_models(&mut self) -> Result<Vec<AvailableModel>, CoreError> {
        let api_key = std::env::var(&self.config.api_key_env).map_err(|_| {
            CoreError::Provider(format!(
                "missing environment variable '{}'",
                self.config.api_key_env
            ))
        })?;
        let url = models_endpoint_for(&self.config.endpoint);
        let resp = self
            .agent
            .get(&url)
            .header("x-api-key", &api_key)
            .header("anthropic-version", ANTHROPIC_API_VERSION)
            .call()
            .map_err(format_ureq_error)
            .and_then(ensure_ok_response)?;
        let body = resp
            .into_body()
            .read_to_string()
            .map_err(|err| CoreError::Provider(format!("Anthropic models read error: {err}")))?;
        let val: Value = serde_json::from_str(&body)?;
        Ok(parse_anthropic_models(&val))
    }

    fn stream_turn_with(
        &mut self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolSpec],
        on_event: &mut dyn FnMut(&ProviderStreamEvent),
    ) -> Result<Vec<ProviderStreamEvent>, CoreError> {
        let never = std::sync::atomic::AtomicBool::new(false);
        self.stream_turn_cancellable(system_prompt, messages, tools, &never, on_event)
    }

    fn stream_turn_cancellable(
        &mut self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolSpec],
        cancel: &std::sync::atomic::AtomicBool,
        on_event: &mut dyn FnMut(&ProviderStreamEvent),
    ) -> Result<Vec<ProviderStreamEvent>, CoreError> {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(CoreError::Cancelled);
        }
        let api_key = std::env::var(&self.config.api_key_env).map_err(|_| {
            CoreError::Provider(format!(
                "missing environment variable '{}'",
                self.config.api_key_env
            ))
        })?;
        let body = self.build_request_body(system_prompt, messages, tools);
        let payload = serde_json::to_string(&body)?;
        let agent = self.agent.clone();
        let endpoint = self.config.endpoint.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let res = agent
                .post(&endpoint)
                .header("x-api-key", &api_key)
                .header("anthropic-version", ANTHROPIC_API_VERSION)
                .header("content-type", "application/json")
                .send(&payload)
                .map_err(format_ureq_error)
                .and_then(ensure_ok_response);
            let _ = tx.send(res);
        });
        let response = loop {
            if cancel.load(std::sync::atomic::Ordering::Acquire) {
                return Err(CoreError::Cancelled);
            }
            match rx.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(res) => break res?,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(CoreError::Provider(
                        "Anthropic HTTP request worker terminated unexpectedly".to_owned(),
                    ));
                }
            }
        };

        let reader = BufReader::new(response.into_body().into_reader());
        let mut acc = SseEventAccumulator::new();
        acc.read_stream_cancellable_owned(reader, cancel, on_event)
    }
}

fn models_endpoint_for(messages_endpoint: &str) -> String {
    if let Some(prefix) = messages_endpoint.strip_suffix("/v1/messages") {
        format!("{}/v1/models", prefix.trim_end_matches('/'))
    } else {
        "https://api.anthropic.com/v1/models".to_owned()
    }
}

fn parse_anthropic_models(val: &Value) -> Vec<AvailableModel> {
    let Some(data) = val.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    data.iter()
        .filter_map(|item| {
            let id = item.get("id").and_then(Value::as_str)?.trim();
            if id.is_empty() {
                return None;
            }
            let display_name = item
                .get("display_name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(id)
                .to_owned();
            Some(AvailableModel {
                id: id.to_owned(),
                display_name,
                quota_percent: None,
                recommended: id.contains("sonnet") || id.contains("opus"),
            })
        })
        .collect()
}

fn ensure_ok_response(
    resp: ureq::http::Response<ureq::Body>,
) -> Result<ureq::http::Response<ureq::Body>, CoreError> {
    let code = resp.status().as_u16();
    if (200..300).contains(&code) {
        return Ok(resp);
    }
    let body = resp.into_body().read_to_string().unwrap_or_default();
    let detail = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| body.trim().to_owned());
    if detail.is_empty() {
        Err(CoreError::Provider(format!("Anthropic HTTP {code}")))
    } else {
        Err(CoreError::Provider(format!(
            "Anthropic HTTP {code}: {detail}"
        )))
    }
}

fn format_ureq_error(err: ureq::Error) -> CoreError {
    CoreError::Provider(format!("Anthropic transport error: {err}"))
}

fn encode_anthropic_messages(messages: &[Message]) -> Vec<Value> {
    let mut encoded = Vec::new();
    let mut pending_tool_results: Vec<Value> = Vec::new();

    for msg in messages {
        if msg.role == Role::Tool {
            let tool_use_id = msg.tool_calls.first().map_or("unknown", |c| c.id.as_str());
            pending_tool_results.push(json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": msg.content,
            }));
            continue;
        }
        flush_tool_results(&mut pending_tool_results, &mut encoded);

        match msg.role {
            Role::System | Role::Tool => {}
            Role::User => encoded.push(json!({
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
                if !blocks.is_empty() {
                    encoded.push(json!({ "role": "assistant", "content": blocks }));
                }
            }
        }
    }
    flush_tool_results(&mut pending_tool_results, &mut encoded);
    encoded
}

fn flush_tool_results(pending: &mut Vec<Value>, encoded: &mut Vec<Value>) {
    if pending.is_empty() {
        return;
    }
    let has_preceding_tool_use = encoded.last().is_some_and(|turn| {
        turn.get("role").and_then(Value::as_str) == Some("assistant")
            && turn
                .get("content")
                .and_then(Value::as_array)
                .is_some_and(|blocks| {
                    blocks
                        .iter()
                        .any(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
                })
    });
    let batch = std::mem::take(pending);
    if has_preceding_tool_use {
        encoded.push(json!({
            "role": "user",
            "content": batch,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ToolCall;

    #[test]
    fn builds_valid_anthropic_payload_with_batched_tool_results() {
        let provider = AnthropicProvider::new(AnthropicConfig::default());
        let mut assistant = Message::new(Role::Assistant, "Reading files");
        assistant.tool_calls.push(ToolCall {
            id: "call_1".to_owned(),
            name: "read".to_owned(),
            arguments: json!({"path": "Cargo.toml"}),
        });
        assistant.tool_calls.push(ToolCall {
            id: "call_2".to_owned(),
            name: "glob".to_owned(),
            arguments: json!({"pattern": "*.rs"}),
        });
        let mut r1 = Message::new(Role::Tool, "toml");
        r1.tool_calls.push(assistant.tool_calls[0].clone());
        let mut r2 = Message::new(Role::Tool, "main.rs");
        r2.tool_calls.push(assistant.tool_calls[1].clone());

        let history = vec![Message::new(Role::User, "check repo"), assistant, r1, r2];
        let tools = vec![ToolSpec {
            name: "read".to_owned(),
            description: "Read file".to_owned(),
            input_schema: json!({"type": "object"}),
        }];

        let body = provider.build_request_body("system prompt", &history, &tools);
        assert_eq!(body["stream"], true);
        assert_eq!(body["tools"][0]["name"], "read");
        assert_eq!(body["messages"][1]["content"][1]["type"], "tool_use");
        assert_eq!(body["messages"][2]["role"], "user");
        assert_eq!(
            body["messages"][2]["content"].as_array().map(Vec::len),
            Some(2)
        );
    }

    #[test]
    fn drops_orphaned_tool_results_without_preceding_tool_use() {
        let provider = AnthropicProvider::new(AnthropicConfig::default());
        let mut orphan = Message::new(Role::Tool, "stale output");
        orphan.tool_calls.push(ToolCall {
            id: "call_0".to_owned(),
            name: "bash".to_owned(),
            arguments: json!({}),
        });
        let history = vec![orphan, Message::new(Role::User, "hello")];
        let body = provider.build_request_body("sys", &history, &[]);
        let msgs = body["messages"].as_array();
        assert_eq!(msgs.map(Vec::len), Some(1));
        assert_eq!(body["messages"][0]["content"], "hello");
    }

    #[test]
    fn includes_model_system_prompt_and_skips_system_role_in_history() {
        let cfg = AnthropicConfig {
            model: "claude-test".to_owned(),
            max_tokens: 1024,
            ..AnthropicConfig::default()
        };
        let provider = AnthropicProvider::new(cfg);
        assert_eq!(provider.name(), "anthropic");

        let history = vec![
            Message::new(Role::System, "ignored inline system"),
            Message::new(Role::User, "hi"),
        ];
        let body = provider.build_request_body("top system", &history, &[]);
        assert_eq!(body["model"], "claude-test");
        assert_eq!(body["max_tokens"], 1024);
        assert_eq!(body["system"], "top system");
        assert_eq!(body["messages"].as_array().map(Vec::len), Some(1));
        assert_eq!(body["messages"][0]["role"], "user");
    }
}
