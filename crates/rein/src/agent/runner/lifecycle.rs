// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Session hydration, compaction, memory query, and rewind lifecycle on AgentLoop

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use rein_core::{
    EventLog, MemoryScope, Message, Provider, ProviderStreamEvent, Role, SessionEvent, ToolCall,
};
use rein_tools::MemoryRecord;

use crate::agent::snapshot::{
    TurnCheckpoint, checkpoints_path_for, load_checkpoints, restore_workspace_snapshot,
    truncate_jsonl_lines,
};
use crate::modes::ModeManager;

use super::agent_loop::{AgentLoop, HydratedEvent};
use super::batch::flush_tool_round;
use super::format::{COMPACT_SYSTEM_PROMPT, format_history_for_summary, format_tool_status};

impl AgentLoop {
    /// Returns recorded per-turn checkpoints.
    #[must_use]
    pub fn checkpoints(&self) -> &[TurnCheckpoint] {
        &self.checkpoints
    }

    /// Rewinds conversation history, session logs, and optionally workspace code to `turn_index`.
    pub fn rewind_to(&mut self, turn_index: usize, restore_code: bool) -> Result<TurnCheckpoint> {
        let checkpoint = self
            .checkpoints
            .get(turn_index)
            .cloned()
            .with_context(|| format!("invalid rewind checkpoint index {turn_index}"))?;
        if restore_code && let Some(snapshot) = &checkpoint.snapshot {
            restore_workspace_snapshot(self.policy.sandbox().root(), snapshot)?;
        }
        self.history.truncate(checkpoint.history_len);
        truncate_jsonl_lines(self.event_log.path(), checkpoint.event_count)?;
        if let Some(full_path) = &self.full_transcript_path {
            truncate_jsonl_lines(full_path, checkpoint.full_transcript_count)?;
        }
        truncate_jsonl_lines(&checkpoints_path_for(self.event_log.path()), turn_index)?;
        self.checkpoints.truncate(turn_index);
        Ok(checkpoint)
    }

    /// Clears active in-memory conversation history (`full-transcript.jsonl` stays preserved).
    pub fn clear_history(&mut self) {
        self.history.clear();
        self.checkpoints.clear();
    }

    /// Rebinds this [`AgentLoop`] to a new session `EventLog` and `full_transcript` archive and clears active history.
    pub fn switch_session(&mut self, event_log: EventLog, full_transcript_path: PathBuf) {
        self.event_log = event_log;
        self.full_transcript_path = Some(full_transcript_path);
        self.history.clear();
        self.checkpoints.clear();
    }

    /// Queries the dual-scope memory store for `scope` and substring `query`.
    pub fn query_memory(&self, scope: MemoryScope, query: &str) -> Result<Vec<MemoryRecord>> {
        self.memory
            .query(scope, query)
            .context("failed to query memory store")
    }

