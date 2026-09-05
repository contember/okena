//! Toggle components.

use crate::theme::{ThemeColors, with_alpha};
use crate::tokens::ui_text_md;
use gpui::prelude::FluentBuilder;
use gpui::*;

/// A single option in a [`segmented_control`]: label + id suffix + selected flag.
pub struct Segment<'a> {
    pub id: SharedString,
    pub label: &'a str,
    pub selected: bool,
}

/// Segmented control — a row of mutually exclusive options in one pill.
///
/// `on_select` receives the index of the clicked segment.
pub fn segmented_control<F>(
    id: &str,
    segments: &[Segment<'_>],
    t: &ThemeColors,
    cx: &App,
    on_select: F,
) -> Div
where
    F: Fn(usize, &mut Window, &mut App) + Clone + 'static,
{
    let mut container = div()
        .flex()
        .items_center()
        .gap(px(1.0))
        .rounded(px(5.0))
        .bg(rgb(t.bg_secondary))
        .border_1()
        .border_color(rgb(t.border))
        .p(px(2.0));

    for (i, seg) in segments.iter().enumerate() {
        let on_select = on_select.clone();
        container = container.child(
            div()
                .id(ElementId::Name(format!("{}-{}", id, seg.id).into()))
                .cursor_pointer()
                .px(px(9.0))
                .py(px(3.0))
                .rounded(px(3.0))
                .text_size(ui_text_md(cx))
                .when(seg.selected, |el| {
                    el.bg(with_alpha(t.border_active, 0.22))
                        .text_color(rgb(t.text_primary))
                        .font_weight(FontWeight::MEDIUM)
                })
                .when(!seg.selected, |el| {
                    el.text_color(rgb(t.text_muted))
                        .hover(|s| s.bg(rgb(t.bg_hover)).text_color(rgb(t.text_secondary)))
                })
                .child(seg.label.to_string())
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    on_select(i, window, cx);
                }),
        );
    }

    container
}

/// Segmented toggle button for switching between options.
///
/// `options` is a slice of `(label, is_active)` pairs.
pub fn segmented_toggle(options: &[(&str, bool)], t: &ThemeColors, cx: &App) -> Div {
    let mut container = div()
        .flex()
        .rounded(px(6.0))
        .bg(rgb(t.bg_secondary))
        .p(px(3.0));

    for (i, &(label, is_active)) in options.iter().enumerate() {
        let mut button = div()
            .px(px(10.0))
            .py(px(4.0))
            .rounded(px(4.0))
            .text_size(ui_text_md(cx))
            .cursor_pointer();

        if is_active {
            button = button
                .bg(rgb(t.bg_primary))
                .text_color(rgb(t.text_primary))
                .shadow_sm();
        } else {
            button = button
                .text_color(rgb(t.text_muted))
                .hover(|s| s.text_color(rgb(t.text_secondary)));
        }

        // Add small gap between buttons
        if i > 0 {
            container = container.child(div().w(px(2.0)));
        }

        container = container.child(button.child(label.to_string()));
    }

    container
}

/// Render a toggle switch.
///
/// Deliberately small: settings panes stack dozens of these, so a loud
/// full-size switch dominates the rows it belongs to.
pub fn toggle_switch(id: impl Into<SharedString>, enabled: bool, t: &ThemeColors) -> Stateful<Div> {
    let track = if enabled {
        with_alpha(t.border_active, 0.9)
    } else {
        with_alpha(t.text_muted, 0.35)
    };
    let knob = if enabled {
        hsla(0.0, 0.0, 1.0, 0.95)
    } else {
        rgb(t.text_secondary).into()
    };

    div()
        .id(ElementId::Name(id.into()))
        .cursor_pointer()
        .w(px(28.0))
        .h(px(16.0))
        .rounded(px(8.0))
        .bg(track)
        .flex()
        .items_center()
        .child(
            div()
                .w(px(12.0))
                .h(px(12.0))
                .rounded_full()
                .bg(knob)
                .ml(if enabled { px(14.0) } else { px(2.0) }),
        )
}
