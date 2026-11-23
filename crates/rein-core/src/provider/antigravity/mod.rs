// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Antigravity provider module exports

mod codec;
mod models;
mod provider;

pub use provider::{
    AntigravityConfig, AntigravityProvider, DEFAULT_AICODE_BASE_URL,
    DEFAULT_ANTIGRAVITY_BASE_URL, DEFAULT_ANTIGRAVITY_MODEL, DEFAULT_ANTIGRAVITY_PROJECT,
    FALLBACK_ANTIGRAVITY_BASE_URL,
};
