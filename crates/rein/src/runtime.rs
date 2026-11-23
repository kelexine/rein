// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Shared Rein session runtime bootstrap reused by both CLI/TUI and Iced GUI frontends

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock, mpsc};

use anyhow::{Context, Result};
use rein_core::{AvailableModel, EventLog, Provider};
use rein_policy::{AuditLog, PolicyEngine, SandboxStatus, WorkspaceSandbox};
use rein_tools::{
    AskQuestionTool, BashTool, CalendarTool, CallMcpTool, EditTool, EmailTool, GlobTool, GrepTool,
    JsonlMemoryStore, ManageTaskTool, McpCatalog, McpClient, MemoryTool, NotesTool, ReadTool,
    ScheduleTool, SwitchModeTool, TaskNotification, TaskRegistry, TodoReadTool, TodoStore,
    TodoWriteTool, ToolError, ToolRegistry, WebFetchTool, WebSearchTool, WriteTool,
};

use crate::agent::{
    AgentLoop, AgentTool, DefineSubagentTool, InvokeSubagentTool, ManageSubagentsTool,
    SendMessageTool, SubagentEnv, SubagentManager, SubagentSessionState, SubagentTracker,
    SwarmManager, SwarmTool,
};
use crate::commands::{CommandSource, SlashCommand, SlashKind, SlashRegistry};
use crate::config::{
    LoadedReinState, ProviderKind, apply_provider_switch, build_provider, ensure_dirs_and_load,
    generate_session_uid, is_valid_session_uid, load_cached_models, provider_startup_notice,
    read_repo_override_keys, resolve_resume_session, write_latest_session_uid,
};
use crate::modes::{Profile, RuntimeContext, utc_today_iso};
use crate::plugins::PluginManager;
use crate::ui::app::SharedQuestionRelay;

/// Session resume target parsed from CLI flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeSpec {
    /// Start a fresh session UID.
    NewSession,
    /// Resume an existing session by UID or unique prefix.
    Uid(String),
}

/// Fully wired Rein session runtime shared by CLI/TUI and GUI frontends.
pub struct ReinSession {
    /// Loaded global and workspace config + filesystem paths.
    pub loaded: LoadedReinState,
    /// Active event-sourced agent loop.
    pub agent: AgentLoop,
    /// Active LLM streaming provider.
    pub provider: Box<dyn Provider>,
    /// Human-readable active provider/model label.
    pub model_label: String,
    /// Active provider kind.
    pub provider_kind: ProviderKind,
    /// Active sandbox status label.
    pub sandbox_label: String,
    /// Collapsed workspace root label.
    pub workspace_label: String,
    /// Active session UID.
    pub session_uid: String,
    /// Whether this session was resumed from an existing log.
    pub should_hydrate: bool,
    /// Non-fatal startup notices (bypass warning, missing creds, MCP warnings).
    pub notices: Vec<String>,
    /// Slash command & skill registry.
    pub slash_registry: SlashRegistry,
    /// Installed plugin manager.
    pub plugin_manager: PluginManager,
    /// Session-scoped todo store.
    pub todo_store: TodoStore,
    /// Background task/cron/timer registry.
    pub task_registry: TaskRegistry,
    /// Inbox receiver for background task/subagent wakeups.
    pub task_rx: mpsc::Receiver<TaskNotification>,
    /// Synchronous subagent tracker.
    pub subagent_tracker: SubagentTracker,
    /// Asynchronous subagent manager.
    pub subagent_manager: SubagentManager,
    /// Collaborative multi-agent swarm manager.
    pub swarm_manager: SwarmManager,
    /// Relay channel for interactive `ask_question` UI prompts.
    pub question_relay: SharedQuestionRelay,
    /// Keys overridden in repository `.rein/config.json`.
    pub repo_overrides: Vec<String>,
    /// Disk-cached model list for the active provider, loaded at startup.
    pub cached_models: Vec<AvailableModel>,
}

