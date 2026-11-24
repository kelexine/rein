// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: AppState task manager, question prompt, config modal, and plugins modal handlers

use std::collections::BTreeSet;
use std::sync::mpsc;
use std::time::Instant;

use rein_tools::{QuestionAnswer, QuestionSpec, TaskKind, TaskStatus};

use crate::agent::{
    ApprovalAction, ApprovalDecision, ApprovalRequest, LiveSubagentStatus, SubagentRunState,
};
use crate::ui::entries::TranscriptEntry;
use crate::ui::mentions::{
    MentionCandidate, active_mention_query, collect_mention_candidates, filter_mention_candidates,
};

/// Active interactive 4-option permission prompt with inline `Tab` feedback editor.
use super::state::AppState;
use super::types::*;

impl AppState {
    /// Kills the currently highlighted running task or subagent in `/tasks` (`k` key).
    pub fn kill_selected_managed_task(&mut self) -> Option<Result<String, String>> {
        let items = self.all_task_entries();
        let mgr = self.pending_task_manager.as_ref()?;
        let entry = items.get(mgr.selected.min(items.len().saturating_sub(1)))?;
        let res = match entry.kind {
            TaskTargetKind::Subagent => {
                if let Some(worker_name) = entry
                    .title
                    .strip_prefix("Agent(swarm: ")
                    .and_then(|rest| rest.strip_suffix(')'))
                    && let Some(sw_mgr) = &self.swarm_manager
                    && sw_mgr.kill_worker("coordinator", worker_name).is_ok()
                {
                    Ok(format!("killed swarm worker {worker_name}"))
                } else if let Some(sub_mgr) = &self.subagent_manager
                    && sub_mgr.kill_tree(&entry.id).is_ok()
                {
                    Ok(format!("killed subagent tree {}", entry.title))
                } else {
                    self.subagent_tracker
                        .kill(&entry.id)
                        .map(|s| format!("killed subagent {}", s.role))
                }
            }
            TaskTargetKind::Shell | TaskTargetKind::Timer | TaskTargetKind::Cron => self
                .task_registry
                .kill_task(&entry.id)
                .map(|t| format!("killed {} ({})", t.id, t.label)),
        };
        Some(res)
    }

    /// Deletes the currently highlighted finished task or subagent from `/tasks` (`x` key).
    pub fn delete_selected_done_task(&mut self) -> Option<Result<String, String>> {
        let items = self.all_task_entries();
        let selected = self.pending_task_manager.as_ref()?.selected;
        if items.is_empty() {
            return None;
        }
        let idx = selected.min(items.len() - 1);
        let entry = &items[idx];
        let res = match entry.kind {
            TaskTargetKind::Subagent => self
                .subagent_tracker
                .delete_done(&entry.id)
                .map(|()| format!("deleted subagent {}", entry.title)),
            TaskTargetKind::Shell | TaskTargetKind::Timer | TaskTargetKind::Cron => self
                .task_registry
                .delete_done_task(&entry.id)
                .map(|()| format!("deleted {}", entry.id)),
        };
        let remaining = self.all_task_entries().len();
        if let Some(mgr) = &mut self.pending_task_manager {
            if remaining == 0 {
                mgr.selected = 0;
            } else {
                mgr.selected = mgr.selected.min(remaining - 1);
            }
        }
        Some(res)
    }

    /// Returns any active **Task Details** screen state.
    #[must_use]
    pub fn pending_task_details(&self) -> Option<&TaskDetailsView> {
        self.pending_task_details.as_ref()
    }

