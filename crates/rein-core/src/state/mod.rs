// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Public exports for the compile-time typestate turn loop

/// Typestate marker structs.
pub mod markers;
/// Typestate turn machine transitions.
pub mod turn;

pub use markers::{AwaitingPolicy, Executing, Idle, Streaming};
pub use turn::Turn;
