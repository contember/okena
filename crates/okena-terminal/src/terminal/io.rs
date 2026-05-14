use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::TermMode;
use std::sync::atomic::Ordering;
use std::time::Instant;

use super::Terminal;
use super::prompt_marks::advance_with_prompt_marks;

impl Terminal {
    /// Process output from PTY
    pub fn process_output(&self, data: &[u8]) {
        let mut _slow = okena_core::timing::SlowGuard::with_detail(
            "Terminal::process_output",
            format!("{} bytes", data.len()),
        );
        let mut term = self.term.lock();
        let mut processor = self.processor.lock();
        let mut sidecar = self.osc_sidecar.lock();
        let mut prompt_sidecar = self.prompt_sidecar.lock();
        let mut prompt_tracker = self.prompt_tracker.lock();

        let history_before = term.grid().history_size();

        // OSC 7 / OSC 9 / XTVERSION observer runs on the full chunk in one
        // pass — it never needs cursor-accurate positioning.
        sidecar.advance(data);

        // OSC 133 requires the main processor and the prompt sidecar to
        // advance in lockstep so we can snapshot the cursor at the exact
        // byte where each mark arrives. `advance_until_terminated` stops
        // the prompt sidecar at every OSC 133 so the main processor can
        // catch up before we read `grid.cursor.point`.
        advance_with_prompt_marks(
            &mut *term,
            &mut *processor,
            &mut prompt_sidecar,
            &mut prompt_tracker,
            data,
        );

        let history_after = term.grid().history_size();
        prompt_tracker.on_history_changed(
            history_before,
            history_after,
            term.grid().topmost_line().0,
        );

        // New output disengages the prompt-jump walker so the next
        // Above jump starts from the newest prompt again.
        *self.prompt_jump_index.lock() = None;

        self.dirty.store(true, Ordering::Relaxed);
        self.content_generation.fetch_add(1, Ordering::Relaxed);
        *self.last_output_time.lock() = Instant::now();
    }

    /// Enqueue output data for deferred processing.
    ///
    /// Used by the remote client's tokio reader thread so it never holds
    /// `term.lock()`. The pending data is drained and parsed on the GPUI
    /// thread just before rendering (see `with_content`).
    pub fn enqueue_output(&self, data: &[u8]) {
        self.pending_output.lock().extend_from_slice(data);
        self.dirty.store(true, Ordering::Relaxed);
        *self.last_output_time.lock() = Instant::now();
    }

    /// Drain all pending output and feed it into the terminal emulator.
    ///
    /// Called automatically by `with_content` before rendering.
    pub(super) fn drain_pending_output(&self) {
        let data = {
            let mut pending = self.pending_output.lock();
            if pending.is_empty() {
                return;
            }
            std::mem::take(&mut *pending)
        };
        let _slow = okena_core::timing::SlowGuard::with_detail(
            "Terminal::drain_pending_output",
            format!("{} bytes", data.len()),
        );
        let mut term = self.term.lock();
        let mut processor = self.processor.lock();
        let mut sidecar = self.osc_sidecar.lock();
        let mut prompt_sidecar = self.prompt_sidecar.lock();
        let mut prompt_tracker = self.prompt_tracker.lock();

        let history_before = term.grid().history_size();
        sidecar.advance(&data);
        advance_with_prompt_marks(
            &mut *term,
            &mut *processor,
            &mut prompt_sidecar,
            &mut prompt_tracker,
            &data,
        );
        let history_after = term.grid().history_size();
        prompt_tracker.on_history_changed(
            history_before,
            history_after,
            term.grid().topmost_line().0,
        );
        self.content_generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Check if terminal has pending changes (and clear the flag).
    /// Used by PTY event loop for direct content pane notification.
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::Relaxed)
    }

    /// Get the current content generation counter.
    pub fn content_generation(&self) -> u64 {
        self.content_generation.load(Ordering::Relaxed)
    }

    /// Send input to the PTY
    /// Automatically scrolls to bottom if scrolled into history
    pub fn send_input(&self, input: &str) {
        self.had_user_input.store(true, Ordering::Relaxed);
        self.scroll_to_bottom();
        self.transport.send_input(&self.terminal_id, input.as_bytes());
    }

    /// Send pasted text to the PTY, wrapping in bracketed paste sequences if the
    /// terminal application has enabled bracketed paste mode (DECSET 2004).
    /// This prevents shells from executing each line of a multi-line paste individually.
    pub fn send_paste(&self, text: &str) {
        self.had_user_input.store(true, Ordering::Relaxed);
        self.scroll_to_bottom();

        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        if bracketed {
            self.write_bracketed_paste(text);
        } else {
            // No bracketed paste mode: convert all newlines to CR so each line lands
            // as Enter for the shell. (Multi-line content will execute line-by-line.)
            let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
            self.transport.send_input(&self.terminal_id, normalized.as_bytes());
        }
    }

    /// Send text wrapped in bracketed-paste sequences regardless of whether the
    /// receiving program enabled DECSET 2004. Used by programmatic-paste paths
    /// (e.g. "Send to Terminal") where the alacritty-tracked mode flag is
    /// unreliable: multiplexers, prompt frameworks that toggle the mode, and
    /// fresh terminals where the shell hasn't sent its startup sequence yet all
    /// cause `BRACKETED_PASTE` to read false even when the receiver supports it.
    /// Receivers that don't support bracketed paste will see the bracket bytes
    /// as literal text — annoying but recoverable, vs. multi-line content
    /// executing each line as a separate command.
    pub fn send_paste_force_bracketed(&self, text: &str) {
        self.had_user_input.store(true, Ordering::Relaxed);
        self.scroll_to_bottom();
        self.write_bracketed_paste(text);
    }

    /// Common bracketed-paste byte assembly for both `send_paste` (when mode is
    /// active) and `send_paste_force_bracketed` (always).
    fn write_bracketed_paste(&self, text: &str) {
        // Inside a bracketed paste, newlines should land as literal LF — readers
        // (zsh's zle, Claude/Codex TUIs, etc.) treat the content as one paste and
        // CR would be misread as Enter, prematurely submitting the line/prompt.
        let normalized = text.replace("\r\n", "\n");
        // Strip any embedded paste markers so callers can't smuggle an early
        // `\x1b[201~` and break out into raw input.
        let sanitized = normalized
            .replace("\x1b[200~", "")
            .replace("\x1b[201~", "");
        let mut buf = Vec::with_capacity(sanitized.len() + 12);
        buf.extend_from_slice(b"\x1b[200~");
        buf.extend_from_slice(sanitized.as_bytes());
        buf.extend_from_slice(b"\x1b[201~");
        self.transport.send_input(&self.terminal_id, &buf);
    }

    /// Send raw bytes to the PTY
    /// Automatically scrolls to bottom if scrolled into history
    pub fn send_bytes(&self, data: &[u8]) {
        self.had_user_input.store(true, Ordering::Relaxed);
        self.scroll_to_bottom();
        self.transport.send_input(&self.terminal_id, data);
    }

    /// Clear the terminal screen by sending the clear sequence
    pub fn clear(&self) {
        // Send ANSI escape sequence to clear screen and move cursor to home
        // \x1b[2J = clear entire screen
        // \x1b[H = move cursor to home position (0,0)
        self.transport.send_input(&self.terminal_id, b"\x1b[2J\x1b[H");
        self.scroll_to_bottom();
    }
}