    /// Builds a live [`TaskDetailsSnapshot`] for the currently open Task Details screen.
    #[must_use]
    pub fn task_details_snapshot(&self) -> Option<TaskDetailsSnapshot> {
        let details = self.pending_task_details.as_ref()?;
        match details.kind {
            TaskTargetKind::Subagent => {
                let s = self.subagent_tracker.get(&details.target_id)?;
                Some(TaskDetailsSnapshot {
                    command: format!("Agent({}: {})", s.agent_type, s.role),
                    status_label: s.run_state.label().to_owned(),
                    type_label: details.kind.display_type().to_owned(),
                    started_full: s.started_full,
                    is_running: !s.run_state.is_done(),
                    is_error: matches!(
                        s.run_state,
                        SubagentRunState::Failed | SubagentRunState::Killed
                    ),
                    output_lines: s.output_lines,
                })
            }
            TaskTargetKind::Shell | TaskTargetKind::Timer | TaskTargetKind::Cron => {
                let t = self
                    .task_registry
                    .task_status(&details.target_id, 500)
                    .ok()?;
                let lines = self.task_registry.task_output_lines(&details.target_id);
                Some(TaskDetailsSnapshot {
                    command: t.label,
                    status_label: t.status.label().to_owned(),
                    type_label: details.kind.display_type().to_owned(),
                    started_full: t.started_full,
                    is_running: !t.status.is_done(),
                    is_error: matches!(
                        t.status,
                        TaskStatus::Failed | TaskStatus::Killed | TaskStatus::Cancelled
                    ),
                    output_lines: lines,
                })
            }
        }
    }

    /// Scrolls the active **Task Details** output pane vertically by `delta` lines.
    pub fn scroll_task_details(&mut self, delta: isize, total_lines: usize, view_height: usize) {
        let Some(details) = &mut self.pending_task_details else {
            return;
        };
        let max_scroll = total_lines.saturating_sub(view_height.max(1));
        let base = if details.follow_bottom {
            max_scroll
        } else {
            details.scroll.min(max_scroll)
        };
        let next = if delta < 0 {
            base.saturating_sub(delta.unsigned_abs())
        } else {
            base.saturating_add(delta.unsigned_abs()).min(max_scroll)
        };
        details.scroll = next;
        details.follow_bottom = next >= max_scroll;
    }

    /// Jumps to the top of **Task Details** output (`Ctrl+Home`).
    pub fn task_details_home(&mut self) {
        if let Some(details) = &mut self.pending_task_details {
            details.scroll = 0;
            details.follow_bottom = false;
        }
    }

    /// Jumps to the bottom of **Task Details** output and re-enables live follow (`Ctrl+End`).
    pub fn task_details_end(&mut self) {
        if let Some(details) = &mut self.pending_task_details {
            details.follow_bottom = true;
        }
    }

    /// Closes the **Task Details** view back to `/tasks` manager or chat.
    pub fn close_task_details(&mut self) {
        if let Some(details) = self.pending_task_details.take()
            && details.return_to_manager
        {
            self.pending_task_manager = Some(PendingTaskManager { selected: 0 });
        }
    }

    /// Opens an interactive `ask_question` picker dialog.
    pub fn open_question_picker(
        &mut self,
        questions: Vec<QuestionSpec>,
        reply_tx: mpsc::Sender<Option<Vec<QuestionAnswer>>>,
    ) {
        let len = questions.len();
        self.pending_question = Some(PendingQuestion {
            questions,
            current: 0,
            cursor: 0,
            selections: vec![BTreeSet::new(); len],
            custom_inputs: vec![String::new(); len],
            editing_other: false,
            reply_tx,
        });
    }

    /// Returns any active `ask_question` dialog state.
    #[must_use]
    pub fn pending_question(&self) -> Option<&PendingQuestion> {
        self.pending_question.as_ref()
    }

    /// Moves option cursor up/down in the active `ask_question` dialog.
    pub fn move_question_cursor(&mut self, delta: isize) {
        let Some(pq) = &mut self.pending_question else {
            return;
        };
        let Some(q) = pq.questions.get(pq.current) else {
            return;
        };
        // Options + 1 final "Other (custom)..." row.
        let count = q.options.len() + 1;
        let len = isize::try_from(count).unwrap_or(1);
        let cur = isize::try_from(pq.cursor.min(count - 1)).unwrap_or(0);
        pq.cursor = usize::try_from((cur + delta).rem_euclid(len)).unwrap_or(0);
        pq.editing_other = pq.cursor == q.options.len();
    }

