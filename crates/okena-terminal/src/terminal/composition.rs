use std::ops::Range;

use super::Terminal;

/// Text the platform IME is still composing (a dead key, a pinyin syllable).
/// It reaches the PTY only once committed; until then it is display state.
impl Terminal {
    pub fn set_marked_text(&self, text: &str) {
        let mut marked = self.marked_text.lock();
        *marked = if text.is_empty() {
            None
        } else {
            Some(text.to_string())
        };
    }

    pub fn marked_text(&self) -> Option<String> {
        self.marked_text.lock().clone()
    }

    /// AppKit's own marked range is unreliable (Zed #46084), so the range is
    /// the whole marked string.
    pub fn marked_text_range_utf16(&self) -> Option<Range<usize>> {
        self.marked_text
            .lock()
            .as_ref()
            .map(|text| 0..text.encode_utf16().count())
    }

    pub fn clear_marked_text(&self) {
        *self.marked_text.lock() = None;
    }
}
