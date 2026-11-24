// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-19
// Purpose: Named swarm worker roster, shared JSONL message board, @mention wakeups, fork, and hot model swaps

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use rein_core::{EventLog, Message, ModeId};
use rein_policy::{AuditLog, PolicyEngine, WorkspaceSandbox};
use rein_tools::{CallMcpTool, McpCatalog, TaskNotification, ToolError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::tool::{SwarmPostTool, SwarmTool};
use crate::agent::runner::AgentLoop;
use crate::agent::snapshot::now_unix_secs;
use crate::agent::subagent::{
    LiveSubagentStatus, SubagentEnv, SubagentPrompter, SubagentRunState,
    build_child_registry_for_sandbox, build_subagent_profile, next_subagent_id,
    short_model_display,
};
use crate::agent::workspace_iso::{
    WorkspaceIsolation, cleanup_subagent_workspace, prepare_subagent_workspace,
};

const MAX_WORKER_NAME_LEN: usize = 24;

/// Lifecycle state of a named member in the swarm roster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwarmMemberState {
    /// Currently executing a turn.
    Working,
    /// Asleep on the shared board, awaiting an `@mention`.
    Waiting,
    /// Inactive / unloaded (revived on `@mention`).
    Stopped,
    /// Permanently terminated and workspace deleted.
    Killed,
}

impl SwarmMemberState {
    /// Lowercase status string exposed in `swarm(action="status")`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Waiting => "waiting",
            Self::Stopped => "stopped",
            Self::Killed => "killed",
        }
    }
}

/// A single entry appended to the shared swarm message board (`swarm-board.jsonl`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwarmBoardPost {
    /// Monotonic 1-based post ID.
    pub id: u64,
    /// Unix timestamp (seconds).
    pub timestamp_secs: u64,
    /// Sender (`"coordinator"` or worker roster name).
    pub author: String,
    /// Message text posted publicly to the board.
    pub content: String,
    /// Extracted `@name` mentions.
    pub mentions: Vec<String>,
}

enum SwarmWake {
    Message { from: String, content: String },
    ModelSwitch { new_model: String, reason: String },
}

struct WorkerRecord {
    name: String,
    tracker_id: String,
    state: SwarmMemberState,
    model_label: String,
    added_by: String,
    workspace_mode: WorkspaceIsolation,
    workspace_path: PathBuf,
    isolated_dir: Option<PathBuf>,
    cancel_flag: Arc<AtomicBool>,
    wake_tx: mpsc::Sender<SwarmWake>,
    history_snapshot: Arc<Mutex<Vec<Message>>>,
}

#[derive(Default)]
struct SwarmInner {
    title: Option<String>,
    board_path: PathBuf,
    next_post_id: u64,
    posts: Vec<SwarmBoardPost>,
    workers: BTreeMap<String, WorkerRecord>,
}

/// Parameters for `swarm(action="add")`.
pub struct SwarmAddRequest<'a> {
    /// Worker name (or base name when `n > 1` or `tasks` is non-empty).
    pub name: &'a str,
    /// Public assignment brief posted on the board as `@<name>`.
    pub brief: &'a str,
    /// Optional model override.
    pub model: Option<&'a str>,
    /// Optional existing member (or `"coordinator"`) to fork history from.
    pub fork: Option<&'a str>,
    /// Workspace mode (`inherit`, `share`, `branch`).
    pub workspace: WorkspaceIsolation,
    /// Optional homogeneous clone count (`<name>-1..n`).
    pub n: Option<usize>,
    /// Optional sharded slice targets (`<name>-1..len`).
    pub tasks: Vec<String>,
}

/// Shared coordinator-owned swarm roster and append-only message board.
#[derive(Clone)]
pub struct SwarmManager {
    env: SubagentEnv,
    inbox_tx: Option<mpsc::Sender<TaskNotification>>,
    mcp_catalog: Option<McpCatalog>,
    inner: Arc<Mutex<SwarmInner>>,
}

impl std::fmt::Debug for SwarmManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SwarmManager").finish_non_exhaustive()
    }
}