    /// Toggles the highlighted option (Space in multi-select) or enters `"Other"` typing mode.
    pub fn toggle_question_option(&mut self) {
        let Some(pq) = &mut self.pending_question else {
            return;
        };
        let Some(q) = pq.questions.get(pq.current) else {
            return;
        };
        if pq.cursor >= q.options.len() {
            pq.editing_other = true;
            return;
        }
        let set = &mut pq.selections[pq.current];
        if q.is_multi_select {
            if !set.remove(&pq.cursor) {
                set.insert(pq.cursor);
            }
        } else {
            set.clear();
            set.insert(pq.cursor);
        }
    }

    /// Appends one character to the custom `"Other"` response for the current question.
    pub fn push_question_other_char(&mut self, ch: char) {
        let Some(pq) = &mut self.pending_question else {
            return;
        };
        if let Some(q) = pq.questions.get(pq.current)
            && pq.cursor == q.options.len()
        {
            pq.editing_other = true;
            pq.custom_inputs[pq.current].push(ch);
        }
    }

    /// Deletes one character from the custom `"Other"` response for the current question.
    pub fn pop_question_other_char(&mut self) {
        let Some(pq) = &mut self.pending_question else {
            return;
        };
        if pq.editing_other {
            pq.custom_inputs[pq.current].pop();
        }
    }

    /// Confirms the current question and either advances or delivers all answers back to `ask_question`.
    pub fn confirm_question_step(&mut self) {
        let Some(pq) = &mut self.pending_question else {
            return;
        };
        let Some(q) = pq.questions.get(pq.current) else {
            return;
        };
        if !q.is_multi_select && pq.cursor < q.options.len() && pq.selections[pq.current].is_empty()
        {
            pq.selections[pq.current].insert(pq.cursor);
        }
        if pq.current + 1 < pq.questions.len() {
            pq.current += 1;
            pq.cursor = 0;
            pq.editing_other = false;
            return;
        }

        let Some(done) = self.pending_question.take() else {
            return;
        };
        let answers: Vec<QuestionAnswer> = done
            .questions
            .iter()
            .enumerate()
            .map(|(idx, q)| {
                let selected: Vec<String> = done.selections[idx]
                    .iter()
                    .filter_map(|&opt_idx| q.options.get(opt_idx).cloned())
                    .collect();
                let custom = done.custom_inputs[idx].trim();
                QuestionAnswer {
                    question: q.question.clone(),
                    selected,
                    custom_text: (!custom.is_empty()).then(|| custom.to_owned()),
                }
            })
            .collect();
        let _ = done.reply_tx.send(Some(answers));
    }

    /// Cancels any open `ask_question` dialog cleanly.
    pub fn cancel_question_picker(&mut self) {
        if let Some(done) = self.pending_question.take() {
            let _ = done.reply_tx.send(None);
        }
    }

    /// Truncates the rendered transcript entries back to the `turn_index`-th user prompt.
    pub fn truncate_entries_to_user_turn(&mut self, turn_index: usize) {
        let mut seen = 0usize;
        let mut cut_idx = self.entries.len();
        for (idx, entry) in self.entries.iter().enumerate() {
            if matches!(entry, TranscriptEntry::UserTurn(_)) {
                if seen == turn_index {
                    cut_idx = idx;
                    break;
                }
                seen += 1;
            }
        }
        self.entries.truncate(cut_idx);
        self.auto_scroll = true;
    }

    /// Sets or clears an interactive 4-option approval prompt from the policy layer.
    pub fn set_pending_approval(&mut self, request: Option<ApprovalRequest>) {
        self.pending_approval = request.map(|req| PendingApproval {
            request: req,
            selected: 0,
            editing_feedback: false,
            feedback: String::new(),
        });
    }

