//! Code block container component.

use crate::color_utils::{raised_surface, raised_surface_border};
use crate::theme::ThemeColors;
use crate::tokens::*;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::v_flex;

/// Code block container with rounded corners, bg, border, overflow_hidden, and optional language label.
///
/// The surface is one small step off the page rather than the darkest theme
/// background, so a document full of code reads as text with code in it instead
/// of a stack of black rectangles.
///
/// Caller adds `.child(...)` for the code content area.
pub fn code_block_container(language: Option<&str>, t: &ThemeColors, cx: &App) -> Div {
    let lang_label = language.unwrap_or("");
    let surface = raised_surface(t.bg_secondary, t.text_primary);
    let border = raised_surface_border(t.bg_secondary, t.border);
    v_flex()
        .rounded(px(6.0))
        .bg(rgb(surface))
        .border_1()
        .border_color(rgb(border))
        .overflow_hidden()
        .when(!lang_label.is_empty(), |d| {
            // A quiet caption, not a title bar — no fill, no divider.
            d.child(
                div()
                    .px(px(14.0))
                    .pt(SPACE_MD)
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(lang_label.to_string()),
            )
        })
}