impl ReinSession {
    /// Bootstraps a complete [`ReinSession`] for `workspace_root` using CLI `args`.
    pub fn bootstrap(workspace_root: &Path, args: &[String]) -> Result<Self> {
        rein_policy::harden_process_dumpable();
        let mut loaded = ensure_dirs_and_load(workspace_root)?;

        if let Some(cli_provider) = parse_flag_value(args, "--provider") {
            let kind = ProviderKind::parse(cli_provider).with_context(|| {
                format!("unknown provider '{cli_provider}' (use anthropic or antigravity)")
            })?;
            apply_provider_switch(&mut loaded.config, kind, None);
        }
        if let Some(cli_model) = parse_flag_value(args, "--model") {
            loaded.config.model = Some(cli_model.to_owned());
        }

        let resume_spec = parse_resume_flag(args)?;
        Self::from_loaded(
            workspace_root,
            loaded,
            has_bypass_permissions_flag(args),
            resume_spec,
        )
    }

    /// Builds a [`ReinSession`] from an already-loaded config state and explicit options.
    pub fn from_loaded(
        workspace_root: &Path,
        loaded: LoadedReinState,
        bypass_permissions: bool,
        resume_spec: ResumeSpec,
    ) -> Result<Self> {
        let sandbox = WorkspaceSandbox::new(workspace_root);
        let memory = JsonlMemoryStore::dual(&loaded.paths.repo_memory, &loaded.paths.global_memory);
        let todo_store = TodoStore::for_session(&loaded.paths.todos_dir, "replay");
        let task_registry = TaskRegistry::new();
        let (task_tx, task_rx) = mpsc::channel::<TaskNotification>();
        task_registry.set_inbox_tx(task_tx.clone());
        let mcp_catalog = McpCatalog::new(loaded.paths.sessions_dir.join("mcp-schemas"));
        let (mut registry, mcp_tools, mcp_warnings) = build_registry(
            &loaded,
            &sandbox,
            memory.clone(),
            todo_store.clone(),
            task_registry.clone(),
            mcp_catalog.clone(),
        );

        let initial_profile = Profile::coding()
            .with_allowed_commands(loaded.allowed_commands.clone())
            .with_extra_tools(&mcp_tools);
        let policy = PolicyEngine::new(
            initial_profile.policy_rules.clone(),
            sandbox,
            AuditLog::new(&loaded.paths.audit_log),
        )
        .with_bypass_permissions(bypass_permissions);

        let sandbox_label = format_sandbox_label(policy.sandbox_status());
        let active_kind = loaded.config.resolved_provider();
        let (provider, model_label) = build_provider(active_kind, &loaded.config);
        let mut notices = Vec::new();
        if policy.is_bypass_permissions() {
            notices.push(
                "auto-approve enabled (--dangerously-bypass-permission / --yolo): all policy checks and tool permissions bypassed".to_owned(),
            );
        }
        if let Some(notice) = provider_startup_notice(active_kind, &loaded.config) {
            notices.push(notice);
        }
        notices.extend(mcp_warnings);

        let (session_uid, event_log_path, full_transcript_path, should_hydrate) = match resume_spec
        {
            ResumeSpec::NewSession => {
                let uid = generate_session_uid(&loaded.paths);
                let (log_path, full_path) = loaded.paths.session_paths_for(&uid);
                (uid, log_path, full_path, false)
            }
            ResumeSpec::Uid(uid_or_prefix) => {
                let (summary, _) = resolve_resume_session(&loaded.paths, Some(&uid_or_prefix))?;
                (
                    summary.uid,
                    summary.event_log,
                    summary.full_transcript,
                    true,
                )
            }
        };
        if is_valid_session_uid(&session_uid) {
            let _ = write_latest_session_uid(&loaded.paths, &session_uid);
        }
        todo_store.set_session_uid(&session_uid);

        let plugin_manager = PluginManager::new(loaded.paths.global_dir.join("plugins"));
        let plugin_bundle = plugin_manager.collect_enabled_components();

        let mut slash_registry = crate::commands::load_slash_registry(&loaded.paths)?;
        for cmd in plugin_bundle.commands {
            slash_registry.register(SlashCommand {
                name: cmd.name,
                description: cmd.description,
                source: CommandSource::Plugin,
                kind: SlashKind::PluginCommand {
                    template: cmd.template,
                },
            });
        }
        let mut skill_names: Vec<String> = slash_registry
            .all()
            .iter()
            .filter(|entry| matches!(entry.kind, SlashKind::Skill { .. }))
            .map(|entry| entry.name.clone())
            .collect();
        for sk in plugin_bundle.skills {
            skill_names.push(sk.name);
        }

        let workspace_label = collapse_home_path(workspace_root);
        let runtime_context = RuntimeContext::new()
            .with_workspace_label(&workspace_label)
            .with_sandbox_label(&sandbox_label)
            .with_current_date(utc_today_iso())
            .with_skill_names(skill_names)
            .with_memory_dirs(
                &loaded.paths.repo_memory_dir,
                &loaded.paths.global_memory_dir,
            )
            .with_mcp_catalog(mcp_catalog.summaries(), mcp_catalog.schemas_root());

        let subagent_tracker = SubagentTracker::new();
        let subagent_state = Arc::new(RwLock::new(SubagentSessionState::default()));
        let subagent_cancel = Arc::new(AtomicBool::new(false));
        let subagent_env = SubagentEnv {
            sandbox: policy.sandbox().clone(),
            audit_log_path: loaded.paths.audit_log.clone(),
            sessions_dir: loaded.paths.sessions_dir.clone(),
            bash_outputs_dir: loaded.paths.repo_dir.join("bash-outputs"),
            notes_file: loaded.paths.notes.clone(),
            calendar_file: loaded.paths.calendar.clone(),
            calendar_ics_dir: loaded
                .config
                .ics_dir
                .clone()
                .unwrap_or_else(|| loaded.paths.global_dir.join("calendar")),
            email_file: loaded.paths.email.clone(),
            maildir_path: loaded
                .config
                .maildir
                .clone()
                .unwrap_or_else(|| loaded.paths.global_dir.join("maildir")),
            memory: memory.clone(),
            todo_store: todo_store.clone(),
            runtime_context: runtime_context.clone(),
            session_state: Arc::clone(&subagent_state),
            tracker: subagent_tracker.clone(),
            cancel: Arc::clone(&subagent_cancel),
            provider_factory: SubagentEnv::config_provider_factory(loaded.config.clone()),
        };
        let mcp_catalog_opt = mcp_catalog.has_servers().then_some(mcp_catalog);
        let subagent_manager = SubagentManager::new(
            subagent_env.clone(),
            Some(task_tx.clone()),
            mcp_catalog_opt.clone(),
        );
        let swarm_manager = SwarmManager::new(subagent_env.clone(), Some(task_tx), mcp_catalog_opt);
        registry.register(Box::new(AgentTool::new(subagent_env)));
        registry.register(Box::new(InvokeSubagentTool::new(subagent_manager.clone())));
        registry.register(Box::new(DefineSubagentTool::new(subagent_manager.clone())));
        registry.register(Box::new(ManageSubagentsTool::new(subagent_manager.clone())));
        registry.register(Box::new(SendMessageTool::new(subagent_manager.clone())));
        registry.register(Box::new(SwarmTool::new(swarm_manager.clone())));

        let question_relay: SharedQuestionRelay = Arc::new(Mutex::new(None));
        let relay_for_tool = Arc::clone(&question_relay);
        registry.register(Box::new(AskQuestionTool::new(Arc::new(move |questions| {
            let sender = relay_for_tool
                .lock()
                .ok()
                .and_then(|g| g.clone())
                .ok_or_else(|| ToolError::InvalidArguments {
                    tool: "ask_question".to_owned(),
                    reason: "interactive question UI is unavailable in headless mode".to_owned(),
                })?;
            let (reply_tx, reply_rx) = mpsc::channel();
            sender
                .send((questions, reply_tx))
                .map_err(|_| ToolError::InvalidArguments {
                    tool: "ask_question".to_owned(),
                    reason: "failed sending question to UI".to_owned(),
                })?;
            reply_rx
                .recv()
                .ok()
                .flatten()
                .ok_or_else(|| ToolError::InvalidArguments {
                    tool: "ask_question".to_owned(),
                    reason: "question cancelled by user".to_owned(),
                })
        }))));

        let event_log = EventLog::new(&event_log_path);
        let agent = AgentLoop::new(event_log, policy, registry, memory, initial_profile)
            .with_subagent_handles(subagent_state, subagent_cancel)
            .with_full_transcript_path(&full_transcript_path)
            .with_command_allowlist_paths(
                &loaded.paths.repo_commands,
                &loaded.paths.global_commands,
            )
            .with_allowed_commands(loaded.allowed_commands.clone())
            .with_extra_tools(mcp_tools)
            .with_runtime_context(runtime_context);
        let repo_overrides = read_repo_override_keys(&loaded.paths.repo_config);
        let cached_models = load_cached_models(&loaded.paths.global_dir, active_kind);

        Ok(Self {
            loaded,
            agent,
            provider,
            model_label,
            provider_kind: active_kind,
            sandbox_label,
            workspace_label,
            session_uid,
            should_hydrate,
            notices,
            slash_registry,
            plugin_manager,
            todo_store,
            task_registry,
            task_rx,
            subagent_tracker,
            subagent_manager,
            swarm_manager,
            question_relay,
            repo_overrides,
            cached_models,
        })
    }
}