    /// Returns any active interactive permission dialog state.
    #[must_use]
    pub fn pending_approval(&self) -> Option<&PendingApproval> {
        self.pending_approval.as_ref()
    }

    /// Moves the highlighted option inside the active approval dialog with wrap-around (`0..4`).
    pub fn move_approval_selection(&mut self, delta: isize) {
        let Some(pending) = &mut self.pending_approval else {
            return;
        };
        let count = 4isize;
        let current = isize::try_from(pending.selected).unwrap_or(0);
        let next = (current + delta).rem_euclid(count);
        pending.selected = usize::try_from(next).unwrap_or(0);
    }

    /// Toggles inline feedback editing (`Tab`) for whichever approval option is highlighted.
    pub fn toggle_approval_feedback(&mut self) {
        if let Some(pending) = &mut self.pending_approval {
            pending.editing_feedback = !pending.editing_feedback;
        }
    }

    /// Appends a character to the approval feedback buffer when editing via `Tab`.
    pub fn push_approval_feedback_char(&mut self, ch: char) {
        if let Some(pending) = &mut self.pending_approval {
            pending.feedback.push(ch);
        }
    }

    /// Pops the trailing character from the approval feedback buffer.
    pub fn pop_approval_feedback_char(&mut self) {
        if let Some(pending) = &mut self.pending_approval {
            let _ = pending.feedback.pop();
        }
    }

    /// Resolves and clears the active approval prompt, updating session status on CWD/Global persistence.
    pub fn take_approval_decision(
        &mut self,
        explicit_idx: Option<usize>,
    ) -> Option<ApprovalDecision> {
        let pending = self.pending_approval.take()?;
        let choice = explicit_idx.unwrap_or(pending.selected);
        let action = match choice {
            0 => ApprovalAction::AllowOnce,
            1 => {
                let pat = pending.request.pattern.clone();
                if !self.configured_commands.contains(&pat) {
                    self.configured_commands.push(pat.clone());
                }
                ApprovalAction::AllowCwd(pat)
            }
            2 => {
                let pat = pending.request.pattern.clone();
                if !self.configured_commands.contains(&pat) {
                    self.configured_commands.push(pat.clone());
                }
                ApprovalAction::AllowGlobal(pat)
            }
            _ => ApprovalAction::Deny,
        };
        let feedback = if pending.feedback.trim().is_empty() {
            None
        } else {
            Some(pending.feedback.trim().to_owned())
        };
        Some(ApprovalDecision { action, feedback })
    }

    /// Marks whether a background model turn, login, or compaction is running.
    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
        if busy {
            if self.busy_started_at.is_none() {
                self.busy_started_at = Some(Instant::now());
            }
        } else {
            self.busy_started_at = None;
            self.finalize_thinking();
            self.streaming_active = false;
            self.busy_status = "Thinking…".to_owned();
        }
    }

    /// Returns elapsed seconds since the current busy operation started.
    #[must_use]
    pub fn busy_elapsed_secs(&self) -> u64 {
        self.busy_started_at
            .map_or(0, |started| started.elapsed().as_secs())
    }

    /// Updates the concise status label next to the animated busy spinner.
    pub fn set_busy_status(&mut self, status: impl Into<String>) {
        self.busy_status = status.into();
    }

    /// Returns the active busy status message (`Thinking…`, `Running bash…`).
    #[must_use]
    pub fn busy_status(&self) -> &str {
        &self.busy_status
    }

    /// Advances the animated spinner frame counter while waiting on background work.
    pub fn advance_spinner(&mut self) {
        self.spinner_tick = self.spinner_tick.wrapping_add(1);
    }

    /// Returns the current spinner animation frame index.
    #[must_use]
    pub fn spinner_tick(&self) -> usize {
        self.spinner_tick
    }

    /// Returns `true` while a turn, login, or compaction is running in the background.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// Returns the effective vertical scroll offset against `max_scroll`.
    #[must_use]
    pub fn effective_scroll(&self, max_scroll: u16) -> u16 {
        if self.auto_scroll {
            max_scroll
        } else {
            self.scroll_offset.min(max_scroll)
        }
    }

    /// Re-enables sticky scroll-to-bottom tracking.
    pub fn scroll_to_bottom(&mut self) {
        self.auto_scroll = true;
    }

    /// Adjusts transcript scroll offset safely against the current viewport max scroll.
    pub fn scroll_by(&mut self, delta: i16, max_scroll: u16) {
        let base = if self.auto_scroll {
            max_scroll
        } else {
            self.scroll_offset.min(max_scroll)
        };
        if delta < 0 {
            self.scroll_offset = base.saturating_sub(delta.unsigned_abs());
            self.auto_scroll = false;
        } else {
            self.scroll_offset = base.saturating_add(delta.unsigned_abs()).min(max_scroll);
            if self.scroll_offset >= max_scroll {
                self.auto_scroll = true;
            }
        }
    }
}

