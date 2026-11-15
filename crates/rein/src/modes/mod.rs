// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Public exports for mode profiles, context strategy, and profile switching

/// Per-profile context window strategy and assembled turn context.
pub mod context;
/// Coding and personal profile definitions.
pub mod profile;
/// Composable system prompt builders and runtime environment formatting.
pub mod prompts;
/// Runtime mode switcher synchronizing policy rules and event logs.
pub mod switcher;

pub use context::{AssembledContext, ContextBuilder, ContextStrategy};
pub use profile::Profile;
pub use prompts::{RuntimeContext, utc_today_iso};
pub use switcher::ModeManager;
