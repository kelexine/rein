// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Public exports for tool trait, schema, errors, and policy-gated registry

/// Tool registry and execution errors.
pub mod error;
/// Tool trait and JSON schema descriptor.
pub mod schema;
/// Policy-enforced tool registry store.
pub mod store;

pub use error::ToolError;
pub use schema::{Tool, ToolSchema};
pub use store::ToolRegistry;