/// Builds the full tool registry (coding, web, tasks, personal, memory, MCP) for `loaded`.
pub fn build_registry(
    loaded: &LoadedReinState,
    sandbox: &WorkspaceSandbox,
    memory: JsonlMemoryStore,
    todo_store: TodoStore,
    task_registry: TaskRegistry,
    mcp_catalog: McpCatalog,
) -> (ToolRegistry, Vec<String>, Vec<String>) {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadTool::new(sandbox.clone())));
    registry.register(Box::new(WriteTool::new(sandbox.clone())));
    registry.register(Box::new(EditTool::new(sandbox.clone())));
    registry.register(Box::new(GlobTool::new(sandbox.clone())));
    registry.register(Box::new(GrepTool::new(sandbox.clone())));
    registry.register(Box::new(
        BashTool::new(sandbox.clone())
            .with_output_dir(loaded.paths.repo_dir.join("bash-outputs"))
            .with_task_registry(task_registry.clone()),
    ));
    registry.register(Box::new(ManageTaskTool::new(task_registry.clone())));
    registry.register(Box::new(ScheduleTool::new(task_registry)));
    registry.register(Box::new(MemoryTool::new(memory)));
    registry.register(Box::new(SwitchModeTool::new()));
    registry.register(Box::new(
        WebSearchTool::new().with_credentials_dir(loaded.paths.global_dir.join("credentials")),
    ));
    registry.register(Box::new(WebFetchTool::new()));
    registry.register(Box::new(TodoWriteTool::with_store(todo_store.clone())));
    registry.register(Box::new(TodoReadTool::with_store(todo_store)));
    registry.register(Box::new(NotesTool::new(&loaded.paths.notes)));

    let mut calendar = CalendarTool::new(&loaded.paths.calendar);
    if let Some(ics_dir) = &loaded.config.ics_dir {
        calendar = calendar.with_ics_dir(ics_dir);
    }
    registry.register(Box::new(calendar));

    let mut email = EmailTool::new(&loaded.paths.email);
    if let Some(maildir) = &loaded.config.maildir {
        email = email.with_maildir(maildir);
    }
    registry.register(Box::new(email));

    let mut mcp_names = Vec::new();
    let mut warnings = Vec::new();
    for server in &loaded.config.mcp_servers {
        let client = McpClient::new(server.clone());
        match client.list_tools() {
            Ok(schemas) => {
                if let Err(err) = mcp_catalog.register_server(client, &schemas) {
                    warnings.push(format!("mcp '{}': {err}", server.name));
                }
            }
            Err(err) => warnings.push(format!("mcp '{}': {err}", server.name)),
        }
    }
    if mcp_catalog.has_servers() {
        registry.register(Box::new(CallMcpTool::new(mcp_catalog)));
        mcp_names.push("call_mcp_tool".to_owned());
    }
    (registry, mcp_names, warnings)
}