    /// Summarizes active conversation via `provider` with zero trimming, referencing `full-transcript.jsonl`.
    pub fn compact_with_provider(
        &mut self,
        focus: Option<&str>,
        provider: &mut dyn Provider,
    ) -> Result<String> {
        if self.history.is_empty() {
            return Ok("Nothing to compact yet.".to_owned());
        }
        let prior_count = self.history.len();
        let digest = format_history_for_summary(&self.history, focus);
        let events = provider.stream_turn(
            COMPACT_SYSTEM_PROMPT,
            &[Message::new(Role::User, digest)],
            &[],
        )?;
        let summary: String = events
            .into_iter()
            .filter_map(|ev| match ev {
                ProviderStreamEvent::TextDelta(delta) => Some(delta),
                _ => None,
            })
            .collect();
        let summary = summary.trim();
        if summary.is_empty() {
            bail!("provider returned empty compaction summary; history left unchanged");
        }

        let archive_note = self
            .full_transcript_path
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "session full transcript".to_owned());
        let compacted = Message::new(
            Role::User,
            format!(
                "[Compacted summary of {prior_count} prior messages; complete verbatim turns are archived in `{archive_note}` and can be inspected anytime with `read` or `grep`]\n\n{summary}"
            ),
        );
        self.event_log
            .append(&SessionEvent::Compacted {
                message: compacted.clone(),
                prior_count,
            })
            .context("failed to append compaction event to session log")?;
        self.history = vec![compacted];
        Ok(format!(
            "compacted {prior_count} messages via provider (archive: {archive_note})"
        ))
    }

    /// Hydrates conversation history, active mode, and clean display transcript lines from the bound session log.
    pub fn hydrate_from_log(&mut self) -> Result<Vec<String>> {
        let structured = self.hydrate_structured()?;
        let mut display_lines = Vec::new();
        for ev in structured {
            match ev {
                HydratedEvent::User(text) => display_lines.push(format!("❯ {text}")),
                HydratedEvent::Assistant(text) => display_lines.push(text),
                HydratedEvent::Note(text) => display_lines.push(format!("· {text}")),
                HydratedEvent::Tool { call, .. } => display_lines.push(format_tool_status(&call)),
            }
        }
        Ok(display_lines)
    }

    /// Hydrates conversation history and structured UI events from the bound session log.
    pub fn hydrate_structured(&mut self) -> Result<Vec<HydratedEvent>> {
        let events = self.event_log.read_all()?;
        let mut items = Vec::new();
        let mut pending_calls: Vec<ToolCall> = Vec::new();
        let mut pending_tools: Vec<Message> = Vec::new();
        let mut round_text = String::new();
        self.history.clear();
        self.checkpoints.clear();

        for (event_idx, ev) in events.into_iter().enumerate() {
            match ev {
                SessionEvent::ModeSwitched { mode } => {
                    let profile = self.build_profile(mode);
                    self.policy.set_rules(profile.policy_rules.clone());
                    self.modes = ModeManager::new(profile);
                }
                SessionEvent::StreamDelta { delta } => {
                    round_text.push_str(&delta);
                }
                SessionEvent::ToolRequested { call } => {
                    pending_calls.push(call);
                }
                SessionEvent::ToolCompleted { output } => {
                    let mut tool_msg = Message::new(Role::Tool, output.content.clone());
                    if let Some(matched) = pending_calls
                        .iter()
                        .find(|c| c.id == output.call_id)
                        .cloned()
                    {
                        items.push(HydratedEvent::Tool {
                            call: matched.clone(),
                            output: output.clone(),
                        });
                        tool_msg.tool_calls.push(matched);
                        pending_tools.push(tool_msg);
                    }
                }
                SessionEvent::MessageRecorded { message } => {
                    if message.role == Role::User && message.content == "[tool_result_continuation]"
                    {
                        flush_tool_round(
                            &mut self.history,
                            &mut items,
                            &mut round_text,
                            &mut pending_calls,
                            &mut pending_tools,
                        );
                        continue;
                    }
                    flush_tool_round(
                        &mut self.history,
                        &mut items,
                        &mut round_text,
                        &mut pending_calls,
                        &mut pending_tools,
                    );
                    round_text.clear();
                    match message.role {
                        Role::User => {
                            let preview: String = message
                                .content
                                .lines()
                                .next()
                                .unwrap_or("")
                                .trim()
                                .chars()
                                .take(80)
                                .collect();
                            self.checkpoints.push(TurnCheckpoint {
                                turn_index: self.checkpoints.len(),
                                prompt_preview: preview,
                                timestamp_secs: 0,
                                history_len: self.history.len(),
                                event_count: event_idx,
                                full_transcript_count: 0,
                                snapshot: None,
                            });
                            items.push(HydratedEvent::User(message.content.clone()));
                        }
                        Role::Assistant => {
                            if !message.content.trim().is_empty() {
                                items.push(HydratedEvent::Assistant(message.content.clone()));
                            }
                        }
                        Role::Tool | Role::System => {}
                    }
                    self.history.push(message);
                }
                SessionEvent::Compacted {
                    message,
                    prior_count,
                } => {
                    flush_tool_round(
                        &mut self.history,
                        &mut items,
                        &mut round_text,
                        &mut pending_calls,
                        &mut pending_tools,
                    );
                    round_text.clear();
                    self.history = vec![message];
                    items.push(HydratedEvent::Note(format!(
                        "compacted {prior_count} prior messages"
                    )));
                }
                SessionEvent::PolicyEvaluated { .. } | SessionEvent::MemoryAccessed { .. } => {}
            }
        }
        flush_tool_round(
            &mut self.history,
            &mut items,
            &mut round_text,
            &mut pending_calls,
            &mut pending_tools,
        );
        if let Ok(persisted) = load_checkpoints(&checkpoints_path_for(self.event_log.path()))
            && !persisted.is_empty()
        {
            self.checkpoints = persisted;
        }
        self.sync_subagent_state();
        Ok(items)
    }
}
