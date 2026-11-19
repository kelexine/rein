// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-15
// Purpose: Automatic AutoDream consolidation scheduler, lock guard, and 4-phase loop

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context, Result};
use rein_core::{MemoryScope, Message, Provider, ProviderStreamEvent, Role, ToolCall};
use rein_tools::{JsonlMemoryStore, now_unix_secs};
use serde::{Deserialize, Serialize};

use crate::config::{DreamConfig, ProviderKind, ReinConfig, ReinPaths, build_provider};
use crate::memory::decay::decay_records;
use crate::memory::vault::DreamVault;
use crate::modes::utc_today_iso;

/// Minimum interval (seconds) between idle directory scans when gates did not trigger.
pub const SESSION_SCAN_INTERVAL_SECS: u64 = 10 * 60;
const STALE_LOCK_SECS: u64 = 3_600;

/// Persisted AutoDream watermark written to `.consolidation_state.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsolidationState {
    /// Unix timestamp (seconds) of the last completed consolidation pass.
    pub last_consolidated_at: Option<u64>,
    /// Optional lock tag reserved for diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_etag: Option<String>,
}

/// Core AutoDream scheduler & consolidation runner following cheapest-first gating.
#[derive(Debug, Clone)]
pub struct AutoDream {
    config: DreamConfig,
    project_memory_dir: PathBuf,
    personal_memory_dir: PathBuf,
    sessions_dir: PathBuf,
    workspace_dir: PathBuf,
    lock_file: PathBuf,
    state_file: PathBuf,
}

impl AutoDream {
    /// Constructs an [`AutoDream`] scheduler for the given workspace paths and [`DreamConfig`].
    #[must_use]
    pub fn new(
        config: DreamConfig,
        project_memory_dir: PathBuf,
        personal_memory_dir: PathBuf,
        sessions_dir: PathBuf,
        workspace_dir: PathBuf,
    ) -> Self {
        let lock_file = project_memory_dir.join(".consolidation_lock");
        let state_file = project_memory_dir.join(".consolidation_state.json");
        Self {
            config,
            project_memory_dir,
            personal_memory_dir,
            sessions_dir,
            workspace_dir,
            lock_file,
            state_file,
        }
    }

    /// Evaluates gates cheapest-first: enabled -> time -> sessions -> lock.
    #[must_use]
    pub fn should_consolidate(&self, state: &ConsolidationState) -> bool {
        if !self.config.enabled {
            return false;
        }
        if !self.time_gate_passes(state) {
            return false;
        }
        if !self.session_gate_passes(state) {
            return false;
        }
        self.lock_gate_passes()
    }

    /// Checks whether elapsed hours since `last_consolidated_at` satisfy `min_hours`.
    #[must_use]
    pub fn time_gate_passes(&self, state: &ConsolidationState) -> bool {
        let Some(last) = state.last_consolidated_at else {
            return true;
        };
        let elapsed_secs = now_unix_secs().saturating_sub(last);
        elapsed_secs >= self.config.min_hours.saturating_mul(3_600)
    }

