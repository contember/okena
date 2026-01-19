//! Dropdown component for selecting from a list of options.
//!
//! Provides a reusable dropdown button with overlay list.

use crate::theme::{with_alpha, ThemeColors};
use gpui::*;
use gpui::prelude::*;

/// Create a dropdown trigger button.
///
/// # Arguments
/// * `id` - Unique element ID
/// * `label` - Currently selected value label
/// * `is_open` - Whether dropdown is currently open
/// * `t` - Theme colors
///
/// # Example
/// ```rust
/// dropdown_button("font-dropdown", &current_font, self.font_dropdown_open, &t)
///     .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
///         this.font_dropdown_open = !this.font_dropdown_open;
///         cx.notify();
///     }))
/// ```
pub fn dropdown_button(
    id: impl Into<SharedString>,
    label: &str,
    is_open: bool,
    t: &ThemeColors,
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
}

/// Create a dropdown overlay container.
///
/// # Arguments
/// * `id` - Unique element ID
/// * `top` - Top offset in pixels
/// * `right` - Right offset in pixels
/// * `t` - Theme colors
///
/// # Example
/// ```rust
/// dropdown_overlay("font-dropdown-list", 140.0, 32.0, &t)
///     .children(options.iter().map(|opt| dropdown_option(...)))
/// ```
pub fn dropdown_overlay(
    id: impl Into<SharedString>,
    top: f32,
    right: f32,
    t: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .absolute()
        .top(px(top))
        .right(px(right))
        .min_w(px(150.0))
        .max_h(px(200.0))
        .overflow_y_scroll()
        .bg(rgb(t.bg_primary))
        .border_1()
        .border_color(rgb(t.border))
        .rounded(px(4.0))
        .shadow_xl()
        .py(px(4.0))
}

/// Create a single dropdown option row.
///
/// # Arguments
/// * `id` - Unique element ID
/// * `label` - Display text
/// * `is_selected` - Whether this option is currently selected
/// * `t` - Theme colors
///
/// # Example
/// ```rust
/// dropdown_option("opt-jetbrains", "JetBrains Mono", is_selected, &t)
///     .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
///         // handle selection
///     }))
/// ```
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
