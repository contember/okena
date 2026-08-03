use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::TermMode;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::prompt_marks::advance_with_prompt_marks;
use super::{InputRepaintRequest, Terminal};

const INPUT_REPAINT_REQUEST_TTL: Duration = Duration::from_secs(5);

impl Terminal {
    /// Process output from PTY
    pub fn process_output(&self, data: &[u8]) {
        self.process_output_with_sequence(data, 0);
    }

    /// Process PTY output and atomically associate it with its broadcast order.
    pub fn process_output_with_sequence(&self, data: &[u8], sequence: u64) {
        let output_epoch =
            (!data.is_empty()).then(|| self.output_epoch.fetch_add(1, Ordering::AcqRel) + 1);
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
        let command_finished = advance_with_prompt_marks(
            &mut *term,
            &mut processor,
            &mut prompt_sidecar,
            &mut prompt_tracker,
            data,
        );
        if command_finished {
            self.command_finished_pending.store(true, Ordering::Relaxed);
        }

        let history_after = term.grid().history_size();
        prompt_tracker.on_history_changed(
            history_before,
            history_after,
            term.grid().topmost_line().0,
        );

        // New output disengages the prompt-jump walker so the next
        // Above jump starts from the newest prompt again.
        *self.prompt_jump_index.lock() = None;
        *self.failed_jump_index.lock() = None;

        self.dirty.store(true, Ordering::Relaxed);
        self.content_generation.fetch_add(1, Ordering::Relaxed);
        if sequence != 0 {
            self.processed_output_sequence
                .store(sequence, Ordering::Release);
        }
        if let Some(output_epoch) = output_epoch {
            self.processed_output_epoch
                .fetch_max(output_epoch, Ordering::Release);
        }
        *self.last_output_time.lock() = Instant::now();
    }

    /// Enqueue output data for deferred processing.
    ///
    /// Used by the remote client's tokio reader thread so it never holds
    /// `term.lock()`. The pending data is drained and parsed on the GPUI
    /// thread just before rendering (see `with_content`).
    pub fn enqueue_output(&self, data: &[u8]) {
        let mut pending = self.pending_output.lock();
        pending.extend_from_slice(data);
        if !data.is_empty() {
            let output_epoch = self.output_epoch.fetch_add(1, Ordering::AcqRel) + 1;
            self.pending_output_epoch
                .store(output_epoch, Ordering::Release);
        }
        drop(pending);
        self.dirty.store(true, Ordering::Relaxed);
        *self.last_output_time.lock() = Instant::now();
    }

    /// Eagerly drain and parse any pending (enqueued) output on the GPUI thread.
    ///
    /// `with_content` parses pending bytes lazily, just before handing the grid
    /// to the renderer. That is too late for sibling/ancestor views that read
    /// derived state — the pane bell border (`TerminalPane::render`) and the
    /// sidebar bell/idle indicators read `has_bell()` / `is_waiting_for_input()`
    /// *before* the `TerminalContent` child drains. For local terminals the
    /// equivalent state is set eagerly in `process_output`; remote terminals only
    /// buffer via `enqueue_output`. The remote manager's activity pump calls this
    /// before emitting targeted pane/sidebar notifications, so derived state is
    /// current when the frame is built without per-pane polling. GPUI thread only.
    pub fn process_pending_output(&self) {
        self.drain_pending_output();
    }

