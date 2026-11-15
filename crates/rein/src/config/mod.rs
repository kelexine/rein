// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Global (~/.rein) and repository (<workspace>/.rein) configuration, paths, and command allowlist exports

/// Loader and filesystem initializers for dual-scope Rein configurations.
pub mod loader;

pub use loader::{
    LoadedReinState, ProviderKind, ReinConfig, ReinPaths, SessionSummary, append_allowed_command,
    apply_provider_switch, build_provider, ensure_dirs_and_load, ensure_dirs_and_load_in,
    generate_session_uid, is_valid_session_uid, list_sessions, provider_startup_notice,
    read_latest_session_uid, resolve_model_target, resolve_resume_session, save_provider_selection,
    write_latest_session_uid,
};
