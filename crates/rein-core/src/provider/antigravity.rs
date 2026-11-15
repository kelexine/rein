// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-15
// Purpose: Google Antigravity Cloud Code PA streaming provider with tool calling and auto-refresh

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::event::CoreError;
use crate::message::{Message, Role, ToolCall};
use crate::provider::antigravity_auth::{
    AntigravityTokenSet, refresh_access_token, resolve_token, save_token_file,
};
use crate::provider::{AvailableModel, Provider, ProviderStreamEvent, ToolSpec, UsageStats};

/// Primary Cloud Code PA endpoint used by Antigravity.
pub const DEFAULT_ANTIGRAVITY_BASE_URL: &str = "https://daily-cloudcode-pa.googleapis.com";

/// Secondary Cloud Code PA endpoint.
pub const FALLBACK_ANTIGRAVITY_BASE_URL: &str = "https://cloudcode-pa.googleapis.com";

/// Default Antigravity model supporting coding & tool calling.
pub const DEFAULT_ANTIGRAVITY_MODEL: &str = "claude-sonnet-5-5-medium";

/// Default consumer companion project ID returned by `loadCodeAssist`.
pub const DEFAULT_ANTIGRAVITY_PROJECT: &str = "aicode-consumers";

const ANTIGRAVITY_USER_AGENT: &str =
    "antigravity/cli/1.1.10 (aidev_client; os_type=linux; arch=amd64; auth_method=consumer)";

/// Configuration for the Google Antigravity Cloud Code PA backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AntigravityConfig {
    /// Cloud Code PA base URL (`https://daily-cloudcode-pa.googleapis.com`).
    pub base_url: String,
    /// Antigravity model identifier or alias (`claude-sonnet-5-5-medium`, `gemini-3.8-flash-medium`, ...).
    pub model: String,
    /// Maximum output tokens per turn.
    pub max_tokens: u32,
    /// Optional explicit Cloud AI companion project ID (`aicode-consumers` by default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Optional explicit OAuth token JSON path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_path: Option<PathBuf>,
}

impl Default for AntigravityConfig {
    fn default() -> Self {
        Self {
            base_url: std::env::var("ANTIGRAVITY_BASE_URL")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_ANTIGRAVITY_BASE_URL.to_owned()),
            model: std::env::var("ANTIGRAVITY_MODEL")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_ANTIGRAVITY_MODEL.to_owned()),
            max_tokens: 8192,
            project_id: std::env::var("ANTIGRAVITY_PROJECT_ID")
                .ok()
                .filter(|s| !s.trim().is_empty()),
            token_path: std::env::var_os("ANTIGRAVITY_TOKEN_PATH").map(PathBuf::from),
        }
    }
}

/// Cloud provider backed by Antigravity Cloud Code PA (`v1internal:streamGenerateContent?alt=sse`).
#[derive(Debug, Clone)]
pub struct AntigravityProvider {
    config: AntigravityConfig,
    agent: ureq::Agent,
    cached_tokens: Option<AntigravityTokenSet>,
    token_file_path: Option<PathBuf>,
    resolved_project: Option<String>,
    thought_signatures: HashMap<String, String>,
    last_models: Vec<AvailableModel>,
    call_seq: u64,
}

impl AntigravityProvider {
    /// Creates an [`AntigravityProvider`] with the given configuration.
    #[must_use]
    pub fn new(config: AntigravityConfig) -> Self {
        Self {
            config,
            agent: ureq::Agent::new(),
            cached_tokens: None,
            token_file_path: None,
            resolved_project: None,
            thought_signatures: HashMap::new(),
            last_models: Vec::new(),
            call_seq: 0,
        }
    }

    /// Returns the provider configuration.
    #[must_use]
    pub fn config(&self) -> &AntigravityConfig {
        &self.config
    }

    /// Normalizes short Antigravity model aliases into concrete Cloud Code PA model IDs.
    #[must_use]
    pub fn normalize_model_id(raw: &str) -> &str {
        match raw.trim() {
            "gemini-3.8-flash"
            | "gemini-3.8-flash-medium"
            | "gemini-3.8-flash-high"
            | "gemini-3.8-flash-low" => "gemini-3.8-flash-tiered",
            "gemini-3.5-flash" => "gemini-3.5-flash-lite",
            "claude-sonnet-5-5" => "claude-sonnet-5-5-medium",
            "claude-opus-5-5" => "claude-opus-5-5-medium",
            "gpt-oss-120b" => "gpt-oss-120b-medium",
            other => other,
        }
    }

