// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-15
// Purpose: Google Antigravity Cloud Code PA streaming provider with tool calling and auto-refresh

use std::collections::{HashMap, HashSet};
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

/// Consumer AI Code endpoint used by newer Antigravity models (e.g. Gemini 4).
pub const DEFAULT_AICODE_BASE_URL: &str = "https://aicode.googleapis.com";

/// Default Antigravity model supporting coding & tool calling.
pub const DEFAULT_ANTIGRAVITY_MODEL: &str = "claude-sonnet-5-5-medium";

/// Default consumer companion project ID returned by `loadCodeAssist`.
pub const DEFAULT_ANTIGRAVITY_PROJECT: &str = "aicode-consumers";

const ANTIGRAVITY_USER_AGENT: &str = "antigravity/cli/1.5.9 (aidev_client; os_type=linux; arch=amd64; cl=1021519450; auth_method=oauth-personal)";

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

/// Cloud provider backed by Antigravity Cloud Code PA (`v1internal:streamGenerateContent?alt=sse`) and AI Code (`v1alpha:streamGenerateContent?alt=sse`).
#[derive(Debug, Clone)]
pub struct AntigravityProvider {
    config: AntigravityConfig,
    agent: ureq::Agent,
    cached_tokens: Option<AntigravityTokenSet>,
    token_file_path: Option<PathBuf>,
    resolved_project: Option<String>,
    paid_tier: Option<String>,
    thought_signatures: HashMap<String, String>,
    last_models: Vec<AvailableModel>,
    effort_wire_map: HashMap<String, (String, String)>,
    aicode_models: HashSet<String>,
    aicode_scope_blocked: bool,
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
            paid_tier: None,
            thought_signatures: HashMap::new(),
            last_models: Vec::new(),
            effort_wire_map: HashMap::new(),
            aicode_models: HashSet::new(),
            aicode_scope_blocked: false,
            call_seq: 0,
        }
    }

    /// Returns the provider configuration.
    #[must_use]
    pub fn config(&self) -> &AntigravityConfig {
        &self.config
    }

    /// Returns the paid subscription tier reported by `loadCodeAssist` (e.g. `g1-pro-tier`), if any.
    #[must_use]
    pub fn paid_tier(&self) -> Option<&str> {
        self.paid_tier.as_deref()
    }

    /// Normalizes short Antigravity model aliases into concrete Cloud Code PA model IDs.
    #[must_use]
    pub fn normalize_model_id(raw: &str) -> &str {
        match raw.trim() {
            "gemini-3.8-flash"
            | "gemini-3.8-flash-minimal"
            | "gemini-3.8-flash-medium"
            | "gemini-3.8-flash-high"
            | "gemini-3.8-flash-xhigh"
            | "gemini-3.8-flash-low" => "gemini-3.8-flash-tiered",
            "gemini-3.5-flash" => "gemini-3.5-flash-lite",
            "claude-sonnet-5-5" => "claude-sonnet-5-5-medium",
            "claude-opus-5-5" => "claude-opus-5-5-medium",
            "gpt-oss-120b" => "gpt-oss-120b-medium",
            other => other,
        }
    }

    /// Resolves the configured model ID into `(wire_model_id, optional_thinking_level)`.
    #[must_use]
    pub fn resolve_model_and_thinking(&self) -> (String, Option<String>) {
        let trimmed = self.config.model.trim();
        if let Some((wire, level)) = self.effort_wire_map.get(trimmed) {
            return (wire.clone(), Some(level.clone()));
        }
        resolve_static_model_and_thinking(trimmed)
    }

    fn build_inner_request(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> (String, Map<String, Value>) {
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
        let (wire_model, thinking_level) = self.resolve_model_and_thinking();
        inner.insert(
            "generationConfig".to_owned(),
            build_generation_config(
                &wire_model,
                thinking_level.as_deref(),
                self.config.max_tokens,
            ),
        );
        (wire_model, inner)
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
        let (wire_model, inner) = self.build_inner_request(system_prompt, messages, tools);
        json!({
            "project": project_id,
            "model": wire_model,
            "request": Value::Object(inner),
        })
    }

    /// Builds the direct `aicode.googleapis.com/v1alpha:streamGenerateContent` request payload.
    #[must_use]
    pub fn build_aicode_request_body(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Value {
        let (wire_model, mut body) = self.build_inner_request(system_prompt, messages, tools);
        body.insert("model".to_owned(), Value::String(wire_model));
        Value::Object(body)
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
                if let Some(text) = part.get("text").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    if is_thought {
                        events.push(ProviderStreamEvent::ThinkingDelta(text.to_owned()));
                    } else {
                        events.push(ProviderStreamEvent::TextDelta(text.to_owned()));
                    }
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
        {
            if let Some(tier) = parsed.pointer("/paidTier/id").and_then(Value::as_str)
                && !tier.trim().is_empty()
            {
                self.paid_tier = Some(tier.to_owned());
            }
            if let Some(project) = parsed.get("cloudaicompanionProject") {
                let extracted = project
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| project.get("id").and_then(Value::as_str).map(str::to_owned));
                if let Some(id) = extracted.filter(|s| !s.is_empty()) {
                    self.resolved_project = Some(id.clone());
                    return id;
                }
            }
        }
        let default = DEFAULT_ANTIGRAVITY_PROJECT.to_owned();
        self.resolved_project = Some(default.clone());
        default
    }

    fn resolve_aicode_base_url(&self) -> Option<String> {
        if let Ok(explicit) = std::env::var("AICODE_ENDPOINT_URL")
            && !explicit.trim().is_empty()
        {
            return Some(explicit.trim_end_matches('/').to_owned());
        }
        let base = self.config.base_url.trim_end_matches('/');
        if base == DEFAULT_ANTIGRAVITY_BASE_URL || base == FALLBACK_ANTIGRAVITY_BASE_URL {
            Some(DEFAULT_AICODE_BASE_URL.to_owned())
        } else {
            None
        }
    }

    fn send_get(&self, url: &str, access_token: &str) -> Result<ureq::Response, Box<ureq::Error>> {
        self.agent
            .get(url)
            .set("Authorization", &format!("Bearer {access_token}"))
            .set("User-Agent", ANTIGRAVITY_USER_AGENT)
            .call()
            .map_err(Box::new)
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

    fn post_with_refresh(
        &mut self,
        url: &str,
        access_token: &mut String,
        payload: &str,
    ) -> Result<ureq::Response, (Option<u16>, bool, CoreError)> {
        match self.post_stream(url, access_token, payload) {
            Ok(resp) => Ok(resp),
            Err(err) if matches!(&*err, ureq::Error::Status(401, _)) => {
                *access_token = self
                    .try_refresh_token()
                    .map_err(|e| (Some(401), false, e))?;
                self.post_stream(url, access_token, payload)
                    .map_err(|e| classify_http_error(*e))
            }
            Err(err) => Err(classify_http_error(*err)),
        }
    }

    fn fetch_aicode_config(
        &mut self,
        access_token: &mut String,
    ) -> Option<(Vec<AvailableModel>, EffortWireMap, HashSet<String>)> {
        if self.aicode_scope_blocked {
            return None;
        }
        let aicode_base = self.resolve_aicode_base_url()?;
        let url = format!("{aicode_base}/v1alpha:fetchConfig");
        let resp = match self.send_get(&url, access_token) {
            Ok(r) => r,
            Err(err) if matches!(&*err, ureq::Error::Status(401, _)) => {
                *access_token = self.try_refresh_token().ok()?;
                self.send_get(&url, access_token).ok()?
            }
            Err(err) => {
                let (_, scope_missing, _) = classify_http_error(*err);
                if scope_missing {
                    self.aicode_scope_blocked = true;
                }
                return None;
            }
        };
        let body = resp.into_string().ok()?;
        let parsed: Value = serde_json::from_str(&body).ok()?;
        Some(parse_aicode_fetch_config(&parsed))
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

        let (mut models, mut wire_map) =
            match self.post_with_refresh(&url, &mut access_token, &payload) {
                Ok(resp) => {
                    let body = resp.into_string().map_err(|err| {
                        CoreError::Provider(format!("Antigravity models read error: {err}"))
                    })?;
                    let parsed: Value = serde_json::from_str(&body)?;
                    parse_available_models_with_wire(&parsed)
                }
                Err((_, _, err)) => {
                    if let Some((ai_models, ai_wire, ai_ids)) =
                        self.fetch_aicode_config(&mut access_token)
                        && !ai_models.is_empty()
                    {
                        self.last_models = ai_models.clone();
                        self.effort_wire_map = ai_wire;
                        self.aicode_models = ai_ids;
                        return Ok(ai_models);
                    }
                    return Err(err);
                }
            };

        if let Some((ai_models, ai_wire, ai_ids)) = self.fetch_aicode_config(&mut access_token) {
            self.aicode_models = ai_ids;
            merge_model_catalogs(&mut models, &mut wire_map, ai_models, ai_wire);
        } else {
            self.aicode_models.clear();
        }
        self.last_models = models.clone();
        self.effort_wire_map = wire_map;
        Ok(models)
    }

    fn active_model_quota(&self) -> Option<u8> {
        let trimmed = self.config.model.trim();
        let (wire_model, _) = self.resolve_model_and_thinking();
        self.last_models
            .iter()
            .find(|m| {
                m.id == trimmed
                    || m.id == wire_model
                    || self
                        .effort_wire_map
                        .get(&m.id)
                        .is_some_and(|(wire, _)| wire == &wire_model || wire == trimmed)
            })
            .and_then(|m| m.quota_percent)
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
        let mut access_token = self.ensure_tokens()?;
        let project_id = self.ensure_project_id(&access_token);
        let (wire_model, _) = self.resolve_model_and_thinking();
        let aicode_base = self.resolve_aicode_base_url();
        let prefer_aicode = !self.aicode_scope_blocked
            && aicode_base.is_some()
            && (self.aicode_models.contains(&wire_model) || wire_model.starts_with("gemini-4-"));

        let response = if prefer_aicode && let Some(ai_base) = aicode_base {
            self.stream_aicode_with_fallback(
                &ai_base,
                &project_id,
                &mut access_token,
                system_prompt,
                messages,
                tools,
            )?
        } else {
            self.stream_v1internal_with_fallback(
                aicode_base.as_deref(),
                &project_id,
                &mut access_token,
                system_prompt,
                messages,
                tools,
            )?
        };

        self.read_sse_events(response, cancel, on_event)
    }
}

impl AntigravityProvider {
    fn stream_aicode_with_fallback(
        &mut self,
        ai_base: &str,
        project_id: &str,
        access_token: &mut String,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Result<ureq::Response, CoreError> {
        match self.post_aicode_stream(ai_base, access_token, system_prompt, messages, tools) {
            Ok(resp) => Ok(resp),
            Err((status, scope_missing, err)) if scope_missing || status == Some(404) => {
                let v1_payload = serde_json::to_string(&self.build_request_body(
                    project_id,
                    system_prompt,
                    messages,
                    tools,
                ))?;
                self.post_v1internal_with_fallback(access_token, &v1_payload)
                    .or(Err(err))
            }
            Err((_, _, err)) => Err(err),
        }
    }

    fn stream_v1internal_with_fallback(
        &mut self,
        aicode_base: Option<&str>,
        project_id: &str,
        access_token: &mut String,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Result<ureq::Response, CoreError> {
        let v1_payload = serde_json::to_string(&self.build_request_body(
            project_id,
            system_prompt,
            messages,
            tools,
        ))?;
        match self.post_v1internal_with_fallback(access_token, &v1_payload) {
            Ok(resp) => Ok(resp),
            Err((Some(404), _, err)) if !self.aicode_scope_blocked => {
                let Some(ai_base) = aicode_base else {
                    return Err(err);
                };
                self.post_aicode_stream(ai_base, access_token, system_prompt, messages, tools)
                    .map_err(|(_, _, ai_err)| ai_err)
            }
            Err((_, _, err)) => Err(err),
        }
    }

    fn post_aicode_stream(
        &mut self,
        ai_base: &str,
        access_token: &mut String,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Result<ureq::Response, (Option<u16>, bool, CoreError)> {
        let ai_payload =
            serde_json::to_string(&self.build_aicode_request_body(system_prompt, messages, tools))
                .map_err(|err| (None, false, CoreError::from(err)))?;
        let ai_url = format!("{ai_base}/v1alpha:streamGenerateContent?alt=sse");
        let res = self.post_with_refresh(&ai_url, access_token, &ai_payload);
        if let Err((_, true, _)) = res {
            self.aicode_scope_blocked = true;
        }
        res
    }

    fn post_v1internal_with_fallback(
        &mut self,
        access_token: &mut String,
        payload: &str,
    ) -> Result<ureq::Response, (Option<u16>, bool, CoreError)> {
        let base = self.config.base_url.trim_end_matches('/');
        let is_default_base = base == DEFAULT_ANTIGRAVITY_BASE_URL;
        let url = format!("{base}/v1internal:streamGenerateContent?alt=sse");
        match self.post_with_refresh(&url, access_token, payload) {
            Ok(resp) => Ok(resp),
            Err((Some(404), _, _)) if is_default_base => {
                let fallback_url = format!(
                    "{FALLBACK_ANTIGRAVITY_BASE_URL}/v1internal:streamGenerateContent?alt=sse"
                );
                self.post_with_refresh(&fallback_url, access_token, payload)
            }
            Err(other) => Err(other),
        }
    }

    fn read_sse_events(
        &mut self,
        response: ureq::Response,
        cancel: &std::sync::atomic::AtomicBool,
        on_event: &mut dyn FnMut(&ProviderStreamEvent),
    ) -> Result<Vec<ProviderStreamEvent>, CoreError> {
        let reader = BufReader::new(response.into_reader());
        let mut emitted = Vec::new();
        for line in reader.lines() {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(CoreError::Cancelled);
            }
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

fn build_generation_config(
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

fn classify_http_error(err: ureq::Error) -> (Option<u16>, bool, CoreError) {
    match err {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let scope_missing = body.contains("ACCESS_TOKEN_SCOPE_INSUFFICIENT");
            let detail = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| {
                    v.pointer("/error/message")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| body.trim().to_owned());
            let core_err = if detail.is_empty() {
                CoreError::Provider(format!("Antigravity HTTP {code}"))
            } else {
                CoreError::Provider(format!("Antigravity HTTP {code}: {detail}"))
            };
            (Some(code), scope_missing, core_err)
        }
        other => (
            None,
            false,
            CoreError::Provider(format!("Antigravity transport error: {other}")),
        ),
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

#[cfg_attr(not(test), allow(dead_code))]
fn parse_available_models(root: &Value) -> Vec<AvailableModel> {
    parse_available_models_with_wire(root).0
}

type EffortWireMap = HashMap<String, (String, String)>;

fn parse_available_models_with_wire(root: &Value) -> (Vec<AvailableModel>, EffortWireMap) {
    let mut wire_map = EffortWireMap::new();
    let Some(models_map) = root.get("models").and_then(Value::as_object) else {
        return (Vec::new(), wire_map);
    };
    let recommended_ids = collect_recommended_ids(root);
    let mut out = Vec::new();
    for id in &recommended_ids {
        if let Some(entry) = models_map.get(id) {
            out.extend(expand_model_entry(id, entry, true, &mut wire_map));
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
        if let Some(entry) = models_map.get(id) {
            out.extend(expand_model_entry(id, entry, false, &mut wire_map));
        }
    }
    (out, wire_map)
}

fn parse_aicode_fetch_config(
    root: &Value,
) -> (Vec<AvailableModel>, EffortWireMap, HashSet<String>) {
    let mut wire_map = EffortWireMap::new();
    let mut out = Vec::new();
    let mut aicode_ids = HashSet::new();
    let mut seen = HashSet::new();

    if let Some(arr) = root.get("availableModels").and_then(Value::as_array) {
        for entry in arr {
            push_aicode_model_entry(
                entry,
                false,
                &mut seen,
                &mut out,
                &mut wire_map,
                &mut aicode_ids,
            );
        }
    }
    if let Some(exps) = root.get("availableExperiences").and_then(Value::as_array) {
        for exp in exps {
            if let Some(entry) = exp.get("model") {
                push_aicode_model_entry(
                    entry,
                    false,
                    &mut seen,
                    &mut out,
                    &mut wire_map,
                    &mut aicode_ids,
                );
            }
        }
    }
    (out, wire_map, aicode_ids)
}

fn push_aicode_model_entry(
    entry: &Value,
    recommended: bool,
    seen: &mut HashSet<String>,
    out: &mut Vec<AvailableModel>,
    wire_map: &mut EffortWireMap,
    aicode_ids: &mut HashSet<String>,
) {
    let Some(id) = extract_aicode_model_id(entry) else {
        return;
    };
    if is_noise_model_id(&id) {
        return;
    }
    aicode_ids.insert(id.clone());
    for model in expand_model_entry(&id, entry, recommended, wire_map) {
        aicode_ids.insert(model.id.clone());
        if seen.insert(model.id.clone()) {
            out.push(model);
        }
    }
}

fn extract_aicode_model_id(entry: &Value) -> Option<String> {
    let raw = entry
        .get("modelId")
        .or_else(|| entry.get("baseModelId"))
        .or_else(|| entry.get("name"))
        .and_then(Value::as_str)?
        .trim();
    let cleaned = raw.strip_prefix("models/").unwrap_or(raw).trim();
    (!cleaned.is_empty()).then(|| cleaned.to_owned())
}

fn merge_model_catalogs(
    base_models: &mut Vec<AvailableModel>,
    base_wire: &mut EffortWireMap,
    ai_models: Vec<AvailableModel>,
    ai_wire: EffortWireMap,
) {
    for model in ai_models {
        if let Some(existing) = base_models.iter_mut().find(|m| m.id == model.id) {
            if existing.quota_percent.is_none() {
                existing.quota_percent = model.quota_percent;
            }
            existing.recommended |= model.recommended;
        } else {
            base_models.push(model);
        }
    }
    for (ui_id, mapping) in ai_wire {
        base_wire.entry(ui_id).or_insert(mapping);
    }
}

fn collect_recommended_ids(root: &Value) -> Vec<String> {
    let mut ids_out = Vec::new();
    if let Some(sorts) = root.get("agentModelSorts").and_then(Value::as_array) {
        for sort in sorts {
            if let Some(groups) = sort.get("groups").and_then(Value::as_array) {
                for group in groups {
                    if let Some(ids) = group.get("modelIds").and_then(Value::as_array) {
                        for id in ids.iter().filter_map(Value::as_str) {
                            if !ids_out.iter().any(|existing| existing == id) {
                                ids_out.push(id.to_owned());
                            }
                        }
                    }
                }
            }
        }
    }
    ids_out
}

fn is_noise_model_id(id: &str) -> bool {
    id.starts_with("chat_") || id.starts_with("tab_") || id.ends_with("-image")
}

fn expand_model_entry(
    id: &str,
    entry: &Value,
    recommended: bool,
    wire_map: &mut EffortWireMap,
) -> Vec<AvailableModel> {
    if entry.get("isInternal").and_then(Value::as_bool) == Some(true) {
        return Vec::new();
    }
    let quota_percent = parse_quota_percent(entry);
    let raw_display = entry
        .get("displayName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());

    if let Some(levels) = entry
        .get("supportedThinkingLevels")
        .and_then(Value::as_array)
        .filter(|arr| !arr.is_empty())
    {
        let expanded = expand_from_supported_levels(
            id,
            raw_display,
            levels,
            quota_percent,
            recommended,
            wire_map,
        );
        if !expanded.is_empty() {
            return expanded;
        }
    }

    if let Some(stem) = id.strip_suffix("-tiered") {
        return expand_default_tiered(id, stem, raw_display, quota_percent, recommended, wire_map);
    }

    vec![AvailableModel {
        id: id.to_owned(),
        display_name: raw_display.unwrap_or(id).to_owned(),
        quota_percent,
        recommended,
    }]
}

fn expand_from_supported_levels(
    id: &str,
    raw_display: Option<&str>,
    levels: &[Value],
    quota_percent: Option<u8>,
    recommended: bool,
    wire_map: &mut EffortWireMap,
) -> Vec<AvailableModel> {
    let base_id = id.strip_suffix("-tiered").unwrap_or(id);
    let base_display = resolve_base_display(base_id, raw_display);
    let mut expanded = Vec::with_capacity(levels.len());
    for level in levels {
        let Some(suffix) = level
            .get("thinkingSuffix")
            .and_then(Value::as_str)
            .map(|s| s.trim().trim_start_matches('-'))
        else {
            continue;
        };
        let Some(enum_val) = thinking_enum_for_suffix(suffix) else {
            continue;
        };
        let label = level
            .get("thinkingDisplayName")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| humanize_model_stem(suffix));
        let variant_id = format!("{base_id}-{suffix}");
        wire_map.insert(variant_id.clone(), (id.to_owned(), enum_val.to_owned()));
        expanded.push(AvailableModel {
            id: variant_id,
            display_name: format!("{base_display} ({label})"),
            quota_percent,
            recommended,
        });
    }
    expanded
}

fn expand_default_tiered(
    wire_id: &str,
    stem: &str,
    raw_display: Option<&str>,
    quota_percent: Option<u8>,
    recommended: bool,
    wire_map: &mut EffortWireMap,
) -> Vec<AvailableModel> {
    let base_display = resolve_base_display(stem, raw_display);
    [
        ("low", "Low", "LOW"),
        ("medium", "Medium", "MEDIUM"),
        ("high", "High", "HIGH"),
    ]
    .into_iter()
    .map(|(suffix, label, enum_val)| {
        let variant_id = format!("{stem}-{suffix}");
        wire_map.insert(
            variant_id.clone(),
            (wire_id.to_owned(), enum_val.to_owned()),
        );
        AvailableModel {
            id: variant_id,
            display_name: format!("{base_display} ({label})"),
            quota_percent,
            recommended,
        }
    })
    .collect()
}

fn resolve_base_display(stem: &str, raw_display: Option<&str>) -> String {
    raw_display
        .map(strip_tiered_label)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| humanize_model_stem(stem))
}

fn parse_quota_percent(entry: &Value) -> Option<u8> {
    entry
        .pointer("/quotaInfo/remainingFraction")
        .and_then(Value::as_f64)
        .map(|frac| {
            let pct = (frac * 100.0).clamp(0.0, 100.0).round();
            format!("{pct:.0}").parse::<u8>().unwrap_or(0)
        })
}

fn strip_tiered_label(display: &str) -> String {
    for suffix in [" (Tiered)", " (tiered)", " Tiered", " tiered"] {
        if let Some(stripped) = display.strip_suffix(suffix) {
            return stripped.trim().to_owned();
        }
    }
    display.trim().to_owned()
}

fn humanize_model_stem(stem: &str) -> String {
    stem.split('-')
        .filter(|seg| !seg.is_empty())
        .map(|seg| {
            let mut chars = seg.chars();
            match chars.next() {
                Some(first) if first.is_ascii_alphabetic() => {
                    format!("{}{}", first.to_ascii_uppercase(), chars.as_str())
                }
                _ => seg.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn thinking_enum_for_suffix(suffix: &str) -> Option<&'static str> {
    match suffix.to_ascii_lowercase().as_str() {
        "minimal" => Some("MINIMAL"),
        "low" => Some("LOW"),
        "medium" => Some("MEDIUM"),
        "high" => Some("HIGH"),
        "xhigh" => Some("XHIGH"),
        _ => None,
    }
}

fn resolve_static_model_and_thinking(trimmed: &str) -> (String, Option<String>) {
    let wire = AntigravityProvider::normalize_model_id(trimmed).to_owned();
    let effort = match trimmed {
        "gemini-3.8-flash-minimal" => Some("MINIMAL"),
        "gemini-3.8-flash-low" => Some("LOW"),
        "gemini-3.8-flash" | "gemini-3.8-flash-medium" => Some("MEDIUM"),
        "gemini-3.8-flash-high" | "gemini-3.8-flash-tiered" => Some("HIGH"),
        "gemini-3.8-flash-xhigh" => Some("XHIGH"),
        other if other.ends_with("-tiered") => Some("HIGH"),
        _ => None,
    };
    (wire, effort.map(str::to_owned))
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
        assert_eq!(events.len(), 3);
        assert_eq!(
            events[0],
            ProviderStreamEvent::ThinkingDelta("internal plan".to_owned())
        );
        assert_eq!(
            events[1],
            ProviderStreamEvent::TextDelta("Let me read that.".to_owned())
        );
        match &events[2] {
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
        assert_eq!(models.len(), 4);
        assert_eq!(models[0].id, "claude-sonnet-5-5-medium");
        assert_eq!(models[0].quota_percent, Some(98));
        assert!(models[0].recommended);
        assert_eq!(models[1].id, "gemini-3.8-flash-low");
        assert_eq!(models[1].display_name, "Gemini 3.8 Flash (Low)");
        assert_eq!(models[1].quota_percent, Some(84));
        assert_eq!(models[2].id, "gemini-3.8-flash-medium");
        assert_eq!(models[3].id, "gemini-3.8-flash-high");

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

    #[test]
    fn sets_thinking_level_for_tiered_gemini_and_drops_orphan_tool_responses() {
        let cfg = AntigravityConfig {
            model: "gemini-3.8-flash-low".to_owned(),
            ..AntigravityConfig::default()
        };
        let provider = AntigravityProvider::new(cfg);
        let mut orphan = Message::new(Role::Tool, "stale bash");
        orphan.tool_calls.push(ToolCall {
            id: "ag_0".to_owned(),
            name: "bash".to_owned(),
            arguments: json!({}),
        });
        let body = provider.build_request_body(
            DEFAULT_ANTIGRAVITY_PROJECT,
            "sys",
            &[orphan, Message::new(Role::User, "hello")],
            &[],
        );
        assert_eq!(body["model"], "gemini-3.8-flash-tiered");
        assert_eq!(
            body["request"]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "LOW"
        );
        let contents = body["request"]["contents"].as_array();
        assert_eq!(contents.map(Vec::len), Some(1));
        assert_eq!(body["request"]["contents"][0]["parts"][0]["text"], "hello");
    }

    #[test]
    fn parses_aicode_fetch_config_with_xhigh_and_builds_top_level_payload()
    -> Result<(), Box<dyn std::error::Error>> {
        let cfg_payload = json!({
            "availableModels": [
                {
                    "modelId": "gemini-4-hydrogen",
                    "displayName": "Gemini 4 Hydrogen",
                    "supportedThinkingLevels": [
                        { "thinkingLevel": "LOW", "thinkingSuffix": "low", "thinkingDisplayName": "Low" },
                        { "thinkingLevel": "HIGH", "thinkingSuffix": "high", "thinkingDisplayName": "High" },
                        { "thinkingLevel": "XHIGH", "thinkingSuffix": "xhigh", "thinkingDisplayName": "XHigh" }
                    ]
                },
                {
                    "name": "models/gemini-4-argon",
                    "displayName": "Gemini 4 Argon"
                }
            ]
        });
        let (ai_models, ai_wire, ai_ids) = parse_aicode_fetch_config(&cfg_payload);
        assert_eq!(ai_models.len(), 4);
        assert_eq!(ai_models[0].id, "gemini-4-hydrogen-low");
        assert_eq!(ai_models[2].id, "gemini-4-hydrogen-xhigh");
        assert_eq!(ai_models[3].id, "gemini-4-argon");
        assert!(ai_ids.contains("gemini-4-hydrogen"));
        assert!(ai_ids.contains("gemini-4-argon"));

        let mut base = vec![AvailableModel {
            id: "claude-sonnet-5-5-medium".to_owned(),
            display_name: "Claude Sonnet 5.5 (Medium)".to_owned(),
            quota_percent: Some(90),
            recommended: true,
        }];
        let mut base_wire = EffortWireMap::new();
        merge_model_catalogs(&mut base, &mut base_wire, ai_models, ai_wire);
        assert_eq!(base.len(), 5);

        let mut provider = AntigravityProvider::new(AntigravityConfig {
            model: "gemini-4-hydrogen-xhigh".to_owned(),
            ..AntigravityConfig::default()
        });
        provider.effort_wire_map = base_wire;
        let ai_body = provider.build_aicode_request_body(
            "Be concise.",
            &[Message::new(Role::User, "hi")],
            &[],
        );
        assert_eq!(ai_body["model"], "gemini-4-hydrogen");
        assert_eq!(
            ai_body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "XHIGH"
        );
        assert!(ai_body.get("request").is_none());

        let events = provider.parse_sse_chunk(
            r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Ready."}]}}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":2,"totalTokenCount":12}}"#,
        )?;
        assert_eq!(
            events[0],
            ProviderStreamEvent::TextDelta("Ready.".to_owned())
        );
        assert_eq!(
            events[1],
            ProviderStreamEvent::Usage(UsageStats {
                input_tokens: 10,
                output_tokens: 2,
                total_tokens: 12,
            })
        );
        Ok(())
    }

    #[test]
    fn normalizes_leading_model_dangling_calls_and_consecutive_model_turns() {
        let provider = AntigravityProvider::new(AntigravityConfig::default());
        let mut stale_fc = Message::new(Role::Assistant, "stale");
        stale_fc.tool_calls.push(ToolCall {
            id: "fc_0".to_owned(),
            name: "bash".to_owned(),
            arguments: json!({}),
        });
        let mut dangling_fc = Message::new(Role::Assistant, "incomplete");
        dangling_fc.tool_calls.push(ToolCall {
            id: "fc_1".to_owned(),
            name: "read".to_owned(),
            arguments: json!({"path": "Cargo.toml"}),
        });
        let mut next_fc = Message::new(Role::Assistant, "retrying");
        next_fc.tool_calls.push(ToolCall {
            id: "fc_2".to_owned(),
            name: "glob".to_owned(),
            arguments: json!({"pattern": "*.rs"}),
        });
        let mut tool_ok = Message::new(Role::Tool, "lib.rs");
        tool_ok.tool_calls.push(next_fc.tool_calls[0].clone());

        let body = provider.build_request_body(
            DEFAULT_ANTIGRAVITY_PROJECT,
            "sys",
            &[
                stale_fc,
                Message::new(Role::User, "hello"),
                dangling_fc,
                next_fc,
                tool_ok,
            ],
            &[],
        );
        let contents = body["request"]["contents"]
            .as_array()
            .expect("contents array");
        // Leading model dropped -> [user("hello"), merged model("incomplete"+"retrying"+fc_2), user(fr_2)]
        assert_eq!(contents.len(), 3);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "hello");
        assert_eq!(contents[1]["role"], "model");
        let model_parts = contents[1]["parts"].as_array().expect("model parts");
        assert_eq!(model_parts.len(), 3);
        assert!(model_parts.iter().any(|p| p.get("functionCall").is_some()));
        assert_eq!(contents[2]["role"], "user");
        assert_eq!(contents[2]["parts"][0]["functionResponse"]["name"], "glob");
    }
}
