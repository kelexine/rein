// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-19
// Purpose: Asynchronous subagent lifecycle (`invoke_subagent`, `define_subagent`, `manage_subagents`, `send_message`)

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use rein_core::{EventLog, ModeId};
use rein_policy::{AuditLog, PolicyEngine, WorkspaceSandbox};
use rein_tools::{CallMcpTool, McpCatalog, TaskNotification, ToolError};

use super::tools::{InvokeSubagentTool, ManageSubagentsTool, SendMessageTool};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agent::runner::AgentLoop;
use crate::agent::snapshot::now_unix_secs;
use crate::agent::subagent::{
    AgentTool, LiveSubagentStatus, SubagentEnv, SubagentPrompter, SubagentRunState,
    build_child_registry_for_sandbox, build_subagent_profile, next_subagent_id,
    short_model_display,
};
use crate::agent::workspace_iso::{
    WorkspaceIsolation, cleanup_subagent_workspace, prepare_subagent_workspace,
};

const MAX_SUBAGENT_DEPTH: u8 = 2;

/// Custom subagent type registered dynamically via `define_subagent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentDefinition {
    /// Unique type name passed to `invoke_subagent`.
    pub name: String,
    /// Usage description for the agent.
    pub description: String,
    /// Role-specific system prompt appended inside child sessions.
    pub system_prompt: String,
    /// Allow write/bash tools when parent mode permits.
    pub enable_write_tools: bool,
    /// Allow `call_mcp_tool` when MCP catalog has servers.
    pub enable_mcp_tools: bool,
    /// Allow nested `invoke_subagent` / `manage_subagents` up to the maximum subagent nesting depth (`2`).
    pub enable_subagent_tools: bool,
}

pub(super) struct MailboxEntry {
    parent_id: Option<String>,
    children: Vec<String>,
    agent_type: String,
    role: String,
    workspace_mode: WorkspaceIsolation,
    workspace_path: PathBuf,
    isolated_dir: Option<PathBuf>,
    msg_tx: mpsc::Sender<String>,
    cancel_flag: Arc<AtomicBool>,
}

#[derive(Default)]
pub(super) struct ManagerInner {
    pub(super) definitions: BTreeMap<String, SubagentDefinition>,
    pub(super) handles: BTreeMap<String, MailboxEntry>,
}

/// Shared registry managing async subagents, custom definitions, mailboxes, and isolated workspaces.
#[derive(Clone)]
pub struct SubagentManager {
    pub(super) env: SubagentEnv,
    inbox_tx: Option<mpsc::Sender<TaskNotification>>,
    mcp_catalog: Option<McpCatalog>,
    pub(super) inner: Arc<Mutex<ManagerInner>>,
}

impl std::fmt::Debug for SubagentManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubagentManager").finish_non_exhaustive()
    }
}

/// Parameters for spawning an asynchronous subagent via [`SubagentManager::invoke`].
pub struct InvokeSubagentRequest<'a> {
    /// Built-in or custom subagent type name.
    pub type_name: &'a str,
    /// Short 2–5 word role title.
    pub role: &'a str,
    /// Initial task prompt.
    pub prompt: &'a str,
    /// Optional model override.
    pub model_override: Option<&'a str>,
    /// Workspace isolation mode (`inherit`, `share`, or `branch`).
    pub workspace_mode: WorkspaceIsolation,
    /// Parent subagent ID (`None` for root orchestrator spawns).
    pub parent_id: Option<&'a str>,
    /// Current nesting depth (`0` for root).
    pub depth: u8,
}

impl SubagentManager {
    /// Creates a [`SubagentManager`] wired to `env`, optional parent wakeup inbox, and optional [`McpCatalog`].
    #[must_use]
    pub fn new(
        env: SubagentEnv,
        inbox_tx: Option<mpsc::Sender<TaskNotification>>,
        mcp_catalog: Option<McpCatalog>,
    ) -> Self {
        Self {
            env,
            inbox_tx,
            mcp_catalog,
            inner: Arc::new(Mutex::new(ManagerInner::default())),
        }
    }

