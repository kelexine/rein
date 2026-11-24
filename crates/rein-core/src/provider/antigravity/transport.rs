// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Antigravity OAuth token refresh, loadCodeAssist, HTTP fallback, and SSE streaming transport

use std::io::{BufRead, BufReader};

use serde_json::{Value, json};

use crate::event::CoreError;
use crate::message::Message;
use crate::provider::antigravity_auth::{refresh_access_token, resolve_token, save_token_file};
use crate::provider::{ProviderStreamEvent, ToolSpec};

use super::codec::validate_antigravity_url;
use super::models::{AicodeConfigParse, parse_aicode_entitlement, parse_aicode_fetch_config};
use super::provider::{
    ANTIGRAVITY_USER_AGENT, AntigravityProvider, DEFAULT_AICODE_BASE_URL,
    DEFAULT_ANTIGRAVITY_BASE_URL, DEFAULT_ANTIGRAVITY_PROJECT, FALLBACK_ANTIGRAVITY_BASE_URL,
    UreqResponse,
};

impl AntigravityProvider {
    pub(super) fn ensure_tokens(&mut self) -> Result<String, CoreError> {
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

    pub(super) fn ensure_project_id(&mut self, access_token: &str) -> String {
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
        if validate_antigravity_url(&url).is_ok()
            && let Ok(resp) = self
                .agent
                .post(&url)
                .header("Authorization", &format!("Bearer {access_token}"))
                .header("Content-Type", "application/json")
                .header("User-Agent", ANTIGRAVITY_USER_AGENT)
                .send(&payload.to_string())
            && resp.status().is_success()
            && let Ok(body) = resp.into_body().read_to_string()
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

    pub(super) fn resolve_aicode_base_url(&self) -> Option<String> {
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

    fn send_get(
        &self,
        url: &str,
        access_token: &str,
    ) -> Result<UreqResponse, (Option<u16>, bool, CoreError)> {
        let resp = self
            .agent
            .get(url)
            .header("Authorization", &format!("Bearer {access_token}"))
            .header("User-Agent", ANTIGRAVITY_USER_AGENT)
            .call()
            .map_err(classify_transport_error)?;
        check_response_status(resp)
    }

    fn get_with_refresh(
        &mut self,
        url: &str,
        access_token: &mut String,
    ) -> Result<UreqResponse, (Option<u16>, bool, CoreError)> {
        if let Err(err) = validate_antigravity_url(url) {
            return Err((None, false, err));
        }
        match self.send_get(url, access_token) {
            Ok(resp) => Ok(resp),
            Err((Some(401), _, _)) => {
                *access_token = self
                    .try_refresh_token()
                    .map_err(|e| (Some(401), false, e))?;
                self.send_get(url, access_token)
            }
            Err(err) => Err(err),
        }
    }

    fn post_stream_cancellable(
        &self,
        url: &str,
        access_token: &str,
        payload: &str,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<UreqResponse, (Option<u16>, bool, CoreError)> {
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            return Err((None, false, CoreError::Cancelled));
        }
        let agent = self.agent.clone();
        let url_owned = url.to_owned();
        let token_owned = access_token.to_owned();
        let payload_owned = payload.to_owned();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let res = agent
                .post(&url_owned)
                .header("Authorization", &format!("Bearer {token_owned}"))
                .header("Content-Type", "application/json")
                .header("User-Agent", ANTIGRAVITY_USER_AGENT)
                .send(&payload_owned);
            let _ = tx.send(res);
        });
        loop {
            if cancel.load(std::sync::atomic::Ordering::Acquire) {
                return Err((None, false, CoreError::Cancelled));
            }
            match rx.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(Ok(resp)) => return check_response_status(resp),
                Ok(Err(err)) => return Err(classify_transport_error(err)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err((
                        None,
                        false,
                        CoreError::Provider(
                            "Antigravity HTTP request worker terminated unexpectedly".to_owned(),
                        ),
                    ));
                }
            }
        }
    }

