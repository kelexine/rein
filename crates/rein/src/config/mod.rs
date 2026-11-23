// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Global (~/.rein) and repository (<workspace>/.rein) configuration, paths, and command allowlist exports

/// Loader and filesystem initializers for dual-scope Rein configurations.
pub mod loader;
/// Persistent per-provider model catalog cache.
pub mod models_cache;

pub use loader::{
    DreamConfig, LoadedReinState, ProviderKind, ReinConfig, ReinPaths, SessionSummary, UiSettings,
    append_allowed_command, apply_provider_switch, build_provider, ensure_dirs_and_load,
    ensure_dirs_and_load_in, generate_session_uid, is_valid_session_uid, list_sessions,
    provider_startup_notice, read_latest_session_uid, read_repo_override_keys,
    resolve_model_target, resolve_resume_session, save_global_settings, save_provider_selection,
    write_latest_session_uid,
};
pub use models_cache::{load_cached_models, save_cached_models};
