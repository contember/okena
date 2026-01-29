//! Shared UI helper functions for badges, keyboard hints, search inputs, and menu items.

use crate::theme::ThemeColors;
use gpui::*;

/// Context menu item with icon and label.
///
/// Returns a Stateful<Div> that can have `.on_click()` chained.
pub fn menu_item(
    id: impl Into<ElementId>,
    icon: impl Into<SharedString>,
    label: impl Into<SharedString>,
    t: &ThemeColors,
) -> Stateful<Div> {
    menu_item_with_color(id, icon, label, t.text_primary, t.text_secondary, t)
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
        .px(px(12.0))
        .py(px(6.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .cursor_pointer()
        .text_size(px(12.0))
        .text_color(rgb(text_color))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .child(
            svg()
                .path(icon)
                .size(px(14.0))
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
        .px(px(12.0))
        .py(px(6.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .text_size(px(12.0))
        .text_color(rgb(t.text_muted))
        .child(
            svg()
                .path(icon)
                .size(px(14.0))
                .text_color(rgb(t.text_muted)),
        )
        .child(label.into())
}

/// Small pill label for categories like "Custom", "worktree", etc.
pub fn badge(text: impl Into<SharedString>, t: &ThemeColors) -> Div {
    div()
        .px(px(6.0))
        .py(px(1.0))
        .rounded(px(3.0))
        .bg(rgb(t.bg_secondary))
        .text_size(px(9.0))
        .text_color(rgb(t.text_muted))
        .child(text.into())
}

/// Keyboard key badge (e.g., "Enter", "Esc").
pub fn kbd(key: impl Into<SharedString>, t: &ThemeColors) -> Div {
    div()
        .px(px(4.0))
        .py(px(1.0))
        .rounded(px(3.0))
        .bg(rgb(t.bg_secondary))
        .text_size(px(10.0))
        .text_color(rgb(t.text_muted))
        .child(key.into())
}

/// Keyboard key badge + description text (e.g., `[Enter] to select`).
pub fn keyboard_hint(key: impl Into<SharedString>, description: impl Into<SharedString>, t: &ThemeColors) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .child(kbd(key, t))
        .child(
            div()
                .text_size(px(10.0))
                .text_color(rgb(t.text_muted))
                .child(description.into()),
        )
}

/// Footer bar with a row of keyboard hints.
///
/// `hints` is a slice of `(key, description)` pairs.
pub fn keyboard_hints_footer(hints: &[(&str, &str)], t: &ThemeColors) -> Div {
    let mut footer = div()
        .px(px(12.0))
        .py(px(8.0))
        .border_t_1()
        .border_color(rgb(t.border))
        .flex()
        .items_center()
        .gap(px(16.0));

    for &(key, desc) in hints {
        footer = footer.child(keyboard_hint(key.to_string(), desc.to_string(), t));
    }

    footer
}

/// Segmented toggle button for switching between options.
///
/// `options` is a slice of `(label, is_active)` pairs.
pub fn segmented_toggle(options: &[(&str, bool)], t: &ThemeColors) -> Div {
    let mut container = div()
        .flex()
        .rounded(px(4.0))
        .bg(rgb(t.bg_secondary))
        .p(px(2.0));

    for (i, &(label, is_active)) in options.iter().enumerate() {
        let mut button = div()
            .px(px(8.0))
            .py(px(3.0))
            .rounded(px(3.0))
            .text_size(px(11.0))
            .cursor_pointer();

        if is_active {
            button = button
                .bg(rgb(t.bg_primary))
                .text_color(rgb(t.text_primary));
        } else {
            button = button
                .text_color(rgb(t.text_muted));
        }

        // Add small gap between buttons
        if i > 0 {
            container = container.child(div().w(px(2.0)));
        }

        container = container.child(button.child(label.to_string()));
    }

    container
}

/// Shell indicator chip showing current shell name with dropdown chevron.
///
/// Returns a Stateful<Div> that can have `.on_mouse_down()` and `.tooltip()` chained.
pub fn shell_indicator_chip(
    id: impl Into<ElementId>,
    shell_name: impl Into<SharedString>,
    t: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .px(px(6.0))
        .h(px(18.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .bg(rgb(t.bg_secondary))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(rgb(t.text_secondary))
                        .child(shell_name.into()),
                )
                .child(
                    svg()
                        .path("icons/chevron-down.svg")
                        .size(px(10.0))
                        .text_color(rgb(t.text_secondary)),
                ),
        )
}

/// Search input area with ">" prefix prompt and query/placeholder display.
pub fn search_input_area(query: &str, placeholder: &str, t: &ThemeColors) -> Div {
    div()
        .px(px(12.0))
        .py(px(10.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .border_b_1()
        .border_color(rgb(t.border))
        .child(
            div()
                .text_size(px(14.0))
                .text_color(rgb(t.text_muted))
                .child(">"),
        )
        .child(
            div()
                .flex_1()
                .text_size(px(14.0))
                .text_color(if query.is_empty() {
                    rgb(t.text_muted)
                } else {
                    rgb(t.text_primary)
                })
                .child(if query.is_empty() {
                    placeholder.to_string()
                } else {
                    query.to_string()
                }),
        )
}
