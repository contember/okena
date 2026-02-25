//! Shared widget helpers for sidebar project/folder/terminal items.
//!
//! Each helper returns a partially-built element that the caller can chain
//! additional handlers onto (e.g. `.on_click()`).

use crate::theme::ThemeColors;
use crate::views::components::{rename_input, SimpleInput, RenameState};
use gpui::*;
use gpui::prelude::*;

/// Expand/collapse arrow (chevron-down/right, 16×16).
///
/// Caller chains `.on_click()` to toggle.
pub fn sidebar_expand_arrow(
    id: impl Into<ElementId>,
    is_expanded: bool,
    t: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .w(px(16.0))
        .h(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .child(
            svg()
                .path(if is_expanded {
                    "icons/chevron-down.svg"
                } else {
                    "icons/chevron-right.svg"
                })
                .size(px(12.0))
                .text_color(rgb(t.text_secondary)),
        )
}

/// Color indicator container (16×16, cursor_pointer, hover opacity).
///
/// `child` is the inner element – either a colored dot or a folder SVG.
/// Caller chains `.on_click()` to show color picker.
pub fn sidebar_color_indicator(
    id: impl Into<ElementId>,
    child: impl IntoElement,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .w(px(16.0))
        .h(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.opacity(0.7))
        .child(child)
}

/// Rename input container with SimpleInput.
///
/// Returns `Some(element)` if renaming is active, `None` otherwise.
/// Caller chains `.on_action(Cancel)` / `.on_key_down(Enter)`.
pub fn sidebar_rename_input<T: 'static + Clone>(
    id: impl Into<ElementId>,
    rename_state: &Option<RenameState<T>>,
    t: &ThemeColors,
) -> Option<Stateful<Div>> {
    let input = rename_input(rename_state)?;
    Some(
        div()
            .id(id)
            .flex_1()
            .min_w_0()
            .bg(rgb(t.bg_hover))
            .rounded(px(2.0))
            .child(SimpleInput::new(input).text_size(px(12.0)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_click(|_, _window, cx| {
                cx.stop_propagation();
            }),
    )
}

/// Name label with ellipsis and standard text styling.
///
/// Caller chains `.on_click()` for select / double-click rename.
pub fn sidebar_name_label(
    id: impl Into<ElementId>,
    name: impl Into<SharedString>,
    t: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .text_size(px(12.0))
        .text_color(rgb(t.text_primary))
        .text_ellipsis()
        .child(name.into())
}

/// Terminal count badge (number) with fixed width for vertical alignment.
/// When there are no terminals, renders an invisible placeholder of the same size.
pub fn sidebar_terminal_badge(
    has_layout: bool,
    count: usize,
    t: &ThemeColors,
) -> AnyElement {
    if has_layout && count > 0 {
        div()
            .flex_shrink_0()
            .min_w(px(18.0))
            .h(px(14.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .bg(rgb(t.bg_secondary))
            .text_size(px(10.0))
            .text_color(rgb(t.text_muted))
            .child(format!("{}", count))
            .into_any_element()
    } else {
        // Invisible placeholder to keep eye icons aligned
        div()
            .flex_shrink_0()
            .min_w(px(18.0))
            .into_any_element()
    }
}

/// Collapsible group header (e.g. "Terminals (3)" or "Services (2)").
///
/// Returns a `Stateful<Div>` so the caller can chain `.on_click()` to toggle collapse.
pub fn sidebar_group_header(
    id: impl Into<ElementId>,
    label: &str,
    count: usize,
    is_collapsed: bool,
    is_cursor: bool,
    left_padding: f32,
    t: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(20.0))
        .pl(px(left_padding))
        .pr(px(8.0))
        .flex()
        .items_center()
        .gap(px(4.0))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .when(is_cursor, |d: Stateful<Div>| d.border_l_2().border_color(rgb(t.border_active)))
        .child(
            // Expand/collapse chevron (smaller than project arrow)
            svg()
                .path(if is_collapsed {
                    "icons/chevron-right.svg"
                } else {
                    "icons/chevron-down.svg"
                })
                .size(px(10.0))
                .text_color(rgb(t.text_muted))
                .flex_shrink_0(),
        )
        .child(
            // Group label
            div()
                .text_size(px(10.0))
                .text_color(rgb(t.text_muted))
                .child(label.to_string()),
        )
        .child(
            // Item count badge
            div()
                .flex_shrink_0()
                .px(px(3.0))
                .py(px(0.0))
                .rounded(px(3.0))
                .bg(rgb(t.bg_secondary))
                .text_size(px(9.0))
                .text_color(rgb(t.text_muted))
                .child(format!("{}", count)),
        )
}

/// Visibility toggle (eye / eye-off).
///
/// Caller chains `.on_click()` to toggle visibility.
pub fn sidebar_visibility_toggle(
    id: impl Into<ElementId>,
    is_visible: bool,
    t: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .cursor_pointer()
        .w(px(18.0))
        .h(px(18.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.0))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .child(
            svg()
                .path(if is_visible {
                    "icons/eye.svg"
                } else {
                    "icons/eye-off.svg"
                })
                .size(px(12.0))
                .text_color(if is_visible {
                    rgb(t.term_blue)
                } else {
                    rgb(t.text_muted)
                }),
        )
}


