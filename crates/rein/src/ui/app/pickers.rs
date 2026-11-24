// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: AppState input editing, history navigation, and model/session/rewind/mention pickers

use std::time::{Duration, Instant};

use rein_core::AvailableModel;

use crate::agent::{TurnCheckpoint, now_unix_secs};
use crate::config::{ProviderKind, SessionSummary};

/// Active interactive 4-option permission prompt with inline `Tab` feedback editor.
use super::modals::{logical_line_ranges, normalize_newlines, subagent_to_entry, task_to_entry};
use super::state::{AppState, is_model_active, model_vendor_group};
use super::types::*;

impl AppState {
    pub(super) fn clamped_cursor(&self) -> usize {
        let mut idx = self.input_cursor.min(self.input.len());
        while idx > 0 && !self.input.is_char_boundary(idx) {
            idx -= 1;
        }
        idx
    }

    /// Replaces the input buffer cleanly (e.g. on `Tab` slash completion).
    pub fn set_input(&mut self, text: impl Into<String>) {
        self.input = normalize_newlines(&text.into());
        self.input_cursor = self.input.len();
        self.slash_selected = 0;
        self.history_cursor = None;
        self.draft_input.clear();
    }

    /// Inserts a typed character at the cursor inside the input buffer.
    pub fn push_input_char(&mut self, ch: char) {
        let normalized = if ch == '\r' { '\n' } else { ch };
        let idx = self.clamped_cursor();
        self.input.insert(idx, normalized);
        self.input_cursor = idx + normalized.len_utf8();
        self.slash_selected = 0;
        self.history_cursor = None;
    }

    /// Inserts pasted multi-character/multiline text at the cursor without submitting.
    pub fn push_input_str(&mut self, raw: &str) {
        let normalized = normalize_newlines(raw);
        if normalized.is_empty() {
            return;
        }
        let idx = self.clamped_cursor();
        self.input.insert_str(idx, &normalized);
        self.input_cursor = idx + normalized.len();
        self.slash_selected = 0;
        self.history_cursor = None;
    }

    /// Removes the character immediately preceding the cursor.
    pub fn pop_input_char(&mut self) {
        let idx = self.clamped_cursor();
        if idx == 0 {
            return;
        }
        if let Some((start, _)) = self.input[..idx].char_indices().next_back() {
            let _ = self.input.remove(start);
            self.input_cursor = start;
        }
        self.slash_selected = 0;
        self.history_cursor = None;
    }

    /// Removes the character at the cursor (`Delete` key).
    pub fn delete_input_char(&mut self) {
        let idx = self.clamped_cursor();
        if idx < self.input.len() {
            let _ = self.input.remove(idx);
        }
        self.slash_selected = 0;
        self.history_cursor = None;
    }

    /// Moves the input cursor one character to the left.
    pub fn move_cursor_left(&mut self) {
        let idx = self.clamped_cursor();
        if let Some((prev, _)) = self.input[..idx].char_indices().next_back() {
            self.input_cursor = prev;
        }
    }

    /// Moves the input cursor one character to the right.
    pub fn move_cursor_right(&mut self) {
        let idx = self.clamped_cursor();
        if let Some(ch) = self.input[idx..].chars().next() {
            self.input_cursor = idx + ch.len_utf8();
        }
    }

    /// Moves the input cursor to the start of its current line.
    pub fn move_cursor_home(&mut self) {
        let idx = self.clamped_cursor();
        let line_start = self.input[..idx].rfind('\n').map_or(0, |pos| pos + 1);
        self.input_cursor = line_start;
    }

    /// Moves the input cursor to the end of its current line.
    pub fn move_cursor_end(&mut self) {
        let idx = self.clamped_cursor();
        let line_end = self.input[idx..]
            .find('\n')
            .map_or(self.input.len(), |rel| idx + rel);
        self.input_cursor = line_end;
    }

    /// Moves the cursor up (`-1`) or down (`+1`) inside multiline input, returning `false` at edges.
    pub fn move_cursor_vertical(&mut self, delta: isize) -> bool {
        if delta == 0 || !self.input.contains('\n') {
            return false;
        }
        let idx = self.clamped_cursor();
        let ranges = logical_line_ranges(&self.input);
        let Some(cur_row) = ranges
            .iter()
            .position(|(start, end)| idx >= *start && idx <= *end)
        else {
            return false;
        };
        let target_row = if delta < 0 {
            if cur_row == 0 {
                return false;
            }
            cur_row - 1
        } else {
            if cur_row + 1 >= ranges.len() {
                return false;
            }
            cur_row + 1
        };
        let col_chars = self.input[ranges[cur_row].0..idx].chars().count();
        let (t_start, t_end) = ranges[target_row];
        let offset = self.input[t_start..t_end]
            .char_indices()
            .nth(col_chars)
            .map_or(t_end - t_start, |(b, _)| b);
        self.input_cursor = t_start + offset;
        true
    }