pub(super) fn normalize_newlines(raw: &str) -> String {
    raw.replace("\r\n", "\n").replace('\r', "\n")
}

pub(super) fn subagent_to_entry(sub: &LiveSubagentStatus, now_secs: u64) -> TaskListEntry {
    let elapsed = sub
        .finished_unix_secs
        .unwrap_or(now_secs)
        .saturating_sub(sub.started_unix_secs);
    let is_running = !sub.run_state.is_done();
    let is_error = matches!(
        sub.run_state,
        SubagentRunState::Failed | SubagentRunState::Killed
    );
    let detail = if is_running {
        format!("{} · {} · {elapsed}s", sub.model_label, sub.status)
    } else {
        format!(
            "{} · {} · {elapsed}s",
            sub.model_label,
            sub.run_state.label()
        )
    };
    TaskListEntry {
        id: sub.id.clone(),
        kind: TaskTargetKind::Subagent,
        title: format!("Agent({}: {})", sub.agent_type, sub.role),
        detail,
        started_hms: sub.started_hms.clone(),
        started_full: sub.started_full.clone(),
        status_label: sub.run_state.label().to_owned(),
        is_running,
        is_error,
    }
}

pub(super) fn task_to_entry(task: &rein_tools::TaskSummary) -> TaskListEntry {
    let kind = match task.kind {
        TaskKind::Shell => TaskTargetKind::Shell,
        TaskKind::Timer => TaskTargetKind::Timer,
        TaskKind::Cron => TaskTargetKind::Cron,
    };
    let is_running = !task.status.is_done();
    let is_error = matches!(
        task.status,
        TaskStatus::Failed | TaskStatus::Killed | TaskStatus::Cancelled
    );
    TaskListEntry {
        id: task.id.clone(),
        kind,
        title: task.label.clone(),
        detail: task.status.label().to_owned(),
        started_hms: task.started_hms.clone(),
        started_full: task.started_full.clone(),
        status_label: task.status.label().to_owned(),
        is_running,
        is_error,
    }
}

pub(super) fn logical_line_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    for (idx, ch) in text.char_indices() {
        if ch == '\n' {
            ranges.push((start, idx));
            start = idx + 1;
        }
    }
    ranges.push((start, text.len()));
    ranges
}

impl AppState {
    // ── @ Mentions ───────────────────────────────────────────────────────────

    /// Refreshes cached `@` autocomplete candidates from `cwd`, agents, plugins, and MCP servers.
    pub fn refresh_mention_candidates(&mut self, cwd: &std::path::Path) {
        let mut extra_agents = Vec::new();
        if let Some(sub_mgr) = &self.subagent_manager {
            for def in sub_mgr.list_definitions() {
                extra_agents.push((def.name, def.description));
            }
        }
        let mut plugins_list = Vec::new();
        if let Some(pm) = &self.plugin_manager {
            let bundle = pm.collect_enabled_components();
            for ag in bundle.agents {
                extra_agents.push((ag.name, ag.description));
            }
            for rec in pm.installed_plugins() {
                if rec.enabled {
                    plugins_list.push((rec.name, rec.description));
                }
            }
        }
        let mcp_names: Vec<String> = self.mcp_servers.iter().map(|s| s.name.clone()).collect();
        self.mention_candidates = collect_mention_candidates(
            cwd,
            self.ui_settings.respect_gitignore_picker,
            &extra_agents,
            &plugins_list,
            &mcp_names,
        );
        self.mention_selected = 0;
    }

