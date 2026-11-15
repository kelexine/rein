// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-15
// Purpose: Automatic memory consolidation (AutoDream), vault tools, and Ebbinghaus salience decay

/// Ebbinghaus exponential salience decay for unconsolidated memory records.
pub mod decay;
/// Cheapest-first AutoDream scheduler, lock management, and 4-phase agentic consolidation loop.
pub mod dream;
/// Strictly scoped Markdown vault, transcript grep, and JSONL consolidation tools for AutoDream.
pub mod vault;

pub use decay::{DECAY_RATE, FLOOR_SALIENCE, decay_records, projected_salience};
pub use dream::{AutoDream, AutoDreamCoordinator, ConsolidationState, SESSION_SCAN_INTERVAL_SECS};