    pub(super) fn post_with_refresh(
        &mut self,
        url: &str,
        access_token: &mut String,
        payload: &str,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<UreqResponse, (Option<u16>, bool, CoreError)> {
        if let Err(err) = validate_antigravity_url(url) {
            return Err((None, false, err));
        }
        match self.post_stream_cancellable(url, access_token, payload, cancel) {
            Ok(resp) => Ok(resp),
            Err((Some(401), _, _)) => {
                *access_token = self
                    .try_refresh_token()
                    .map_err(|e| (Some(401), false, e))?;
                self.post_stream_cancellable(url, access_token, payload, cancel)
            }
            Err(err) => Err(err),
        }
    }

    fn fetch_aicode_entitlement(&mut self, aicode_base: &str, access_token: &mut String) {
        if self.aicode_entitlement.is_some() {
            return;
        }
        let url = format!("{aicode_base}/v1alpha:fetchLicenses");
        let Ok(resp) = self.get_with_refresh(&url, access_token) else {
            return;
        };
        let Ok(body) = resp.into_body().read_to_string() else {
            return;
        };
        let Ok(parsed) = serde_json::from_str::<Value>(&body) else {
            return;
        };
        if let Some(ent) = parse_aicode_entitlement(&parsed) {
            self.aicode_entitlement = Some(ent);
        }
    }

    pub(super) fn fetch_aicode_config(
        &mut self,
        access_token: &mut String,
    ) -> Option<AicodeConfigParse> {
        if self.aicode_scope_blocked {
            return None;
        }
        let aicode_base = self.resolve_aicode_base_url()?;
        let url = format!("{aicode_base}/v1alpha:fetchConfig");
        let resp = match self.get_with_refresh(&url, access_token) {
            Ok(r) => r,
            Err((_, scope_missing, _)) => {
                if scope_missing {
                    self.aicode_scope_blocked = true;
                }
                return None;
            }
        };
        let body = resp.into_body().read_to_string().ok()?;
        let parsed: Value = serde_json::from_str(&body).ok()?;
        self.fetch_aicode_entitlement(&aicode_base, access_token);
        Some(parse_aicode_fetch_config(&parsed))
    }

    pub(super) fn should_prefer_aicode(&self, wire_model: &str) -> bool {
        let trimmed = self.config.model.trim();
        !self.aicode_scope_blocked
            && (self.aicode_models.contains(trimmed)
                || self.aicode_models.contains(wire_model)
                || self.aicode_experiences.contains_key(trimmed))
    }
}

impl AntigravityProvider {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn stream_aicode_with_fallback(
        &mut self,
        ai_base: &str,
        project_id: &str,
        access_token: &mut String,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolSpec],
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<UreqResponse, CoreError> {
        match self.post_aicode_stream(
            ai_base,
            access_token,
            system_prompt,
            messages,
            tools,
            cancel,
        ) {
            Ok(resp) => Ok(resp),
            Err((status, scope_missing, err))
                if scope_missing || matches!(status, Some(400 | 404)) =>
            {
                let v1_payload = serde_json::to_string(&self.build_request_body(
                    project_id,
                    system_prompt,
                    messages,
                    tools,
                ))?;
                self.post_v1internal_with_fallback(access_token, &v1_payload, cancel)
                    .or(Err(err))
            }
            Err((_, _, err)) => Err(err),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn stream_v1internal_with_fallback(
        &mut self,
        aicode_base: Option<&str>,
        project_id: &str,
        access_token: &mut String,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolSpec],
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<UreqResponse, CoreError> {
        let v1_payload = serde_json::to_string(&self.build_request_body(
            project_id,
            system_prompt,
            messages,
            tools,
        ))?;
        match self.post_v1internal_with_fallback(access_token, &v1_payload, cancel) {
            Ok(resp) => Ok(resp),
            Err((Some(404), _, err)) if !self.aicode_scope_blocked => {
                let Some(ai_base) = aicode_base else {
                    return Err(err);
                };
                self.post_aicode_stream(
                    ai_base,
                    access_token,
                    system_prompt,
                    messages,
                    tools,
                    cancel,
                )
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
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<UreqResponse, (Option<u16>, bool, CoreError)> {
        if self.aicode_entitlement.is_none() {
            self.fetch_aicode_entitlement(ai_base, access_token);
        }
        let ai_payload =
            serde_json::to_string(&self.build_aicode_request_body(system_prompt, messages, tools))
                .map_err(|err| (None, false, CoreError::from(err)))?;
        let api_ver = if self.aicode_entitlement.is_some() {
            "v1alpha"
        } else {
            "v1beta"
        };
        let ai_url = format!("{ai_base}/{api_ver}:streamGenerateContent?alt=sse");
        let res = self.post_with_refresh(&ai_url, access_token, &ai_payload, cancel);
        if let Err((_, true, _)) = res {
            self.aicode_scope_blocked = true;
        }
        res
    }

    fn post_v1internal_with_fallback(
        &mut self,
        access_token: &mut String,
        payload: &str,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<UreqResponse, (Option<u16>, bool, CoreError)> {
        let base = self.config.base_url.trim_end_matches('/');
        let is_default_base = base == DEFAULT_ANTIGRAVITY_BASE_URL;
        let url = format!("{base}/v1internal:streamGenerateContent?alt=sse");
        match self.post_with_refresh(&url, access_token, payload, cancel) {
            Ok(resp) => Ok(resp),
            Err((Some(404), _, _)) if is_default_base => {
                let fallback_url = format!(
                    "{FALLBACK_ANTIGRAVITY_BASE_URL}/v1internal:streamGenerateContent?alt=sse"
                );
                self.post_with_refresh(&fallback_url, access_token, payload, cancel)
            }
            Err(other) => Err(other),
        }
    }

    pub(super) fn read_sse_events(
        &mut self,
        response: UreqResponse,
        cancel: &std::sync::atomic::AtomicBool,
        on_event: &mut dyn FnMut(&ProviderStreamEvent),
    ) -> Result<Vec<ProviderStreamEvent>, CoreError> {
        let reader = BufReader::new(response.into_body().into_reader());
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
                    let line = line_res.map_err(|err| {
                        CoreError::Provider(format!("Antigravity SSE read error: {err}"))
                    })?;
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
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        on_event(&ProviderStreamEvent::Done);
        emitted.push(ProviderStreamEvent::Done);
        Ok(emitted)
    }
}

fn check_response_status(
    resp: UreqResponse,
) -> Result<UreqResponse, (Option<u16>, bool, CoreError)> {
    let code = resp.status().as_u16();
    if (200..300).contains(&code) {
        return Ok(resp);
    }
    let body = resp.into_body().read_to_string().unwrap_or_default();
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
    Err((Some(code), scope_missing, core_err))
}

fn classify_transport_error(err: ureq::Error) -> (Option<u16>, bool, CoreError) {
    (
        None,
        false,
        CoreError::Provider(format!("Antigravity transport error: {err}")),
    )
}
