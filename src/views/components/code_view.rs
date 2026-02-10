//! Code view utilities.
//!
//! Provides shared utilities for virtualized code viewers:
//! - Scrollbar geometry calculation
//! - Scrollbar drag handling
//! - Text selection utilities

use super::syntax::{HighlightedLine, HighlightedSpan};
use crate::ui::SelectionState;
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
    state.base_handle.set_offset(point(px(0.0), px(-new_scroll)));
}

/// Build a StyledText with optional background highlights (e.g. selection or word-level diff).
/// Splits syntax color highlights at background range boundaries to produce
/// non-overlapping highlights (required by `StyledText::compute_runs`).
pub fn build_styled_text_with_backgrounds(
    spans: &[HighlightedSpan],
    bg_ranges: &[(std::ops::Range<usize>, Hsla)],
) -> StyledText {
    let mut text = String::new();
    let mut highlights = Vec::new();

    for span in spans {
        text.push_str(&span.text);
    }

    if bg_ranges.is_empty() {
        // Fast path: no background highlights, just syntax colors
        let mut offset = 0;
        for span in spans {
            let start = offset;
            offset += span.text.len();
            if start < offset {
                highlights.push((
                    start..offset,
                    HighlightStyle {
                        color: Some(span.color.into()),
                        ..Default::default()
                    },
                ));
            }
        }
    } else {
        // Split syntax spans at background range boundaries so no highlights overlap
        let mut offset = 0;
        for span in spans {
            let span_start = offset;
            let span_end = offset + span.text.len();
            offset = span_end;

            if span_start >= span_end {
                continue;
            }

            // Collect boundary points from bg_ranges that fall within this span
            let mut boundaries = vec![span_start];
            for (br, _) in bg_ranges {
                if br.start > span_start && br.start < span_end {
                    boundaries.push(br.start);
                }
                if br.end > span_start && br.end < span_end {
                    boundaries.push(br.end);
                }
            }
            boundaries.push(span_end);
            boundaries.sort();
            boundaries.dedup();

            for window in boundaries.windows(2) {
                let sub_start = window[0];
                let sub_end = window[1];
                if sub_start >= sub_end {
                    continue;
                }

                let mut style = HighlightStyle {
                    color: Some(span.color.into()),
                    ..Default::default()
                };

                // Apply background if this sub-range falls within any background range
                for (br, bg_color) in bg_ranges {
                    if sub_start >= br.start && sub_end <= br.end {
                        style.background_color = Some(*bg_color);
                        break;
                    }
                }

                highlights.push((sub_start..sub_end, style));
            }
        }
    }

    StyledText::new(text).with_highlights(highlights)
}

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
    let sel_start = if line_index == start_line { start_col.min(line_len) } else { 0 };
    let sel_end = if line_index == end_line { end_col.min(line_len) } else { line_len };
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

    if result.is_empty() { None } else { Some(result) }
}

/// Get selected text from highlighted lines (convenience wrapper).
pub fn get_selected_text(lines: &[HighlightedLine], selection: &CodeSelection) -> Option<String> {
    extract_selected_text(selection, lines.len(), |i| &lines[i].plain_text)
}