    /// Builds the `v1internal:streamGenerateContent` request envelope, batching tool results cleanly.
    #[must_use]
    pub fn build_request_body(
        &self,
        project_id: &str,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Value {
        let contents = encode_antigravity_contents(messages, &self.thought_signatures);
        let mut inner = Map::new();
        inner.insert("contents".to_owned(), Value::Array(contents));
        if !system_prompt.trim().is_empty() {
            inner.insert(
                "systemInstruction".to_owned(),
                json!({ "parts": [{ "text": system_prompt }] }),
            );
        }
        if !tools.is_empty() {
            let decls: Vec<Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": sanitize_gemini_schema(&t.input_schema),
                    })
                })
                .collect();
            inner.insert(
                "tools".to_owned(),
                json!([{ "functionDeclarations": decls }]),
            );
        }
        inner.insert(
            "generationConfig".to_owned(),
            json!({ "maxOutputTokens": self.config.max_tokens }),
        );

        json!({
            "project": project_id,
            "model": Self::normalize_model_id(&self.config.model),
            "request": Value::Object(inner),
        })
    }

    /// Decodes one Cloud Code PA SSE `data:` JSON payload into [`ProviderStreamEvent`] items.
    pub fn parse_sse_chunk(
        &mut self,
        json_payload: &str,
    ) -> Result<Vec<ProviderStreamEvent>, CoreError> {
        let val: Value = serde_json::from_str(json_payload)?;
        if let Some(err_obj) = val.get("error") {
            let msg = err_obj
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Antigravity stream error");
            return Err(CoreError::Provider(msg.to_owned()));
        }

        let root = val.get("response").unwrap_or(&val);
        let mut events = Vec::new();
        if let Some(candidates) = root.get("candidates").and_then(Value::as_array)
            && let Some(candidate) = candidates.first()
            && let Some(parts) = candidate
                .pointer("/content/parts")
                .and_then(Value::as_array)
        {
            for part in parts {
                let is_thought = part.get("thought").and_then(Value::as_bool) == Some(true);
                if !is_thought
                    && let Some(text) = part.get("text").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    events.push(ProviderStreamEvent::TextDelta(text.to_owned()));
                }
                if let Some(fc) = part.get("functionCall")
                    && let Some(name) = fc.get("name").and_then(Value::as_str)
                {
                    let id = fc
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .unwrap_or_else(|| {
                            self.call_seq += 1;
                            format!("ag_call_{}", self.call_seq)
                        });
                    if let Some(sig) = part
                        .get("thoughtSignature")
                        .or_else(|| part.get("thought_signature"))
                        .and_then(Value::as_str)
                    {
                        self.thought_signatures.insert(id.clone(), sig.to_owned());
                    }
                    let arguments = fc.get("args").cloned().unwrap_or_else(|| json!({}));
                    events.push(ProviderStreamEvent::ToolUse(ToolCall {
                        id,
                        name: name.to_owned(),
                        arguments,
                    }));
                }
            }
        }
        if let Some(usage) = extract_usage_metadata(root) {
            events.push(ProviderStreamEvent::Usage(usage));
        }
        Ok(events)
    }

    fn ensure_tokens(&mut self) -> Result<String, CoreError> {
        if let Some(tokens) = &self.cached_tokens {
            return Ok(tokens.access_token.clone());
        }
        let (tokens, path) = resolve_token(self.config.token_path.as_deref())?;
        let access = tokens.access_token.clone();
        self.cached_tokens = Some(tokens);
        self.token_file_path = path;
        Ok(access)
    }

    fn try_refresh_token(&mut self) -> Result<String, CoreError> {
        let refresh = self
            .cached_tokens
            .as_ref()
            .and_then(|t| t.refresh_token.clone())
            .ok_or_else(|| {
                CoreError::Provider(
                    "Antigravity token expired and no refresh_token is available; run `rein login`"
                        .to_owned(),
                )
            })?;
        let (new_access, expires_in) = refresh_access_token(&self.agent, &refresh)?;
        if let Some(cached) = &mut self.cached_tokens {
            cached.access_token = new_access.clone();
            if let Some(path) = &self.token_file_path
                && path.exists()
            {
                let _ = save_token_file(path, cached, expires_in);
            }
        }
        Ok(new_access)
    }

    fn ensure_project_id(&mut self, access_token: &str) -> String {
        if let Some(explicit) = &self.config.project_id
            && !explicit.trim().is_empty()
        {
            return explicit.clone();
        }
        if let Some(cached) = &self.resolved_project {
            return cached.clone();
        }
        let base = self.config.base_url.trim_end_matches('/');
        let url = format!("{base}/v1internal:loadCodeAssist");
        let payload = json!({
            "metadata": {
                "ideType": "IDE_UNSPECIFIED",
                "platform": "PLATFORM_UNSPECIFIED",
                "pluginType": "GEMINI"
            }
        });
        if let Ok(resp) = self
            .agent
            .post(&url)
            .set("Authorization", &format!("Bearer {access_token}"))
            .set("Content-Type", "application/json")
            .set("User-Agent", ANTIGRAVITY_USER_AGENT)
            .send_string(&payload.to_string())
            && let Ok(body) = resp.into_string()
            && let Ok(parsed) = serde_json::from_str::<Value>(&body)
            && let Some(project) = parsed.get("cloudaicompanionProject")
        {
            let extracted = project
                .as_str()
                .map(str::to_owned)
                .or_else(|| project.get("id").and_then(Value::as_str).map(str::to_owned));
            if let Some(id) = extracted.filter(|s| !s.is_empty()) {
                self.resolved_project = Some(id.clone());
                return id;
            }
        }
        let default = DEFAULT_ANTIGRAVITY_PROJECT.to_owned();
        self.resolved_project = Some(default.clone());
        default
    }

    fn post_stream(
        &self,
        url: &str,
        access_token: &str,
        payload: &str,
    ) -> Result<ureq::Response, Box<ureq::Error>> {
        self.agent
            .post(url)
            .set("Authorization", &format!("Bearer {access_token}"))
            .set("Content-Type", "application/json")
            .set("User-Agent", ANTIGRAVITY_USER_AGENT)
            .send_string(payload)
            .map_err(Box::new)
    }
}

