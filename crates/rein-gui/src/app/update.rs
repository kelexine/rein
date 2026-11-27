// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Elm update() logic handling UI messages, worker events, auto-scroll, and inspector actions

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use iced::Task;
use iced::clipboard;
use iced::widget::markdown;
use iced::widget::operation::{RelativeOffset, snap_to};
use rein::agent::{ApprovalAction, ApprovalDecision};
use rein::config::{
    DreamConfig, append_allowed_command, apply_provider_switch, remove_allowed_command,
    save_config_file, save_global_settings,
};
use rein_core::{ModeId, ToolCall, ToolOutput};
use rein_tools::{McpServerConfig, QuestionAnswer, TaskNotification, TaskStatus};

use crate::views::tool_card::{format_tool_title, summarize_done};
use crate::worker::{PendingApprovalPrompt, PendingQuestionPrompt, WorkerCommand, WorkerEvent};

use super::message::{Message, SettingsMessage};
use super::state::{
    GuiApp, InspectorPane, ToolItemState, TranscriptItem, collapse_preview, format_size_and_lines,
};

const STREAM_MD_THROTTLE_MS: u64 = 200;

/// Processes a [`Message`] and updates [`GuiApp`] state.
pub fn update(app: &mut GuiApp, message: Message) -> Task<Message> {
    match message {
        Message::Tick => handle_tick(app),
        Message::InputChanged(val) => {
            app.input = val;
            Task::none()
        }
        Message::SubmitPrompt => {
            let trimmed = app.input.trim().to_owned();
            if trimmed.is_empty() {
                return Task::none();
            }
            if app.busy {
                if app.worker.send_steer(trimmed.clone(), trimmed.clone()) {
                    app.input.clear();
                    app.stick_to_bottom = true;
                    app.items.push(TranscriptItem::user(trimmed));
                    return scroll_transcript_to_bottom(app);
                }
                return Task::none();
            }
            app.input.clear();
            app.model_picker_open = false;
            app.plus_menu_open = false;
            if let Some(task) = try_handle_slash_command(app, &trimmed) {
                return task;
            }
            app.stick_to_bottom = true;
            app.items.push(TranscriptItem::user(trimmed.clone()));
            begin_busy(app, "Thinking…");
            app.worker
                .send(WorkerCommand::RunPrompt { prompt: trimmed });
            scroll_transcript_to_bottom(app)
        }
        Message::QuickPrompt(prompt) => {
            if app.busy {
                return Task::none();
            }
            app.model_picker_open = false;
            app.plus_menu_open = false;
            app.stick_to_bottom = true;
            app.items.push(TranscriptItem::user(prompt.clone()));
            begin_busy(app, "Thinking…");
            app.worker.send(WorkerCommand::RunPrompt { prompt });
            scroll_transcript_to_bottom(app)
        }
        Message::CancelTurn => {
            if let Some(pending) = app.pending_approval.take() {
                let fb = pending.feedback.trim().to_owned();
                app.worker.send_approval(ApprovalDecision {
                    action: ApprovalAction::Deny,
                    feedback: (!fb.is_empty()).then_some(fb),
                });
            }
            if let Some(q) = app.pending_question.take() {
                let _ = q.reply_tx.send(None);
                app.question_answers.clear();
                app.question_step = 0;
                app.question_selected.clear();
                app.question_custom_text.clear();
            }
            if app.busy {
                app.worker.cancel_turn();
                app.busy_status = "Stopping…".to_owned();
            } else if app.settings_open {
                app.settings_open = false;
            } else if app.plus_menu_open {
                app.plus_menu_open = false;
            } else if app.model_picker_open {
                app.model_picker_open = false;
            } else if app.task_drawer_open {
                app.task_drawer_open = false;
            } else if app.inspector.is_some() {
                app.inspector = None;
            }
            Task::none()
        }
        Message::ToggleSidebar => {
            app.sidebar_open = !app.sidebar_open;
            if app.sidebar_open {
                app.refresh_sessions();
            }
            Task::none()
        }
        Message::NewSession => {
            if !app.busy {
                app.settings_open = false;
                app.inspector = None;
                app.worker.send(WorkerCommand::NewSession);
            }
            Task::none()
        }
        Message::ResumeSession(uid) => {
            if !app.busy {
                app.settings_open = false;
                app.inspector = None;
                app.worker.send(WorkerCommand::ResumeSession(uid));
            }
            Task::none()
        }
        Message::SwitchMode(mode) => {
            app.plus_menu_open = false;
            if !app.busy {
                app.worker.send(WorkerCommand::SwitchMode(mode));
            }
            Task::none()
        }
        Message::TogglePlusMenu => {
            app.plus_menu_open = !app.plus_menu_open;
            if app.plus_menu_open {
                app.model_picker_open = false;
            }
            Task::none()
        }
        Message::ToggleModelPicker => {
            app.model_picker_open = !app.model_picker_open;
            if app.model_picker_open {
                app.plus_menu_open = false;
                if app.available_models.is_empty() {
                    app.worker.send(WorkerCommand::FetchModels);
                }
            }
            Task::none()
        }
        Message::SelectModel(model_id) => {
            app.model_picker_open = false;
            app.worker.send(WorkerCommand::SelectModel(model_id));
            Task::none()
        }
        Message::CompactHistory => {
            if !app.busy {
                begin_busy(app, "Compacting…");
                app.worker.send(WorkerCommand::Compact);
            }
            Task::none()
        }
        Message::ToggleExpand(idx) => {
            if let Some(
                TranscriptItem::Thinking { expanded, .. } | TranscriptItem::Tool { expanded, .. },
            ) = app.items.get_mut(idx)
            {
                *expanded = !*expanded;
            }
            Task::none()
        }
        Message::ToggleToolGroup(start_idx) => {
            if !app.expanded_tool_groups.remove(&start_idx) {
                app.expanded_tool_groups.insert(start_idx);
            }
            Task::none()
        }
        Message::ToggleUserExpand(idx) => {
            if let Some(TranscriptItem::User { expanded, .. }) = app.items.get_mut(idx) {
                *expanded = !*expanded;
            }
            Task::none()
        }
        Message::TranscriptScrolled(viewport) => {
            let fits = viewport.content_bounds().height <= viewport.bounds().height + 2.0;
            app.stick_to_bottom = fits || viewport.relative_offset().y >= 0.97;
            Task::none()
        }
        Message::OpenInspector {
            title,
            subtitle,
            content,
        } => {
            app.inspector = Some(InspectorPane::new(title, subtitle, content));
            Task::none()
        }
        Message::CloseInspector => {
            app.inspector = None;
            Task::none()
        }
        Message::CopyText(text) => clipboard::write(text),
        Message::ApprovalFeedbackChanged(fb) => {
            if let Some(pending) = &mut app.pending_approval {
                pending.feedback = fb;
            }
            Task::none()
        }
        Message::ResolveApproval(action) => {
            if let Some(pending) = app.pending_approval.take() {
                let fb = pending.feedback.trim().to_owned();
                app.worker.send_approval(ApprovalDecision {
                    action,
                    feedback: (!fb.is_empty()).then_some(fb),
                });
            }
            Task::none()
        }
        Message::SelectQuestionOption(opt_idx) => {
            let is_multi = app
                .pending_question
                .as_ref()
                .and_then(|p| p.questions.get(app.question_step))
                .is_some_and(|q| q.is_multi_select);
            if is_multi {
                if let Some(pos) = app.question_selected.iter().position(|&x| x == opt_idx) {
                    app.question_selected.remove(pos);
                } else {
                    app.question_selected.push(opt_idx);
                    app.question_selected.sort_unstable();
                }
            } else {
                app.question_selected.clear();
                app.question_selected.push(opt_idx);
            }
            Task::none()
        }
        Message::QuestionCustomTextChanged(text) => {
            app.question_custom_text = text;
            Task::none()
        }
        Message::ConfirmQuestion => confirm_question_step(app),
        Message::CancelQuestion => {
            if let Some(q) = app.pending_question.take() {
                let _ = q.reply_tx.send(None);
            }
            app.question_answers.clear();
            app.question_step = 0;
            app.question_selected.clear();
            app.question_custom_text.clear();
            Task::none()
        }
        Message::LinkClicked(_url) => Task::none(),
        Message::ToggleTaskDrawer => {
            app.task_drawer_open = !app.task_drawer_open;
            if app.task_drawer_open {
                app.tasks_snapshot = app.task_registry.list_tasks();
            }
            Task::none()
        }
        Message::KillBackgroundTask(task_id) => {
            let _ = app.task_registry.kill_task(&task_id);
            app.tasks_snapshot = app.task_registry.list_tasks();
            Task::none()
        }
        Message::ToggleSettings => {
            app.settings_open = !app.settings_open;
            if app.settings_open {
                app.plus_menu_open = false;
                app.model_picker_open = false;
                app.settings
                    .refresh_disk_snapshots(&app.paths, &app.plugin_manager);
                app.settings.status_notice = None;
            }
            Task::none()
        }
        Message::Settings(msg) => {
            handle_settings_message(app, msg);
            Task::none()
        }
    }
}