    /// Takes and clears the current input buffer.
    pub fn take_input(&mut self) -> String {
        self.slash_selected = 0;
        self.history_cursor = None;
        self.input_cursor = 0;
        self.draft_input.clear();
        std::mem::take(&mut self.input)
    }

    /// Records a consecutive `Ctrl+C`; returns `true` when pressed twice within 2 seconds.
    pub fn arm_or_confirm_ctrl_c(&mut self, now: Instant) -> bool {
        let confirmed = self
            .ctrl_c_armed_at
            .is_some_and(|prev| now.duration_since(prev) <= Duration::from_secs(2));
        if confirmed {
            self.ctrl_c_armed_at = None;
            true
        } else {
            self.ctrl_c_armed_at = Some(now);
            false
        }
    }

    /// Clears the pending double-`Ctrl+C` confirmation window when any other key is pressed.
    pub fn clear_ctrl_c_arm(&mut self) {
        self.ctrl_c_armed_at = None;
    }

    /// Returns `true` when one `Ctrl+C` was pressed within the last 2 seconds.
    #[must_use]
    pub fn ctrl_c_exit_armed(&self) -> bool {
        self.ctrl_c_armed_at
            .is_some_and(|prev| prev.elapsed() <= Duration::from_secs(2))
    }

    /// Navigates previously submitted prompts in the input box (`-1` older, `+1` newer), restoring the live draft at the end.
    pub fn navigate_history(&mut self, delta: isize) {
        if self.prompt_history.is_empty() || delta == 0 {
            return;
        }
        let next_idx = if delta < 0 {
            match self.history_cursor {
                None => {
                    self.draft_input = self.input.clone();
                    Some(self.prompt_history.len().saturating_sub(1))
                }
                Some(idx) => Some(idx.saturating_sub(1)),
            }
        } else {
            match self.history_cursor {
                None => return,
                Some(idx) if idx + 1 < self.prompt_history.len() => Some(idx + 1),
                Some(_) => None,
            }
        };
        self.history_cursor = next_idx;
        self.input = match next_idx {
            Some(idx) => self.prompt_history.get(idx).cloned().unwrap_or_default(),
            None => std::mem::take(&mut self.draft_input),
        };
        self.input_cursor = self.input.len();
        self.slash_selected = 0;
    }

    /// Opens the interactive `/model` selector populated from server models, grouped by vendor.
    pub fn open_model_picker(
        &mut self,
        mut items: Vec<AvailableModel>,
        provider_kind: ProviderKind,
        current_model: &str,
    ) {
        items.sort_by_key(|m| model_vendor_group(&m.id).1);
        let selected = items
            .iter()
            .position(|m| is_model_active(current_model, &m.id))
            .unwrap_or(0);
        self.pending_model_picker = Some(PendingModelPicker {
            items,
            selected,
            provider_kind,
        });
    }

    /// Returns any active interactive `/model` picker state.
    #[must_use]
    pub fn pending_model_picker(&self) -> Option<&PendingModelPicker> {
        self.pending_model_picker.as_ref()
    }

    /// Moves selection up/down inside the interactive `/model` picker with wrap-around.
    pub fn move_model_selection(&mut self, delta: isize) {
        let Some(picker) = &mut self.pending_model_picker else {
            return;
        };
        let count = picker.items.len();
        if count == 0 {
            picker.selected = 0;
            return;
        }
        let len = isize::try_from(count).unwrap_or(1);
        let current = isize::try_from(picker.selected.min(count - 1)).unwrap_or(0);
        picker.selected = usize::try_from((current + delta).rem_euclid(len)).unwrap_or(0);
    }

    /// Closes the interactive `/model` picker and returns the chosen `(ProviderKind, AvailableModel)`.
    pub fn take_selected_model(&mut self) -> Option<(ProviderKind, AvailableModel)> {
        let picker = self.pending_model_picker.take()?;
        let chosen = picker.items.into_iter().nth(picker.selected)?;
        Some((picker.provider_kind, chosen))
    }

    /// Dismisses the interactive `/model` picker without making changes.
    pub fn clear_model_picker(&mut self) {
        self.pending_model_picker = None;
    }

