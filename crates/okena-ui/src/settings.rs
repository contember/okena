//! Settings panel components.

use crate::theme::{ThemeColors, with_alpha};
use crate::tokens::{ui_text, ui_text_ms, ui_text_sm, ui_text_xl};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{h_flex, v_flex};

/// Horizontal padding shared by section headers, notes and section cards, so
/// labels line up down the whole pane.
const GUTTER: Pixels = px(16.0);
/// Inner padding of a single row inside a section card.
const ROW_PAD_X: Pixels = px(12.0);
const ROW_PAD_Y: Pixels = px(9.0);

/// Render a section header.
pub fn section_header(title: &str, t: &ThemeColors, cx: &App) -> impl IntoElement {
    div()
        .px(GUTTER)
        .pt(px(14.0))
        .pb(px(6.0))
        .text_size(ui_text_sm(cx))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(t.text_muted))
        .child(title.to_uppercase())
}

/// Explanatory paragraph that sits between a section header and its card.
pub fn section_note(text: impl Into<SharedString>, t: &ThemeColors, cx: &App) -> Div {
    div()
        .px(GUTTER)
        .pb(px(6.0))
        .text_size(ui_text_ms(cx))
        .text_color(rgb(t.text_muted))
        .child(text.into())
}

/// Render a settings section container.
pub fn section_container(t: &ThemeColors) -> Div {
    div()
        .mx(GUTTER)
        .mb(px(6.0))
        .rounded(px(6.0))
        .bg(with_alpha(t.bg_secondary, 0.5))
        .border_1()
        .border_color(rgb(t.border))
        .overflow_hidden()
}

/// Shared row scaffold: label column that shrinks, control column that does not.
fn row_base(id: impl Into<SharedString>, has_border: bool, t: &ThemeColors) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .px(ROW_PAD_X)
        .py(ROW_PAD_Y)
        .gap(GUTTER)
        .flex()
        .items_center()
        .justify_between()
        .when(has_border, |row| {
            row.border_b_1().border_color(rgb(t.border))
        })
}

/// Label text for a settings row.
fn row_label(label: &str, t: &ThemeColors, cx: &App) -> Div {
    div()
        .text_size(ui_text(13.0, cx))
        .text_color(rgb(t.text_primary))
        .child(label.to_string())
}

/// Secondary description text under a row label.
fn row_desc(desc: &str, t: &ThemeColors, cx: &App) -> Div {
    div()
        .text_size(ui_text_sm(cx))
        .text_color(rgb(t.text_muted))
        .child(desc.to_string())
}

/// Render a settings row container.
///
/// Children added by the caller are the row's control and are kept at their
/// natural width — the label column absorbs the remaining space.
pub fn settings_row(
    id: impl Into<SharedString>,
    label: &str,
    t: &ThemeColors,
    cx: &App,
    has_border: bool,
) -> Stateful<Div> {
    row_base(id, has_border, t).child(div().flex_1().min_w_0().child(row_label(label, t, cx)))
}

/// Render a settings row with label and description.
pub fn settings_row_with_desc(
    id: impl Into<SharedString>,
    label: &str,
    desc: &str,
    t: &ThemeColors,
    cx: &App,
    has_border: bool,
) -> Stateful<Div> {
    row_base(id, has_border, t).child(
        v_flex()
            .flex_1()
            .min_w_0()
            .gap(px(2.0))
            .child(row_label(label, t, cx))
            .child(row_desc(desc, t, cx)),
    )
}

/// Render a stacked row: label + description above, full-width control below.
///
/// Use this instead of [`settings_row_with_desc`] whenever the control is a
/// text input — a side-by-side input either squeezes the description or gets
/// pushed out of the panel by it.
pub fn settings_input_row(
    id: impl Into<SharedString>,
    label: &str,
    desc: &str,
    t: &ThemeColors,
    cx: &App,
    has_border: bool,
) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .px(ROW_PAD_X)
        .py(px(10.0))
        .flex()
        .flex_col()
        .gap(px(6.0))
        .when(has_border, |row| {
            row.border_b_1().border_color(rgb(t.border))
        })
        .child(
            v_flex()
                .gap(px(2.0))
                .child(row_label(label, t, cx))
                .when(!desc.is_empty(), |el| el.child(row_desc(desc, t, cx))),
        )
}

/// Bordered wrapper for a text input inside a settings row.
pub fn input_box(t: &ThemeColors) -> Div {
    div()
        .w_full()
        .bg(rgb(t.bg_primary))
        .border_1()
        .border_color(rgb(t.border))
        .rounded(px(4.0))
        .px(px(6.0))
        .py(px(3.0))
}

/// Render a +/- stepper as one joined control: `[-][ value ][+]`.
///
/// `on_step` receives `-1` for decrement and `1` for increment.
pub fn stepper<F>(id: &str, value: String, width: f32, t: &ThemeColors, cx: &App, on_step: F) -> Div
where
    F: Fn(i32, &mut Window, &mut App) + Clone + 'static,
{
    let dec = on_step.clone();
    h_flex()
        .rounded(px(5.0))
        .bg(rgb(t.bg_secondary))
        .border_1()
        .border_color(rgb(t.border))
        .overflow_hidden()
        .child(
            stepper_button(format!("{}-dec", id), "\u{2212}", t, cx)
                .on_mouse_down(MouseButton::Left, move |_, window, cx| dec(-1, window, cx)),
        )
        .child(
            div()
                .w(px(width))
                .h(px(22.0))
                .flex()
                .items_center()
                .justify_center()
                .border_l_1()
                .border_r_1()
                .border_color(rgb(t.border))
                .text_size(ui_text(12.0, cx))
                .font_family("monospace")
                .text_color(rgb(t.text_primary))
                .child(value),
        )
        .child(
            stepper_button(format!("{}-inc", id), "+", t, cx)
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    on_step(1, window, cx)
                }),
        )
}

/// Render a single +/- stepper button.
pub fn stepper_button(
    id: impl Into<SharedString>,
    label: &str,
    t: &ThemeColors,
    cx: &App,
) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .cursor_pointer()
        .w(px(22.0))
        .h(px(22.0))
        .flex()
        .items_center()
        .justify_center()
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .text_size(ui_text_xl(cx))
        .text_color(rgb(t.text_secondary))
        .child(label.to_string())
}

/// Render a value display box.
pub fn value_display(value: String, width: f32, t: &ThemeColors, cx: &App) -> Div {
    div()
        .w(px(width))
        .h(px(22.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .bg(rgb(t.bg_secondary))
        .text_size(ui_text(12.0, cx))
        .font_family("monospace")
        .text_color(rgb(t.text_primary))
        .child(value)
}