/// Returns `true` when `--yolo` or `--dangerously-bypass-permission` is present in `args`.
#[must_use]
pub fn has_bypass_permissions_flag(args: &[String]) -> bool {
    args.iter()
        .skip(1)
        .any(|arg| arg == "--dangerously-bypass-permission" || arg == "--yolo")
}

/// Parses `--resume <uid>` or `--resume=<uid>` from `args`.
pub fn parse_resume_flag(args: &[String]) -> Result<ResumeSpec> {
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        if let Some(uid) = arg.strip_prefix("--resume=") {
            let trimmed = uid.trim();
            if trimmed.is_empty() {
                anyhow::bail!("--resume requires a session uid (usage: rein --resume <uid>)");
            }
            return Ok(ResumeSpec::Uid(trimmed.to_owned()));
        }
        if arg == "--resume" {
            if let Some(next) = iter.next()
                && !next.starts_with('-')
                && !next.trim().is_empty()
            {
                return Ok(ResumeSpec::Uid(next.trim().to_owned()));
            }
            anyhow::bail!("--resume requires a session uid (usage: rein --resume <uid>)");
        }
    }
    Ok(ResumeSpec::NewSession)
}

/// Extracts `--flag value` or `--flag=value` from `args`.
#[must_use]
pub fn parse_flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == flag {
            return iter.next().map(String::as_str);
        }
        if let Some(rest) = arg.strip_prefix(&format!("{flag}=")) {
            return Some(rest);
        }
    }
    None
}

