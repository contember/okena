//! Menu components for context menus.

use crate::theme::ThemeColors;
use crate::tokens::*;
use gpui::*;

/// Menu item text size (13px) — slightly larger than TEXT_MD for readability.
const MENU_TEXT: Pixels = px(13.0);

/// Menu item icon size (15px).
const MENU_ICON: Pixels = px(15.0);

/// Context menu item with icon and label.
///
/// Returns a Stateful<Div> that can have `.on_click()` chained.
pub fn menu_item(
    id: impl Into<ElementId>,
    icon: impl Into<SharedString>,
    label: impl Into<SharedString>,
    t: &ThemeColors,
) -> Stateful<Div> {
    menu_item_with_color(id, icon, label, t.text_primary, t.text_muted, t)
}

/// Context menu item with custom text and icon colors.
///
/// Use this for items with warning/error colors or disabled states.
pub fn menu_item_with_color(
    id: impl Into<ElementId>,
    icon: impl Into<SharedString>,
    label: impl Into<SharedString>,
    text_color: u32,
    icon_color: u32,
    t: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .mx(SPACE_SM)
        .px(SPACE_LG)
        .py(SPACE_SM)
        .flex()
        .items_center()
        .gap(SPACE_LG)
        .rounded(RADIUS_STD)
        .cursor_pointer()
        .text_size(MENU_TEXT)
        .text_color(rgb(text_color))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .child(
            svg()
                .path(icon)
                .size(MENU_ICON)
                .text_color(rgb(icon_color)),
        )
        .child(label.into())
}

/// Context menu item in disabled state (no hover, default cursor).
pub fn menu_item_disabled(
    id: impl Into<ElementId>,
    icon: impl Into<SharedString>,
    label: impl Into<SharedString>,
    t: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .mx(SPACE_SM)
        .px(SPACE_LG)
        .py(SPACE_SM)
        .flex()
        .items_center()
        .gap(SPACE_LG)
        .rounded(RADIUS_STD)
        .text_size(MENU_TEXT)
        .text_color(rgb(t.text_muted))
        .child(
            svg()
                .path(icon)
                .size(MENU_ICON)
                .text_color(rgb(t.text_muted)),
        )
        .child(label.into())
}

/// Context menu item with conditional enabled/disabled state.
///
/// When `enabled` is true: shows pointer cursor, hover effect, and primary colors.
/// When `enabled` is false: shows default cursor, no hover, and muted colors.
///
/// Returns a Stateful<Div> that can have `.on_click()` chained (caller should guard with `enabled`).
pub fn menu_item_conditional(
    id: impl Into<ElementId>,
    icon: impl Into<SharedString>,
    label: impl Into<SharedString>,
    enabled: bool,
    t: &ThemeColors,
) -> Stateful<Div> {
    let (text_color, icon_color) = if enabled {
        (t.text_primary, t.text_muted)
    } else {
        (t.text_muted, t.text_muted)
    };

    let bg_hover = t.bg_hover;

    let base = div()
        .id(id)
        .mx(SPACE_SM)
        .px(SPACE_LG)
        .py(SPACE_SM)
        .flex()
        .items_center()
        .gap(SPACE_LG)
        .rounded(RADIUS_STD)
        .text_size(MENU_TEXT)
        .text_color(rgb(text_color))
        .cursor(if enabled { CursorStyle::PointingHand } else { CursorStyle::Arrow })
        .child(
            svg()
                .path(icon)
                .size(MENU_ICON)
                .text_color(rgb(icon_color)),
        )
        .child(label.into());

    if enabled {
        base.hover(move |s| s.bg(rgb(bg_hover)))
    } else {
        base
    }
}

/// Context menu panel with standard styling (bg, border, shadow, min_w, py).
///
/// Comes with stop-propagation handlers on left-click, right-click, and scroll.
/// Caller adds `.child(menu_item(...))` for content.
pub fn context_menu_panel(id: impl Into<ElementId>, t: &ThemeColors) -> Stateful<Div> {
    div()
        .id(id)
        .bg(rgb(t.bg_primary))
        .border_1()
        .border_color(rgb(t.border))
        .rounded(px(8.0))
        .shadow_xl()
        .min_w(px(240.0))
        .py(SPACE_SM)
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_down(MouseButton::Right, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_scroll_wheel(|_, _, cx| {
            cx.stop_propagation();
        })
}

/// Menu separator - 1px horizontal line.
pub fn menu_separator(t: &ThemeColors) -> Div {
    div()
        .h(px(1.0))
        .mx(SPACE_XL)
        .my(SPACE_SM)
        .bg(rgb(t.border))
}
