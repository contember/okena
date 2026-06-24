use alacritty_terminal::index::{Column, Line, Point};

use super::Terminal;

impl Terminal {
    /// Get the terminal title (from OSC sequences)
    pub fn title(&self) -> Option<String> {
        self.title.lock().clone()
    }

    /// Check if terminal has unread bell notification
    pub fn has_bell(&self) -> bool {
        *self.has_bell.lock()
    }

    /// Take any pending OSC 52 clipboard writes. Called by the GPUI thread
    /// on each render; returns the texts to write to the system clipboard.
    pub fn take_pending_clipboard_writes(&self) -> Vec<String> {
        std::mem::take(&mut *self.pending_clipboard.lock())
    }

    /// Whether the running app has queued any OSC 52 clipboard *read* requests
    /// (`OSC 52 ; c ; ?`) since the last drain. The PTY event loop checks this
    /// per dirty terminal before deciding whether to read the system clipboard
    /// (only when the opt-in setting is on) or silently drop the requests.
    pub fn has_pending_clipboard_reads(&self) -> bool {
        !self.pending_clipboard_reads.lock().is_empty()
    }

    /// Answer all queued OSC 52 clipboard *read* requests with `content`,
    /// draining the queue. For each queued formatter we build the reply
    /// (`OSC 52 ; c ; <base64> ST`) and write it straight back to the PTY via
    /// the transport — not `send_bytes` — so a clipboard read doesn't set
    /// `had_user_input` or scroll the view, mirroring how color-query replies
    /// are written directly in the event listener. Only call this when the
    /// user has opted into clipboard reads.
    pub fn answer_clipboard_reads(&self, content: &str) {
        let responders = std::mem::take(&mut *self.pending_clipboard_reads.lock());
        for responder in responders {
            let reply = responder(content);
            self.transport.send_input(&self.terminal_id, reply.as_bytes());
        }
    }

    /// Drop all queued OSC 52 clipboard *read* requests without replying.
    /// Used when the `allow_clipboard_read` setting is off: the request is
    /// silently denied (the app gets no response), but the queue is still
    /// cleared so it stays bounded across batches.
    pub fn drop_clipboard_reads(&self) {
        self.pending_clipboard_reads.lock().clear();
    }

    /// Take any pending `OSC 9` / `OSC 777` notifications. The GPUI thread
    /// drains these in the PTY event loop to surface native desktop
    /// notifications for background panes whose command finished or needs
    /// input while the user was elsewhere.
    pub fn take_pending_notifications(&self) -> Vec<super::TerminalNotification> {
        std::mem::take(&mut *self.pending_notifications.lock())
    }

    /// Active `OSC 9 ; 4` (ConEmu / Windows Terminal) progress report, or
    /// `None` when the running program isn't reporting progress (it never
    /// started one, or sent `st=0` to clear it). Read each render to drive a
    /// per-tab / per-pane progress indicator.
    pub fn progress(&self) -> Option<super::TerminalProgress> {
        *self.progress.lock()
    }

    /// Latest agent status reported via the agent-status OSC (`OSC 9001`), or
    /// `None` when no agent has reported one (or it was cleared). Read each
    /// render to drive the per-tab indicator and the sidebar "Agents" section.
    pub fn agent_status(&self) -> Option<okena_core::agent_status::AgentStatus> {
        self.agent_status.lock().clone()
    }

    /// Consume the one-shot "remote-visible state changed since last drain"
    /// edge. Returns true if anything marked it since the previous call, then
    /// resets it. The PTY event loop uses this to bump the remote
    /// `state_version` so remote / mobile clients re-fetch.
    pub fn take_remote_dirty(&self) -> bool {
        self.remote_dirty
            .swap(false, std::sync::atomic::Ordering::Relaxed)
    }

    /// The durable agent session captured for this pane (`agent` + `session_id`
    /// + optional `transcript_path`), or `None` if none has been reported.
    /// Unlike [`agent_status`](Self::agent_status) this is **sticky** — it
    /// survives `st=clear` — since it's the identity used for resume + transcript
    /// stats, persisted into `workspace.json` by the app layer.
    pub fn agent_session(&self) -> Option<okena_core::agent_session::AgentSession> {
        self.agent_session.lock().clone()
    }

