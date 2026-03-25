//! Settings panel components.

use crate::theme::ThemeColors;
use crate::tokens::{ui_text, ui_text_sm, ui_text_ms, ui_text_xl};
use gpui::*;
use gpui_component::v_flex;

/// Render a section header.
pub fn section_header(title: &str, t: &ThemeColors, cx: &App) -> impl IntoElement {
    div()
        .px(px(16.0))
        .py(px(8.0))
        .text_size(ui_text_ms(cx))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(t.text_muted))
        .child(title.to_uppercase())
}

/// Render a settings section container.
pub fn section_container(t: &ThemeColors) -> Div {
    div()
        .mx(px(16.0))
        .mb(px(12.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(rgb(t.border))
        .overflow_hidden()
}

/// Render a settings row container.
pub fn settings_row(id: impl Into<SharedString>, label: &str, t: &ThemeColors, cx: &App, has_border: bool) -> Stateful<Div> {
    let row = div()
        .id(ElementId::Name(id.into()))
        .px(px(12.0))
        .py(px(8.0))
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(ui_text(13.0, cx))
                .text_color(rgb(t.text_primary))
                .child(label.to_string()),
        );

    if has_border {
        row.border_b_1().border_color(rgb(t.border))
    } else {
        row
    }
}

/// Render a settings row with label and description.
pub fn settings_row_with_desc(id: impl Into<SharedString>, label: &str, desc: &str, t: &ThemeColors, cx: &App, has_border: bool) -> Stateful<Div> {
    let row = div()
        .id(ElementId::Name(id.into()))
        .px(px(12.0))
        .py(px(8.0))
        .flex()
        .items_center()
        .justify_between()
        .child(
            v_flex()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(ui_text(13.0, cx))
                        .text_color(rgb(t.text_primary))
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(t.text_muted))
                        .child(desc.to_string()),
                ),
        );

    if has_border {
        row.border_b_1().border_color(rgb(t.border))
    } else {
        row
    }
}

/// Render a +/- stepper button.
pub fn stepper_button(id: impl Into<SharedString>, label: &str, t: &ThemeColors, cx: &App) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .cursor_pointer()
        .w(px(24.0))
        .h(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .bg(rgb(t.bg_secondary))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .text_size(ui_text_xl(cx))
        .text_color(rgb(t.text_secondary))
        .child(label.to_string())
}

/// Render a value display box.
pub fn value_display(value: String, width: f32, t: &ThemeColors, cx: &App) -> Div {
    div()
        .w(px(width))
        .h(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .bg(rgb(t.bg_secondary))
        .text_size(ui_text(13.0, cx))
        .font_family("monospace")
        .text_color(rgb(t.text_primary))
        .child(value)
}
