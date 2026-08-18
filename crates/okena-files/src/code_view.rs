//! Code view utilities.
//!
//! Provides shared utilities for virtualized code viewers:
//! - Scrollbar geometry calculation
//! - Scrollbar drag handling
//! - Text selection utilities

use crate::selection::SelectionState;
use crate::syntax::HighlightedLine;
use gpui::*;

/// Type alias for code selection (line index, column).
pub type CodeSelection = SelectionState<(usize, usize)>;

/// Selection highlight background color (consistent across all code viewers).
pub const SELECTION_BG: Rgba = Rgba {
    r: 0.25,
    g: 0.45,
    b: 0.75,
    a: 0.35,
};

/// State for scrollbar dragging.
#[derive(Clone, Copy)]
pub struct ScrollbarDrag {
    pub start_y: f32,
    pub start_scroll_y: f32,
}

/// Get scrollbar geometry if scrollable.
/// Returns (viewport_height, content_height, thumb_y, thumb_height).
pub fn get_scrollbar_geometry(
    scroll_handle: &UniformListScrollHandle,
) -> Option<(f32, f32, f32, f32)> {
    let state = scroll_handle.0.borrow();
    let item_size = state.last_item_size?;

    let viewport_height = f32::from(item_size.item.height);
    let content_height = f32::from(item_size.contents.height);

    if content_height <= viewport_height {
        return None;
    }

    let scroll_offset = state.base_handle.offset();
    let scroll_y = -f32::from(scroll_offset.y);

    let thumb_height = (viewport_height / content_height * viewport_height).max(20.0);
    let scrollable_content = content_height - viewport_height;
    let scrollable_track = viewport_height - thumb_height;
    let scroll_ratio = (scroll_y / scrollable_content).clamp(0.0, 1.0);
    let thumb_y = scroll_ratio * scrollable_track;

    Some((viewport_height, content_height, thumb_y, thumb_height))
}

/// Start scrollbar drag. Returns a drag state with start_y set to 0 (caller should set it).
pub fn start_scrollbar_drag(scroll_handle: &UniformListScrollHandle) -> ScrollbarDrag {
    let state = scroll_handle.0.borrow();
    let scroll_y = -f32::from(state.base_handle.offset().y);
    ScrollbarDrag {
        start_y: 0.0, // Caller should set this
        start_scroll_y: scroll_y,
    }
}

/// Update scrollbar during drag.
pub fn update_scrollbar_drag(
    scroll_handle: &UniformListScrollHandle,
    drag: ScrollbarDrag,
    current_y: f32,
) {
    let Some((viewport_height, content_height, _, thumb_height)) =
        get_scrollbar_geometry(scroll_handle)
    else {
        return;
    };

    let scrollable_content = content_height - viewport_height;
    let scrollable_track = viewport_height - thumb_height;

    if scrollable_track <= 0.0 {
        return;
    }

    let delta_y = current_y - drag.start_y;
    let delta_scroll = delta_y * scrollable_content / scrollable_track;
    let new_scroll = (drag.start_scroll_y + delta_scroll).clamp(0.0, scrollable_content);

    let state = scroll_handle.0.borrow_mut();
    state
        .base_handle
        .set_offset(point(px(0.0), px(-new_scroll)));
}

// Lives in `okena-highlight` (shared with the markdown renderer); re-exported so
// the code viewers keep their `code_view::build_styled_text_with_backgrounds`
// import.
pub use okena_highlight::styled::build_styled_text_with_backgrounds;

/// Compute selection background ranges for a single line.
///
/// Returns bg_ranges suitable for passing to `build_styled_text_with_backgrounds`.
/// Empty vec if the line is not selected.
pub fn selection_bg_ranges(
    selection: &CodeSelection,
    line_index: usize,
    line_len: usize,
) -> Vec<(std::ops::Range<usize>, Hsla)> {
    let Some(((start_line, start_col), (end_line, end_col))) = selection.normalized() else {
        return vec![];
    };
    if line_index < start_line || line_index > end_line {
        return vec![];
    }
    let sel_start = if line_index == start_line {
        start_col.min(line_len)
    } else {
        0
    };
    let sel_end = if line_index == end_line {
        end_col.min(line_len)
    } else {
        line_len
    };
    if sel_start < sel_end {
        vec![(sel_start..sel_end, SELECTION_BG.into())]
    } else {
        vec![]
    }
}

/// Extract selected text from lines using a closure to get plain text per line.
///
/// Generic over any line source — callers provide a closure that returns
/// the plain text for a given line index.
pub fn extract_selected_text<'a>(
    selection: &CodeSelection,
    line_count: usize,
    get_plain_text: impl Fn(usize) -> &'a str,
) -> Option<String> {
    let ((start_line, start_col), (end_line, end_col)) = selection.normalized()?;

    let mut result = String::new();

    for line_idx in start_line..=end_line {
        if line_idx >= line_count {
            break;
        }

        let text = get_plain_text(line_idx);

        if start_line == end_line {
            let start = start_col.min(text.len());
            let end = end_col.min(text.len());
            result.push_str(&text[start..end]);
        } else if line_idx == start_line {
            let start = start_col.min(text.len());
            result.push_str(&text[start..]);
            result.push('\n');
        } else if line_idx == end_line {
            let end = end_col.min(text.len());
            result.push_str(&text[..end]);
        } else {
            result.push_str(text);
            result.push('\n');
        }
    }

    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

/// Get selected text from highlighted lines (convenience wrapper).
pub fn get_selected_text(lines: &[HighlightedLine], selection: &CodeSelection) -> Option<String> {
    extract_selected_text(selection, lines.len(), |i| &lines[i].plain_text)
}

pub use okena_ui::text_utils::find_word_boundaries;

#[cfg(test)]
mod tests {
    use super::find_word_boundaries;

    #[test]
    fn test_empty_string() {
        assert_eq!(find_word_boundaries("", 0), (0, 0));
    }

    #[test]
    fn test_single_word() {
        assert_eq!(find_word_boundaries("hello", 2), (0, 5));
    }

    #[test]
    fn test_word_at_start() {
        assert_eq!(find_word_boundaries("hello world", 0), (0, 5));
    }

    #[test]
    fn test_word_at_end() {
        assert_eq!(find_word_boundaries("hello world", 8), (6, 11));
    }

    #[test]
    fn test_word_in_middle() {
        assert_eq!(find_word_boundaries("foo bar baz", 5), (4, 7));
    }

    #[test]
    fn test_underscore_included() {
        assert_eq!(find_word_boundaries("foo_bar baz", 3), (0, 7));
    }

    #[test]
    fn test_punctuation_boundary() {
        // Clicking on the dot should select just the dot
        assert_eq!(find_word_boundaries("foo.bar", 3), (3, 3));
    }

    #[test]
    fn test_col_beyond_length() {
        // Byte offsets past the end clamp to text.len(), which sits on no char,
        // so the result is an empty selection at the end rather than the last word.
        assert_eq!(find_word_boundaries("hello", 100), (5, 5));
    }
}