    /// Consume the one-shot "agent session changed since last drain" edge. The
    /// PTY event loop uses this to persist the new session into `workspace.json`.
    pub fn take_agent_session_dirty(&self) -> bool {
        self.agent_session_dirty
            .swap(false, std::sync::atomic::Ordering::Relaxed)
    }

    /// Push the active theme palette so the event listener can answer
    /// OSC 10/11/12/4 color queries with real theme colors. Called from the
    /// render loop on every frame; writes are cheap and uncontested.
    pub fn set_palette(&self, colors: okena_core::theme::ThemeColors) {
        *self.palette.lock() = Some(colors);
    }

    /// Return the OSC 8 hyperlink URI at the given visual cell, if any.
    /// `visual_row` is the on-screen row (0..screen_lines); scrolling is
    /// handled via `display_offset` so history cells work too.
    pub fn hyperlink_at(&self, col: usize, visual_row: i32) -> Option<String> {
        let term = self.term.lock();
        let display_offset = term.grid().display_offset() as i32;
        let buffer_row = visual_row - display_offset;
        let cell = &term.grid()[Point::new(Line(buffer_row), Column(col))];
        cell.hyperlink().map(|h| h.uri().to_owned())
    }

    /// Clear the bell notification flag (call when terminal receives focus)
    pub fn clear_bell(&self) {
        *self.has_bell.lock() = false;
    }

    /// Consume the one-shot "bell rang since last drain" edge. Returns true if
    /// the terminal rang the bell since the previous call, then resets it. The
    /// PTY event loop uses this to raise a desktop notification exactly once
    /// per bell (distinct from `has_bell`, the sticky UI flag).
    pub fn take_pending_bell(&self) -> bool {
        self.bell_pending
            .swap(false, std::sync::atomic::Ordering::Relaxed)
    }

    /// Consume the one-shot "a command finished (OSC 133 ;D) since last drain"
    /// edge. Returns true if a command completed since the previous call, then
    /// resets it. The PTY event loop uses this to bump the owning project's
    /// activity timestamp once per finished command (drives the activity-sorted
    /// sidebar view). Shells without OSC 133 shell integration never raise it.
    pub fn take_pending_command_finished(&self) -> bool {
        self.command_finished_pending
            .swap(false, std::sync::atomic::Ordering::Relaxed)
    }

    /// Mark that this pane raised an OSC 9/777 desktop notification. Drives the
    /// pane's attention border until focus clears it. Set by the app when it
    /// actually fires a notification, so it inherits the user's settings and
    /// the focused-pane suppression. Mirrors the sticky `has_bell`.
    pub fn mark_notification(&self) {
        self.has_notification
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether this pane has an unseen OSC 9/777 notification (drives the border).
    pub fn has_notification(&self) -> bool {
        self.has_notification
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Clear the unseen-notification flag (call when the pane receives focus).
    pub fn clear_notification(&self) {
        self.has_notification
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }

    /// Get the initial working directory for this terminal
    pub fn initial_cwd(&self) -> &str {
        &self.initial_cwd
    }

    /// Get the working directory most recently reported by the shell via
    /// `OSC 7 ; file://host/path`. Returns `None` until the shell has emitted
    /// at least one such sequence.
    pub fn reported_cwd(&self) -> Option<String> {
        self.reported_cwd.lock().clone()
    }

    /// Best known working directory for the shell running in this terminal.
    /// Prefers the shell-reported cwd (OSC 7) and falls back to the directory
    /// the PTY was originally spawned in. Use this when resolving relative
    /// paths, opening "new tab here", or syncing sidebar selection.
    pub fn current_cwd(&self) -> String {
        self.reported_cwd
            .lock()
            .clone()
            .unwrap_or_else(|| self.initial_cwd.clone())
    }
}
