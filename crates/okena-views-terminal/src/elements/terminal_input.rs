use gpui::*;
use okena_terminal::terminal::Terminal;
use std::ops::Range;
use std::sync::Arc;

/// ASCII DEL character - what terminals expect for backspace
const DEL: u8 = 0x7f;

/// macOS function key character range (U+F700-U+F8FF)
/// GPUI sends these for arrow keys, function keys, etc.
/// but we handle those separately via on_key_down -> key_to_bytes
const MACOS_FUNCTION_KEY_RANGE: std::ops::RangeInclusive<char> = '\u{F700}'..='\u{F8FF}';

/// Input handler for terminal text input. Rebuilt every frame, so the IME
/// composition it reports lives on the `Terminal`.
pub(crate) struct TerminalInputHandler {
    pub terminal: Arc<Terminal>,
    pub viewer_id: u64,
}

/// Committed text ends the composition and only then reaches the PTY: the
/// marked text (`ˇ`) is never sent, only what it composes into (`ď`).
pub(crate) fn commit_text(terminal: &Terminal, text: &str, viewer_id: u64) {
    terminal.clear_marked_text();
    send_filtered_input(terminal, text, viewer_id);
}

/// Send text input to terminal, filtering macOS function keys and handling control characters
fn send_filtered_input(terminal: &Terminal, text: &str, viewer_id: u64) {
    if text.is_empty() {
        return;
    }
    // Local keyboard input reclaims resize authority from remote clients
    terminal.claim_resize_local();

    // Filter out macOS function key characters
    let filtered: String = text
        .chars()
        .filter(|&c| !MACOS_FUNCTION_KEY_RANGE.contains(&c))
        .collect();

    if filtered.is_empty() {
        return;
    }

    // Fast path: no control characters, send entire string at once
    if !filtered.chars().any(|c| matches!(c, '\n' | '\r' | '\u{8}')) {
        terminal.send_input_from_viewer(&filtered, viewer_id);
        return;
    }

    // Slow path: handle control characters individually
    for c in filtered.chars() {
        match c {
            '\u{8}' => terminal.send_bytes_from_viewer(&[DEL], viewer_id),
            '\n' | '\r' => terminal.send_bytes_from_viewer(b"\r", viewer_id),
            _ => {
                let mut buf = [0u8; 4];
                let s = c.encode_utf8(&mut buf);
                terminal.send_input_from_viewer(s, viewer_id);
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
        self.terminal.marked_text_range_utf16()
    }

    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<String> {
        let marked = self.terminal.marked_text()?;
        let units: Vec<u16> = marked.encode_utf16().collect();
        let end = range_utf16.end.min(units.len());
        let start = range_utf16.start.min(end);
        Some(String::from_utf16_lossy(&units[start..end]))
    }

    fn replace_text_in_range(
        &mut self,
        _replacement_range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        _cx: &mut App,
    ) {
        commit_text(&self.terminal, text, self.viewer_id);
    }

    // Marking changes what the pane shows but produces no PTY output, so
    // nothing else schedules the repaint.
    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        _cx: &mut App,
    ) {
        self.terminal.set_marked_text(new_text);
        window.refresh();
    }

    fn unmark_text(&mut self, window: &mut Window, _cx: &mut App) {
        self.terminal.clear_marked_text();
        window.refresh();
    }

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

// `use gpui::*` above pulls in `gpui::test`, which would shadow `#[test]` here.
#[cfg(test)]
mod tests {
    use super::commit_text;
    use okena_terminal::terminal::{Terminal, TerminalSize, TerminalTransport};
    use parking_lot::Mutex;
    use std::sync::Arc;

    struct CapturingTransport {
        writes: Mutex<Vec<Vec<u8>>>,
    }

    impl CapturingTransport {
        fn writes(&self) -> Vec<Vec<u8>> {
            self.writes.lock().clone()
        }
    }

    impl TerminalTransport for CapturingTransport {
        fn send_input(&self, _terminal_id: &str, data: &[u8]) {
            self.writes.lock().push(data.to_vec());
        }
        fn resize(&self, _terminal_id: &str, _cols: u16, _rows: u16) {}
        fn uses_mouse_backend(&self) -> bool {
            false
        }
    }

    fn terminal_with_capture() -> (Arc<Terminal>, Arc<CapturingTransport>) {
        let transport = Arc::new(CapturingTransport {
            writes: Mutex::new(Vec::new()),
        });
        let terminal = Arc::new(Terminal::new(
            "ime".into(),
            TerminalSize::default(),
            transport.clone(),
            String::new(),
        ));
        (terminal, transport)
    }

    #[test]
    fn a_commit_ends_the_composition_and_sends_only_the_composed_text() {
        let (terminal, transport) = terminal_with_capture();
        terminal.set_marked_text("ˇ");

        commit_text(&terminal, "ď", 1);

        assert_eq!(transport.writes(), vec!["ď".as_bytes().to_vec()]);
        assert_eq!(terminal.marked_text(), None);
    }

    #[test]
    fn a_commit_still_maps_control_characters() {
        let (terminal, transport) = terminal_with_capture();

        commit_text(&terminal, "a\u{8}\n", 1);

        assert_eq!(
            transport.writes(),
            vec![b"a".to_vec(), b"\x7f".to_vec(), b"\r".to_vec()]
        );
    }

    #[test]
    fn a_commit_drops_macos_function_key_placeholders() {
        let (terminal, transport) = terminal_with_capture();

        commit_text(&terminal, "\u{F700}", 1);

        assert!(transport.writes().is_empty());
    }
}
