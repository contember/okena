//! Empty state placeholder for lists and containers.

use crate::theme::ThemeColors;
use gpui::*;

/// Empty state placeholder message for lists.
///
/// Centered muted text, typically shown when a filtered list has no results.
pub fn empty_state(message: impl Into<SharedString>, t: &ThemeColors) -> Div {
    div()
        .px(px(12.0))
        .py(px(20.0))
        .text_size(px(13.0))
        .text_color(rgb(t.text_muted))
        .child(message.into())
}