    /// Returns active `@query` matches when the `@` popup should be shown.
    #[must_use]
    pub fn active_mention_matches(&self) -> Option<(usize, String, Vec<&MentionCandidate>)> {
        if self.busy || self.input.starts_with('/') {
            return None;
        }
        let (at_idx, query) = active_mention_query(&self.input, self.input_cursor)?;
        if self.mention_dismissed_at == Some(at_idx) {
            return None;
        }
        let matches = filter_mention_candidates(&self.mention_candidates, &query);
        if matches.is_empty() {
            return None;
        }
        Some((at_idx, query, matches))
    }

    /// Returns the highlighted `@` candidate index.
    #[must_use]
    pub fn mention_selected(&self) -> usize {
        self.mention_selected
    }

    /// Moves the `@` popup selection up or down with wrap-around.
    pub fn move_mention_selection(&mut self, delta: isize) {
        let count = self.active_mention_matches().map_or(0, |(_, _, m)| m.len());
        if count == 0 {
            self.mention_selected = 0;
            return;
        }
        let len = count as isize;
        let current = (self.mention_selected.min(count - 1)) as isize;
        self.mention_selected = (current + delta).rem_euclid(len) as usize;
    }

    /// Inserts the highlighted `@` candidate into the input buffer and advances the cursor.
    pub fn accept_selected_mention(&mut self) -> bool {
        let Some((at_idx, _, matches)) = self.active_mention_matches() else {
            return false;
        };
        let Some(chosen) = matches.get(self.mention_selected).copied() else {
            return false;
        };
        let insert = format!("@{} ", chosen.insert_text);
        let tail = self.input[self.input_cursor..].to_owned();
        self.input.truncate(at_idx);
        self.input.push_str(&insert);
        self.input_cursor = self.input.len();
        self.input.push_str(&tail);
        self.mention_selected = 0;
        self.mention_dismissed_at = None;
        true
    }

    /// Dismisses the `@` popup at the current `@` token position.
    pub fn dismiss_mention_popup(&mut self) {
        if let Some((at_idx, _)) = active_mention_query(&self.input, self.input_cursor) {
            self.mention_dismissed_at = Some(at_idx);
        }
    }

    /// Clears `@` dismissal if the cursor is no longer on that `@` token.
    pub fn sync_mention_dismissal(&mut self) {
        if let Some(dismissed) = self.mention_dismissed_at {
            match active_mention_query(&self.input, self.input_cursor) {
                Some((at_idx, _)) if at_idx == dismissed => {}
                _ => self.mention_dismissed_at = None,
            }
        }
    }

    // ── /config modal ────────────────────────────────────────────────────────

    /// Opens the interactive `/config` modal.
    pub fn open_config_modal(&mut self) {
        self.pending_config_modal = Some(PendingConfigModal {
            tab: ConfigTab::Settings,
            search: String::new(),
            searching: false,
            selected: 0,
        });
    }

    /// Returns the active `/config` modal state.
    #[must_use]
    pub fn pending_config_modal(&self) -> Option<&PendingConfigModal> {
        self.pending_config_modal.as_ref()
    }

    /// Mutable access to `/config` modal state.
    pub fn pending_config_modal_mut(&mut self) -> Option<&mut PendingConfigModal> {
        self.pending_config_modal.as_mut()
    }

    /// Closes `/config` modal.
    pub fn close_config_modal(&mut self) {
        self.pending_config_modal = None;
    }

