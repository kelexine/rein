// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-15
// Purpose: Google Antigravity Cloud Code PA streaming provider with tool calling and auto-refresh

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::event::CoreError;
use crate::message::{Message, ToolCall};
use crate::provider::antigravity_auth::AntigravityTokenSet;
use crate::provider::{AvailableModel, Provider, ProviderStreamEvent, ToolSpec};

use super::codec::{
    build_generation_config, encode_antigravity_contents, extract_usage_metadata,
    sanitize_gemini_schema,
};
use super::models::{
    EffortWireMap, ExperienceMap, merge_model_catalogs, parse_available_models_with_wire,
    resolve_static_model_and_thinking,
};

/// Primary Cloud Code PA endpoint used by Antigravity.
pub const DEFAULT_ANTIGRAVITY_BASE_URL: &str = "https://daily-cloudcode-pa.googleapis.com";

/// Secondary Cloud Code PA endpoint.
pub const FALLBACK_ANTIGRAVITY_BASE_URL: &str = "https://cloudcode-pa.googleapis.com";

/// Consumer AI Code endpoint used by newer Antigravity models.
pub const DEFAULT_AICODE_BASE_URL: &str = "https://aicode.googleapis.com";

/// Default Antigravity model supporting coding & tool calling.
pub const DEFAULT_ANTIGRAVITY_MODEL: &str = "claude-sonnet-5-5-medium";

/// Default consumer companion project ID returned by `loadCodeAssist`.
pub const DEFAULT_ANTIGRAVITY_PROJECT: &str = "aicode-consumers";

pub(super) const ANTIGRAVITY_USER_AGENT: &str = "antigravity/cli/1.5.9 (aidev_client; os_type=linux; arch=amd64; cl=1021519450; auth_method=oauth-personal)";

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

/// Cloud provider backed by Antigravity Cloud Code PA (`v1internal:streamGenerateContent?alt=sse`) and AI Code (`v1alpha`/`v1beta:streamGenerateContent?alt=sse`).
#[derive(Debug, Clone)]
pub struct AntigravityProvider {
    pub(super) config: AntigravityConfig,
    pub(super) agent: ureq::Agent,
    pub(super) cached_tokens: Option<AntigravityTokenSet>,
    pub(super) token_file_path: Option<PathBuf>,
    pub(super) resolved_project: Option<String>,
    pub(super) paid_tier: Option<String>,
    thought_signatures: HashMap<String, String>,
    last_models: Vec<AvailableModel>,
    effort_wire_map: EffortWireMap,
    pub(super) aicode_models: HashSet<String>,
    pub(super) aicode_experiences: ExperienceMap,
    pub(super) aicode_scope_blocked: bool,
    pub(super) aicode_entitlement: Option<Value>,
    call_seq: u64,
}

pub(super) type UreqResponse = ureq::http::Response<ureq::Body>;

