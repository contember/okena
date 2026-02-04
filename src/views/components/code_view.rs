//! Code view utilities.
//!
//! Provides shared utilities for virtualized code viewers:
//! - Scrollbar geometry calculation
//! - Scrollbar drag handling
//! - Text selection utilities

use super::syntax::HighlightedLine;
use crate::ui::SelectionState;
use gpui::*;

/// Type alias for code selection (line index, column).
pub type CodeSelection = SelectionState<(usize, usize)>;

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
    state.base_handle.set_offset(point(px(0.0), px(-new_scroll)));
}

/// Get selected text from highlighted lines.
pub fn get_selected_text(lines: &[HighlightedLine], selection: &CodeSelection) -> Option<String> {
    let ((start_line, start_col), (end_line, end_col)) = selection.normalized()?;

    let mut result = String::new();

    for line_idx in start_line..=end_line {
        if line_idx >= lines.len() {
            break;
        }

        let line = &lines[line_idx];
        let text = &line.plain_text;

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