    /// Returns filtered setting rows for `/config`.
    #[must_use]
    pub fn config_setting_rows(&self) -> Vec<ConfigSettingRow> {
        let on_off = |b: bool| if b { "on" } else { "off" }.to_owned();
        let has_override = |k: &str| self.repo_override_keys.iter().any(|r| r == k);
        let all = vec![
            ConfigSettingRow {
                key: "auto_compact",
                label: "Auto-compact",
                value: on_off(self.ui_settings.auto_compact),
                repo_overridden: false,
            },
            ConfigSettingRow {
                key: "thinking_mode",
                label: "Thinking mode",
                value: on_off(self.ui_settings.thinking_mode),
                repo_overridden: false,
            },
            ConfigSettingRow {
                key: "verbose_output",
                label: "Verbose output",
                value: on_off(self.ui_settings.verbose_output),
                repo_overridden: false,
            },
            ConfigSettingRow {
                key: "show_turn_duration",
                label: "Show turn duration",
                value: on_off(self.ui_settings.show_turn_duration),
                repo_overridden: false,
            },
            ConfigSettingRow {
                key: "auto_scroll",
                label: "Auto-scroll",
                value: on_off(self.ui_settings.auto_scroll),
                repo_overridden: false,
            },
            ConfigSettingRow {
                key: "respect_gitignore_picker",
                label: "Respect .gitignore in file picker",
                value: on_off(self.ui_settings.respect_gitignore_picker),
                repo_overridden: false,
            },
            ConfigSettingRow {
                key: "default_permission_mode",
                label: "Default permission mode",
                value: self.ui_settings.default_permission_mode.clone(),
                repo_overridden: false,
            },
            ConfigSettingRow {
                key: "editor_mode",
                label: "Editor mode",
                value: self.ui_settings.editor_mode.clone(),
                repo_overridden: false,
            },
            ConfigSettingRow {
                key: "queued_messages",
                label: "Queued Messages",
                value: if self.ui_settings.queued_messages == "queue" {
                    "Queue".to_owned()
                } else {
                    "Send Immediately".to_owned()
                },
                repo_overridden: false,
            },
            ConfigSettingRow {
                key: "show_tips",
                label: "Show tips",
                value: on_off(self.ui_settings.show_tips),
                repo_overridden: false,
            },
            ConfigSettingRow {
                key: "model",
                label: "Model",
                value: format!("{} >", self.model_name),
                repo_overridden: has_override("model"),
            },
        ];
        let query = self
            .pending_config_modal
            .as_ref()
            .map(|m| m.search.trim().to_ascii_lowercase())
            .unwrap_or_default();
        if query.is_empty() {
            all
        } else {
            all.into_iter()
                .filter(|r| {
                    r.label.to_ascii_lowercase().contains(&query)
                        || r.value.to_ascii_lowercase().contains(&query)
                })
                .collect()
        }
    }

    /// Toggles or cycles the selected `/config` setting and returns `(key, open_model_picker)`.
    pub fn activate_selected_config_setting(&mut self) -> Option<(&'static str, bool)> {
        let rows = self.config_setting_rows();
        let sel = self.pending_config_modal.as_ref()?.selected;
        let row = rows.get(sel)?;
        let key = row.key;
        match key {
            "auto_compact" => self.ui_settings.auto_compact = !self.ui_settings.auto_compact,
            "thinking_mode" => self.ui_settings.thinking_mode = !self.ui_settings.thinking_mode,
            "verbose_output" => {
                self.ui_settings.verbose_output = !self.ui_settings.verbose_output;
                self.expand_outputs = self.ui_settings.verbose_output;
            }
            "show_turn_duration" => {
                self.ui_settings.show_turn_duration = !self.ui_settings.show_turn_duration;
            }
            "auto_scroll" => {
                self.ui_settings.auto_scroll = !self.ui_settings.auto_scroll;
                self.auto_scroll = self.ui_settings.auto_scroll;
            }
            "respect_gitignore_picker" => {
                self.ui_settings.respect_gitignore_picker =
                    !self.ui_settings.respect_gitignore_picker;
            }
            "default_permission_mode" => {
                self.ui_settings.default_permission_mode =
                    if self.ui_settings.default_permission_mode == "policy-gated" {
                        "auto-bypass".to_owned()
                    } else {
                        "policy-gated".to_owned()
                    };
            }
            "editor_mode" => {
                self.ui_settings.editor_mode = if self.ui_settings.editor_mode == "normal" {
                    "vi".to_owned()
                } else {
                    "normal".to_owned()
                };
            }
            "queued_messages" => {
                self.ui_settings.queued_messages = if self.ui_settings.queued_messages == "queue" {
                    "send_immediately".to_owned()
                } else {
                    "queue".to_owned()
                };
            }
            "show_tips" => self.ui_settings.show_tips = !self.ui_settings.show_tips,
            "model" => return Some((key, true)),
            _ => {}
        }
        Some((key, false))
    }

