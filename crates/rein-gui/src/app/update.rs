// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Elm update() logic handling UI messages, worker events, auto-scroll, and inspector actions

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use iced::Task;
use iced::clipboard;
use iced::widget::markdown;
use iced::widget::operation::{RelativeOffset, snap_to};
use rein::agent::ApprovalDecision;
use rein_core::{ToolCall, ToolOutput};
use rein_tools::{QuestionAnswer, TaskNotification, TaskStatus};

use crate::worker::{PendingApprovalPrompt, PendingQuestionPrompt, WorkerCommand, WorkerEvent};

use super::message::Message;
use super::state::{GuiApp, InspectorPane, ToolItemState, TranscriptItem, collapse_preview};

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
            if trimmed.is_empty() || app.busy {
                return Task::none();
            }
            app.input.clear();
            app.model_picker_open = false;
            app.plus_menu_open = false;
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
            app.items.push(TranscriptItem::user(prompt.clone()));
            begin_busy(app, "Thinking…");
            app.worker.send(WorkerCommand::RunPrompt { prompt });
            scroll_transcript_to_bottom(app)
        }
        Message::CancelTurn => {
            if app.busy {
                app.worker.cancel_turn();
                app.busy_status = "Stopping…".to_owned();
            } else if app.plus_menu_open {
                app.plus_menu_open = false;
            } else if app.model_picker_open {
                app.model_picker_open = false;
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
                app.inspector = None;
                app.worker.send(WorkerCommand::NewSession);
            }
            Task::none()
        }
        Message::ResumeSession(uid) => {
            if !app.busy {
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
        Message::OpenInspector {
            title,
            subtitle,
            content,
        } => {
            app.inspector = Some(InspectorPane {
                title,
                subtitle,
                content,
            });
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
            app.question_selected = Some(opt_idx);
            Task::none()
        }
        Message::ConfirmQuestion => confirm_question_step(app),
        Message::CancelQuestion => {
            if let Some(q) = app.pending_question.take() {
                let _ = q.reply_tx.send(None);
            }
            app.question_answers.clear();
            app.question_step = 0;
            app.question_selected = None;
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
        if matches!(
            ev,
            WorkerEvent::TextDelta(_)
                | WorkerEvent::ToolStart(_)
                | WorkerEvent::ToolFinish(_, _)
                | WorkerEvent::SessionHydrated { .. }
        ) {
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
        WorkerEvent::ToolFinish(call, output) => finish_tool(app, &call, &output),
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
            app.question_selected = Some(0);
        }
        WorkerEvent::ModelsLoaded(models) => app.available_models = models,
        WorkerEvent::ModelSelected(label) => app.model_label = label,
        WorkerEvent::SessionHydrated {
            session_uid,
            events,
            profile,
        } => {
            app.session_uid = session_uid;
            app.active_profile = *profile;
            app.items.clear();
            app.apply_hydrated(events);
            app.refresh_sessions();
        }
        WorkerEvent::SessionReset {
            session_uid,
            profile,
        } => {
            app.session_uid = session_uid;
            app.active_profile = *profile;
            app.items.clear();
            app.usage = rein_core::UsageStats::default();
            app.refresh_sessions();
        }
        WorkerEvent::Note(note) => app.items.push(TranscriptItem::System(note)),
        WorkerEvent::Error(err) => app.items.push(TranscriptItem::Error(err)),
        WorkerEvent::TurnFinished => {
            finalize_thinking(app);
            flush_trailing_markdown(app);
            app.busy = false;
            app.busy_flag.store(false, Ordering::Release);
            app.busy_started_at = None;
            app.busy_status = "Thinking…".to_owned();
            app.refresh_sessions();
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
    app.items.push(TranscriptItem::Tool {
        call_id: call.id.clone(),
        name: call.name.clone(),
        arguments: call.arguments.clone(),
        state: ToolItemState::Running,
        background_task_id: None,
        output: String::new(),
        expanded: false,
    });
}

fn finish_tool(app: &mut GuiApp, call: &ToolCall, output: &ToolOutput) {
    app.busy_status = "Thinking…".to_owned();
    let bg_id = (!output.is_error)
        .then(|| extract_background_id(&output.content))
        .flatten();
    if let Some(TranscriptItem::Tool {
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
    if let Some(TranscriptItem::Tool { state, output, .. }) =
        app.items.iter_mut().rev().find(|item| {
            matches!(
                item,
                TranscriptItem::Tool {
                    background_task_id: Some(id),
                    ..
                } if id == task_id
            )
        })
    {
        *state = if is_error {
            ToolItemState::Error
        } else {
            ToolItemState::Done
        };
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

    if app.task_drawer_open {
        app.tasks_snapshot = app.task_registry.list_tasks();
    }
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
    let selected_label = app
        .question_selected
        .and_then(|idx| q.options.get(idx))
        .cloned()
        .unwrap_or_else(|| q.options.first().cloned().unwrap_or_default());
    app.question_answers.push(QuestionAnswer {
        question: q.question.clone(),
        selected: vec![selected_label],
        custom_text: None,
    });
    if app.question_step + 1 < pending.questions.len() {
        app.question_step += 1;
        app.question_selected = Some(0);
        return Task::none();
    }
    if let Some(done) = app.pending_question.take() {
        let answers = std::mem::take(&mut app.question_answers);
        let _ = done.reply_tx.send(Some(answers));
    }
    app.question_step = 0;
    app.question_selected = None;
    Task::none()
}