impl Provider for AntigravityProvider {
    fn name(&self) -> &str {
        "antigravity"
    }

    fn list_models(&mut self) -> Result<Vec<AvailableModel>, CoreError> {
        let mut access_token = self.ensure_tokens()?;
        let project_id = self.ensure_project_id(&access_token);
        let base = self.config.base_url.trim_end_matches('/');
        let url = format!("{base}/v1internal:fetchAvailableModels");
        let payload = json!({ "project": project_id }).to_string();

        let resp = match self.post_stream(&url, &access_token, &payload) {
            Ok(resp) => resp,
            Err(err) if matches!(&*err, ureq::Error::Status(401, _)) => {
                access_token = self.try_refresh_token()?;
                self.post_stream(&url, &access_token, &payload)
                    .map_err(|e| format_antigravity_error(*e))?
            }
            Err(err) => return Err(format_antigravity_error(*err)),
        };

        let body = resp
            .into_string()
            .map_err(|err| CoreError::Provider(format!("Antigravity models read error: {err}")))?;
        let parsed: Value = serde_json::from_str(&body)?;
        let models = parse_available_models(&parsed);
        self.last_models = models.clone();
        Ok(models)
    }

    fn active_model_quota(&self) -> Option<u8> {
        let normalized = Self::normalize_model_id(&self.config.model);
        self.last_models
            .iter()
            .find(|m| m.id == self.config.model || m.id == normalized)
            .and_then(|m| m.quota_percent)
    }

    fn stream_turn_with(
        &mut self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolSpec],
        on_event: &mut dyn FnMut(&ProviderStreamEvent),
    ) -> Result<Vec<ProviderStreamEvent>, CoreError> {
        let mut access_token = self.ensure_tokens()?;
        let project_id = self.ensure_project_id(&access_token);
        let body = self.build_request_body(&project_id, system_prompt, messages, tools);
        let payload = serde_json::to_string(&body)?;

        let base = self.config.base_url.trim_end_matches('/');
        let url = format!("{base}/v1internal:streamGenerateContent?alt=sse");

        let response = match self.post_stream(&url, &access_token, &payload) {
            Ok(resp) => resp,
            Err(err) if matches!(&*err, ureq::Error::Status(401, _)) => {
                access_token = self.try_refresh_token()?;
                self.post_stream(&url, &access_token, &payload)
                    .map_err(|e| format_antigravity_error(*e))?
            }
            Err(err) => return Err(format_antigravity_error(*err)),
        };

        let reader = BufReader::new(response.into_reader());
        let mut emitted = Vec::new();
        for line in reader.lines() {
            let line = line
                .map_err(|err| CoreError::Provider(format!("Antigravity SSE read error: {err}")))?;
            let trimmed = line.trim();
            if let Some(raw) = trimmed.strip_prefix("data:") {
                let data = raw.trim();
                if data.is_empty() || data == "[DONE]" {
                    continue;
                }
                for ev in self.parse_sse_chunk(data)? {
                    on_event(&ev);
                    emitted.push(ev);
                }
            }
        }
        on_event(&ProviderStreamEvent::Done);
        emitted.push(ProviderStreamEvent::Done);
        Ok(emitted)
    }
}

