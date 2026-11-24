// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Public exports for policy evaluation engine and rule sets

/// Policy decisions and profile rule sets.
pub mod decision;
/// Policy error types.
pub mod error;
/// Capability evaluator and audit dispatcher.
pub mod evaluator;
mod suggest;

pub use decision::{PolicyDecision, PolicyRuleSet};
pub use error::PolicyError;
pub use evaluator::PolicyEngine;
pub use suggest::suggest_command_allow_pattern;