impl AntigravityProvider {
    /// Creates an [`AntigravityProvider`] with the given configuration.
    #[must_use]
    pub fn new(config: AntigravityConfig) -> Self {
        let cfg = ureq::config::Config::builder()
            .http_status_as_error(false)
            .build();
        Self {
            config,
            agent: ureq::Agent::new_with_config(cfg),
            cached_tokens: None,
            token_file_path: None,
            resolved_project: None,
            paid_tier: None,
            thought_signatures: HashMap::new(),
            last_models: Vec::new(),
            effort_wire_map: HashMap::new(),
            aicode_models: HashSet::new(),
            aicode_experiences: HashMap::new(),
            aicode_scope_blocked: false,
            aicode_entitlement: None,
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

    /// Builds the direct `aicode.googleapis.com` request payload using `aicode.experience`.
    #[must_use]
    pub fn build_aicode_request_body(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Value {
        let (wire_model, mut body) = self.build_inner_request(system_prompt, messages, tools);
        let trimmed = self.config.model.trim();
        let experience = self
            .aicode_experiences
            .get(trimmed)
            .cloned()
            .unwrap_or(wire_model);
        body.insert("aicode".to_owned(), json!({ "experience": experience }));
        if let Some(entitlement) = &self.aicode_entitlement {
            body.insert("entitlement".to_owned(), entitlement.clone());
        }
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

        let never = std::sync::atomic::AtomicBool::new(false);
        let (mut models, mut wire_map) =
            match self.post_with_refresh(&url, &mut access_token, &payload, &never) {
                Ok(resp) => {
                    let body = resp.into_body().read_to_string().map_err(|err| {
                        CoreError::Provider(format!("Antigravity models read error: {err}"))
                    })?;
                    let parsed: Value = serde_json::from_str(&body)?;
                    parse_available_models_with_wire(&parsed)
                }
                Err((_, _, err)) => {
                    if let Some(ai) = self.fetch_aicode_config(&mut access_token)
                        && !ai.models.is_empty()
                    {
                        self.last_models = ai.models.clone();
                        self.effort_wire_map = ai.wire_map;
                        self.aicode_models = ai.aicode_ids;
                        self.aicode_experiences = ai.experiences;
                        return Ok(ai.models);
                    }
                    return Err(err);
                }
            };

        if let Some(ai) = self.fetch_aicode_config(&mut access_token) {
            self.aicode_models = ai.aicode_ids;
            self.aicode_experiences = ai.experiences;
            merge_model_catalogs(&mut models, &mut wire_map, ai.models, ai.wire_map);
        } else {
            self.aicode_models.clear();
            self.aicode_experiences.clear();
        }
        self.last_models = models.clone();
        self.effort_wire_map = wire_map;
        Ok(models)
    }

    fn cached_models(&self) -> &[AvailableModel] {
        &self.last_models
    }

    fn set_model(&mut self, model: &str) -> bool {
        self.config.model = model.to_owned();
        true
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
        let prefer_aicode = aicode_base.is_some() && self.should_prefer_aicode(&wire_model);

        let response = if prefer_aicode && let Some(ai_base) = aicode_base {
            self.stream_aicode_with_fallback(
                &ai_base,
                &project_id,
                &mut access_token,
                system_prompt,
                messages,
                tools,
                cancel,
            )?
        } else {
            self.stream_v1internal_with_fallback(
                aicode_base.as_deref(),
                &project_id,
                &mut access_token,
                system_prompt,
                messages,
                tools,
                cancel,
            )?
        };

        self.read_sse_events(response, cancel, on_event)
    }
}

#[cfg(test)]
mod tests {
    use super::super::codec::validate_antigravity_url;
    use super::super::models::{
        parse_aicode_entitlement, parse_aicode_fetch_config, parse_available_models,
    };
    use super::*;
    use crate::message::Role;
    use crate::provider::UsageStats;

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
    fn parses_aicode_experiences_and_builds_aicode_experience_payload()
    -> Result<(), Box<dyn std::error::Error>> {
        let cfg_payload = json!({
            "availableExperiences": [
                {
                    "name": "claude-sonnet-5-5-low",
                    "model": {
                        "modelId": "claude-sonnet-5-5",
                        "displayName": "Claude Sonnet 5.5 (Low)",
                        "quotaInfo": { "remainingFraction": 0.9 }
                    }
                },
                {
                    "name": "gemini-3.8-flash-high",
                    "model": {
                        "modelId": "gemini-3.8-flash",
                        "displayName": "Gemini 3.8 Flash (High)"
                    }
                }
            ]
        });
        let ai = parse_aicode_fetch_config(&cfg_payload);
        assert_eq!(ai.models.len(), 2);
        assert_eq!(ai.models[0].id, "claude-sonnet-5-5-low");
        assert_eq!(ai.models[0].quota_percent, Some(90));
        assert_eq!(ai.models[1].id, "gemini-3.8-flash-high");
        assert_eq!(
            ai.experiences
                .get("claude-sonnet-5-5-low")
                .map(String::as_str),
            Some("claude-sonnet-5-5-low")
        );

        let lic_payload = json!({ "licenses": [{ "userTier": "g1-ultra-tier" }] });
        let ent = parse_aicode_entitlement(&lic_payload);
        assert_eq!(ent, Some(json!({ "userTier": "g1-ultra-tier" })));

        let mut provider = AntigravityProvider::new(AntigravityConfig {
            model: "claude-sonnet-5-5-low".to_owned(),
            ..AntigravityConfig::default()
        });
        provider.aicode_models = ai.aicode_ids;
        provider.aicode_experiences = ai.experiences;
        provider.aicode_entitlement = ent;

        assert!(provider.should_prefer_aicode("claude-sonnet-5-5-low"));
        let ai_body = provider.build_aicode_request_body(
            "Be concise.",
            &[Message::new(Role::User, "hi")],
            &[],
        );
        assert!(ai_body.get("model").is_none());
        assert_eq!(ai_body["aicode"]["experience"], "claude-sonnet-5-5-low");
        assert_eq!(ai_body["entitlement"]["userTier"], "g1-ultra-tier");
        assert!(ai_body.get("request").is_none());

        // Unlisted gemini-4-* must NOT be routed to aicode speculatively.
        provider.config.model = "gemini-4-hydrogen-medium".to_owned();
        assert!(!provider.should_prefer_aicode("gemini-4-hydrogen-medium"));

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

    #[test]
    fn validates_antigravity_urls_against_google_https_allowlist() {
        assert!(
            validate_antigravity_url(
                "https://daily-cloudcode-pa.googleapis.com/v1internal:streamGenerateContent"
            )
            .is_ok()
        );
        assert!(validate_antigravity_url("https://oauth2.googleapis.com/token").is_ok());
        assert!(validate_antigravity_url("https://accounts.google.com/o/oauth2/v2/auth").is_ok());

        assert!(
            validate_antigravity_url("http://daily-cloudcode-pa.googleapis.com/v1internal:foo")
                .is_err()
        );
        assert!(validate_antigravity_url("http://127.0.0.1:8080/exfil").is_err());
        assert!(validate_antigravity_url("https://evil.example.com/steal").is_err());
        assert!(
            validate_antigravity_url("https://daily-cloudcode-pa.googleapis.com@evil.example.com/")
                .is_err()
        );
        assert!(validate_antigravity_url("https://googleapis.com/v1").is_err());
    }
}
