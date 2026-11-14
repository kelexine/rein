// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Public exports for model providers, SSE parser, tool specs, and stream events

/// Anthropic Messages streaming provider configuration and backend.
pub mod anthropic;
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
pub use replay::ReplayProvider;
pub use sse::SseEventAccumulator;
pub use traits::{Provider, ProviderStreamEvent, ToolSpec};
