//! Dropdown component for selecting from a list of options.
//!
//! Provides a reusable dropdown button with overlay list.

use crate::theme::{with_alpha, ThemeColors};
use gpui::*;
use gpui::prelude::*;

/// Create a dropdown trigger button that tracks its own bounds for overlay positioning.
///
/// The `on_bounds` callback is called during each paint with the button's window-absolute bounds.
/// Use these bounds with `dropdown_anchored_below()` to position the overlay.
pub fn dropdown_button(
    id: impl Into<SharedString>,
    label: &str,
    is_open: bool,
    t: &ThemeColors,
    on_bounds: impl Fn(Bounds<Pixels>, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .cursor_pointer()
        .min_w(px(150.0))
        .h(px(28.0))
        .px(px(10.0))
        .rounded(px(4.0))
        .bg(rgb(t.bg_secondary))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(12.0))
                .text_color(rgb(t.text_primary))
                .child(label.to_string()),
        )
        .child(
            div()
                .text_size(px(10.0))
                .text_color(rgb(t.text_muted))
                .child(if is_open { "▲" } else { "▼" }),
        )
        .child(canvas(on_bounds, |_, _, _, _| {}).absolute().size_full())
}

/// Position a dropdown overlay below the given trigger bounds.
pub fn dropdown_anchored_below(bounds: Bounds<Pixels>, child: impl IntoElement) -> Deferred {
    deferred(
        anchored()
            .position(point(bounds.origin.x, bounds.origin.y + bounds.size.height + px(2.0)))
            .snap_to_window()
            .child(child)
    )
}

/// Create a dropdown overlay container.
pub fn dropdown_overlay(
    id: impl Into<SharedString>,
    t: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .occlude()
        .min_w(px(150.0))
        .max_h(px(200.0))
        .overflow_y_scroll()
        .bg(rgb(t.bg_primary))
        .border_1()
        .border_color(rgb(t.border))
        .rounded(px(4.0))
        .shadow_xl()
        .py(px(4.0))
        // Prevent scroll events from propagating to terminal underneath
        .on_scroll_wheel(|_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
}

/// Create a single dropdown option row.
pub fn dropdown_option(
    id: impl Into<SharedString>,
    label: &str,
    is_selected: bool,
    t: &ThemeColors,
) -> Stateful<Div> {
    let row = div()
        .id(ElementId::Name(id.into()))
        .px(px(10.0))
        .py(px(6.0))
        .cursor_pointer()
        .text_size(px(12.0))
        .text_color(rgb(t.text_primary))
        .when(is_selected, |d| d.bg(with_alpha(t.border_active, 0.2)))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .flex()
        .items_center()
        .justify_between()
        .child(label.to_string());

    if is_selected {
        row.child(
            div()
                .text_size(px(10.0))
                .text_color(rgb(t.border_active))
                .child("✓"),
        )
    } else {
        row
    }
}