fn scroll_transcript_to_bottom(app: &GuiApp) -> Task<Message> {
    snap_to(app.transcript_id.clone(), RelativeOffset::END)
}

fn begin_busy(app: &mut GuiApp, status: &str) {
    app.busy = true;
    app.busy_flag.store(true, Ordering::Release);
    app.busy_status = status.to_owned();
    app.busy_started_at = Some(Instant::now());
}

fn handle_tick(app: &mut GuiApp) -> Task<Message> {
    if app.busy {
        app.spinner_tick = app.spinner_tick.wrapping_add(1);
    }
    let mut should_scroll = false;
    for ev in app.worker.drain_events() {
        if matches!(ev, WorkerEvent::SessionHydrated { .. })
            || (app.stick_to_bottom
                && matches!(
                    ev,
                    WorkerEvent::TextDelta(_)
                        | WorkerEvent::ToolStart(_)
                        | WorkerEvent::ToolFinish(_, _)
                ))
        {
            should_scroll = true;
        }
        apply_worker_event(app, ev);
    }
    if !app.busy && app.pending_approval.is_none() && app.pending_question.is_none() {
        drain_wakeups(app);
    }
    if should_scroll {
        scroll_transcript_to_bottom(app)
    } else {
        Task::none()
    }
}

fn apply_worker_event(app: &mut GuiApp, ev: WorkerEvent) {
    match ev {
        WorkerEvent::ThinkingDelta(delta) => append_thinking(app, &delta),
        WorkerEvent::TextDelta(delta) => append_assistant_delta(app, &delta),
        WorkerEvent::ToolStart(call) => push_tool_start(app, &call),
        WorkerEvent::ToolFinish(call, output) => {
            finish_tool(app, &call, &output);
            app.sync_runtime_snapshots();
        }
        WorkerEvent::Usage(u) => app.usage.add_assign_turn(u),
        WorkerEvent::ModeSwitched(profile) => {
            let mode = profile.mode;
            app.active_profile = *profile;
            app.items
                .push(TranscriptItem::System(format!("mode switched to {mode:?}")));
        }
        WorkerEvent::ApprovalRequested(request) => {
            app.pending_approval = Some(PendingApprovalPrompt {
                request,
                feedback: String::new(),
            });
        }
        WorkerEvent::QuestionRequested {
            questions,
            reply_tx,
        } => {
            app.pending_question = Some(PendingQuestionPrompt {
                questions,
                reply_tx,
            });
            app.question_answers.clear();
            app.question_step = 0;
            app.question_selected = vec![0];
            app.question_custom_text.clear();
        }
        WorkerEvent::ModelsLoaded(models) => app.available_models = models,
        WorkerEvent::ModelSelected(label) => {
            app.model_label.clone_from(&label);
            if let Some((_, m)) = label.split_once('/') {
                app.config.model = Some(m.to_owned());
            }
        }
        WorkerEvent::ProviderUpdated {
            provider_kind,
            model_label,
            models,
        } => {
            app.provider_kind = provider_kind;
            app.model_label = model_label;
            if !models.is_empty() {
                app.available_models = models;
            }
        }
        WorkerEvent::SessionHydrated {
            session_uid,
            events,
            profile,
        } => {
            app.session_uid = session_uid;
            app.active_profile = *profile;
            app.items.clear();
            app.expanded_tool_groups.clear();
            app.thinking_started_at = None;
            app.inspector = None;
            app.stick_to_bottom = true;
            app.apply_hydrated(events);
            app.refresh_sessions();
            app.sync_runtime_snapshots();
        }
        WorkerEvent::SessionReset {
            session_uid,
            profile,
        } => {
            app.session_uid = session_uid;
            app.active_profile = *profile;
            app.items.clear();
            app.expanded_tool_groups.clear();
            app.thinking_started_at = None;
            app.inspector = None;
            app.stick_to_bottom = true;
            app.usage = rein_core::UsageStats::default();
            app.refresh_sessions();
            app.sync_runtime_snapshots();
        }
        WorkerEvent::Note(note) => app.items.push(TranscriptItem::System(note)),
        WorkerEvent::SteerDelivered(msg) => app
            .items
            .push(TranscriptItem::System(format!("steered: {msg}"))),
        WorkerEvent::Error(err) => app.items.push(TranscriptItem::Error(err)),
        WorkerEvent::TurnFinished => {
            finalize_thinking(app);
            flush_trailing_markdown(app);
            app.busy = false;
            app.busy_flag.store(false, Ordering::Release);
            app.busy_started_at = None;
            app.busy_status = "Thinking…".to_owned();
            app.refresh_sessions();
            app.sync_runtime_snapshots();
        }
    }
}