impl SwarmManager {
    /// Creates a [`SwarmManager`] persisting board posts to `sessions_dir/swarm-board.jsonl`.
    #[must_use]
    pub fn new(
        env: SubagentEnv,
        inbox_tx: Option<mpsc::Sender<TaskNotification>>,
        mcp_catalog: Option<McpCatalog>,
    ) -> Self {
        let board_path = env.sessions_dir.join("swarm-board.jsonl");
        let inner = SwarmInner {
            board_path,
            ..SwarmInner::default()
        };
        Self {
            env,
            inbox_tx,
            mcp_catalog,
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    /// Sets the UI/session title for this swarm (coordinator only).
    pub fn set_title(&self, title: &str) -> Result<String, String> {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Err("swarm title cannot be empty".to_owned());
        }
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| "swarm lock poisoned".to_owned())?;
        guard.title = Some(trimmed.to_owned());
        Ok(trimmed.to_owned())
    }

    /// Returns a JSON snapshot of the swarm title, worker roster, and recent board posts.
    #[must_use]
    pub fn status_json(&self) -> Value {
        let Ok(guard) = self.inner.lock() else {
            return json!({});
        };
        let members: Vec<Value> = guard
            .workers
            .values()
            .map(|w| {
                json!({
                    "name": w.name,
                    "state": w.state.as_str(),
                    "model": w.model_label,
                    "added_by": w.added_by,
                    "workspace_mode": w.workspace_mode.as_str(),
                    "workspace_path": w.workspace_path,
                })
            })
            .collect();
        let recent_posts: Vec<&SwarmBoardPost> = guard
            .posts
            .iter()
            .rev()
            .take(15)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        json!({
            "title": guard.title,
            "board_path": guard.board_path,
            "members": members,
            "recent_posts": recent_posts,
        })
    }

    /// Returns available models reported for swarm assignment.
    #[must_use]
    pub fn models_json(&self, all: bool) -> Value {
        let mut suggested = vec![
            "claude-sonnet-4-20250514",
            "claude-opus-4-20250514",
            "gemini-2.5-pro",
            "gemini-2.5-flash",
            "gemini-2.5-flash-lite",
        ];
        if all {
            suggested.extend([
                "claude-3-5-haiku-20241022",
                "gemini-3-pro",
                "gemini-3-flash",
            ]);
        }
        let (_, default_label) = (self.env.provider_factory)(None);
        json!({
            "default_model": default_label,
            "models": suggested,
        })
    }

    /// Spawns one or more named workers (`n` clones or sharded `tasks`) and posts their `@brief` on the board.
    pub fn add_workers(&self, caller: &str, req: SwarmAddRequest<'_>) -> Result<Value, ToolError> {
        let state = self
            .env
            .session_state
            .read()
            .map(|s| s.clone())
            .unwrap_or_default();
        if state.active_mode == ModeId::Plan && req.workspace != WorkspaceIsolation::Inherit {
            return Err(ToolError::InvalidArguments {
                tool: "swarm".to_owned(),
                reason: "Plan mode confines swarm workers to workspace='inherit'".to_owned(),
            });
        }

        let slices: Vec<(String, String)> = if !req.tasks.is_empty() {
            req.tasks
                .iter()
                .enumerate()
                .map(|(i, slice)| {
                    (
                        format!("{}-{}", req.name, i + 1),
                        format!("{}\n\nAssigned slice #{}: {}", req.brief, i + 1, slice),
                    )
                })
                .collect()
        } else if let Some(count) = req.n.filter(|&c| c > 1) {
            (1..=count)
                .map(|i| (format!("{}-{i}", req.name), req.brief.to_owned()))
                .collect()
        } else {
            vec![(req.name.to_owned(), req.brief.to_owned())]
        };

        for (worker_name, _) in &slices {
            validate_worker_name(worker_name)?;
        }

        let seed_history = self.resolve_fork_history(req.fork, &state.coordinator_history)?;
        let mut spawned = Vec::with_capacity(slices.len());
        for (worker_name, assignment) in slices {
            let entry = self.spawn_single_worker(
                caller,
                &worker_name,
                &assignment,
                req.model,
                req.workspace,
                seed_history.clone(),
                &state,
            )?;
            spawned.push(entry);
        }
        Ok(json!({ "added": spawned }))
    }