    /// Opens the interactive `/resume` selector populated from sessions in the current directory.
    pub fn open_session_picker(&mut self, items: Vec<SessionSummary>) {
        let selected = items
            .iter()
            .position(|s| s.uid == self.session_id)
            .unwrap_or(0);
        self.pending_session_picker = Some(PendingSessionPicker { items, selected });
    }

    /// Returns any active interactive `/resume` session picker state.
    #[must_use]
    pub fn pending_session_picker(&self) -> Option<&PendingSessionPicker> {
        self.pending_session_picker.as_ref()
    }

    /// Moves selection up/down inside the interactive `/resume` picker with wrap-around.
    pub fn move_session_selection(&mut self, delta: isize) {
        let Some(picker) = &mut self.pending_session_picker else {
            return;
        };
        let count = picker.items.len();
        if count == 0 {
            picker.selected = 0;
            return;
        }
        let len = isize::try_from(count).unwrap_or(1);
        let current = isize::try_from(picker.selected.min(count - 1)).unwrap_or(0);
        picker.selected = usize::try_from((current + delta).rem_euclid(len)).unwrap_or(0);
    }

    /// Closes the interactive `/resume` picker and returns the chosen [`SessionSummary`].
    pub fn take_selected_session(&mut self) -> Option<SessionSummary> {
        let picker = self.pending_session_picker.take()?;
        picker.items.into_iter().nth(picker.selected)
    }

    /// Dismisses the interactive `/resume` picker without making changes.
    pub fn clear_session_picker(&mut self) {
        self.pending_session_picker = None;
    }

    /// Opens the interactive `/rewind` selector populated from recorded turn checkpoints.
    pub fn open_rewind_picker(&mut self, items: Vec<TurnCheckpoint>) {
        let selected = items.len();
        self.pending_rewind_picker = Some(PendingRewindPicker {
            items,
            selected,
            step: RewindStep::SelectTurn,
        });
    }

    /// Returns any active interactive `/rewind` picker state.
    #[must_use]
    pub fn pending_rewind_picker(&self) -> Option<&PendingRewindPicker> {
        self.pending_rewind_picker.as_ref()
    }

    /// Moves selection up/down inside the interactive `/rewind` picker with wrap-around.
    pub fn move_rewind_selection(&mut self, delta: isize) {
        let Some(picker) = &mut self.pending_rewind_picker else {
            return;
        };
        let count = match picker.step {
            RewindStep::SelectTurn => picker.items.len() + 1,
            RewindStep::SelectAction { turn_index } => {
                let has_snapshot = picker
                    .items
                    .get(turn_index)
                    .is_some_and(|c| c.snapshot.is_some());
                if has_snapshot { 2 } else { 1 }
            }
        };
        if count == 0 {
            picker.selected = 0;
            return;
        }
        let len = isize::try_from(count).unwrap_or(1);
        let current = isize::try_from(picker.selected.min(count - 1)).unwrap_or(0);
        picker.selected = usize::try_from((current + delta).rem_euclid(len)).unwrap_or(0);
    }

    /// Advances `/rewind` from turn selection to action selection, or returns `(turn_index, restore_code)` on confirm.
    pub fn confirm_rewind_step(&mut self) -> Option<(usize, bool)> {
        let picker = self.pending_rewind_picker.as_mut()?;
        match picker.step {
            RewindStep::SelectTurn => {
                if picker.selected >= picker.items.len() {
                    self.pending_rewind_picker = None;
                    return None;
                }
                let turn_index = picker.selected;
                let has_snapshot = picker
                    .items
                    .get(turn_index)
                    .is_some_and(|c| c.snapshot.is_some());
                picker.step = RewindStep::SelectAction { turn_index };
                picker.selected = usize::from(has_snapshot);
                None
            }
            RewindStep::SelectAction { turn_index } => {
                let has_snapshot = picker
                    .items
                    .get(turn_index)
                    .is_some_and(|c| c.snapshot.is_some());
                let restore_code = has_snapshot && picker.selected == 1;
                self.pending_rewind_picker = None;
                Some((turn_index, restore_code))
            }
        }
    }

    /// Cancels `/rewind` step.
    pub fn cancel_rewind_step(&mut self) {
        let Some(picker) = &mut self.pending_rewind_picker else {
            return;
        };
        match picker.step {
            RewindStep::SelectTurn => {
                self.pending_rewind_picker = None;
            }
            RewindStep::SelectAction { turn_index } => {
                picker.step = RewindStep::SelectTurn;
                picker.selected = turn_index;
            }
        }
    }