fn append_thinking(app: &mut GuiApp, delta: &str) {
    if delta.is_empty() {
        return;
    }
    app.busy_status = "Reasoning…".to_owned();
    let elapsed = app.thinking_started_at.map_or(0, |t| t.elapsed().as_secs());
    if let Some(TranscriptItem::Thinking {
        content,
        elapsed_secs,
        active: true,
        ..
    }) = app.items.last_mut()
    {
        content.push_str(delta);
        *elapsed_secs = elapsed;
    } else {
        app.thinking_started_at = Some(Instant::now());
        app.items.push(TranscriptItem::Thinking {
            content: delta.to_owned(),
            preview: String::new(),
            elapsed_secs: 0,
            active: true,
            expanded: false,
        });
    }
}

fn finalize_thinking(app: &mut GuiApp) {
    let elapsed = app
        .thinking_started_at
        .take()
        .map_or(1, |t| t.elapsed().as_secs().max(1));
    if let Some(TranscriptItem::Thinking {
        content,
        preview,
        elapsed_secs,
        active,
        ..
    }) = app.items.last_mut()
        && *active
    {
        *active = false;
        *elapsed_secs = (*elapsed_secs).max(elapsed);
        *preview = collapse_preview(content, 100);
    }
}

fn append_assistant_delta(app: &mut GuiApp, delta: &str) {
    if delta.is_empty() {
        return;
    }
    finalize_thinking(app);
    app.busy_status = "Responding…".to_owned();
    if let Some(TranscriptItem::Assistant { raw, parsed }) = app.items.last_mut() {
        raw.push_str(delta);
        if app.last_md_parse.elapsed() >= Duration::from_millis(STREAM_MD_THROTTLE_MS) {
            *parsed = markdown::Content::parse(raw);
            app.last_md_parse = Instant::now();
        }
    } else {
        app.last_md_parse = Instant::now();
        app.items.push(TranscriptItem::Assistant {
            parsed: markdown::Content::parse(delta),
            raw: delta.to_owned(),
        });
    }
}

