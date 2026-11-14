// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Minimalist borderless TUI state driving Core, Policy, Tools, and Modes in-process

use crate::modes::Profile;

/// State owned by the in-process minimalist Ratatui frontend.
#[derive(Debug)]
pub struct AppState {
    active_profile: Profile,
    sandbox_summary: String,
    model_name: String,
    transcript: Vec<String>,
    input: String,
    pending_prompt: Option<String>,
    scroll_offset: u16,
    auto_scroll: bool,
    busy: bool,
    streaming_active: bool,
}

impl AppState {
    /// Creates a new [`AppState`] starting in `initial` profile.
    #[must_use]
    pub fn new(
        initial: Profile,
        sandbox_summary: impl Into<String>,
        model_name: impl Into<String>,
    ) -> Self {
        Self {
            active_profile: initial,
            sandbox_summary: sandbox_summary.into(),
            model_name: model_name.into(),
            transcript: Vec::new(),
            input: String::new(),
            pending_prompt: None,
            scroll_offset: 0,
            auto_scroll: true,
            busy: false,
            streaming_active: false,
        }
    }

    /// Updates the displayed active [`Profile`].
    pub fn set_active_profile(&mut self, profile: Profile) {
        self.active_profile = profile;
    }

    /// Returns the currently displayed [`Profile`].
    #[must_use]
    pub fn active_profile(&self) -> &Profile {
        &self.active_profile
    }

    /// Returns the concise sandbox label shown in the status bar.
    #[must_use]
    pub fn sandbox_summary(&self) -> &str {
        &self.sandbox_summary
    }

    /// Returns the active model name shown in the status bar.
    #[must_use]
    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    /// Returns the rendered transcript lines.
    #[must_use]
    pub fn transcript(&self) -> &[String] {
        &self.transcript
    }

    /// Replaces the transcript with hydrated lines (used by `--resume`).
    pub fn extend_transcript(&mut self, lines: impl IntoIterator<Item = String>) {
        self.transcript.extend(lines);
        self.auto_scroll = true;
    }

    /// Returns the current input buffer.
    #[must_use]
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Pushes a typed character to the input buffer.
    pub fn push_input_char(&mut self, ch: char) {
        self.input.push(ch);
    }

    /// Removes the last character from the input buffer.
    pub fn pop_input_char(&mut self) {
        let _ = self.input.pop();
    }

    /// Takes and clears the current input buffer.
    pub fn take_input(&mut self) -> String {
        std::mem::take(&mut self.input)
    }

    /// Sets or clears an interactive confirmation prompt from the policy layer.
    pub fn set_pending_prompt(&mut self, prompt: Option<String>) {
        self.pending_prompt = prompt;
    }

    /// Returns any active interactive permission prompt from the policy layer.
    #[must_use]
    pub fn pending_prompt(&self) -> Option<&str> {
        self.pending_prompt.as_deref()
    }

    /// Marks whether a background model turn is currently streaming.
    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
        if !busy {
            self.streaming_active = false;
        }
    }

    /// Returns `true` while a turn is streaming on the background worker thread.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// Appends a standalone line to the transcript (user prompt, tool status, or error).
    pub fn push_line(&mut self, line: impl Into<String>) {
        self.streaming_active = false;
        self.transcript.push(line.into());
        self.auto_scroll = true;
    }

    /// Appends streamed assistant tokens cleanly, handling embedded newlines.
    pub fn append_stream_delta(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        let mut parts = delta.split('\n');
        if let Some(first) = parts.next() {
            if self.streaming_active
                && let Some(last) = self.transcript.last_mut()
            {
                last.push_str(first);
            } else {
                self.transcript.push(first.to_owned());
                self.streaming_active = true;
            }
        }
        for rest in parts {
            self.transcript.push(rest.to_owned());
        }
        self.auto_scroll = true;
    }

    /// Returns whether auto-scroll to the newest line is active.
    #[must_use]
    pub fn auto_scroll(&self) -> bool {
        self.auto_scroll
    }

    /// Returns the current manual scroll offset, or updates it relative to max scroll.
    #[must_use]
    pub fn effective_scroll(&self, max_scroll: u16) -> u16 {
        if self.auto_scroll {
            max_scroll
        } else {
            self.scroll_offset.min(max_scroll)
        }
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