    fn session_gate_passes(&self, state: &ConsolidationState) -> bool {
        if self.config.min_sessions == 0 {
            return true;
        }
        if !self.sessions_dir.exists() {
            return false;
        }
        let last_secs = state.last_consolidated_at.unwrap_or(0);
        let Ok(entries) = std::fs::read_dir(&self.sessions_dir) else {
            return false;
        };
        let mut count = 0usize;
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_primary_session_log(&path) {
                continue;
            }
            if modified_secs(&path).is_some_and(|mtime| mtime > last_secs) {
                count += 1;
                if count >= self.config.min_sessions {
                    return true;
                }
            }
        }
        false
    }

    fn lock_gate_passes(&self) -> bool {
        if !self.lock_file.exists() {
            return true;
        }
        modified_secs(&self.lock_file)
            .is_none_or(|mtime| now_unix_secs().saturating_sub(mtime) > STALE_LOCK_SECS)
    }

    /// Loads persisted [`ConsolidationState`], returning `Default` when missing or unreadable.
    #[must_use]
    pub fn load_state(&self) -> ConsolidationState {
        std::fs::read_to_string(&self.state_file)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    /// Stamps `last_consolidated_at = now` and persists `.consolidation_state.json`.
    pub fn save_state(&self, state: &mut ConsolidationState) -> Result<()> {
        static STATE_TMP_SEQ: AtomicU64 = AtomicU64::new(0);
        state.last_consolidated_at = Some(now_unix_secs());
        if let Some(parent) = self.state_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(state)?;
        let seq = STATE_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let file_name = self
            .state_file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(".consolidation_state.json");
        let tmp = self
            .state_file
            .with_file_name(format!(".{file_name}.{pid}.{seq}.tmp"));
        std::fs::write(&tmp, format!("{json}\n"))?;
        std::fs::rename(&tmp, &self.state_file)?;
        Ok(())
    }

    /// Builds the 4-phase consolidation system prompt aligned with Rein's dual-scope vaults.
    #[must_use]
    pub fn consolidation_prompt(&self) -> String {
        let today = utc_today_iso();
        format!(
            "# Dream: Automatic Memory Consolidation\n\n\
             You are performing an automatic background dream — a reflective pass over Rein's \
             scoped memory vaults and recent session transcripts so future turns orient quickly.\n\n\
             - Project memory vault: `{proj_dir}` (JSONL + `MEMORY.md` + topic `.md` files)\n\
             - Personal memory vault: `{pers_dir}` (JSONL + `MEMORY.md` + topic `.md` files)\n\
             - Session transcripts: `{sessions}` (large JSONL files — use `grep_transcripts`, never read whole files)\n\
             - Workspace root (read-only docs): `{workspace}` (`GEMINI.md`, `CLAUDE.md`, `AGENTS.md`)\n\
             - Current Date (UTC): `{today}`\n\n\
             ## Phase 1 — Orient\n\
             - Call `list_vault` and `read_vault` (`MEMORY.md`) for both `project` and `personal` scopes.\n\
             - Call `memory_list` for both scopes and `read_project_doc` on `GEMINI.md` / `CLAUDE.md` / `AGENTS.md` if present.\n\n\
             ## Phase 2 — Gather recent signal\n\
             - Inspect drifted memories contradicting checked-in project docs or current workspace conventions.\n\
             - Use `grep_transcripts` narrowly only for topics you already suspect matter.\n\n\
             ## Phase 3 — Consolidate & Reconcile\n\
             - Merge near-duplicates into existing topic `.md` files and `memory_upsert` records (`project` vs `personal` kept strictly separated; never promote personal items into project scope automatically).\n\
             - Convert relative dates (\"yesterday\", \"last week\") to absolute `YYYY-MM-DD` dates using `{today}`.\n\
             - Reconcile against `GEMINI.md`/`CLAUDE.md`/`AGENTS.md`: delete stale procedural memories contradicted by checked-in docs, or annotate doc drift (`contradicts repo docs — verify which is current`) without editing repo files.\n\
             - Remove contradicted or obsolete JSONL entries with `memory_delete`.\n\n\
             ## Phase 4 — Prune and index\n\
             - Update each scope's `MEMORY.md` via `update_index` (strictly under 200 lines and 25KB; concise one-line links `- [Title](topic.md) — hook`).\n\
             - Delete obsolete topic files via `delete_vault`.\n\n\
             Return a concise summary of what was consolidated, updated, reconciled, or pruned (or note if nothing changed).",
            proj_dir = self.project_memory_dir.display(),
            pers_dir = self.personal_memory_dir.display(),
            sessions = self.sessions_dir.display(),
            workspace = self.workspace_dir.display(),
        )
    }

    /// Acquires `.consolidation_lock`, decays unconsolidated salience, runs the 4-phase loop, and stamps state.
    pub fn run_once(
        &self,
        provider: &mut dyn Provider,
        store: &JsonlMemoryStore,
        cancel: &AtomicBool,
    ) -> Result<String> {
        let _lock = LockGuard::acquire(&self.lock_file)?;
        apply_salience_decay(store)?;

        let vault = DreamVault::new(
            self.project_memory_dir.clone(),
            self.personal_memory_dir.clone(),
            self.sessions_dir.clone(),
            self.workspace_dir.clone(),
            store.clone(),
        );
        let summary = run_dream_turns(
            provider,
            &self.consolidation_prompt(),
            &vault,
            self.config.max_turns.max(1),
            cancel,
        )?;

        let mut state = self.load_state();
        self.save_state(&mut state)?;
        let _ = write_last_dream_summary(&self.project_memory_dir, &summary);
        Ok(summary)
    }
}

/// Non-blocking coordinator that checks gates automatically and spawns background AutoDream runs.
#[derive(Debug, Clone)]
pub struct AutoDreamCoordinator {
    in_flight: Arc<AtomicBool>,
    last_scan_secs: Arc<AtomicU64>,
}

