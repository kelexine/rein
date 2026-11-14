// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: In-process TUI state driving Core, Policy, Tools, and Modes without IPC

use crate::modes::Profile;

/// State owned by the in-process Ratatui frontend.
#[derive(Debug)]
pub struct AppState {
    active_profile: Profile,
    sandbox_summary: String,
    transcript: Vec<String>,
    input: String,
    pending_prompt: Option<String>,
    scroll_offset: u16,
}

impl AppState {
    /// Creates a new [`AppState`] starting in the given [`Profile`].
    #[must_use]
    pub fn new(initial: Profile, sandbox_summary: impl Into<String>) -> Self {
        Self {
            active_profile: initial,
            sandbox_summary: sandbox_summary.into(),
            transcript: Vec::new(),
            input: String::new(),
            pending_prompt: None,
            scroll_offset: 0,
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

    /// Returns the sandbox status summary shown in the footer.
    #[must_use]
    pub fn sandbox_summary(&self) -> &str {
        &self.sandbox_summary
    }

    /// Returns the rendered transcript lines.
    #[must_use]
    pub fn transcript(&self) -> &[String] {
        &self.transcript
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

    /// Appends a line to the TUI transcript view.
    pub fn push_line(&mut self, line: impl Into<String>) {
        self.transcript.push(line.into());
    }

    /// Appends streamed text into the latest assistant transcript line.
    pub fn append_stream_delta(&mut self, delta: &str) {
        if let Some(last) = self.transcript.last_mut()
            && last.starts_with("Rein: ")
        {
            last.push_str(delta);
        } else {
            self.transcript.push(format!("Rein: {delta}"));
        }
    }

    /// Returns the vertical scroll offset.
    #[must_use]
    pub fn scroll_offset(&self) -> u16 {
        self.scroll_offset
    }

    /// Adjusts transcript scroll offset safely.
    pub fn scroll_by(&mut self, delta: i16) {
        if delta < 0 {
            self.scroll_offset = self.scroll_offset.saturating_sub(delta.unsigned_abs());
        } else {
            self.scroll_offset = self.scroll_offset.saturating_add(delta.unsigned_abs());
        }
    }
}
