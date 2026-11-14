// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Core domain types, JSONL event log, typestate turn loop, and LLM provider trait

//! Core domain model and deterministic event-sourced loop for Rein.
#![forbid(unsafe_code)]

/// Append-only JSONL event log and session event definitions.
pub mod event;
/// Message, role, mode identifier, memory scope, and tool call structures.
pub mod message;
/// Model provider trait, Anthropic backend, and deterministic replay provider.
pub mod provider;
/// Compile-time typestate turn loop enforcing policy gating before tool execution.
pub mod state;

pub use event::{CoreError, EventLog, SessionEvent};
pub use message::{MemoryScope, Message, ModeId, Role, ToolCall, ToolOutput};
pub use provider::{
    ANTHROPIC_API_VERSION, AnthropicConfig, AnthropicProvider, Provider, ProviderStreamEvent,
    ReplayProvider, SseEventAccumulator, ToolSpec,
};
pub use state::{AwaitingPolicy, Executing, Idle, Streaming, Turn};