    /// Drain all pending output and feed it into the terminal emulator.
    ///
    /// Called automatically by `with_content` before rendering.
    pub(super) fn drain_pending_output(&self) {
        let (data, output_epoch) = {
            let mut pending = self.pending_output.lock();
            if pending.is_empty() {
                return;
            }
            let output_epoch = self.pending_output_epoch.load(Ordering::Acquire);
            (std::mem::take(&mut *pending), output_epoch)
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
        let command_finished = advance_with_prompt_marks(
            &mut *term,
            &mut processor,
            &mut prompt_sidecar,
            &mut prompt_tracker,
            &data,
        );
        if command_finished {
            self.command_finished_pending.store(true, Ordering::Relaxed);
        }
        let history_after = term.grid().history_size();
        prompt_tracker.on_history_changed(
            history_before,
            history_after,
            term.grid().topmost_line().0,
        );
        self.content_generation.fetch_add(1, Ordering::Relaxed);
        self.processed_output_epoch
            .fetch_max(output_epoch, Ordering::Release);
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

    fn mark_user_input(&self, has_payload: bool) {
        self.had_user_input.store(true, Ordering::Relaxed);
        if !has_payload {
            return;
        }

        // Synchronize with remote enqueue so output already buffered before the
        // input cannot consume this request. The next enqueue receives this epoch.
        let after_output_epoch = {
            let _pending = self.pending_output.lock();
            self.output_epoch.load(Ordering::Acquire).saturating_add(1)
        };
        *self.input_repaint_request.lock() = Some(InputRepaintRequest {
            after_output_epoch,
            expires_at: Instant::now() + INPUT_REPAINT_REQUEST_TTL,
        });
    }

    /// Consume the one-shot request once parsed output crosses the output epoch
    /// captured immediately before user input reached the transport.
    pub fn take_input_repaint_request(&self) -> bool {
        self.take_input_repaint_request_at(Instant::now())
    }

    pub(crate) fn take_input_repaint_request_at(&self, now: Instant) -> bool {
        let processed_output_epoch = self.processed_output_epoch.load(Ordering::Acquire);
        let mut request = self.input_repaint_request.lock();
        let Some(current) = *request else {
            return false;
        };
        if now >= current.expires_at {
            *request = None;
            return false;
        }
        if processed_output_epoch < current.after_output_epoch {
            return false;
        }
        *request = None;
        true
    }

    /// Send input to the PTY
    /// Automatically scrolls to bottom if scrolled into history
    pub fn send_input(&self, input: &str) {
        self.send_input_inner(input, None);
    }

    /// Send text input and associate latency samples with its originating viewer.
    pub fn send_input_from_viewer(&self, input: &str, viewer: u64) {
        self.send_input_inner(input, Some(viewer));
    }

    fn send_input_inner(&self, input: &str, viewer: Option<u64>) {
        self.mark_user_input(!input.is_empty());
        self.scroll_to_bottom();
        if let Some(viewer) = viewer {
            okena_core::latency_probe::client_start(&self.terminal_id, viewer, input.as_bytes());
        }
        self.transport
            .send_input(&self.terminal_id, input.as_bytes());
    }

    /// Send pasted text to the PTY, wrapping in bracketed paste sequences if the
    /// terminal application has enabled bracketed paste mode (DECSET 2004).
    /// This prevents shells from executing each line of a multi-line paste individually.
    pub fn send_paste(&self, text: &str) {
        self.mark_user_input(!text.is_empty());
        self.scroll_to_bottom();

        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        if bracketed {
            self.write_bracketed_paste(text);
        } else {
            // No bracketed paste mode: convert all newlines to CR so each line lands
            // as Enter for the shell. (Multi-line content will execute line-by-line.)
            let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
            self.transport
                .send_input(&self.terminal_id, normalized.as_bytes());
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
        self.mark_user_input(!text.is_empty());
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
        let sanitized = normalized.replace("\x1b[200~", "").replace("\x1b[201~", "");
        let mut buf = Vec::with_capacity(sanitized.len() + 12);
        buf.extend_from_slice(b"\x1b[200~");
        buf.extend_from_slice(sanitized.as_bytes());
        buf.extend_from_slice(b"\x1b[201~");
        self.transport.send_input(&self.terminal_id, &buf);
    }

    /// Send raw bytes to the PTY
    /// Automatically scrolls to bottom if scrolled into history
    pub fn send_bytes(&self, data: &[u8]) {
        self.send_bytes_inner(data, None);
    }

    /// Send raw input and associate latency samples with its originating viewer.
    pub fn send_bytes_from_viewer(&self, data: &[u8], viewer: u64) {
        self.send_bytes_inner(data, Some(viewer));
    }

    fn send_bytes_inner(&self, data: &[u8], viewer: Option<u64>) {
        self.mark_user_input(!data.is_empty());
        self.scroll_to_bottom();
        if let Some(viewer) = viewer {
            okena_core::latency_probe::client_start(&self.terminal_id, viewer, data);
        }
        self.transport.send_input(&self.terminal_id, data);
    }

    /// Send a named special key (Esc / Tab / Shift+Tab) the way the running app
    /// expects, honoring app-cursor and kitty-keyboard modes.
    ///
    /// These keys are delivered via dedicated GPUI actions (so they take part
    /// in context routing — e.g. Esc closes search in the search bar but goes
    /// to the PTY here) instead of the raw `on_key_down` path, so they bypass
    /// the encoder unless routed back through it here. Reusing `key_to_bytes`
    /// keeps them in lockstep with normal key handling (notably the kitty
    /// `CSI u` disambiguation: `Esc` → `CSI 27 u`, `Shift+Tab` → `CSI 9 ; 2 u`).
    fn send_named_key(&self, key: &str, shift: bool) {
        let event = crate::input::KeyEvent {
            key: key.to_string(),
            key_char: None,
            modifiers: crate::input::KeyModifiers {
                shift,
                ..Default::default()
            },
        };
        if let Some(bytes) = crate::input::key_to_bytes(
            &event,
            self.is_app_cursor_mode(),
            self.kitty_keyboard_flags(),
        ) {
            self.send_bytes(&bytes);
        }
    }

    /// Send the Escape key (kitty-aware: `CSI 27 u` in disambiguate mode).
    pub fn send_escape(&self) {
        self.send_named_key("escape", false);
    }

    /// Send the Tab key (plain `\t`; unchanged by kitty level 1).
    pub fn send_tab(&self) {
        self.send_named_key("tab", false);
    }

    /// Send Shift+Tab / backtab (kitty-aware: `CSI 9 ; 2 u` in disambiguate mode).
    pub fn send_backtab(&self) {
        self.send_named_key("tab", true);
    }

    /// Clear the terminal screen by sending the clear sequence
    pub fn clear(&self) {
        // Send ANSI escape sequence to clear screen and move cursor to home
        // \x1b[2J = clear entire screen
        // \x1b[H = move cursor to home position (0,0)
        self.transport
            .send_input(&self.terminal_id, b"\x1b[2J\x1b[H");
        self.scroll_to_bottom();
    }
}
