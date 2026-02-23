use crate::terminal::terminal::Terminal;
use gpui::*;
use std::ops::Range;
use std::sync::Arc;

/// ASCII DEL character - what terminals expect for backspace
const DEL: u8 = 0x7f;

/// macOS function key character range (U+F700-U+F8FF)
/// GPUI sends these for arrow keys, function keys, etc.
/// but we handle those separately via on_key_down -> key_to_bytes
const MACOS_FUNCTION_KEY_RANGE: std::ops::RangeInclusive<char> = '\u{F700}'..='\u{F8FF}';

/// Input handler for terminal text input
pub(crate) struct TerminalInputHandler {
    pub terminal: Arc<Terminal>,
}

impl TerminalInputHandler {
    /// Send text input to terminal, filtering macOS function keys and handling control characters
    fn send_filtered_input(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        // Local keyboard input reclaims resize authority from remote clients
        self.terminal.claim_resize_local();

        // Filter out macOS function key characters
        let filtered: String = text
            .chars()
            .filter(|&c| !MACOS_FUNCTION_KEY_RANGE.contains(&c))
            .collect();

        if filtered.is_empty() {
            return;
        }

        // Predict printable chars for remote terminals
        if self.terminal.is_remote() {
            for c in filtered.chars() {
                if c.is_ascii_graphic() || c == ' ' || (!c.is_control() && !c.is_ascii()) {
                    self.terminal.predict_char(c);
                }
            }
        }

        // Fast path: no control characters, send entire string at once
        if !filtered.chars().any(|c| matches!(c, '\n' | '\r' | '\u{8}')) {
            self.terminal.send_input(&filtered);
            return;
        }

        // Slow path: handle control characters individually
        for c in filtered.chars() {
            match c {
                '\u{8}' => self.terminal.send_bytes(&[DEL]),
                '\n' | '\r' => self.terminal.send_bytes(&[b'\r']),
                _ => {
                    let mut buf = [0u8; 4];
                    let s = c.encode_utf8(&mut buf);
                    self.terminal.send_input(s);
                }
            }
        }
    }
}

impl InputHandler for TerminalInputHandler {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(&mut self, _window: &mut Window, _cx: &mut App) -> Option<Range<usize>> {
        None
    }

    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<String> {
        None
    }

    fn replace_text_in_range(
        &mut self,
        _replacement_range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        _cx: &mut App,
    ) {
        self.send_filtered_input(text);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut App,
    ) {
        self.send_filtered_input(new_text);
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut App) {}

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        None
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        None
    }

    fn accepts_text_input(&mut self, _window: &mut Window, _cx: &mut App) -> bool {
        true
    }
}