    /// Returns currently running subagents and background tasks rendered in the bottom strip below the input box.
    #[must_use]
    pub fn running_strip_items(&self) -> Vec<TaskListEntry> {
        let now = now_unix_secs();
        let mut items: Vec<TaskListEntry> = self
            .subagent_tracker
            .snapshot()
            .into_iter()
            .map(|s| subagent_to_entry(&s, now))
            .collect();
        for t in self.task_registry.list_tasks() {
            if !t.status.is_done() {
                items.push(task_to_entry(&t));
            }
        }
        items
    }

    /// Returns all tracked subagents and background tasks (running + done) for `/tasks` management.
    #[must_use]
    pub fn all_task_entries(&self) -> Vec<TaskListEntry> {
        let now = now_unix_secs();
        let mut items: Vec<TaskListEntry> = self
            .subagent_tracker
            .all_records()
            .into_iter()
            .map(|s| subagent_to_entry(&s, now))
            .collect();
        for t in self.task_registry.list_tasks() {
            items.push(task_to_entry(&t));
        }
        items
    }

    /// Returns the currently highlighted index inside the bottom live task strip, if focused.
    #[must_use]
    pub fn strip_selected(&self) -> Option<usize> {
        let items = self.running_strip_items();
        if items.is_empty() {
            return None;
        }
        self.strip_selected.map(|idx| idx.min(items.len() - 1))
    }

    /// Moves selection down/up inside the bottom live strip (`Up` from index `0` returns focus to input).
    pub fn move_strip_selection(&mut self, delta: isize) -> bool {
        let count = self.running_strip_items().len();
        if count == 0 {
            self.strip_selected = None;
            return false;
        }
        match self.strip_selected {
            None => {
                if delta > 0 {
                    self.strip_selected = Some(0);
                    true
                } else {
                    false
                }
            }
            Some(cur) => {
                if delta < 0 && cur == 0 {
                    self.strip_selected = None;
                    return true;
                }
                let max_idx = count.saturating_sub(1);
                let next = if delta < 0 {
                    cur.saturating_sub(delta.unsigned_abs())
                } else {
                    cur.saturating_add(delta.unsigned_abs()).min(max_idx)
                };
                self.strip_selected = Some(next);
                true
            }
        }
    }

    /// Returns focus from the bottom live strip back to the input box.
    pub fn clear_strip_selection(&mut self) {
        self.strip_selected = None;
    }

    /// Opens the **Task Details** screen for the currently highlighted item in the bottom strip.
    pub fn open_selected_strip_details(&mut self) -> bool {
        let items = self.running_strip_items();
        let Some(idx) = self.strip_selected() else {
            return false;
        };
        let Some(entry) = items.get(idx) else {
            return false;
        };
        self.strip_selected = None;
        self.pending_task_details = Some(TaskDetailsView {
            target_id: entry.id.clone(),
            kind: entry.kind,
            scroll: 0,
            follow_bottom: true,
            return_to_manager: false,
        });
        true
    }

    /// Opens the interactive `/tasks` manager overlay (`k` kills running, `x` deletes done).
    pub fn open_task_manager(&mut self) {
        self.strip_selected = None;
        self.pending_task_details = None;
        self.pending_task_manager = Some(PendingTaskManager { selected: 0 });
    }

    /// Returns any active `/tasks` manager overlay state.
    #[must_use]
    pub fn pending_task_manager(&self) -> Option<&PendingTaskManager> {
        self.pending_task_manager.as_ref()
    }

    /// Moves selection up/down inside `/tasks` manager with wrap-around.
    pub fn move_task_manager_selection(&mut self, delta: isize) {
        let count = self.all_task_entries().len();
        let Some(mgr) = &mut self.pending_task_manager else {
            return;
        };
        if count == 0 {
            mgr.selected = 0;
            return;
        }
        let len = isize::try_from(count).unwrap_or(1);
        let current = isize::try_from(mgr.selected.min(count - 1)).unwrap_or(0);
        mgr.selected = usize::try_from((current + delta).rem_euclid(len)).unwrap_or(0);
    }

    /// Closes the `/tasks` manager overlay.
    pub fn close_task_manager(&mut self) {
        self.pending_task_manager = None;
    }

    /// Opens the **Task Details** view for the selected row in `/tasks` manager.
    pub fn open_selected_manager_details(&mut self) -> bool {
        let items = self.all_task_entries();
        let Some(mgr) = &self.pending_task_manager else {
            return false;
        };
        if items.is_empty() {
            return false;
        }
        let entry = &items[mgr.selected.min(items.len() - 1)];
        self.pending_task_details = Some(TaskDetailsView {
            target_id: entry.id.clone(),
            kind: entry.kind,
            scroll: 0,
            follow_bottom: true,
            return_to_manager: true,
        });
        true
    }
}
