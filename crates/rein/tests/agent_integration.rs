// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-18
// Purpose: Integration tests for AgentLoop + ReplayProvider, --resume session recovery, and rein replay CLI

//! Integration tests for `rein` agent loop, session resume, and replay CLI.

use std::path::PathBuf;
use std::process::Command;

use rein::agent::{AgentLoop, ApprovalAction, ApprovalDecision, ApprovalPrompter, ApprovalRequest};
use rein::config::{ensure_dirs_and_load_in, resolve_resume_session, write_latest_session_uid};
use rein::modes::Profile;
use rein_core::{EventLog, ReplayProvider, Role, SessionEvent, ToolCall};
use rein_policy::{AuditLog, PolicyEngine, WorkspaceSandbox};
use rein_tools::{JsonlMemoryStore, ReadTool, ToolRegistry, WriteTool};
use serde_json::json;

struct AutoDenyPrompter;
impl ApprovalPrompter for AutoDenyPrompter {
    fn confirm(&mut self, _request: &ApprovalRequest) -> anyhow::Result<ApprovalDecision> {
        Ok(ApprovalDecision {
            action: ApprovalAction::Deny,
            feedback: None,
        })
    }
}

fn fresh_temp_dir(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!("rein_bin_it_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[test]
fn agent_loop_executes_tool_round_with_replay_provider() -> Result<(), Box<dyn std::error::Error>> {
    let dir = fresh_temp_dir("agent_loop")?;
    let workspace = dir.join("workspace");
    let home = dir.join("home");
    std::fs::create_dir_all(&workspace)?;
    std::fs::create_dir_all(&home)?;
    std::fs::write(workspace.join("hello.txt"), "rein integration ok")?;

    let loaded = ensure_dirs_and_load_in(&workspace, &home)?;
    let sandbox = WorkspaceSandbox::new(&workspace);
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadTool::new(sandbox.clone())));

    let profile = Profile::coding();
    let policy = PolicyEngine::new(
        profile.policy_rules.clone(),
        sandbox,
        AuditLog::new(&loaded.paths.audit_log),
    );
    let memory = JsonlMemoryStore::dual(&loaded.paths.repo_memory, &loaded.paths.global_memory);
    let session_path = loaded.paths.sessions_dir.join("it-session.jsonl");
    let mut agent = AgentLoop::new(
        EventLog::new(&session_path),
        policy,
        registry,
        memory,
        profile,
    );

    let recorded = vec![
        SessionEvent::ToolRequested {
            call: ToolCall {
                id: "call_read_1".to_owned(),
                name: "read".to_owned(),
                arguments: json!({ "path": "hello.txt" }),
            },
        },
        SessionEvent::ToolCompleted {
            output: rein_core::ToolOutput {
                call_id: "call_read_1".to_owned(),
                content: "rein integration ok".to_owned(),
                is_error: false,
            },
        },
        SessionEvent::StreamDelta {
            delta: "Read complete.".to_owned(),
        },
    ];

    let mut provider = ReplayProvider::from_events(&recorded);
    let mut prompter = AutoDenyPrompter;
    let answer = agent.run_user_prompt("read hello.txt", &mut provider, &mut prompter)?;
    assert_eq!(answer, "Read complete.");

    let persisted = EventLog::new(&session_path).read_all()?;
    assert!(persisted.iter().any(|ev| matches!(
        ev,
        SessionEvent::ToolCompleted { output } if !output.is_error && output.content.contains("rein integration ok")
    )));
    assert!(loaded.paths.audit_log.exists());

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn resume_session_resolves_latest_and_hydrates_history() -> Result<(), Box<dyn std::error::Error>> {
    let dir = fresh_temp_dir("resume")?;
    let workspace = dir.join("workspace");
    let home = dir.join("home");
    std::fs::create_dir_all(&workspace)?;
    std::fs::create_dir_all(&home)?;

    let loaded = ensure_dirs_and_load_in(&workspace, &home)?;
    let uid = "20261118-resume-test";
    let session_path = loaded.paths.sessions_dir.join(format!("{uid}.jsonl"));
    let log = EventLog::new(&session_path);
    log.append(&SessionEvent::MessageRecorded {
        message: rein_core::Message::new(Role::User, "first prompt"),
    })?;
    log.append(&SessionEvent::MessageRecorded {
        message: rein_core::Message::new(Role::Assistant, "first reply"),
    })?;
    write_latest_session_uid(&loaded.paths, uid)?;

    let (summary, _from_latest) = resolve_resume_session(&loaded.paths, None)?;
    assert_eq!(summary.uid, uid);
    assert_eq!(summary.event_log, session_path);

    let (by_uid, _) = resolve_resume_session(&loaded.paths, Some(uid))?;
    assert_eq!(by_uid.uid, uid);
    assert_eq!(by_uid.event_log, session_path);

    let sandbox = WorkspaceSandbox::new(&workspace);
    let profile = Profile::coding();
    let policy = PolicyEngine::new(
        profile.policy_rules.clone(),
        sandbox,
        AuditLog::new(&loaded.paths.audit_log),
    );
    let memory = JsonlMemoryStore::dual(&loaded.paths.repo_memory, &loaded.paths.global_memory);
    let mut agent = AgentLoop::new(
        EventLog::new(&summary.event_log),
        policy,
        ToolRegistry::new(),
        memory,
        profile,
    );
    let lines = agent.hydrate_from_log()?;
    assert_eq!(lines.len(), 2);
    assert_eq!(agent.history().len(), 2);
    assert_eq!(agent.history()[0].content, "first prompt");
    assert_eq!(agent.history()[1].content, "first reply");
    assert_eq!(agent.checkpoints().len(), 1);
    assert_eq!(agent.checkpoints()[0].prompt_preview, "first prompt");

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn binary_replay_subcommand_runs_recorded_session() -> Result<(), Box<dyn std::error::Error>> {
    let dir = fresh_temp_dir("cli_replay")?;
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(&workspace)?;

    let replay_file = dir.join("recorded.jsonl");
    let log = EventLog::new(&replay_file);
    log.append(&SessionEvent::MessageRecorded {
        message: rein_core::Message::new(Role::User, "hello"),
    })?;
    log.append(&SessionEvent::StreamDelta {
        delta: "CLI_REPLAY_OK".to_owned(),
    })?;

    let output = Command::new(env!("CARGO_BIN_EXE_rein"))
        .arg("replay")
        .arg(&replay_file)
        .env("HOME", &dir)
        .current_dir(&workspace)
        .output()?;

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("CLI_REPLAY_OK"), "stdout: {stdout}");

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn bypass_permissions_allows_all_tools_across_modes_without_prompting()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = fresh_temp_dir("bypass_yolo")?;
    let workspace = dir.join("workspace");
    let home = dir.join("home");
    std::fs::create_dir_all(&workspace)?;
    std::fs::create_dir_all(&home)?;

    let loaded = ensure_dirs_and_load_in(&workspace, &home)?;
    let sandbox = WorkspaceSandbox::new(&workspace);
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(WriteTool::new(sandbox.clone())));

    // Plan mode normally blocks `write` via both allowed_tools and PolicyRuleSet::plan_default().
    let profile = Profile::plan();
    let policy = PolicyEngine::new(
        profile.policy_rules.clone(),
        sandbox,
        AuditLog::new(&loaded.paths.audit_log),
    )
    .with_bypass_permissions(true);
    assert!(policy.is_bypass_permissions());

    let memory = JsonlMemoryStore::dual(&loaded.paths.repo_memory, &loaded.paths.global_memory);
    let session_path = loaded.paths.sessions_dir.join("yolo-session.jsonl");
    let mut agent = AgentLoop::new(
        EventLog::new(&session_path),
        policy,
        registry,
        memory,
        profile,
    );

    let recorded = vec![
        SessionEvent::ToolRequested {
            call: ToolCall {
                id: "call_write_yolo".to_owned(),
                name: "write".to_owned(),
                arguments: json!({ "path": "yolo.txt", "content": "auto approved" }),
            },
        },
        SessionEvent::ToolCompleted {
            output: rein_core::ToolOutput {
                call_id: "call_write_yolo".to_owned(),
                content: "wrote yolo.txt".to_owned(),
                is_error: false,
            },
        },
        SessionEvent::StreamDelta {
            delta: "YOLO_DONE".to_owned(),
        },
    ];

    let mut provider = ReplayProvider::from_events(&recorded);
    let mut prompter = AutoDenyPrompter;
    let answer = agent.run_user_prompt("write yolo.txt", &mut provider, &mut prompter)?;
    assert_eq!(answer, "YOLO_DONE");
    assert_eq!(
        std::fs::read_to_string(workspace.join("yolo.txt"))?,
        "auto approved"
    );

    let audit_raw = std::fs::read_to_string(&loaded.paths.audit_log)?;
    assert!(audit_raw.contains("dangerously-bypass-permission"));

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn agent_loop_rewind_restores_messages_and_code_from_turns()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = fresh_temp_dir("rewind")?;
    let workspace = dir.join("workspace");
    let home = dir.join("home");
    std::fs::create_dir_all(&workspace)?;
    std::fs::create_dir_all(&home)?;
    std::fs::write(workspace.join("app.txt"), "v1\n")?;

    let loaded = ensure_dirs_and_load_in(&workspace, &home)?;
    let sandbox = WorkspaceSandbox::new(&workspace);
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(WriteTool::new(sandbox.clone())));

    let profile = Profile::coding();
    let policy = PolicyEngine::new(
        profile.policy_rules.clone(),
        sandbox,
        AuditLog::new(&loaded.paths.audit_log),
    )
    .with_bypass_permissions(true);
    let memory = JsonlMemoryStore::dual(&loaded.paths.repo_memory, &loaded.paths.global_memory);
    let session_path = loaded.paths.sessions_dir.join("rewind-session.jsonl");
    let mut agent = AgentLoop::new(
        EventLog::new(&session_path),
        policy,
        registry,
        memory,
        profile,
    );

    let recorded = vec![
        SessionEvent::StreamDelta {
            delta: "Turn 1 ok.".to_owned(),
        },
        SessionEvent::MessageRecorded {
            message: rein_core::Message::new(Role::Assistant, "Turn 1 ok."),
        },
        SessionEvent::ToolRequested {
            call: ToolCall {
                id: "call_w2".to_owned(),
                name: "write".to_owned(),
                arguments: json!({ "path": "app.txt", "content": "v2\n" }),
            },
        },
        SessionEvent::ToolCompleted {
            output: rein_core::ToolOutput {
                call_id: "call_w2".to_owned(),
                content: "wrote".to_owned(),
                is_error: false,
            },
        },
        SessionEvent::StreamDelta {
            delta: "Turn 2 ok.".to_owned(),
        },
    ];

    let mut provider = ReplayProvider::from_events(&recorded);
    let mut prompter = AutoDenyPrompter;
    assert_eq!(
        agent.run_user_prompt("turn 1", &mut provider, &mut prompter)?,
        "Turn 1 ok."
    );
    assert_eq!(
        agent.run_user_prompt("turn 2", &mut provider, &mut prompter)?,
        "Turn 2 ok."
    );
    assert_eq!(std::fs::read_to_string(workspace.join("app.txt"))?, "v2\n");
    assert_eq!(agent.checkpoints().len(), 2);

    // Rewind to turn 2 (index 1) with restore_code = true -> restores app.txt to "v1\n" and trims history.
    agent.rewind_to(1, true)?;
    assert_eq!(std::fs::read_to_string(workspace.join("app.txt"))?, "v1\n");
    assert_eq!(agent.checkpoints().len(), 1);
    assert_eq!(agent.history().len(), 2);

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn parallel_subagents_execute_concurrently_and_confine_to_parent_mode()
-> Result<(), Box<dyn std::error::Error>> {
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex, RwLock};

    use rein::agent::{AgentTool, SubagentEnv, SubagentSessionState, SubagentTracker};
    use rein::modes::RuntimeContext;
    use rein_core::Provider;

    let dir = fresh_temp_dir("subagents")?;
    let workspace = dir.join("workspace");
    let home = dir.join("home");
    std::fs::create_dir_all(&workspace)?;
    std::fs::create_dir_all(&home)?;
    std::fs::write(workspace.join("a.rs"), "pub fn a() {}\n")?;

    let loaded = ensure_dirs_and_load_in(&workspace, &home)?;
    let sandbox = WorkspaceSandbox::new(&workspace);

    let child_reports = Arc::new(Mutex::new(std::collections::VecDeque::from([
        vec![
            SessionEvent::StreamDelta {
                delta: "Audit A complete.".to_owned(),
            },
            SessionEvent::MessageRecorded {
                message: rein_core::Message::new(Role::Assistant, "Audit A complete."),
            },
        ],
        vec![
            SessionEvent::StreamDelta {
                delta: "Audit B complete.".to_owned(),
            },
            SessionEvent::MessageRecorded {
                message: rein_core::Message::new(Role::Assistant, "Audit B complete."),
            },
        ],
    ])));
    let factory_reports = Arc::clone(&child_reports);
    let provider_factory: rein::agent::ProviderFactory = Arc::new(move |_| {
        let events = factory_reports
            .lock()
            .ok()
            .and_then(|mut q| q.pop_front())
            .unwrap_or_default();
        let prov: Box<dyn Provider> = Box::new(ReplayProvider::from_events(&events));
        (prov, "replay/test".to_owned())
    });

    let subagent_state = Arc::new(RwLock::new(SubagentSessionState::default()));
    let subagent_cancel = Arc::new(AtomicBool::new(false));
    let tracker = SubagentTracker::new();

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadTool::new(sandbox.clone())));
    registry.register(Box::new(AgentTool::new(SubagentEnv {
        sandbox: sandbox.clone(),
        audit_log_path: loaded.paths.audit_log.clone(),
        sessions_dir: loaded.paths.sessions_dir.clone(),
        bash_outputs_dir: loaded.paths.repo_dir.join("bash-outputs"),
        notes_file: loaded.paths.notes.clone(),
        calendar_file: loaded.paths.calendar.clone(),
        calendar_ics_dir: loaded.paths.global_dir.join("calendar"),
        email_file: loaded.paths.email.clone(),
        maildir_path: loaded.paths.global_dir.join("maildir"),
        memory: JsonlMemoryStore::dual(&loaded.paths.repo_memory, &loaded.paths.global_memory),
        todo_store: rein_tools::TodoStore::for_session(&loaded.paths.todos_dir, "sub"),
        runtime_context: RuntimeContext::new(),
        session_state: Arc::clone(&subagent_state),
        tracker,
        cancel: Arc::clone(&subagent_cancel),
        provider_factory,
    })));

    // Start parent in Plan mode: two `research` subagents must succeed in parallel,
    // while a `coder` subagent must be denied by Plan mode confinement.
    let profile = Profile::plan();
    let policy = PolicyEngine::new(
        profile.policy_rules.clone(),
        sandbox,
        AuditLog::new(&loaded.paths.audit_log),
    );
    let memory = JsonlMemoryStore::dual(&loaded.paths.repo_memory, &loaded.paths.global_memory);
    let session_path = loaded.paths.sessions_dir.join("parent-subagents.jsonl");
    let mut agent = AgentLoop::new(
        EventLog::new(&session_path),
        policy,
        registry,
        memory,
        profile,
    )
    .with_subagent_handles(subagent_state, subagent_cancel);

    let recorded = vec![
        SessionEvent::ToolRequested {
            call: ToolCall {
                id: "sub_1".to_owned(),
                name: "agent".to_owned(),
                arguments: json!({
                    "type": "research",
                    "role": "Auditor A",
                    "prompt": "Inspect module A"
                }),
            },
        },
        SessionEvent::ToolRequested {
            call: ToolCall {
                id: "sub_2".to_owned(),
                name: "agent".to_owned(),
                arguments: json!({
                    "type": "research",
                    "role": "Auditor B",
                    "prompt": "Inspect module B"
                }),
            },
        },
        SessionEvent::ToolRequested {
            call: ToolCall {
                id: "sub_denied".to_owned(),
                name: "agent".to_owned(),
                arguments: json!({
                    "type": "coder",
                    "role": "Unauthorized Coder",
                    "prompt": "Edit a.rs"
                }),
            },
        },
        SessionEvent::ToolCompleted {
            output: rein_core::ToolOutput {
                call_id: "sub_1".to_owned(),
                content: "ok".to_owned(),
                is_error: false,
            },
        },
        SessionEvent::StreamDelta {
            delta: "All reports collected.".to_owned(),
        },
    ];

    let mut provider = ReplayProvider::from_events(&recorded);
    let mut prompter = AutoDenyPrompter;
    let answer = agent.run_user_prompt("run audits", &mut provider, &mut prompter)?;
    assert_eq!(answer, "All reports collected.");

    let events = EventLog::new(&session_path).read_all()?;
    let completed: Vec<&rein_core::ToolOutput> = events
        .iter()
        .filter_map(|ev| match ev {
            SessionEvent::ToolCompleted { output } => Some(output),
            _ => None,
        })
        .collect();
    assert_eq!(completed.len(), 3);
    assert!(!completed[0].is_error);
    assert!(!completed[1].is_error);
    assert!(completed[2].is_error);
    assert!(
        completed[2]
            .content
            .contains("plan mode confines subagents")
    );

    let subagent_logs: Vec<_> = std::fs::read_dir(&loaded.paths.sessions_dir)?
        .filter_map(Result::ok)
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with("subagent-") && !name.ends_with(".checkpoints.jsonl")
        })
        .collect();
    assert_eq!(subagent_logs.len(), 2);

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
