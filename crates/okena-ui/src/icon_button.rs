//! Reusable icon button component.
//!
//! A small square button containing an SVG icon with hover background.

use crate::theme::ThemeColors;
use gpui::*;

/// Small icon button (default 18x18, 12px icon).
///
/// Returns a Stateful<Div> ready for `.on_click()`, `.tooltip()`, etc.
///
/// # Example
///
/// ```rust,ignore
/// icon_button("close-btn", "icons/close.svg", &t)
///     .on_click(cx.listener(|this, _, _, cx| this.close(cx)))
/// ```
pub fn icon_button(
    id: impl Into<ElementId>,
    icon: impl Into<SharedString>,
    t: &ThemeColors,
) -> Stateful<Div> {
    icon_button_sized(id, icon, 18.0, 12.0, t)
}

/// Icon button with custom button and icon sizes.
///
/// Common sizes: 18x18 / 12px icon, 24x24 / 14px icon.
pub fn icon_button_sized(
    id: impl Into<ElementId>,
    icon: impl Into<SharedString>,
    button_size: f32,
    icon_size: f32,
    t: &ThemeColors,
) -> Stateful<Div> {
    icon_button_bare_sized(id, icon, button_size, icon_size, t).hover(|s| s.bg(rgb(t.bg_hover)))
}

/// Icon button without the built-in hover style, for callers that need to own
/// `.hover()` themselves — e.g. a button that also reveals itself on hover.
/// GPUI permits only one hover style per element and `debug_assert!`s on the
/// second call, so layering another `.hover()` on [`icon_button`] would panic.
pub fn icon_button_bare(
    id: impl Into<ElementId>,
    icon: impl Into<SharedString>,
    t: &ThemeColors,
) -> Stateful<Div> {
    icon_button_bare_sized(id, icon, 18.0, 12.0, t)
}

/// [`icon_button_sized`] without the hover style. See [`icon_button_bare`].
pub fn icon_button_bare_sized(
    id: impl Into<ElementId>,
    icon: impl Into<SharedString>,
    button_size: f32,
    icon_size: f32,
    t: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .cursor_pointer()
        .w(px(button_size))
        .h(px(button_size))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.0))
        .child(
            svg()
                .path(icon)
                .size(px(icon_size))
                .text_color(rgb(t.text_secondary)),
        )
}