impl Default for AutoDreamCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl AutoDreamCoordinator {
    /// Creates a new [`AutoDreamCoordinator`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            in_flight: Arc::new(AtomicBool::new(false)),
            last_scan_secs: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Checks scan throttle and cheapest-first AutoDream gates; spawns a silent background thread if ready.
    pub fn maybe_spawn(&self, config: &ReinConfig, paths: &ReinPaths, store: &JsonlMemoryStore) {
        if self.in_flight.load(Ordering::Relaxed) {
            return;
        }
        let dream_cfg = config.dream.clone().unwrap_or_default();
        if !dream_cfg.enabled {
            return;
        }
        let now = now_unix_secs();
        let prev = self.last_scan_secs.load(Ordering::Relaxed);
        if prev != 0 && now.saturating_sub(prev) < SESSION_SCAN_INTERVAL_SECS {
            return;
        }
        self.last_scan_secs.store(now, Ordering::Relaxed);

        let dream = AutoDream::new(
            dream_cfg,
            paths.repo_memory_dir.clone(),
            paths.global_memory_dir.clone(),
            paths.sessions_dir.clone(),
            paths.workspace_dir.clone(),
        );
        let state = dream.load_state();
        if !dream.should_consolidate(&state) {
            return;
        }
        if !provider_has_credentials(config.resolved_provider(), config) {
            return;
        }
        if self
            .in_flight
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }

        let cfg_clone = config.clone();
        let store_clone = store.clone();
        let flag = Arc::clone(&self.in_flight);
        std::thread::spawn(move || {
            let (mut provider, _) = build_provider(cfg_clone.resolved_provider(), &cfg_clone);
            let cancel = AtomicBool::new(false);
            let _ = dream.run_once(provider.as_mut(), &store_clone, &cancel);
            flag.store(false, Ordering::SeqCst);
        });
    }
}

struct LockGuard {
    path: PathBuf,
}

impl LockGuard {
    fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, format!("{}\n", now_unix_secs()))
            .with_context(|| format!("failed to acquire lock '{}'", path.display()))?;
        Ok(Self {
            path: path.to_path_buf(),
        })
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn apply_salience_decay(store: &JsonlMemoryStore) -> Result<()> {
    let now = now_unix_secs();
    for scope in [MemoryScope::Project, MemoryScope::Personal] {
        let current = store.query(scope, "")?;
        if current.is_empty() {
            continue;
        }
        let decayed = decay_records(&current, now);
        store.replace_scope(scope, &decayed)?;
    }
    Ok(())
}

fn run_dream_turns(
    provider: &mut dyn Provider,
    system_prompt: &str,
    vault: &DreamVault,
    max_turns: usize,
    cancel: &AtomicBool,
) -> Result<String> {
    let specs = DreamVault::tool_specs();
    let mut history = vec![Message::new(
        Role::User,
        "Run the 4-phase memory consolidation pass over Rein's project and personal vaults.",
    )];

    for _turn in 0..max_turns {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let events = provider.stream_turn_cancellable(
            system_prompt,
            &history,
            &specs,
            cancel,
            &mut |_| {},
        )?;
        let mut text = String::new();
        let mut calls: Vec<ToolCall> = Vec::new();
        for ev in events {
            match ev {
                ProviderStreamEvent::TextDelta(delta) => text.push_str(&delta),
                ProviderStreamEvent::ToolUse(call) => calls.push(call),
                ProviderStreamEvent::ThinkingDelta(_)
                | ProviderStreamEvent::Usage(_)
                | ProviderStreamEvent::Done => {}
            }
        }
        if calls.is_empty() {
            return Ok(text);
        }
        let mut assistant_msg = Message::new(Role::Assistant, text);
        assistant_msg.tool_calls = calls.clone();
        history.push(assistant_msg);
        for call in calls {
            let (out, _) = vault.execute(&call);
            let mut tool_msg = Message::new(Role::Tool, out);
            tool_msg.tool_calls = vec![call];
            history.push(tool_msg);
        }
    }
    Ok("AutoDream reached turn budget and saved consolidated state.".to_owned())
}

fn is_primary_session_log(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    name.ends_with(".jsonl") && !name.ends_with(".full.jsonl")
}

fn modified_secs(path: &Path) -> Option<u64> {
    let mtime = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(
        mtime
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs(),
    )
}

fn write_last_dream_summary(project_memory_dir: &Path, summary: &str) -> Result<()> {
    let path = project_memory_dir.join(".last_dream.md");
    std::fs::write(path, format!("{summary}\n"))?;
    Ok(())
}