    /// Hot-switches a `waiting` worker's model while preserving its conversation history.
    pub fn switch_worker_model(
        &self,
        caller: &str,
        target_name: &str,
        new_model: &str,
        why: &str,
    ) -> Result<Value, String> {
        let wake_tx = {
            let guard = self
                .inner
                .lock()
                .map_err(|_| "swarm lock poisoned".to_owned())?;
            let w = guard
                .workers
                .get(target_name)
                .ok_or_else(|| format!("unknown swarm worker '{target_name}'"))?;
            if caller != "coordinator" && w.name != caller && w.added_by != caller {
                return Err(format!(
                    "worker '{caller}' may only change model for itself or helpers it added"
                ));
            }
            if w.state != SwarmMemberState::Waiting {
                return Err(format!(
                    "worker '{target_name}' is '{}' (must be 'waiting' to switch model cleanly)",
                    w.state.as_str()
                ));
            }
            w.wake_tx.clone()
        };

        wake_tx
            .send(SwarmWake::ModelSwitch {
                new_model: new_model.to_owned(),
                reason: why.to_owned(),
            })
            .map_err(|_| format!("worker '{target_name}' is not reachable"))?;
        Ok(json!({
            "worker": target_name,
            "model": new_model,
            "status": "switched",
        }))
    }

    /// Permanently terminates a worker and deletes its isolated workspace.
    pub fn kill_worker(&self, caller: &str, target_name: &str) -> Result<Value, String> {
        let (tracker_id, flag, isolated) = {
            let mut guard = self
                .inner
                .lock()
                .map_err(|_| "swarm lock poisoned".to_owned())?;
            let w = guard
                .workers
                .get_mut(target_name)
                .ok_or_else(|| format!("unknown swarm worker '{target_name}'"))?;
            if caller != "coordinator" && w.added_by != caller {
                return Err(format!("worker '{caller}' may only kill helpers it added"));
            }
            w.state = SwarmMemberState::Killed;
            w.cancel_flag.store(true, Ordering::Release);
            (
                w.tracker_id.clone(),
                Arc::clone(&w.cancel_flag),
                w.isolated_dir.take(),
            )
        };
        flag.store(true, Ordering::Release);
        let _ = self.env.tracker.kill(&tracker_id);
        cleanup_subagent_workspace(isolated.as_deref());
        Ok(json!({
            "killed": target_name,
            "state": "killed",
        }))
    }

    /// Terminates every active swarm worker and cleans up all isolated workspaces.
    pub fn kill_all(&self) {
        let names: Vec<String> = self
            .inner
            .lock()
            .map(|g| {
                g.workers
                    .iter()
                    .filter(|(_, w)| w.state != SwarmMemberState::Killed)
                    .map(|(n, _)| n.clone())
                    .collect()
            })
            .unwrap_or_default();
        for name in names {
            let _ = self.kill_worker("coordinator", &name);
        }
    }

    /// Appends a message to the shared board and wakes any `@mentioned` waiting workers / coordinator.
    pub fn post_board_message(
        &self,
        author: &str,
        content: &str,
    ) -> Result<SwarmBoardPost, String> {
        let mentions = extract_mentions(content);
        let (post, wake_targets) = {
            let mut guard = self
                .inner
                .lock()
                .map_err(|_| "swarm lock poisoned".to_owned())?;
            guard.next_post_id += 1;
            let post = SwarmBoardPost {
                id: guard.next_post_id,
                timestamp_secs: now_unix_secs(),
                author: author.to_owned(),
                content: content.to_owned(),
                mentions: mentions.clone(),
            };
            guard.posts.push(post.clone());
            append_board_post(&guard.board_path, &post);

            let mut targets = Vec::new();
            for m in &mentions {
                if m != author
                    && let Some(w) = guard.workers.get(m)
                    && w.state != SwarmMemberState::Killed
                {
                    targets.push(w.wake_tx.clone());
                }
            }
            (post, targets)
        };

        for tx in wake_targets {
            let _ = tx.send(SwarmWake::Message {
                from: author.to_owned(),
                content: content.to_owned(),
            });
        }
        if mentions.iter().any(|m| m == "coordinator")
            && author != "coordinator"
            && let Some(inbox) = &self.inbox_tx
        {
            let _ = inbox.send(TaskNotification::SubagentMessage {
                conversation_id: author.to_owned(),
                role: format!("swarm:{author}"),
                agent_type: "swarm".to_owned(),
                content: content.to_owned(),
                finished: false,
            });
        }
        Ok(post)
    }

