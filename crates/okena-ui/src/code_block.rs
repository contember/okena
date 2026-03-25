//! Code block container component.

use crate::theme::ThemeColors;
use crate::tokens::*;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::v_flex;

/// Code block container with rounded corners, bg, border, overflow_hidden, and optional language label.
///
/// Caller adds `.child(...)` for the code content area.
pub fn code_block_container(language: Option<&str>, t: &ThemeColors, cx: &App) -> Div {
    let lang_label = language.unwrap_or("");
    v_flex()
        .rounded(px(6.0))
        .bg(rgb(t.bg_primary))
        .border_1()
        .border_color(rgb(t.border))
        .overflow_hidden()
        .when(!lang_label.is_empty(), |d| {
            d.child(
                div()
                    .px(SPACE_LG)
                    .py(SPACE_XS)
                    .bg(rgb(t.bg_header))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(lang_label.to_string()),
            )
        })
}