fn format_antigravity_error(err: ureq::Error) -> CoreError {
    match err {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let detail = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| {
                    v.pointer("/error/message")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| body.trim().to_owned());
            if detail.is_empty() {
                CoreError::Provider(format!("Antigravity HTTP {code}"))
            } else {
                CoreError::Provider(format!("Antigravity HTTP {code}: {detail}"))
            }
        }
        other => CoreError::Provider(format!("Antigravity transport error: {other}")),
    }
}

fn encode_antigravity_contents(
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

        match msg.role {
            Role::System | Role::Tool => {}
            Role::User => {
                let text = if msg.content.is_empty() {
                    " "
                } else {
                    msg.content.as_str()
                };
                contents.push(json!({ "role": "user", "parts": [{ "text": text }] }));
            }
            Role::Assistant => {
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
                if !parts.is_empty() {
                    contents.push(json!({ "role": "model", "parts": parts }));
                }
            }
        }
    }
    flush_function_responses(&mut pending_responses, &mut contents);
    contents
}

fn flush_function_responses(pending: &mut Vec<Value>, contents: &mut Vec<Value>) {
    if !pending.is_empty() {
        contents.push(json!({
            "role": "user",
            "parts": std::mem::take(pending),
        }));
    }
}

fn extract_usage_metadata(root: &Value) -> Option<UsageStats> {
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

fn parse_available_models(root: &Value) -> Vec<AvailableModel> {
    let Some(models_map) = root.get("models").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut recommended_ids = Vec::new();
    if let Some(sorts) = root.get("agentModelSorts").and_then(Value::as_array) {
        for sort in sorts {
            if let Some(groups) = sort.get("groups").and_then(Value::as_array) {
                for group in groups {
                    if let Some(ids) = group.get("modelIds").and_then(Value::as_array) {
                        for id in ids.iter().filter_map(Value::as_str) {
                            if !recommended_ids.iter().any(|existing| existing == id) {
                                recommended_ids.push(id.to_owned());
                            }
                        }
                    }
                }
            }
        }
    }

    let mut out = Vec::new();
    for id in &recommended_ids {
        if let Some(entry) = models_map.get(id)
            && let Some(model) = model_from_entry(id, entry, true)
        {
            out.push(model);
        }
    }

    let mut extra_ids: Vec<&String> = models_map
        .keys()
        .filter(|id| !recommended_ids.iter().any(|r| r == *id))
        .collect();
    extra_ids.sort();
    for id in extra_ids {
        if is_noise_model_id(id) {
            continue;
        }
        if let Some(entry) = models_map.get(id)
            && let Some(model) = model_from_entry(id, entry, false)
        {
            out.push(model);
        }
    }
    out
}

fn is_noise_model_id(id: &str) -> bool {
    id.starts_with("chat_") || id.starts_with("tab_") || id.ends_with("-image")
}

fn model_from_entry(id: &str, entry: &Value, recommended: bool) -> Option<AvailableModel> {
    if entry.get("isInternal").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let display_name = entry
        .get("displayName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(id)
        .to_owned();
    let quota_percent = entry
        .pointer("/quotaInfo/remainingFraction")
        .and_then(Value::as_f64)
        .map(|frac| {
            let pct = (frac * 100.0).clamp(0.0, 100.0).round();
            format!("{pct:.0}").parse::<u8>().unwrap_or(0)
        });
    Some(AvailableModel {
        id: id.to_owned(),
        display_name,
        quota_percent,
        recommended,
    })
}

fn sanitize_gemini_schema(schema: &Value) -> Value {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_known_model_aliases() {
        assert_eq!(
            AntigravityProvider::normalize_model_id("gemini-3.8-flash-medium"),
            "gemini-3.8-flash-tiered"
        );
        assert_eq!(
            AntigravityProvider::normalize_model_id("claude-sonnet-5-5"),
            "claude-sonnet-5-5-medium"
        );
        assert_eq!(
            AntigravityProvider::normalize_model_id("claude-opus-5-5-medium"),
            "claude-opus-5-5-medium"
        );
    }

    #[test]
    fn builds_envelope_with_sanitized_tools_and_batched_function_responses() {
        let provider = AntigravityProvider::new(AntigravityConfig::default());
        let mut assistant = Message::new(Role::Assistant, "Checking Cargo.toml");
        assistant.tool_calls.push(ToolCall {
            id: "ag_1".to_owned(),
            name: "read".to_owned(),
            arguments: json!({"path": "Cargo.toml"}),
        });
        let mut r1 = Message::new(Role::Tool, "[workspace]");
        r1.tool_calls.push(assistant.tool_calls[0].clone());

        let tools = vec![ToolSpec {
            name: "read".to_owned(),
            description: "Read file".to_owned(),
            input_schema: json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "additionalProperties": false,
                "properties": { "path": { "type": "string" } }
            }),
        }];

        let body = provider.build_request_body(
            DEFAULT_ANTIGRAVITY_PROJECT,
            "Be concise.",
            &[Message::new(Role::User, "hi"), assistant, r1],
            &tools,
        );

        assert_eq!(body["project"], "aicode-consumers");
        assert_eq!(body["model"], DEFAULT_ANTIGRAVITY_MODEL);
        let params = &body["request"]["tools"][0]["functionDeclarations"][0]["parameters"];
        assert!(params.get("$schema").is_none());
        assert!(params.get("additionalProperties").is_none());
        assert_eq!(
            body["request"]["contents"][2]["parts"][0]["functionResponse"]["name"],
            "read"
        );
    }

    #[test]
    fn decodes_sse_text_thought_and_function_call_chunks() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut provider = AntigravityProvider::new(AntigravityConfig::default());
        let chunk = r#"{
            "response": {
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [
                            { "text": "internal plan", "thought": true },
                            { "text": "Let me read that." },
                            {
                                "functionCall": { "name": "read", "args": { "path": "Cargo.toml" } },
                                "thoughtSignature": "sig-xyz"
                            }
                        ]
                    }
                }]
            }
        }"#;
        let events = provider.parse_sse_chunk(chunk)?;
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0],
            ProviderStreamEvent::TextDelta("Let me read that.".to_owned())
        );
        match &events[1] {
            ProviderStreamEvent::ToolUse(call) => {
                assert_eq!(call.name, "read");
                assert_eq!(call.id, "ag_call_1");
                assert_eq!(
                    provider
                        .thought_signatures
                        .get("ag_call_1")
                        .map(String::as_str),
                    Some("sig-xyz")
                );
            }
            other => return Err(format!("unexpected event: {other:?}").into()),
        }
        Ok(())
    }

    #[test]
    fn parses_available_models_and_usage_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let payload = json!({
            "agentModelSorts": [{
                "displayName": "Recommended",
                "groups": [{ "modelIds": ["claude-sonnet-5-5-medium"] }]
            }],
            "models": {
                "claude-sonnet-5-5-medium": {
                    "displayName": "Claude Sonnet 5.5 (Medium)",
                    "quotaInfo": { "remainingFraction": 0.984 }
                },
                "gemini-3.8-flash-tiered": {
                    "displayName": "Gemini 3.8 Flash (Tiered)",
                    "quotaInfo": { "remainingFraction": 0.844 }
                },
                "chat_20706": {
                    "displayName": "Internal Chat",
                    "isInternal": true
                }
            }
        });
        let models = parse_available_models(&payload);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "claude-sonnet-5-5-medium");
        assert_eq!(models[0].quota_percent, Some(98));
        assert!(models[0].recommended);
        assert_eq!(models[1].id, "gemini-3.8-flash-tiered");
        assert_eq!(models[1].quota_percent, Some(84));

        let mut provider = AntigravityProvider::new(AntigravityConfig::default());
        let events = provider.parse_sse_chunk(
            r#"{"response":{"usageMetadata":{"promptTokenCount":120,"candidatesTokenCount":30,"totalTokenCount":150}}}"#,
        )?;
        assert_eq!(
            events,
            vec![ProviderStreamEvent::Usage(UsageStats {
                input_tokens: 120,
                output_tokens: 30,
                total_tokens: 150,
            })]
        );
        Ok(())
    }
}