fn flush_trailing_markdown(app: &mut GuiApp) {
    if let Some(TranscriptItem::Assistant { raw, parsed }) = app.items.last_mut() {
        *parsed = markdown::Content::parse(raw);
    }
}

fn push_tool_start(app: &mut GuiApp, call: &ToolCall) {
    finalize_thinking(app);
    app.busy_status = format!("Running {}…", call.name);
    let (title, subtitle) = format_tool_title(&call.name, &call.arguments);
    app.items.push(TranscriptItem::Tool {
        call_id: call.id.clone(),
        name: call.name.clone(),
        title,
        subtitle,
        done_summary: String::new(),
        inspector_subtitle: String::new(),
        arguments: call.arguments.clone(),
        state: ToolItemState::Running,
        background_task_id: None,
        output: String::new(),
        expanded: app.config.settings.verbose_output,
    });
}

fn finish_tool(app: &mut GuiApp, call: &ToolCall, output: &ToolOutput) {
    app.busy_status = "Thinking…".to_owned();
    let bg_id = (!output.is_error)
        .then(|| extract_background_id(&output.content))
        .flatten();
    if let Some(TranscriptItem::Tool {
        name,
        done_summary,
        inspector_subtitle,
        state,
        background_task_id,
        output: out_buf,
        ..
    }) =
        app.items.iter_mut().rev().find(
            |item| matches!(item, TranscriptItem::Tool { call_id, .. } if call_id == &call.id),
        )
    {
        if let Some(id) = bg_id {
            *state = ToolItemState::Running;
            *background_task_id = Some(id);
        } else {
            *state = if output.is_error {
                ToolItemState::Error
            } else {
                ToolItemState::Done
            };
        }
        *done_summary = summarize_done(name, &output.content);
        let line_count = output.content.lines().count().max(1);
        *inspector_subtitle = format_size_and_lines(output.content.len(), line_count);
        *out_buf = output.content.clone();
    }
}

