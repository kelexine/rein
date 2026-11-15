// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Global (~/.rein) and repository (<workspace>/.rein) configuration, paths, and command allowlist exports

/// Loader and filesystem initializers for dual-scope Rein configurations.
pub mod loader;

pub use loader::{
    LoadedReinState, ProviderKind, ReinConfig, ReinPaths, append_allowed_command,
    apply_provider_switch, build_provider, ensure_dirs_and_load, ensure_dirs_and_load_in,
    provider_startup_notice, save_provider_selection,
};
