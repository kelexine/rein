// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Public exports for model providers, SSE parser, tool specs, and stream events

/// Anthropic Messages streaming provider configuration and backend.
pub mod anthropic;
/// Google Antigravity Cloud Code PA streaming provider and model normalizer.
pub mod antigravity;
/// Google OAuth 2.0 loopback browser login, keyring lookup, and token refresh for Antigravity.
pub mod antigravity_auth;
/// Deterministic event-log replay provider.
pub mod replay;
/// Server-Sent Events (SSE) parser for Anthropic Messages streaming responses.
pub mod sse;
/// Core provider trait, tool specification, and streaming event definitions.
pub mod traits;

pub use anthropic::{
    ANTHROPIC_API_VERSION, AnthropicConfig, AnthropicProvider, DEFAULT_ANTHROPIC_ENDPOINT,
    DEFAULT_ANTHROPIC_MODEL, DEFAULT_MAX_TOKENS,
};
pub use antigravity::{
    AntigravityConfig, AntigravityProvider, DEFAULT_ANTIGRAVITY_BASE_URL,
    DEFAULT_ANTIGRAVITY_MODEL, DEFAULT_ANTIGRAVITY_PROJECT, FALLBACK_ANTIGRAVITY_BASE_URL,
};
pub use antigravity_auth::{
    AntigravityLoginResult, AntigravityTokenSet, default_rein_token_path, login_with_browser,
    login_with_browser_until, logout_rein_token, logout_rein_token_at, parse_token_json,
    resolve_token,
};
pub use replay::ReplayProvider;
pub use sse::SseEventAccumulator;
pub use traits::{AvailableModel, Provider, ProviderStreamEvent, ToolSpec, UsageStats};