fn provider_has_credentials(kind: ProviderKind, config: &ReinConfig) -> bool {
    match kind {
        ProviderKind::Anthropic => {
            let cfg = config.to_anthropic_config();
            std::env::var_os(&cfg.api_key_env).is_some()
        }
        ProviderKind::Antigravity => {
            let cfg = config.to_antigravity_config();
            rein_core::resolve_token(cfg.token_path.as_deref()).is_ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rein_core::CoreError;
    use rein_tools::MemoryRecord;
    use serde_json::json;

    struct ScriptedDreamProvider {
        step: usize,
    }

    impl Provider for ScriptedDreamProvider {
        fn name(&self) -> &str {
            "scripted-dream"
        }

        fn stream_turn_with(
            &mut self,
            _system_prompt: &str,
            _messages: &[Message],
            _tools: &[rein_core::ToolSpec],
            _on_event: &mut dyn FnMut(&ProviderStreamEvent),
        ) -> Result<Vec<ProviderStreamEvent>, CoreError> {
            self.step += 1;
            if self.step == 1 {
                Ok(vec![
                    ProviderStreamEvent::ToolUse(ToolCall {
                        id: "d1".to_owned(),
                        name: "update_index".to_owned(),
                        arguments: json!({"scope": "project", "content": "- [Build](build.md) — cargo test"}),
                    }),
                    ProviderStreamEvent::ToolUse(ToolCall {
                        id: "d2".to_owned(),
                        name: "memory_upsert".to_owned(),
                        arguments: json!({"scope": "project", "key": "build", "value": "cargo test --workspace", "kind": "project"}),
                    }),
                    ProviderStreamEvent::Done,
                ])
            } else {
                Ok(vec![
                    ProviderStreamEvent::TextDelta("Consolidated 1 project memory.".to_owned()),
                    ProviderStreamEvent::Done,
                ])
            }
        }
    }

    #[test]
    fn gates_enforce_time_sessions_and_lock() -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("rein_autodream_gates_test");
        let _ = std::fs::remove_dir_all(&dir);
        let proj_mem = dir.join("proj_mem");
        let pers_mem = dir.join("pers_mem");
        let sessions = dir.join("sessions");
        std::fs::create_dir_all(&proj_mem)?;
        std::fs::create_dir_all(&sessions)?;

        let dream = AutoDream::new(
            DreamConfig {
                enabled: true,
                min_hours: 24,
                min_sessions: 2,
                max_turns: 4,
            },
            proj_mem.clone(),
            pers_mem,
            sessions.clone(),
            dir.clone(),
        );

        let mut state = ConsolidationState::default();
        assert!(dream.time_gate_passes(&state));
        assert!(!dream.should_consolidate(&state));

        std::fs::write(sessions.join("s1.jsonl"), "{}\n")?;
        std::fs::write(sessions.join("s1.full.jsonl"), "{}\n")?;
        assert!(!dream.should_consolidate(&state));

        std::fs::write(sessions.join("s2.jsonl"), "{}\n")?;
        assert!(dream.should_consolidate(&state));

        std::fs::write(proj_mem.join(".consolidation_lock"), "999")?;
        assert!(!dream.should_consolidate(&state));
        std::fs::remove_file(proj_mem.join(".consolidation_lock"))?;

        dream.save_state(&mut state)?;
        assert!(!dream.time_gate_passes(&state));

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn run_once_updates_index_consolidates_store_and_releases_lock()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("rein_autodream_run_test");
        let _ = std::fs::remove_dir_all(&dir);
        let proj_mem = dir.join("proj_mem");
        let pers_mem = dir.join("pers_mem");
        let sessions = dir.join("sessions");
        std::fs::create_dir_all(&proj_mem)?;
        std::fs::create_dir_all(&pers_mem)?;
        std::fs::create_dir_all(&sessions)?;

        let store = JsonlMemoryStore::dual(dir.join("proj.jsonl"), dir.join("pers.jsonl"));
        store.append(&MemoryRecord::new(MemoryScope::Project, "build", "old"))?;

        let dream = AutoDream::new(
            DreamConfig::default(),
            proj_mem.clone(),
            pers_mem,
            sessions,
            dir.clone(),
        );
        let mut provider = ScriptedDreamProvider { step: 0 };
        let cancel = AtomicBool::new(false);
        let summary = dream.run_once(&mut provider, &store, &cancel)?;

        assert!(summary.contains("Consolidated 1"));
        assert!(!proj_mem.join(".consolidation_lock").exists());
        assert!(dream.load_state().last_consolidated_at.is_some());
        assert!(std::fs::read_to_string(proj_mem.join("MEMORY.md"))?.contains("cargo test"));
        let records = store.query(MemoryScope::Project, "build")?;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].value, "cargo test --workspace");
        assert!(records[0].consolidated);

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
