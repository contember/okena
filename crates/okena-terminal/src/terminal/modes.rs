use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::CursorShape as VteCursorShape;

use super::Terminal;
use super::types::AppCursorShape;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalModeState(u32);

impl TerminalModeState {
    const SHOW_CURSOR: u32 = 1 << 0;
    const APP_CURSOR: u32 = 1 << 1;
    const APP_KEYPAD: u32 = 1 << 2;
    const MOUSE_REPORT_CLICK: u32 = 1 << 3;
    const MOUSE_DRAG: u32 = 1 << 4;
    const MOUSE_MOTION: u32 = 1 << 5;
    const BRACKETED_PASTE: u32 = 1 << 6;
    const SGR_MOUSE: u32 = 1 << 7;
    const UTF8_MOUSE: u32 = 1 << 8;
    const LINE_WRAP: u32 = 1 << 9;
    const LINE_FEED_NEW_LINE: u32 = 1 << 10;
    const INSERT: u32 = 1 << 11;
    const FOCUS_IN_OUT: u32 = 1 << 12;
    const ALT_SCREEN: u32 = 1 << 13;
    const ALTERNATE_SCROLL: u32 = 1 << 14;
    const DISAMBIGUATE_ESC_CODES: u32 = 1 << 15;
    const REPORT_EVENT_TYPES: u32 = 1 << 16;
    const REPORT_ALTERNATE_KEYS: u32 = 1 << 17;
    const REPORT_ALL_KEYS_AS_ESC: u32 = 1 << 18;
    const REPORT_ASSOCIATED_TEXT: u32 = 1 << 19;
    const ALL: u32 = (1 << 20) - 1;

    pub(super) fn from_term_mode(mode: &TermMode) -> Self {
        let mut bits = 0;
        for (term_mode, state_bit) in [
            (TermMode::SHOW_CURSOR, Self::SHOW_CURSOR),
            (TermMode::APP_CURSOR, Self::APP_CURSOR),
            (TermMode::APP_KEYPAD, Self::APP_KEYPAD),
            (TermMode::MOUSE_REPORT_CLICK, Self::MOUSE_REPORT_CLICK),
            (TermMode::MOUSE_DRAG, Self::MOUSE_DRAG),
            (TermMode::MOUSE_MOTION, Self::MOUSE_MOTION),
            (TermMode::BRACKETED_PASTE, Self::BRACKETED_PASTE),
            (TermMode::SGR_MOUSE, Self::SGR_MOUSE),
            (TermMode::UTF8_MOUSE, Self::UTF8_MOUSE),
            (TermMode::LINE_WRAP, Self::LINE_WRAP),
            (TermMode::LINE_FEED_NEW_LINE, Self::LINE_FEED_NEW_LINE),
            (TermMode::INSERT, Self::INSERT),
            (TermMode::FOCUS_IN_OUT, Self::FOCUS_IN_OUT),
            (TermMode::ALT_SCREEN, Self::ALT_SCREEN),
            (TermMode::ALTERNATE_SCROLL, Self::ALTERNATE_SCROLL),
            (
                TermMode::DISAMBIGUATE_ESC_CODES,
                Self::DISAMBIGUATE_ESC_CODES,
            ),
            (TermMode::REPORT_EVENT_TYPES, Self::REPORT_EVENT_TYPES),
            (TermMode::REPORT_ALTERNATE_KEYS, Self::REPORT_ALTERNATE_KEYS),
            (
                TermMode::REPORT_ALL_KEYS_AS_ESC,
                Self::REPORT_ALL_KEYS_AS_ESC,
            ),
            (
                TermMode::REPORT_ASSOCIATED_TEXT,
                Self::REPORT_ASSOCIATED_TEXT,
            ),
        ] {
            if mode.contains(term_mode) {
                bits |= state_bit;
            }
        }
        Self(bits)
    }

    pub(crate) fn encode(self) -> String {
        format!("{:x}", self.0)
    }

    pub(crate) fn decode(value: &str) -> Option<Self> {
        let bits = u32::from_str_radix(value.trim(), 16).ok()?;
        (bits & !Self::ALL == 0).then_some(Self(bits))
    }

    pub(super) fn write_screen_selection(self, buf: &mut Vec<u8>) {
        if self.contains(Self::ALT_SCREEN) {
            buf.extend_from_slice(b"\x1b[?1049h");
        } else {
            buf.extend_from_slice(b"\x1b[?1049l");
        }
    }

    pub(super) fn to_ansi(self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(160);
        self.write_screen_selection(&mut buf);
        buf.extend_from_slice(
            b"\x1b[?1;6;7;25;1000;1002;1003;1004;1005;1006;1007;2004l\x1b[4;20l\x1b>",
        );

        for (bit, code) in [
            (Self::APP_CURSOR, "1"),
            (Self::LINE_WRAP, "7"),
            (Self::SHOW_CURSOR, "25"),
            (Self::MOUSE_REPORT_CLICK, "1000"),
            (Self::MOUSE_DRAG, "1002"),
            (Self::MOUSE_MOTION, "1003"),
            (Self::FOCUS_IN_OUT, "1004"),
            (Self::UTF8_MOUSE, "1005"),
            (Self::SGR_MOUSE, "1006"),
            (Self::ALTERNATE_SCROLL, "1007"),
            (Self::BRACKETED_PASTE, "2004"),
        ] {
            if self.contains(bit) {
                buf.extend_from_slice(b"\x1b[?");
                buf.extend_from_slice(code.as_bytes());
                buf.push(b'h');
            }
        }
        if self.contains(Self::INSERT) {
            buf.extend_from_slice(b"\x1b[4h");
        }
        if self.contains(Self::LINE_FEED_NEW_LINE) {
            buf.extend_from_slice(b"\x1b[20h");
        }
        if self.contains(Self::APP_KEYPAD) {
            buf.extend_from_slice(b"\x1b=");
        }

        let kitty_flags = (self.contains(Self::DISAMBIGUATE_ESC_CODES) as u8)
            | ((self.contains(Self::REPORT_EVENT_TYPES) as u8) << 1)
            | ((self.contains(Self::REPORT_ALTERNATE_KEYS) as u8) << 2)
            | ((self.contains(Self::REPORT_ALL_KEYS_AS_ESC) as u8) << 3)
            | ((self.contains(Self::REPORT_ASSOCIATED_TEXT) as u8) << 4);
        buf.extend_from_slice(format!("\x1b[={kitty_flags}u").as_bytes());
        buf
    }

    fn contains(self, bit: u32) -> bool {
        self.0 & bit != 0
    }
}

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

    /// Active kitty keyboard protocol enhancement flags requested by the app.
    /// Only the level-1 "disambiguate escape codes" flag is honored today; the
    /// higher progressive-enhancement levels are a follow-up.
    pub fn kitty_keyboard_flags(&self) -> crate::input::KittyKeyboardFlags {
        crate::input::KittyKeyboardFlags {
            disambiguate_escape_codes: self
                .term
                .lock()
                .mode()
                .contains(TermMode::DISAMBIGUATE_ESC_CODES),
        }
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