    /// Registers a reusable custom subagent definition.
    pub fn define(&self, def: SubagentDefinition) -> Result<(), String> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| "subagent manager lock poisoned".to_owned())?;
        guard.definitions.insert(def.name.clone(), def);
        Ok(())
    }

    /// Delivers `message` to subagent `recipient` (or to the parent session inbox when `recipient == "parent"`).
    pub fn send_message(
        &self,
        sender_label: &str,
        recipient: &str,
        message: &str,
    ) -> Result<String, String> {
        if recipient == "parent" || recipient == "coordinator" {
            let Some(tx) = &self.inbox_tx else {
                return Err("parent inbox channel is not attached".to_owned());
            };
            let _ = tx.send(TaskNotification::SubagentMessage {
                conversation_id: sender_label.to_owned(),
                role: sender_label.to_owned(),
                agent_type: "subagent".to_owned(),
                content: message.to_owned(),
                finished: false,
            });
            return Ok(format!("delivered message to parent from {sender_label}"));
        }

        let tx = {
            let guard = self
                .inner
                .lock()
                .map_err(|_| "subagent manager lock poisoned".to_owned())?;
            let entry = guard
                .handles
                .get(recipient)
                .ok_or_else(|| format!("unknown subagent conversationId '{recipient}'"))?;
            if entry.cancel_flag.load(Ordering::Acquire) {
                return Err(format!("subagent '{recipient}' was already killed"));
            }
            entry.msg_tx.clone()
        };
        tx.send(message.to_owned())
            .map_err(|_| format!("subagent '{recipient}' is no longer listening"))?;
        Ok(format!("delivered message to {recipient}"))
    }

    /// Returns all custom [`SubagentDefinition`] entries registered in this session.
    #[must_use]
    pub fn list_definitions(&self) -> Vec<SubagentDefinition> {
        self.inner
            .lock()
            .map(|g| g.definitions.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Lists all tracked async subagents and their current states.
    #[must_use]
    pub fn list(&self) -> Vec<Value> {
        let Ok(guard) = self.inner.lock() else {
            return Vec::new();
        };
        guard
            .handles
            .iter()
            .map(|(id, h)| {
                let state = self
                    .env
                    .tracker
                    .get(id)
                    .map_or("unknown", |s| s.run_state.label());
                json!({
                    "conversation_id": id,
                    "type": h.agent_type,
                    "role": h.role,
                    "state": state,
                    "workspace_mode": h.workspace_mode.as_str(),
                    "workspace_path": h.workspace_path,
                    "children": h.children,
                })
            })
            .collect()
    }

    /// Kills `id` and recursively kills all descendant subagents, removing isolated workspaces.
    pub fn kill_tree(&self, id: &str) -> Result<Vec<String>, String> {
        let mut killed = Vec::new();
        let to_clean = self.collect_subtree(id);
        if to_clean.is_empty() {
            return Err(format!("unknown subagent conversationId '{id}'"));
        }
        for (target_id, flag, isolated) in to_clean {
            flag.store(true, Ordering::Release);
            let _ = self.env.tracker.kill(&target_id);
            cleanup_subagent_workspace(isolated.as_deref());
            killed.push(target_id);
        }
        Ok(killed)
    }

    /// Kills every active subagent and cleans up all isolated workspaces.
    pub fn kill_all(&self) -> Vec<String> {
        let roots: Vec<String> = self
            .inner
            .lock()
            .map(|g| {
                g.handles
                    .iter()
                    .filter(|(_, h)| h.parent_id.is_none())
                    .map(|(id, _)| id.clone())
                    .collect()
            })
            .unwrap_or_default();
        let mut all_killed = Vec::new();
        for id in roots {
            if let Ok(mut list) = self.kill_tree(&id) {
                all_killed.append(&mut list);
            }
        }
        all_killed
    }

    fn collect_subtree(&self, root_id: &str) -> Vec<(String, Arc<AtomicBool>, Option<PathBuf>)> {
        let Ok(mut guard) = self.inner.lock() else {
            return Vec::new();
        };
        if !guard.handles.contains_key(root_id) {
            return Vec::new();
        }
        let mut stack = vec![root_id.to_owned()];
        let mut out = Vec::new();
        while let Some(cur) = stack.pop() {
            if let Some(entry) = guard.handles.get_mut(&cur) {
                stack.extend(entry.children.clone());
                let isolated = entry.isolated_dir.take();
                out.push((cur, Arc::clone(&entry.cancel_flag), isolated));
            }
        }
        out
    }

    /// Spawns a background subagent returning its `conversation_id` immediately.
    pub fn invoke(&self, req: InvokeSubagentRequest<'_>) -> Result<(String, PathBuf), ToolError> {
        let InvokeSubagentRequest {
            type_name,
            role,
            prompt,
            model_override,
            workspace_mode,
            parent_id,
            depth,
        } = req;
        if depth > MAX_SUBAGENT_DEPTH {
            return Err(ToolError::InvalidArguments {
                tool: "invoke_subagent".to_owned(),
                reason: format!("subagent recursion depth exceeded ({MAX_SUBAGENT_DEPTH})"),
            });
        }

        let state = self
            .env
            .session_state
            .read()
            .map(|s| s.clone())
            .unwrap_or_default();
        let custom_def = self
            .inner
            .lock()
            .ok()
            .and_then(|g| g.definitions.get(type_name).cloned());

        let (norm_type, target_mode) = if let Some(def) = &custom_def {
            let desired = if def.enable_write_tools {
                if state.active_mode == ModeId::Plan {
                    return Err(ToolError::InvalidArguments {
                        tool: "invoke_subagent".to_owned(),
                        reason: format!("Plan mode forbids write-capable subagent '{}'", def.name),
                    });
                }
                state.active_mode
            } else {
                ModeId::Plan
            };
            (def.name.clone(), desired)
        } else {
            let args = json!({ "type": type_name, "role": role, "prompt": prompt });
            let (t, _, m) = AgentTool::resolve_target(&args, state.active_mode)?;
            if state.active_mode == ModeId::Plan && m != ModeId::Plan {
                return Err(ToolError::InvalidArguments {
                    tool: "invoke_subagent".to_owned(),
                    reason: format!("Plan mode confines subagents to research/plan (got '{t}')"),
                });
            }
            (t, m)
        };

        if state.active_mode == ModeId::Plan && workspace_mode != WorkspaceIsolation::Inherit {
            return Err(ToolError::InvalidArguments {
                tool: "invoke_subagent".to_owned(),
                reason: "Plan mode only permits workspace='inherit'".to_owned(),
            });
        }

        let sub_id = next_subagent_id();
        let workspaces_root = self.env.sessions_dir.join("workspaces");
        let (effective_ws, isolated_dir) = prepare_subagent_workspace(
            self.env.sandbox.root(),
            &workspaces_root,
            &sub_id,
            workspace_mode,
        )?;
        let ws_sandbox = WorkspaceSandbox::new(&effective_ws);

        let (mut provider, model_label) = (self.env.provider_factory)(model_override);
        let _ = std::fs::create_dir_all(&self.env.sessions_dir);
        let log_path = self.env.sessions_dir.join(format!("{sub_id}.jsonl"));

        let mut profile =
            build_subagent_profile(target_mode, &norm_type, role, &state.allowed_commands);
        if target_mode == state.active_mode {
            profile.policy_rules = state.policy_rules.clone();
        }
        if let Some(def) = &custom_def {
            profile.system_prompt.push_str("\n\n");
            profile.system_prompt.push_str(&def.system_prompt);
            if !def.enable_write_tools {
                profile
                    .allowed_tools
                    .retain(|t| !matches!(t.as_str(), "write" | "edit" | "bash"));
            }
            if def.enable_mcp_tools && self.mcp_catalog.is_some() {
                profile.allowed_tools.push("call_mcp_tool".to_owned());
            }
            if def.enable_subagent_tools && depth < MAX_SUBAGENT_DEPTH {
                profile.allowed_tools.push("invoke_subagent".to_owned());
                profile.allowed_tools.push("manage_subagents".to_owned());
            }
        }
        profile.allowed_tools.push("send_message".to_owned());

        let policy = PolicyEngine::with_sandbox_status(
            profile.policy_rules.clone(),
            ws_sandbox.clone(),
            state.sandbox_status.clone(),
            AuditLog::new(&self.env.audit_log_path),
        )
        .with_bypass_permissions(state.bypass_permissions);

        let mut registry = build_child_registry_for_sandbox(&self.env, &ws_sandbox);
        registry.register(Box::new(SendMessageTool::with_sender(
            self.clone(),
            sub_id.clone(),
        )));
        if let Some(def) = &custom_def {
            if def.enable_mcp_tools
                && let Some(cat) = &self.mcp_catalog
            {
                registry.register(Box::new(CallMcpTool::new(cat.clone())));
            }
            if def.enable_subagent_tools && depth < MAX_SUBAGENT_DEPTH {
                registry.register(Box::new(InvokeSubagentTool::nested(
                    self.clone(),
                    sub_id.clone(),
                    depth + 1,
                )));
                registry.register(Box::new(ManageSubagentsTool::new(self.clone())));
            }
        }

        let mut child_agent = AgentLoop::new(
            EventLog::new(&log_path),
            policy,
            registry,
            self.env.memory.clone(),
            profile,
        )
        .with_allowed_commands(state.allowed_commands)
        .with_runtime_context(self.env.runtime_context.clone());

        let sub_cancel = Arc::new(AtomicBool::new(false));
        let (msg_tx, msg_rx) = mpsc::channel::<String>();

        self.env.tracker.start(LiveSubagentStatus {
            id: sub_id.clone(),
            agent_type: norm_type.clone(),
            role: role.to_owned(),
            model_label: short_model_display(&model_label),
            status: "Thinking…".to_owned(),
            run_state: SubagentRunState::Running,
            started_unix_secs: now_unix_secs(),
            started_hms: String::new(),
            started_full: String::new(),
            finished_unix_secs: None,
            output_lines: vec![format!("[prompt] {prompt}")],
            cancel_flag: Arc::clone(&sub_cancel),
        });

        if let Ok(mut guard) = self.inner.lock() {
            if let Some(pid) = parent_id
                && let Some(parent_entry) = guard.handles.get_mut(pid)
            {
                parent_entry.children.push(sub_id.clone());
            }
            guard.handles.insert(
                sub_id.clone(),
                MailboxEntry {
                    parent_id: parent_id.map(str::to_owned),
                    children: Vec::new(),
                    agent_type: norm_type.clone(),
                    role: role.to_owned(),
                    workspace_mode,
                    workspace_path: effective_ws.clone(),
                    isolated_dir,
                    msg_tx,
                    cancel_flag: Arc::clone(&sub_cancel),
                },
            );
        }

        let mgr = self.clone();
        let tid = sub_id.clone();
        let role_owned = role.to_owned();
        let type_owned = norm_type;
        let prompt_owned = prompt.to_owned();
        let parent_id_owned = parent_id.map(str::to_owned);
        std::thread::spawn(move || {
            let mut prompter = SubagentPrompter {
                id: tid.clone(),
                tracker: mgr.env.tracker.clone(),
                parent_cancel: Arc::new(AtomicBool::new(false)),
                sub_cancel: Arc::clone(&sub_cancel),
            };
            let initial_prompt = format!(
                "[Subagent Role: {role_owned} ({type_owned}) | ID: {tid}]\nTask:\n{prompt_owned}"
            );
            let mut next_input = Some(initial_prompt);

            while let Some(turn_prompt) = next_input.take() {
                if sub_cancel.load(Ordering::Acquire) {
                    break;
                }
                mgr.env.tracker.set_running(&tid, "Thinking…");
                match child_agent.run_user_prompt_cancellable(
                    &turn_prompt,
                    provider.as_mut(),
                    &mut prompter,
                    &sub_cancel,
                ) {
                    Ok(answer) => {
                        let trimmed = answer.trim().to_owned();
                        mgr.env.tracker.set_waiting(&tid, &trimmed);
                        mgr.notify_parent(
                            parent_id_owned.as_deref(),
                            &tid,
                            &role_owned,
                            &type_owned,
                            &trimmed,
                            false,
                        );
                    }
                    Err(err) => {
                        let msg = format!("sub-agent '{role_owned}' failed: {err}");
                        mgr.env.tracker.finish(&tid, SubagentRunState::Failed, &msg);
                        mgr.notify_parent(
                            parent_id_owned.as_deref(),
                            &tid,
                            &role_owned,
                            &type_owned,
                            &msg,
                            true,
                        );
                        return;
                    }
                }

                // Wait asleep on mailbox for follow-up `send_message` until killed.
                while !sub_cancel.load(Ordering::Acquire) {
                    match msg_rx.recv_timeout(Duration::from_millis(200)) {
                        Ok(msg) => {
                            mgr.env.tracker.append_log(&tid, format!("[message] {msg}"));
                            next_input = Some(msg);
                            break;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
            }
            if sub_cancel.load(Ordering::Acquire) {
                let _ = mgr.env.tracker.kill(&tid);
            }
        });

        Ok((sub_id, effective_ws))
    }

    fn notify_parent(
        &self,
        parent_id: Option<&str>,
        sub_id: &str,
        role: &str,
        agent_type: &str,
        content: &str,
        finished: bool,
    ) {
        if let Some(pid) = parent_id {
            let _ = self.send_message(sub_id, pid, content);
            return;
        }
        if let Some(tx) = &self.inbox_tx {
            let _ = tx.send(TaskNotification::SubagentMessage {
                conversation_id: sub_id.to_owned(),
                role: role.to_owned(),
                agent_type: agent_type.to_owned(),
                content: content.to_owned(),
                finished,
            });
        }
    }
}

/// Asynchronous tool spawning an isolated background subagent (`invoke_subagent`).
#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::RwLock;

    use rein_core::{CoreError, Message, Provider, ProviderStreamEvent, ToolSpec};
    use rein_policy::SandboxStatus;
    use rein_tools::{JsonlMemoryStore, TodoStore, Tool};

    use super::*;
    use crate::agent::subagent::{SubagentSessionState, SubagentTracker};
    use crate::modes::RuntimeContext;

    struct EchoMockProvider {
        replies: Mutex<VecDeque<String>>,
    }

    impl Provider for EchoMockProvider {
        fn name(&self) -> &str {
            "mock"
        }

        fn stream_turn_with(
            &mut self,
            _system: &str,
            messages: &[Message],
            _tools: &[ToolSpec],
            on_event: &mut dyn FnMut(&ProviderStreamEvent),
        ) -> Result<Vec<ProviderStreamEvent>, CoreError> {
            let reply = self
                .replies
                .lock()
                .ok()
                .and_then(|mut q| q.pop_front())
                .unwrap_or_else(|| {
                    format!(
                        "ack: {}",
                        messages.last().map_or("", |m| m.content.as_str())
                    )
                });
            let ev = ProviderStreamEvent::TextDelta(reply);
            on_event(&ev);
            Ok(vec![ev])
        }
    }

    #[test]
    fn async_subagent_wakes_parent_accepts_send_message_and_cleans_workspace()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("rein_sub_async_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let ws = root.join("ws");
        std::fs::create_dir_all(&ws)?;
        std::fs::write(ws.join("main.rs"), "fn main() {}\n")?;

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
            tracker: tracker.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
            provider_factory: Arc::new(|_| {
                let p = EchoMockProvider {
                    replies: Mutex::new(VecDeque::from([
                        "initial finding".to_owned(),
                        "follow-up reply".to_owned(),
                    ])),
                };
                (Box::new(p), "mock/test".to_owned())
            }),
        };

        let mgr = SubagentManager::new(env, Some(tx), None);
        let invoke = InvokeSubagentTool::new(mgr.clone());
        let send_msg = SendMessageTool::new(mgr.clone());
        let manage = ManageSubagentsTool::new(mgr.clone());

        let spawn_raw = invoke.execute(&json!({
            "type_name": "coder",
            "role": "Isolated Worker",
            "prompt": "Inspect main.rs",
            "workspace": "share"
        }))?;
        let spawn_val: Value = serde_json::from_str(&spawn_raw)?;
        let conv_id = spawn_val["conversation_id"]
            .as_str()
            .ok_or("missing conversation_id")?
            .to_owned();
        let iso_ws = PathBuf::from(
            spawn_val["workspace_path"]
                .as_str()
                .ok_or("missing workspace_path")?,
        );
        assert!(iso_ws.join("main.rs").exists());

        let ev1 = rx.recv_timeout(Duration::from_secs(3))?;
        assert!(matches!(
            ev1,
            TaskNotification::SubagentMessage {
                ref conversation_id,
                ref content,
                finished: false,
                ..
            } if conversation_id == &conv_id && content == "initial finding"
        ));

        send_msg.execute(&json!({
            "recipient": conv_id,
            "message": "check again"
        }))?;
        let ev2 = rx.recv_timeout(Duration::from_secs(3))?;
        assert!(matches!(
            ev2,
            TaskNotification::SubagentMessage {
                ref conversation_id,
                ref content,
                ..
            } if conversation_id == &conv_id && content == "follow-up reply"
        ));

        let kill_raw = manage.execute(&json!({
            "action": "kill",
            "conversation_ids": [conv_id]
        }))?;
        assert!(kill_raw.contains(&conv_id));
        assert!(!iso_ws.exists());

        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }
}