fn extract_background_id(content: &str) -> Option<String> {
    let val: serde_json::Value = serde_json::from_str(content).ok()?;
    let status = val.get("status").and_then(serde_json::Value::as_str)?;
    if status != "backgrounded" && status != "running" {
        return None;
    }
    val.get("task_id")
        .or_else(|| val.get("conversation_id"))
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn complete_background_tool(app: &mut GuiApp, task_id: &str, is_error: bool, text: String) {
    if let Some(TranscriptItem::Tool {
        name,
        done_summary,
        inspector_subtitle,
        state,
        output,
        ..
    }) = app.items.iter_mut().rev().find(|item| {
        matches!(
            item,
            TranscriptItem::Tool {
                background_task_id: Some(id),
                ..
            } if id == task_id
        )
    }) {
        *state = if is_error {
            ToolItemState::Error
        } else {
            ToolItemState::Done
        };
        *done_summary = summarize_done(name, &text);
        let line_count = text.lines().count().max(1);
        *inspector_subtitle = format_size_and_lines(text.len(), line_count);
        *output = text;
    }
}

fn drain_wakeups(app: &mut GuiApp) {
    let mut events = Vec::new();
    while let Ok(ev) = app.task_rx.try_recv() {
        events.push(ev);
    }
    if events.is_empty() {
        return;
    }

    let mut labels = Vec::new();
    let mut sections = Vec::new();
    for ev in events {
        match ev {
            TaskNotification::TaskExited {
                task_id,
                command,
                status,
                exit_code,
                tail,
                ..
            } => {
                app.task_registry.notify_external_event(&task_id);
                let failed = exit_code.is_some_and(|c| c != 0)
                    || matches!(status, TaskStatus::Failed | TaskStatus::Killed);
                complete_background_tool(app, &task_id, failed, tail.clone());
                labels.push(format!("{task_id} ({})", status.label()));
                sections.push(format!(
                    "- Background task `{task_id}` (`{command}`) finished with status `{}`.\nOutput tail:\n```\n{tail}\n```",
                    status.label()
                ));
            }
            TaskNotification::ScheduleFired {
                task_id,
                label,
                prompt,
                iteration,
                ..
            } => {
                app.task_registry.notify_external_event(&task_id);
                labels.push(format!("{task_id} ({label})"));
                sections.push(format!(
                    "- Scheduled task `{task_id}` (`{label}`, trigger #{iteration}) fired:\n{prompt}"
                ));
            }
            TaskNotification::SubagentMessage {
                conversation_id,
                role,
                agent_type,
                content,
                finished,
            } => {
                app.task_registry.notify_external_event(&conversation_id);
                if finished {
                    complete_background_tool(app, &conversation_id, false, content.clone());
                }
                labels.push(role.clone());
                sections.push(format!(
                    "- Subagent `{conversation_id}` (`{agent_type}: {role}`) sent:\n{content}"
                ));
            }
        }
    }

    app.sync_runtime_snapshots();
    let display = format!("[task wakeup] {}", labels.join(", "));
    let prompt = format!(
        "[Background Task / Schedule Wakeup Notification]\n{}\n\nRespond to these completed task/schedule events and continue any pending work.",
        sections.join("\n\n")
    );
    app.items.push(TranscriptItem::System(display));
    begin_busy(app, "Thinking…");
    app.worker.send(WorkerCommand::RunPrompt { prompt });
}

fn confirm_question_step(app: &mut GuiApp) -> Task<Message> {
    let Some(pending) = &app.pending_question else {
        return Task::none();
    };
    let Some(q) = pending.questions.get(app.question_step) else {
        return Task::none();
    };
    let custom = app.question_custom_text.trim().to_owned();
    let mut selected: Vec<String> = app
        .question_selected
        .iter()
        .filter_map(|&idx| q.options.get(idx).cloned())
        .collect();
    if selected.is_empty()
        && custom.is_empty()
        && let Some(first) = q.options.first()
    {
        selected.push(first.clone());
    }
    app.question_answers.push(QuestionAnswer {
        question: q.question.clone(),
        selected,
        custom_text: (!custom.is_empty()).then_some(custom),
    });
    app.question_custom_text.clear();
    if app.question_step + 1 < pending.questions.len() {
        app.question_step += 1;
        app.question_selected = vec![0];
        return Task::none();
    }
    if let Some(done) = app.pending_question.take() {
        let answers = std::mem::take(&mut app.question_answers);
        let _ = done.reply_tx.send(Some(answers));
    }
    app.question_step = 0;
    app.question_selected.clear();
    Task::none()
}

fn try_handle_slash_command(app: &mut GuiApp, input: &str) -> Option<Task<Message>> {
    match input {
        "/new" | "/clear" => Some(update(app, Message::NewSession)),
        "/compact" => Some(update(app, Message::CompactHistory)),
        "/tasks" => Some(update(app, Message::ToggleTaskDrawer)),
        "/model" => Some(update(app, Message::ToggleModelPicker)),
        "/config" | "/settings" | "/plugins" | "/mcp" => Some(update(app, Message::ToggleSettings)),
        "/coding" | "/mode coding" => Some(update(app, Message::SwitchMode(ModeId::Coding))),
        "/plan" | "/mode plan" => Some(update(app, Message::SwitchMode(ModeId::Plan))),
        "/personal" | "/mode personal" => Some(update(app, Message::SwitchMode(ModeId::Personal))),
        _ => None,
    }
}

fn handle_settings_message(app: &mut GuiApp, msg: SettingsMessage) {
    match msg {
        SettingsMessage::SelectCategory(cat) => {
            app.settings.category = cat;
            app.settings.status_notice = None;
        }
        SettingsMessage::SelectProvider(kind) => {
            apply_provider_switch(&mut app.config, kind, None);
            app.provider_kind = kind;
            app.worker.send(WorkerCommand::SwitchProvider(kind));
            app.settings.status_notice =
                Some(format!("Switched active provider to {}", kind.as_str()));
        }
        SettingsMessage::RefreshModels => {
            app.worker.send(WorkerCommand::FetchModels);
            app.settings.status_notice = Some("Refreshing model catalog from provider…".to_owned());
        }
        SettingsMessage::StartOAuthLogin => {
            app.worker.send(WorkerCommand::StartOAuthLogin);
            app.settings.status_notice =
                Some("Opened Google Sign-In in browser for Antigravity…".to_owned());
        }
        SettingsMessage::MaxTokensChanged(v) => app.settings.max_tokens_input = v,
        SettingsMessage::EndpointChanged(v) => app.settings.endpoint_input = v,
        SettingsMessage::ApiKeyEnvChanged(v) => app.settings.api_key_env_input = v,
        SettingsMessage::AntigravityBaseUrlChanged(v) => {
            app.settings.antigravity_base_url_input = v;
        }
        SettingsMessage::AntigravityProjectIdChanged(v) => {
            app.settings.antigravity_project_id_input = v;
        }
        SettingsMessage::SaveProviderConfig => {
            let max_tok = app.settings.max_tokens_input.trim();
            app.config.max_tokens = if max_tok.is_empty() {
                None
            } else {
                max_tok.parse::<u32>().ok()
            };
            let ep = app.settings.endpoint_input.trim();
            app.config.endpoint = (!ep.is_empty()).then(|| ep.to_owned());
            let env_k = app.settings.api_key_env_input.trim();
            app.config.api_key_env = (!env_k.is_empty()).then(|| env_k.to_owned());
            let agy_url = app.settings.antigravity_base_url_input.trim();
            app.config.antigravity_base_url = (!agy_url.is_empty()).then(|| agy_url.to_owned());
            let agy_proj = app.settings.antigravity_project_id_input.trim();
            app.config.antigravity_project_id = (!agy_proj.is_empty()).then(|| agy_proj.to_owned());
            app.worker
                .send(WorkerCommand::ApplyConfig(Box::new(app.config.clone())));
            app.settings.status_notice =
                Some("Saved provider & endpoint settings to ~/.rein/config.json".to_owned());
        }
        SettingsMessage::ToggleUiBool(key) => {
            match key {
                "thinking_mode" => {
                    app.config.settings.thinking_mode = !app.config.settings.thinking_mode;
                }
                "auto_compact" => {
                    app.config.settings.auto_compact = !app.config.settings.auto_compact;
                }
                "verbose_output" => {
                    app.config.settings.verbose_output = !app.config.settings.verbose_output;
                    let expand = app.config.settings.verbose_output;
                    for item in &mut app.items {
                        if let TranscriptItem::Tool { expanded, .. } = item {
                            *expanded = expand;
                        }
                    }
                }
                "show_turn_duration" => {
                    app.config.settings.show_turn_duration =
                        !app.config.settings.show_turn_duration;
                }
                "auto_scroll" => {
                    app.config.settings.auto_scroll = !app.config.settings.auto_scroll;
                    app.stick_to_bottom = app.config.settings.auto_scroll;
                }
                "respect_gitignore_picker" => {
                    app.config.settings.respect_gitignore_picker =
                        !app.config.settings.respect_gitignore_picker;
                }
                "show_tips" => {
                    app.config.settings.show_tips = !app.config.settings.show_tips;
                }
                _ => {}
            }
            let _ = save_global_settings(&app.paths.global_config, &app.config.settings);
            app.settings.status_notice = Some(format!("Updated '{key}' in ~/.rein/config.json"));
        }
        SettingsMessage::CycleUiChoice(key) => {
            match key {
                "default_permission_mode" => {
                    app.config.settings.default_permission_mode =
                        if app.config.settings.default_permission_mode == "policy-gated" {
                            "auto-bypass".to_owned()
                        } else {
                            "policy-gated".to_owned()
                        };
                    let bypass = app.config.settings.default_permission_mode == "auto-bypass";
                    app.worker.send(WorkerCommand::SyncPolicy {
                        allowed_commands: app.settings.merged_allowed_commands(),
                        bypass_permissions: bypass,
                    });
                }
                "editor_mode" => {
                    app.config.settings.editor_mode = if app.config.settings.editor_mode == "normal"
                    {
                        "vi".to_owned()
                    } else {
                        "normal".to_owned()
                    };
                }
                "queued_messages" => {
                    app.config.settings.queued_messages =
                        if app.config.settings.queued_messages == "queue" {
                            "send_immediately".to_owned()
                        } else {
                            "queue".to_owned()
                        };
                }
                _ => {}
            }
            let _ = save_global_settings(&app.paths.global_config, &app.config.settings);
            app.settings.status_notice = Some(format!("Updated '{key}' in ~/.rein/config.json"));
        }
        SettingsMessage::RepoCommandInputChanged(v) => app.settings.new_repo_command = v,
        SettingsMessage::AddRepoCommand => {
            let pat = app.settings.new_repo_command.trim().to_owned();
            if !pat.is_empty() {
                let _ = append_allowed_command(&app.paths.repo_commands, &pat);
                app.settings.new_repo_command.clear();
                app.settings
                    .refresh_disk_snapshots(&app.paths, &app.plugin_manager);
                sync_policy_to_worker(app);
                app.settings.status_notice = Some(format!("Added '{pat}' to workspace allowlist"));
            }
        }
        SettingsMessage::RemoveRepoCommand(pat) => {
            let _ = remove_allowed_command(&app.paths.repo_commands, &pat);
            app.settings
                .refresh_disk_snapshots(&app.paths, &app.plugin_manager);
            sync_policy_to_worker(app);
            app.settings.status_notice = Some(format!("Removed '{pat}' from workspace allowlist"));
        }
        SettingsMessage::GlobalCommandInputChanged(v) => app.settings.new_global_command = v,
        SettingsMessage::AddGlobalCommand => {
            let pat = app.settings.new_global_command.trim().to_owned();
            if !pat.is_empty() {
                let _ = append_allowed_command(&app.paths.global_commands, &pat);
                app.settings.new_global_command.clear();
                app.settings
                    .refresh_disk_snapshots(&app.paths, &app.plugin_manager);
                sync_policy_to_worker(app);
                app.settings.status_notice = Some(format!("Added '{pat}' to global allowlist"));
            }
        }
        SettingsMessage::RemoveGlobalCommand(pat) => {
            let _ = remove_allowed_command(&app.paths.global_commands, &pat);
            app.settings
                .refresh_disk_snapshots(&app.paths, &app.plugin_manager);
            sync_policy_to_worker(app);
            app.settings.status_notice = Some(format!("Removed '{pat}' from global allowlist"));
        }
        SettingsMessage::McpNameChanged(v) => app.settings.mcp_name_input = v,
        SettingsMessage::McpCommandChanged(v) => app.settings.mcp_command_input = v,
        SettingsMessage::McpArgsChanged(v) => app.settings.mcp_args_input = v,
        SettingsMessage::AddMcpServer => {
            let name = app.settings.mcp_name_input.trim().to_owned();
            let command = app.settings.mcp_command_input.trim().to_owned();
            if !name.is_empty() && !command.is_empty() {
                let args: Vec<String> = app
                    .settings
                    .mcp_args_input
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect();
                app.config.mcp_servers.retain(|s| s.name != name);
                app.config.mcp_servers.push(McpServerConfig {
                    name: name.clone(),
                    command,
                    args,
                });
                let _ = save_config_file(&app.paths.global_config, &app.config);
                app.settings.mcp_name_input.clear();
                app.settings.mcp_command_input.clear();
                app.settings.mcp_args_input.clear();
                app.settings.status_notice =
                    Some(format!("Added MCP server '{name}' to ~/.rein/config.json"));
            }
        }
        SettingsMessage::RemoveMcpServer(name) => {
            app.config.mcp_servers.retain(|s| s.name != name);
            let _ = save_config_file(&app.paths.global_config, &app.config);
            app.settings.status_notice = Some(format!("Removed MCP server '{name}'"));
        }
        SettingsMessage::MarketplaceInputChanged(v) => app.settings.marketplace_input = v,
        SettingsMessage::AddMarketplace => {
            let spec = app.settings.marketplace_input.trim().to_owned();
            if !spec.is_empty() {
                match app.plugin_manager.add_marketplace(&spec) {
                    Ok(mp) => {
                        app.settings.marketplace_input.clear();
                        app.settings
                            .refresh_disk_snapshots(&app.paths, &app.plugin_manager);
                        app.settings.status_notice =
                            Some(format!("Registered marketplace '{}'", mp.name));
                    }
                    Err(err) => {
                        app.settings.status_notice = Some(format!("Error: {err}"));
                    }
                }
            }
        }
        SettingsMessage::RefreshMarketplaces => {
            let names: Vec<String> = app
                .plugin_manager
                .list_marketplaces()
                .into_iter()
                .map(|m| m.name)
                .collect();
            let mut total = 0usize;
            for name in &names {
                if let Ok(c) = app.plugin_manager.refresh_marketplace(name) {
                    total += c;
                }
            }
            app.settings
                .refresh_disk_snapshots(&app.paths, &app.plugin_manager);
            app.settings.status_notice = Some(format!(
                "Synced {} marketplaces ({total} plugins available)",
                names.len()
            ));
        }
        SettingsMessage::RemoveMarketplace(name) => {
            let _ = app.plugin_manager.remove_marketplace(&name);
            app.settings
                .refresh_disk_snapshots(&app.paths, &app.plugin_manager);
            app.settings.status_notice = Some(format!("Removed marketplace '{name}'"));
        }
        SettingsMessage::InstallPlugin(name) => {
            if let Some(entry) = app
                .settings
                .discover_plugins
                .iter()
                .find(|p| p.name == name)
                .cloned()
            {
                match app.plugin_manager.install_plugin(&entry) {
                    Ok(rec) => {
                        app.settings
                            .refresh_disk_snapshots(&app.paths, &app.plugin_manager);
                        app.settings.status_notice =
                            Some(format!("Installed plugin '{}'", rec.name));
                    }
                    Err(err) => {
                        app.settings.status_notice = Some(format!("Install failed: {err}"));
                    }
                }
            }
        }
        SettingsMessage::TogglePlugin(name) => {
            if let Ok(enabled) = app.plugin_manager.toggle_plugin_enabled(&name) {
                app.settings
                    .refresh_disk_snapshots(&app.paths, &app.plugin_manager);
                app.settings.status_notice = Some(format!(
                    "Plugin '{name}' {}",
                    if enabled { "enabled" } else { "disabled" }
                ));
            }
        }
        SettingsMessage::UninstallPlugin(name) => {
            let _ = app.plugin_manager.uninstall_plugin(&name);
            app.settings
                .refresh_disk_snapshots(&app.paths, &app.plugin_manager);
            app.settings.status_notice = Some(format!("Uninstalled plugin '{name}'"));
        }
        SettingsMessage::ToggleDreamEnabled => {
            app.settings.dream_enabled = !app.settings.dream_enabled;
        }
        SettingsMessage::DreamMinHoursChanged(v) => app.settings.dream_min_hours_input = v,
        SettingsMessage::DreamMinSessionsChanged(v) => app.settings.dream_min_sessions_input = v,
        SettingsMessage::DreamMaxTurnsChanged(v) => app.settings.dream_max_turns_input = v,
        SettingsMessage::IcsDirChanged(v) => app.settings.ics_dir_input = v,
        SettingsMessage::MaildirChanged(v) => app.settings.maildir_input = v,
        SettingsMessage::SaveMemoryAndPersonalConfig => {
            let min_hours = app
                .settings
                .dream_min_hours_input
                .trim()
                .parse::<u64>()
                .unwrap_or(24);
            let min_sessions = app
                .settings
                .dream_min_sessions_input
                .trim()
                .parse::<usize>()
                .unwrap_or(5);
            let max_turns = app
                .settings
                .dream_max_turns_input
                .trim()
                .parse::<usize>()
                .unwrap_or(12);
            app.config.dream = Some(DreamConfig {
                enabled: app.settings.dream_enabled,
                min_hours,
                min_sessions,
                max_turns,
            });
            let ics = app.settings.ics_dir_input.trim();
            app.config.ics_dir = (!ics.is_empty()).then(|| PathBuf::from(ics));
            let mail = app.settings.maildir_input.trim();
            app.config.maildir = (!mail.is_empty()).then(|| PathBuf::from(mail));
            let _ = save_config_file(&app.paths.global_config, &app.config);
            app.settings.status_notice =
                Some("Saved AutoDream & Personal paths to ~/.rein/config.json".to_owned());
        }
    }
}

fn sync_policy_to_worker(app: &GuiApp) {
    let bypass = app.config.settings.default_permission_mode == "auto-bypass";
    app.worker.send(WorkerCommand::SyncPolicy {
        allowed_commands: app.settings.merged_allowed_commands(),
        bypass_permissions: bypass,
    });
}
