// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Antigravity request encoding, schema sanitization, usage parsing, and URL allowlist

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use crate::event::CoreError;
use crate::message::{Message, Role};
use crate::provider::UsageStats;

pub(super) fn build_generation_config(
    wire_model: &str,
    thinking_level: Option<&str>,
    max_tokens: u32,
) -> Value {
    if wire_model.starts_with("gpt-oss") {
        return json!({ "maxOutputTokens": max_tokens });
    }
    let mut thinking = Map::new();
    thinking.insert("includeThoughts".to_owned(), Value::Bool(true));
    if let Some(level) = thinking_level {
        thinking.insert("thinkingLevel".to_owned(), Value::String(level.to_owned()));
    }
    json!({
        "maxOutputTokens": max_tokens,
        "thinkingConfig": Value::Object(thinking),
    })
}

pub(super) fn encode_antigravity_contents(
    messages: &[Message],
    thought_signatures: &HashMap<String, String>,
) -> Vec<Value> {
    let mut contents = Vec::new();
    let mut pending_responses: Vec<Value> = Vec::new();

    for msg in messages {
        if msg.role == Role::Tool {
            let call = msg.tool_calls.first();
            let name = call.map_or("tool", |c| c.name.as_str());
            let mut fr = json!({
                "name": name,
                "response": { "result": msg.content },
            });
            if let Some(id) = call.map(|c| c.id.as_str()).filter(|id| !id.is_empty())
                && let Some(obj) = fr.as_object_mut()
            {
                obj.insert("id".to_owned(), Value::String(id.to_owned()));
            }
            pending_responses.push(json!({ "functionResponse": fr }));
            continue;
        }
        flush_function_responses(&mut pending_responses, &mut contents);
        strip_dangling_function_calls(&mut contents);

        match msg.role {
            Role::System | Role::Tool => {}
            Role::User => {
                let text = if msg.content.is_empty() {
                    " "
                } else {
                    msg.content.as_str()
                };
                push_user_text_turn(&mut contents, text);
            }
            Role::Assistant => {
                let parts = build_model_parts(msg, thought_signatures);
                push_model_turn(&mut contents, parts);
            }
        }
    }
    flush_function_responses(&mut pending_responses, &mut contents);
    strip_dangling_function_calls(&mut contents);
    contents
}

fn build_model_parts(msg: &Message, thought_signatures: &HashMap<String, String>) -> Vec<Value> {
    let mut parts = Vec::new();
    if !msg.content.is_empty() {
        parts.push(json!({ "text": msg.content }));
    }
    for call in &msg.tool_calls {
        let mut fc = json!({
            "name": call.name,
            "args": call.arguments,
        });
        if !call.id.is_empty()
            && let Some(obj) = fc.as_object_mut()
        {
            obj.insert("id".to_owned(), Value::String(call.id.clone()));
        }
        let mut part = json!({ "functionCall": fc });
        if let Some(sig) = thought_signatures.get(&call.id)
            && let Some(obj) = part.as_object_mut()
        {
            obj.insert("thoughtSignature".to_owned(), Value::String(sig.clone()));
        }
        parts.push(part);
    }
    parts
}

fn push_user_text_turn(contents: &mut Vec<Value>, text: &str) {
    let can_merge = contents.last().is_some_and(|turn| {
        turn.get("role").and_then(Value::as_str) == Some("user")
            && turn
                .get("parts")
                .and_then(Value::as_array)
                .is_some_and(|parts| !parts.iter().any(|p| p.get("functionResponse").is_some()))
    });
    if can_merge
        && let Some(parts) = contents
            .last_mut()
            .and_then(|turn| turn.get_mut("parts"))
            .and_then(Value::as_array_mut)
    {
        parts.push(json!({ "text": text }));
        return;
    }
    contents.push(json!({ "role": "user", "parts": [{ "text": text }] }));
}

