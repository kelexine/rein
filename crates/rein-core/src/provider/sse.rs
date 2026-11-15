// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Incremental Server-Sent Events (SSE) parser for Anthropic Messages streaming

use std::collections::BTreeMap;
use std::io::BufRead;

use serde_json::Value;

use crate::event::CoreError;
use crate::message::ToolCall;
use crate::provider::{ProviderStreamEvent, UsageStats};

#[derive(Debug, Clone)]
struct PendingToolBlock {
    id: String,
    name: String,
    json_buf: String,
}

/// Stateful parser decoding Anthropic SSE `data:` frames into [`ProviderStreamEvent`] items.
#[derive(Debug, Default)]
pub struct SseEventAccumulator {
    pending_tools: BTreeMap<usize, PendingToolBlock>,
    pending_input_tokens: u64,
}

impl SseEventAccumulator {
    /// Creates an empty [`SseEventAccumulator`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads an SSE byte stream to completion, emitting [`ProviderStreamEvent`] items live.
    pub fn read_stream<R: BufRead>(
        &mut self,
        reader: R,
        on_event: &mut dyn FnMut(&ProviderStreamEvent),
    ) -> Result<Vec<ProviderStreamEvent>, CoreError> {
        let mut emitted = Vec::new();
        for line in reader.lines() {
            let line = line.map_err(|err| CoreError::Provider(format!("SSE read error: {err}")))?;
            let trimmed = line.trim();
            if let Some(payload) = trimmed.strip_prefix("data:") {
                let data = payload.trim();
                if data.is_empty() || data == "[DONE]" {
                    continue;
                }
                for ev in self.feed_json(data)? {
                    on_event(&ev);
                    emitted.push(ev);
                }
            }
        }
        Ok(emitted)
    }

    /// Parses a single Anthropic SSE JSON payload into zero or more [`ProviderStreamEvent`]s.
    pub fn feed_json(&mut self, json_payload: &str) -> Result<Vec<ProviderStreamEvent>, CoreError> {
        let val: Value = serde_json::from_str(json_payload)?;
        let kind = val.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "message_start" => {
                if let Some(input) = val
                    .pointer("/message/usage/input_tokens")
                    .and_then(Value::as_u64)
                {
                    self.pending_input_tokens = input;
                }
                Ok(Vec::new())
            }
            "content_block_start" => {
                self.handle_block_start(&val);
                Ok(Vec::new())
            }
            "content_block_delta" => Ok(self.handle_block_delta(&val)),
            "content_block_stop" => self.handle_block_stop(&val),
            "message_delta" => {
                let Some(usage) = val.get("usage") else {
                    return Ok(Vec::new());
                };
                let input_tokens = usage
                    .get("input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(self.pending_input_tokens);
                let output_tokens = usage
                    .get("output_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                Ok(vec![ProviderStreamEvent::Usage(UsageStats {
                    input_tokens,
                    output_tokens,
                    total_tokens: input_tokens.saturating_add(output_tokens),
                })])
            }
            "message_stop" => Ok(vec![ProviderStreamEvent::Done]),
            "error" => {
                let msg = val
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown Anthropic stream error");
                Err(CoreError::Provider(msg.to_owned()))
            }
            _ => Ok(Vec::new()),
        }
    }

    fn handle_block_start(&mut self, val: &Value) {
        let Some(idx) = val
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
        else {
            return;
        };
        let Some(block) = val.get("content_block") else {
            return;
        };
        if block.get("type").and_then(Value::as_str) == Some("tool_use") {
            let id = block
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let name = block
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            self.pending_tools.insert(
                idx,
                PendingToolBlock {
                    id,
                    name,
                    json_buf: String::new(),
                },
            );
        }
    }

    fn handle_block_delta(&mut self, val: &Value) -> Vec<ProviderStreamEvent> {
        let Some(delta) = val.get("delta") else {
            return Vec::new();
        };
        match delta.get("type").and_then(Value::as_str) {
            Some("text_delta") => delta
                .get("text")
                .and_then(Value::as_str)
                .map(|t| vec![ProviderStreamEvent::TextDelta(t.to_owned())])
                .unwrap_or_default(),
            Some("input_json_delta") => {
                if let Some(idx) = val
                    .get("index")
                    .and_then(Value::as_u64)
                    .and_then(|n| usize::try_from(n).ok())
                    && let Some(partial) = delta.get("partial_json").and_then(Value::as_str)
                    && let Some(block) = self.pending_tools.get_mut(&idx)
                {
                    block.json_buf.push_str(partial);
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn handle_block_stop(&mut self, val: &Value) -> Result<Vec<ProviderStreamEvent>, CoreError> {
        let Some(idx) = val
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
        else {
            return Ok(Vec::new());
        };
        let Some(block) = self.pending_tools.remove(&idx) else {
            return Ok(Vec::new());
        };
        let arguments: Value = if block.json_buf.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&block.json_buf)?
        };
        Ok(vec![ProviderStreamEvent::ToolUse(ToolCall {
            id: block.id,
            name: block.name,
            arguments,
        })])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn decodes_text_deltas_and_partial_tool_use_sse_frames()
    -> Result<(), Box<dyn std::error::Error>> {
        let sse = concat!(
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Checking \"}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"read\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"Cargo.toml\\\"}\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let mut acc = SseEventAccumulator::new();
        let mut streamed_count = 0usize;
        let events = acc.read_stream(Cursor::new(sse.as_bytes()), &mut |_| streamed_count += 1)?;

        assert_eq!(streamed_count, 3);
        assert_eq!(
            events[0],
            ProviderStreamEvent::TextDelta("Checking ".to_owned())
        );
        assert_eq!(
            events[1],
            ProviderStreamEvent::ToolUse(ToolCall {
                id: "toolu_1".to_owned(),
                name: "read".to_owned(),
                arguments: serde_json::json!({"path": "Cargo.toml"}),
            })
        );
        assert_eq!(events[2], ProviderStreamEvent::Done);
        Ok(())
    }
}