    // ── /plugins modal ───────────────────────────────────────────────────────

    /// Opens the interactive `/plugins` modal.
    pub fn open_plugins_modal(&mut self) {
        self.pending_plugins_modal = Some(PendingPluginsModal {
            tab: PluginsTab::Discover,
            search: String::new(),
            searching: false,
            adding_marketplace: false,
            marketplace_input: String::new(),
            selected: 0,
            marketplace_filter: None,
            status_notice: None,
        });
    }

    /// Returns the active `/plugins` modal state.
    #[must_use]
    pub fn pending_plugins_modal(&self) -> Option<&PendingPluginsModal> {
        self.pending_plugins_modal.as_ref()
    }

    /// Mutable access to `/plugins` modal state.
    pub fn pending_plugins_modal_mut(&mut self) -> Option<&mut PendingPluginsModal> {
        self.pending_plugins_modal.as_mut()
    }

    /// Closes `/plugins` modal.
    pub fn close_plugins_modal(&mut self) {
        self.pending_plugins_modal = None;
    }

    // ── Queued / Steering Messages ───────────────────────────────────────────

    /// Enqueues a `(display, resolved_prompt)` follow-up message while the model is busy.
    pub fn enqueue_queued_message(&mut self, display: String, resolved_prompt: String) {
        self.queued_messages.push((display, resolved_prompt));
    }

    /// Pops the oldest queued message for immediate steering (`Alt+M`) or post-turn execution.
    pub fn take_next_queued_message(&mut self) -> Option<(String, String)> {
        if self.queued_messages.is_empty() {
            None
        } else {
            Some(self.queued_messages.remove(0))
        }
    }

    /// Pops the most recently queued message back into the input box for editing (`Up`).
    pub fn pop_last_queued_for_edit(&mut self) -> Option<String> {
        self.queued_messages.pop().map(|(display, _)| display)
    }

    /// Returns queued follow-up messages `(display, resolved_prompt)`.
    #[must_use]
    pub fn queued_messages(&self) -> &[(String, String)] {
        &self.queued_messages
    }

    /// Records a steering message currently being delivered (`Sending to agent...`).
    pub fn push_steering_pending(&mut self, display: String) {
        self.steering_pending.push(display);
    }

    /// Marks the oldest pending steering message as delivered into the active turn.
    pub fn mark_steering_delivered(&mut self, display: &str) {
        if let Some(pos) = self.steering_pending.iter().position(|s| s == display) {
            self.steering_pending.remove(pos);
        } else if !self.steering_pending.is_empty() {
            self.steering_pending.remove(0);
        }
        self.push_user_turn(display);
    }

    /// Clears any undelivered steering-pending indicators (e.g. when a turn ends).
    pub fn clear_steering_pending(&mut self) {
        self.steering_pending.clear();
    }

    /// Returns steering messages currently waiting to be injected into the active turn.
    #[must_use]
    pub fn steering_pending(&self) -> &[String] {
        &self.steering_pending
    }
}

pub(super) fn extract_background_task_id(output_content: &str) -> Option<String> {
    let val: serde_json::Value = serde_json::from_str(output_content).ok()?;
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
