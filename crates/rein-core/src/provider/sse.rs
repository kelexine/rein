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
        let never = std::sync::atomic::AtomicBool::new(false);
        self.read_stream_cancellable(reader, &never, on_event)
    }

    /// Reads an SSE byte stream to completion or until `cancel` is set, emitting events live.
    pub fn read_stream_cancellable<R: BufRead>(
        &mut self,
        reader: R,
        cancel: &std::sync::atomic::AtomicBool,
        on_event: &mut dyn FnMut(&ProviderStreamEvent),
    ) -> Result<Vec<ProviderStreamEvent>, CoreError> {
        let mut emitted = Vec::new();
        for line in reader.lines() {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(CoreError::Cancelled);
            }
            let line = line.map_err(|err| CoreError::Provider(format!("SSE read error: {err}")))?;
            self.process_sse_line(&line, on_event, &mut emitted)?;
        }
        Ok(emitted)
    }

    /// Reads an owned SSE stream with non-blocking cancellation polling (wakes every 50 ms).
    pub fn read_stream_cancellable_owned<R: BufRead + Send + 'static>(
        &mut self,
        reader: R,
        cancel: &std::sync::atomic::AtomicBool,
        on_event: &mut dyn FnMut(&ProviderStreamEvent),
    ) -> Result<Vec<ProviderStreamEvent>, CoreError> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in reader.lines() {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut emitted = Vec::new();
        loop {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(CoreError::Cancelled);
            }
            match rx.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(line_res) => {
                    let line = line_res
                        .map_err(|err| CoreError::Provider(format!("SSE read error: {err}")))?;
                    self.process_sse_line(&line, on_event, &mut emitted)?;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        Ok(emitted)
    }

    fn process_sse_line(
        &mut self,
        line: &str,
        on_event: &mut dyn FnMut(&ProviderStreamEvent),
        emitted: &mut Vec<ProviderStreamEvent>,
    ) -> Result<(), CoreError> {
        let trimmed = line.trim();
        if let Some(payload) = trimmed.strip_prefix("data:") {
            let data = payload.trim();
            if data.is_empty() || data == "[DONE]" {
                return Ok(());
            }
            for ev in self.feed_json(data)? {
                on_event(&ev);
                emitted.push(ev);
            }
        }
        Ok(())
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
            Some("thinking_delta") => delta
                .get("thinking")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
                .map(|t| vec![ProviderStreamEvent::ThinkingDelta(t.to_owned())])
                .unwrap_or_default(),
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

    #[test]
    fn handles_crlf_frames_empty_tool_args_and_rejects_bad_tool_json()
    -> Result<(), Box<dyn std::error::Error>> {
        let sse = concat!(
            "event: content_block_start\r\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"todo_read\"}}\r\n\r\n",
            "event: content_block_stop\r\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\r\n\r\n",
            "event: ping\r\n",
            "data: {\"type\":\"ping\"}\r\n\r\n",
            "event: message_stop\r\n",
            "data: {\"type\":\"message_stop\"}\r\n\r\n",
        );
        let mut acc = SseEventAccumulator::new();
        let events = acc.read_stream(Cursor::new(sse.as_bytes()), &mut |_| {})?;
        assert_eq!(
            events,
            vec![
                ProviderStreamEvent::ToolUse(ToolCall {
                    id: "t1".to_owned(),
                    name: "todo_read".to_owned(),
                    arguments: serde_json::json!({}),
                }),
                ProviderStreamEvent::Done,
            ]
        );

        let bad_sse = concat!(
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t2\",\"name\":\"read\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{not-json\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        );
        let mut bad_acc = SseEventAccumulator::new();
        assert!(
            bad_acc
                .read_stream(Cursor::new(bad_sse.as_bytes()), &mut |_| {})
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn read_stream_cancellable_owned_unblocks_promptly_when_cancelled()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::io::{BufReader, Read};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct BlockingReader(std::sync::mpsc::Receiver<()>);
        impl Read for BlockingReader {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                let _ = self.0.recv();
                Ok(0)
            }
        }

        let (_tx, rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_bg = Arc::clone(&cancel);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            cancel_bg.store(true, Ordering::Relaxed);
        });

        let mut acc = SseEventAccumulator::new();
        let started = std::time::Instant::now();
        let res = acc.read_stream_cancellable_owned(
            BufReader::new(BlockingReader(rx)),
            &cancel,
            &mut |_| {},
        );
        assert!(matches!(res, Err(CoreError::Cancelled)));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        Ok(())
    }
}