/// Formats [`SandboxStatus`] into a concise UI badge label.
#[must_use]
pub fn format_sandbox_label(status: &SandboxStatus) -> String {
    match status {
        SandboxStatus::PolicyOnly => "policy-gated".to_owned(),
        SandboxStatus::Enforced { abi_version } => format!("landlock v{abi_version}"),
        SandboxStatus::Unavailable { .. } => "sandbox off".to_owned(),
    }
}

/// Collapses `$HOME` prefix in `path` to `~` for display.
#[must_use]
pub fn collapse_home_path(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME")
        && let Ok(rel) = path.strip_prefix(Path::new(&home))
    {
        return if rel.as_os_str().is_empty() {
            "~".to_owned()
        } else {
            format!("~/{}", rel.display())
        };
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_resume_requires_explicit_uid() {
        let plain = vec!["rein".to_owned()];
        assert_eq!(parse_resume_flag(&plain).ok(), Some(ResumeSpec::NewSession));

        let bare = vec!["rein".to_owned(), "--resume".to_owned()];
        assert!(parse_resume_flag(&bare).is_err());

        let empty_eq = vec!["rein".to_owned(), "--resume=".to_owned()];
        assert!(parse_resume_flag(&empty_eq).is_err());

        let with_flag = vec![
            "rein".to_owned(),
            "--resume".to_owned(),
            "--model".to_owned(),
        ];
        assert!(parse_resume_flag(&with_flag).is_err());

        let valid = vec![
            "rein".to_owned(),
            "--resume".to_owned(),
            "a1b2c3d4".to_owned(),
        ];
        assert_eq!(
            parse_resume_flag(&valid).ok(),
            Some(ResumeSpec::Uid("a1b2c3d4".to_owned()))
        );

        let valid_eq = vec!["rein".to_owned(), "--resume=deadbeef".to_owned()];
        assert_eq!(
            parse_resume_flag(&valid_eq).ok(),
            Some(ResumeSpec::Uid("deadbeef".to_owned()))
        );
    }

    #[test]
    fn bypass_permissions_requires_explicit_yolo_or_dangerously_flag() {
        assert!(!has_bypass_permissions_flag(&["rein".to_owned()]));
        assert!(!has_bypass_permissions_flag(&[
            "rein".to_owned(),
            "--model".to_owned(),
            "yolo".to_owned(),
        ]));
        assert!(has_bypass_permissions_flag(&[
            "rein".to_owned(),
            "--yolo".to_owned(),
        ]));
        assert!(has_bypass_permissions_flag(&[
            "rein".to_owned(),
            "--dangerously-bypass-permission".to_owned(),
        ]));
    }
}