    fn resolve_fork_history(
        &self,
        fork_from: Option<&str>,
        coord_history: &[Message],
    ) -> Result<Vec<Message>, ToolError> {
        let Some(source) = fork_from.map(str::trim).filter(|s| !s.is_empty()) else {
            return Ok(Vec::new());
        };
        if source == "coordinator" {
            return Ok(coord_history.to_vec());
        }
        let guard = self.inner.lock().map_err(|_| ToolError::InvalidArguments {
            tool: "swarm".to_owned(),
            reason: "swarm lock poisoned".to_owned(),
        })?;
        let w = guard
            .workers
            .get(source)
            .ok_or_else(|| ToolError::InvalidArguments {
                tool: "swarm".to_owned(),
                reason: format!("cannot fork unknown swarm member '{source}'"),
            })?;
        Ok(w.history_snapshot
            .lock()
            .map(|h| h.clone())
            .unwrap_or_default())
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_single_worker(
        &self,
        caller: &str,
        worker_name: &str,
        assignment: &str,
        model_override: Option<&str>,
        workspace_mode: WorkspaceIsolation,
        seed_history: Vec<Message>,
        parent_state: &crate::agent::subagent::SubagentSessionState,
    ) -> Result<Value, ToolError> {
        {
            let guard = self.inner.lock().map_err(|_| ToolError::InvalidArguments {
                tool: "swarm".to_owned(),
                reason: "swarm lock poisoned".to_owned(),
            })?;
            if let Some(existing) = guard.workers.get(worker_name)
                && existing.state != SwarmMemberState::Killed
            {
                return Err(ToolError::InvalidArguments {
                    tool: "swarm".to_owned(),
                    reason: format!("swarm worker '{worker_name}' already exists"),
                });
            }
        }

        let tracker_id = next_subagent_id();
        let workspaces_root = self.env.sessions_dir.join("swarm-workspaces");
        let (effective_ws, isolated_dir) = prepare_subagent_workspace(
            self.env.sandbox.root(),
            &workspaces_root,
            worker_name,
            workspace_mode,
        )?;
        let ws_sandbox = WorkspaceSandbox::new(&effective_ws);

        let target_mode = parent_state.active_mode;
        let (mut provider, initial_model_label) = (self.env.provider_factory)(model_override);
        let display_model = short_model_display(&initial_model_label);
        let _ = std::fs::create_dir_all(&self.env.sessions_dir);
        let log_path = self
            .env
            .sessions_dir
            .join(format!("swarm-{worker_name}.jsonl"));

        let mut profile = build_subagent_profile(
            target_mode,
            "swarm",
            worker_name,
            &parent_state.allowed_commands,
        );
        profile.policy_rules = parent_state.policy_rules.clone();
        profile.allowed_tools.push("swarm".to_owned());
        profile.allowed_tools.push("swarm_post".to_owned());
        if self.mcp_catalog.is_some() {
            profile.allowed_tools.push("call_mcp_tool".to_owned());
        }
        profile.system_prompt.push_str(&format!(
            "\n\n## Swarm Worker\n- Roster name: `{worker_name}` (caller: `{caller}`).\n- Post updates on the shared board using `swarm_post` and mention `@coordinator` or `@{caller}` when done or blocked."
        ));

        let policy = PolicyEngine::with_sandbox_status(
            profile.policy_rules.clone(),
            ws_sandbox.clone(),
            parent_state.sandbox_status.clone(),
            AuditLog::new(&self.env.audit_log_path),
        )
        .with_bypass_permissions(parent_state.bypass_permissions);

        let mut registry = build_child_registry_for_sandbox(&self.env, &ws_sandbox);
        registry.register(Box::new(SwarmTool::for_worker(
            self.clone(),
            worker_name.to_owned(),
        )));
        registry.register(Box::new(SwarmPostTool::new(
            self.clone(),
            worker_name.to_owned(),
        )));
        if let Some(cat) = &self.mcp_catalog {
            registry.register(Box::new(CallMcpTool::new(cat.clone())));
        }

        let mut child_agent = AgentLoop::new(
            EventLog::new(&log_path),
            policy,
            registry,
            self.env.memory.clone(),
            profile,
        )
        .with_seed_history(seed_history)
        .with_allowed_commands(parent_state.allowed_commands.clone())
        .with_runtime_context(self.env.runtime_context.clone());

        let cancel_flag = Arc::new(AtomicBool::new(false));
        let (wake_tx, wake_rx) = mpsc::channel::<SwarmWake>();
        let history_snapshot = Arc::new(Mutex::new(child_agent.history().to_vec()));

        self.env.tracker.start(LiveSubagentStatus {
            id: tracker_id.clone(),
            agent_type: "swarm".to_owned(),
            role: worker_name.to_owned(),
            model_label: display_model.clone(),
            status: "Working…".to_owned(),
            run_state: SubagentRunState::Running,
            started_unix_secs: now_unix_secs(),
            started_hms: String::new(),
            started_full: String::new(),
            finished_unix_secs: None,
            output_lines: vec![format!("[brief] {assignment}")],
            cancel_flag: Arc::clone(&cancel_flag),
        });

        // Public brief posted on board as @<worker_name> before registering so we don't self-wake twice.
        let _ = self.post_board_message(caller, &format!("@{worker_name} {assignment}"));

        if let Ok(mut guard) = self.inner.lock() {
            guard.workers.insert(
                worker_name.to_owned(),
                WorkerRecord {
                    name: worker_name.to_owned(),
                    tracker_id: tracker_id.clone(),
                    state: SwarmMemberState::Working,
                    model_label: display_model.clone(),
                    added_by: caller.to_owned(),
                    workspace_mode,
                    workspace_path: effective_ws.clone(),
                    isolated_dir,
                    cancel_flag: Arc::clone(&cancel_flag),
                    wake_tx,
                    history_snapshot: Arc::clone(&history_snapshot),
                },
            );
        }

        let mgr = self.clone();
        let wname = worker_name.to_owned();
        let tid = tracker_id;
        let caller_owned = caller.to_owned();
        let initial_prompt = format!("@{wname}\n{assignment}");
        std::thread::spawn(move || {
            let mut prompter = SubagentPrompter {
                id: tid.clone(),
                tracker: mgr.env.tracker.clone(),
                parent_cancel: Arc::new(AtomicBool::new(false)),
                sub_cancel: Arc::clone(&cancel_flag),
            };
            let mut next_prompt = Some(initial_prompt);

            while let Some(turn_prompt) = next_prompt.take() {
                if cancel_flag.load(Ordering::Acquire) {
                    break;
                }
                mgr.set_worker_state(&wname, SwarmMemberState::Working);
                mgr.env.tracker.set_running(&tid, "Working…");

                match child_agent.run_user_prompt_cancellable(
                    &turn_prompt,
                    provider.as_mut(),
                    &mut prompter,
                    &cancel_flag,
                ) {
                    Ok(answer) => {
                        if let Ok(mut hs) = history_snapshot.lock() {
                            *hs = child_agent.history().to_vec();
                        }
                        let reply = answer.trim().to_owned();
                        mgr.set_worker_state(&wname, SwarmMemberState::Waiting);
                        mgr.env.tracker.set_waiting(&tid, &reply);
                        let _ = mgr.post_board_message(&wname, &reply);
                        if !extract_mentions(&reply).iter().any(|m| m == "coordinator")
                            && caller_owned == "coordinator"
                            && let Some(inbox) = &mgr.inbox_tx
                        {
                            let _ = inbox.send(TaskNotification::SubagentMessage {
                                conversation_id: wname.clone(),
                                role: format!("swarm:{wname}"),
                                agent_type: "swarm".to_owned(),
                                content: reply,
                                finished: false,
                            });
                        }
                    }
                    Err(err) => {
                        let msg = format!("swarm worker '{wname}' failed: {err}");
                        mgr.set_worker_state(&wname, SwarmMemberState::Stopped);
                        mgr.env.tracker.finish(&tid, SubagentRunState::Failed, &msg);
                        if let Some(inbox) = &mgr.inbox_tx {
                            let _ = inbox.send(TaskNotification::SubagentMessage {
                                conversation_id: wname.clone(),
                                role: format!("swarm:{wname}"),
                                agent_type: "swarm".to_owned(),
                                content: msg,
                                finished: true,
                            });
                        }
                        return;
                    }
                }

                // Sleep in `waiting` state until @mentioned or model-switched or killed.
                while !cancel_flag.load(Ordering::Acquire) {
                    match wake_rx.recv_timeout(Duration::from_millis(200)) {
                        Ok(SwarmWake::Message { from, content }) => {
                            mgr.env
                                .tracker
                                .append_log(&tid, format!("[@{from}] {content}"));
                            next_prompt = Some(format!("[Board message from @{from}]\n{content}"));
                            break;
                        }
                        Ok(SwarmWake::ModelSwitch { new_model, reason }) => {
                            let (next_p, next_label) = (mgr.env.provider_factory)(Some(&new_model));
                            provider = next_p;
                            let short = short_model_display(&next_label);
                            mgr.update_worker_model(&wname, &short);
                            next_prompt = Some(format!(
                                "[System: Your model was switched to {new_model} ({reason}). Resume any unfinished work.]"
                            ));
                            break;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
            }
            if cancel_flag.load(Ordering::Acquire) {
                mgr.set_worker_state(&wname, SwarmMemberState::Killed);
                let _ = mgr.env.tracker.kill(&tid);
            }
        });

        Ok(json!({
            "name": worker_name,
            "state": "working",
            "model": display_model,
            "workspace_mode": workspace_mode.as_str(),
            "workspace_path": effective_ws,
        }))
    }

    fn set_worker_state(&self, name: &str, state: SwarmMemberState) {
        if let Ok(mut guard) = self.inner.lock()
            && let Some(w) = guard.workers.get_mut(name)
            && w.state != SwarmMemberState::Killed
        {
            w.state = state;
        }
    }

    fn update_worker_model(&self, name: &str, model_label: &str) {
        if let Ok(mut guard) = self.inner.lock()
            && let Some(w) = guard.workers.get_mut(name)
        {
            w.model_label = model_label.to_owned();
        }
    }
}

/// Validates swarm roster names (`^[a-z0-9][a-z0-9_-]{0,23}$`, excluding reserved names).
pub fn validate_worker_name(name: &str) -> Result<(), ToolError> {
    let bad = |reason: String| ToolError::InvalidArguments {
        tool: "swarm".to_owned(),
        reason,
    };
    if name.is_empty() || name.len() > MAX_WORKER_NAME_LEN {
        return Err(bad(format!(
            "worker name '{name}' must be 1..={MAX_WORKER_NAME_LEN} chars"
        )));
    }
    if name == "all" || name == "coordinator" || name.starts_with("coordinator-") {
        return Err(bad(format!("worker name '{name}' is reserved")));
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(bad("worker name cannot be empty".to_owned()));
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(bad(format!(
            "worker name '{name}' must start with a lowercase letter or digit"
        )));
    }
    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') {
        return Err(bad(format!(
            "worker name '{name}' contains disallowed characters (use a-z, 0-9, -, _)"
        )));
    }
    Ok(())
}

pub(super) fn extract_mentions(text: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for word in text.split_whitespace() {
        let trimmed = word
            .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '@' && c != '-' && c != '_');
        if let Some(raw) = trimmed.strip_prefix('@') {
            let clean = raw
                .trim_end_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
                .to_ascii_lowercase();
            if !clean.is_empty() && seen.insert(clean.clone()) {
                out.push(clean);
            }
        }
    }
    out
}

pub(super) fn append_board_post(path: &PathBuf, post: &SwarmBoardPost) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        && let Ok(line) = serde_json::to_string(post)
    {
        let _ = writeln!(file, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::RwLock;

    use rein_core::{CoreError, Provider, ProviderStreamEvent, ToolSpec};
    use rein_policy::SandboxStatus;
    use rein_tools::{JsonlMemoryStore, TodoStore, Tool};

    use super::super::tool::SwarmTool;
    use super::*;
    use crate::agent::subagent::{SubagentSessionState, SubagentTracker};
    use crate::modes::RuntimeContext;

    struct MockSwarmProvider {
        replies: Mutex<VecDeque<String>>,
    }

    impl Provider for MockSwarmProvider {
        fn name(&self) -> &str {
            "mock"
        }

        fn stream_turn_with(
            &mut self,
            _system: &str,
            _messages: &[Message],
            _tools: &[ToolSpec],
            on_event: &mut dyn FnMut(&ProviderStreamEvent),
        ) -> Result<Vec<ProviderStreamEvent>, CoreError> {
            let reply = self
                .replies
                .lock()
                .ok()
                .and_then(|mut q| q.pop_front())
                .unwrap_or_else(|| "done".to_owned());
            let ev = ProviderStreamEvent::TextDelta(reply);
            on_event(&ev);
            Ok(vec![ev])
        }
    }

    #[test]
    fn validates_worker_names_and_extracts_mentions() {
        assert!(validate_worker_name("auditor-1").is_ok());
        assert!(validate_worker_name("0_worker").is_ok());
        assert!(validate_worker_name("all").is_err());
        assert!(validate_worker_name("coordinator").is_err());
        assert!(validate_worker_name("coordinator-1").is_err());
        assert!(validate_worker_name("Upper").is_err());
        assert!(validate_worker_name("-bad").is_err());
        assert!(validate_worker_name("a".repeat(25).as_str()).is_err());

        assert_eq!(
            extract_mentions("Hey @alice-1, check with @bob and @coordinator!"),
            vec!["alice-1", "bob", "coordinator"]
        );
    }

    #[test]
    fn swarm_spawns_sharded_workers_wakes_via_mention_swaps_model_and_cleans_up()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("rein_swarm_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let ws = root.join("ws");
        std::fs::create_dir_all(&ws)?;
        std::fs::write(ws.join("lib.rs"), "pub fn x() {}\n")?;

        let tracker = SubagentTracker::new();
        let (tx, rx) = mpsc::channel();
        let env = SubagentEnv {
            sandbox: WorkspaceSandbox::new(&ws),
            audit_log_path: root.join("audit.jsonl"),
            sessions_dir: root.join("sessions"),
            bash_outputs_dir: root.join("bash-out"),
            notes_file: root.join("notes.jsonl"),
            calendar_file: root.join("cal.jsonl"),
            calendar_ics_dir: root.join("ics"),
            email_file: root.join("email.jsonl"),
            maildir_path: root.join("maildir"),
            memory: JsonlMemoryStore::dual(root.join("m1.jsonl"), root.join("m2.jsonl")),
            todo_store: TodoStore::for_session(root.join("todos"), "s1"),
            runtime_context: RuntimeContext::new(),
            session_state: Arc::new(RwLock::new(SubagentSessionState {
                active_mode: ModeId::Coding,
                sandbox_status: SandboxStatus::PolicyOnly,
                ..SubagentSessionState::default()
            })),
            tracker,
            cancel: Arc::new(AtomicBool::new(false)),
            provider_factory: Arc::new(|_| {
                let p = MockSwarmProvider {
                    replies: Mutex::new(VecDeque::from([
                        "shard complete".to_owned(),
                        "mention handled".to_owned(),
                        "resumed on new model".to_owned(),
                    ])),
                };
                (Box::new(p), "mock/swarm".to_owned())
            }),
        };

        let mgr = SwarmManager::new(env, Some(tx), None);
        let tool = SwarmTool::new(mgr.clone());

        tool.execute(&json!({ "action": "title", "title": "Audit Swarm" }))?;
        let added = tool.execute(&json!({
            "action": "add",
            "name": "worker",
            "brief": "Audit slice",
            "workspace": "share",
            "tasks": ["partA", "partB"]
        }))?;
        assert!(added.contains("worker-1"));
        assert!(added.contains("worker-2"));

        // Receive both initial turn completions.
        let _ = rx.recv_timeout(Duration::from_secs(3))?;
        let _ = rx.recv_timeout(Duration::from_secs(3))?;

        // Post @worker-1 mention to wake it from waiting.
        tool.execute(&json!({
            "action": "post",
            "message": "@worker-1 double-check partA"
        }))?;
        let wake_ev = rx.recv_timeout(Duration::from_secs(3))?;
        assert!(matches!(
            wake_ev,
            TaskNotification::SubagentMessage {
                ref conversation_id,
                ..
            } if conversation_id == "worker-1"
        ));

        // Hot-swap waiting worker's model cleanly.
        let swapped = tool.execute(&json!({
            "action": "model",
            "name": "worker-2",
            "model": "gemini-2.5-flash",
            "why": "quota"
        }))?;
        assert!(swapped.contains("switched"));
        let _ = rx.recv_timeout(Duration::from_secs(3))?;

        // Kill removes worker & isolated directory.
        let ws1 = root.join("sessions/swarm-workspaces/worker-1");
        assert!(ws1.exists());
        tool.execute(&json!({ "action": "kill", "name": "worker-1" }))?;
        assert!(!ws1.exists());

        mgr.kill_all();
        assert!(!root.join("sessions/swarm-workspaces/worker-2").exists());
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }
}