fn push_model_turn(contents: &mut Vec<Value>, parts: Vec<Value>) {
    if parts.is_empty() || contents.is_empty() {
        return;
    }
    if contents
        .last()
        .is_some_and(|turn| turn.get("role").and_then(Value::as_str) == Some("model"))
        && let Some(existing) = contents
            .last_mut()
            .and_then(|turn| turn.get_mut("parts"))
            .and_then(Value::as_array_mut)
    {
        existing.extend(parts);
        return;
    }
    contents.push(json!({ "role": "model", "parts": parts }));
}

fn strip_dangling_function_calls(contents: &mut Vec<Value>) {
    let is_dangling_model = contents.last().is_some_and(|turn| {
        turn.get("role").and_then(Value::as_str) == Some("model")
            && turn
                .get("parts")
                .and_then(Value::as_array)
                .is_some_and(|parts| parts.iter().any(|p| p.get("functionCall").is_some()))
    });
    if !is_dangling_model {
        return;
    }
    let becomes_empty = {
        let Some(parts) = contents
            .last_mut()
            .and_then(|turn| turn.get_mut("parts"))
            .and_then(Value::as_array_mut)
        else {
            return;
        };
        parts.retain(|p| p.get("functionCall").is_none());
        parts.is_empty()
    };
    if becomes_empty {
        let _ = contents.pop();
    }
}

fn flush_function_responses(pending: &mut Vec<Value>, contents: &mut Vec<Value>) {
    if pending.is_empty() {
        return;
    }
    let has_preceding_call = contents.last().is_some_and(|turn| {
        turn.get("role").and_then(Value::as_str) == Some("model")
            && turn
                .get("parts")
                .and_then(Value::as_array)
                .is_some_and(|parts| parts.iter().any(|p| p.get("functionCall").is_some()))
    });
    let batch = std::mem::take(pending);
    if has_preceding_call {
        contents.push(json!({
            "role": "user",
            "parts": batch,
        }));
    }
}

pub(super) fn extract_usage_metadata(root: &Value) -> Option<UsageStats> {
    let meta = root.get("usageMetadata")?;
    let input_tokens = meta
        .get("promptTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = meta
        .get("candidatesTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total_tokens = meta
        .get("totalTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| input_tokens.saturating_add(output_tokens));
    if input_tokens == 0 && output_tokens == 0 && total_tokens == 0 {
        return None;
    }
    Some(UsageStats {
        input_tokens,
        output_tokens,
        total_tokens,
    })
}

pub(super) fn sanitize_gemini_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => {
            let mut clean = Map::new();
            for (k, v) in map {
                if matches!(
                    k.as_str(),
                    "$schema" | "$id" | "$defs" | "additionalProperties" | "default"
                ) {
                    continue;
                }
                clean.insert(k.clone(), sanitize_gemini_schema(v));
            }
            Value::Object(clean)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(sanitize_gemini_schema).collect()),
        other => other.clone(),
    }
}

pub(super) fn validate_antigravity_url(url: &str) -> Result<(), CoreError> {
    let rest = url.strip_prefix("https://").ok_or_else(|| {
        CoreError::Provider(format!("refusing non-HTTPS Antigravity request URL: {url}"))
    })?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return Err(CoreError::Provider(format!(
            "refusing Antigravity URL with invalid authority: {url}"
        )));
    }
    let host_raw = if let Some((h, port)) = authority.rsplit_once(':') {
        if port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()) {
            return Err(CoreError::Provider(format!(
                "refusing Antigravity URL with invalid port: {url}"
            )));
        }
        h
    } else {
        authority
    };
    let host = host_raw.to_ascii_lowercase();
    let allowed = host == "accounts.google.com"
        || host == "oauth2.googleapis.com"
        || (host.ends_with(".googleapis.com") && host.len() > ".googleapis.com".len());
    if !allowed {
        return Err(CoreError::Provider(format!(
            "refusing Antigravity request to untrusted host '{host}'"
        )));
    }
    Ok(())
}
