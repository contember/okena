//! Empty state placeholder for lists and containers.

use crate::theme::ThemeColors;
use crate::tokens::ui_text;
use gpui::*;

/// Empty state placeholder message for lists.
///
/// Centered muted text, typically shown when a filtered list has no results.
pub fn empty_state(message: impl Into<SharedString>, t: &ThemeColors, cx: &App) -> Div {
    div()
        .px(px(12.0))
        .py(px(20.0))
        .text_size(ui_text(13.0, cx))
        .text_color(rgb(t.text_muted))
        .child(message.into())
}
