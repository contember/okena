use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::CursorShape as VteCursorShape;

use super::Terminal;
use super::types::AppCursorShape;

impl Terminal {
    /// Check if terminal is in mouse reporting mode (for tmux, vim, etc.)
    /// Also returns true if using tmux backend (which handles mouse with `set mouse on`)
    pub fn is_mouse_mode(&self) -> bool {
        // If using tmux backend, tmux handles mouse events directly
        if self.transport.uses_mouse_backend() {
            return true;
        }
        // Otherwise check if the terminal itself requested mouse mode
        let term = self.term.lock();
        term.mode().intersects(TermMode::MOUSE_MODE)
    }

    /// Check if terminal is in application cursor keys mode (DECCKM)
    /// When enabled, arrow keys should send SS3 sequences (\x1bOA) instead of CSI (\x1b[A)
    /// This is used by applications like less, vim, htop, etc.
    pub fn is_app_cursor_mode(&self) -> bool {
        let term = self.term.lock();
        term.mode().contains(TermMode::APP_CURSOR)
    }

    /// Check if terminal is using the alternate screen buffer.
    /// TUI apps (vim, less, htop, Claude Code CLI) use alternate screen.
    pub fn is_alt_screen(&self) -> bool {
        let term = self.term.lock();
        term.mode().contains(TermMode::ALT_SCREEN)
    }

    /// Cursor shape requested by the terminal application via DECSCUSR, if any.
    ///
    /// Returns `None` when the app has not overridden the shape (or has reset
    /// it with `\x1b[0 q`), so callers can fall back to the user setting.
    pub fn app_cursor_shape(&self) -> Option<AppCursorShape> {
        let term = self.term.lock();
        let style = term.cursor_style();
        match style.shape {
            VteCursorShape::HollowBlock => None,
            VteCursorShape::Block => Some(AppCursorShape::Block),
            VteCursorShape::Beam => Some(AppCursorShape::Bar),
            VteCursorShape::Underline => Some(AppCursorShape::Underline),
            VteCursorShape::Hidden => None,
        }
    }

    /// Cursor blinking flag from DECSCUSR, if the app has set a shape.
    ///
    /// Returns `None` when the app has not spoken (HollowBlock sentinel),
    /// so callers can fall back to the user's cursor_blink setting.
    pub fn app_cursor_blinking(&self) -> Option<bool> {
        let term = self.term.lock();
        let style = term.cursor_style();
        if style.shape == VteCursorShape::HollowBlock {
            None
        } else {
            Some(style.blinking)
        }
    }

    /// True if the active app has enabled focus event reporting (DEC mode 1004).
    pub fn wants_focus_events(&self) -> bool {
        let term = self.term.lock();
        term.mode().contains(TermMode::FOCUS_IN_OUT)
    }

    /// Send a focus-in (`\x1b[I`) or focus-out (`\x1b[O`) report to the PTY.
    /// Caller should gate on `wants_focus_events()`.
    pub fn send_focus(&self, focused: bool) {
        let bytes: &[u8] = if focused { b"\x1b[I" } else { b"\x1b[O" };
        self.send_bytes(bytes);
    }

    /// Update one rendered view's focus state and report the aggregate focus
    /// state for this terminal if it changed.
    pub fn update_focus_reporter(&self, viewer_id: u64, focused: bool) {
        let aggregate_focused = {
            let mut state = self.focus_report_state.lock();
            state.viewers.insert(viewer_id, focused);
            state.viewers.values().any(|focused| *focused)
        };

        self.send_aggregate_focus_if_changed(aggregate_focused);
    }

    /// Remove one rendered view from focus aggregation.
    pub fn remove_focus_reporter(&self, viewer_id: u64) {
        let aggregate_focused = {
            let mut state = self.focus_report_state.lock();
            if state.viewers.remove(&viewer_id).is_none() {
                return;
            }
            state.viewers.values().any(|focused| *focused)
        };

        self.send_aggregate_focus_if_changed(aggregate_focused);
    }

    fn send_aggregate_focus_if_changed(&self, focused: bool) {
        if !self.wants_focus_events() {
            self.focus_report_state.lock().last_reported = None;
            return;
        }

        let should_send = {
            let mut state = self.focus_report_state.lock();
            if state.last_reported == Some(focused) {
                false
            } else {
                state.last_reported = Some(focused);
                true
            }
        };

        if should_send {
            self.send_focus(focused);
        }
    }
}
