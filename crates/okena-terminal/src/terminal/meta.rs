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

    /// Take any pending iTerm2-style `OSC 9 ; message` notifications. The
    /// GPUI thread drains these on each render to surface toasts or native
    /// desktop notifications for long-running commands that finished while
    /// the user was in another pane.
    pub fn take_pending_notifications(&self) -> Vec<String> {
        std::mem::take(&mut *self.pending_notifications.lock())
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
