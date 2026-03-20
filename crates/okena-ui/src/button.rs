//! Button components.

use crate::theme::ThemeColors;
use crate::tokens::*;
use gpui::*;

/// Standard secondary button.
///
/// Default padding is 12px horizontal, 6px vertical. Override with `.px()` and `.py()` if needed.
/// Returns a Stateful<Div> that can have `.on_click()` chained.
pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    t: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .px(SPACE_LG)
        .py(SPACE_SM)
        .rounded(RADIUS_STD)
        .bg(rgb(t.bg_secondary))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .text_size(TEXT_MD)
        .text_color(rgb(t.text_secondary))
        .child(label.into())
}

/// Primary action button (e.g., "Add", "Create", "Save").
///
/// Default padding is 12px horizontal, 6px vertical. Override with `.px()` and `.py()` if needed.
/// Returns a Stateful<Div> that can have `.on_click()` chained.
pub fn button_primary(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    t: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .px(SPACE_LG)
        .py(SPACE_SM)
        .rounded(RADIUS_STD)
        .bg(rgb(t.button_primary_bg))
        .hover(|s| s.bg(rgb(t.button_primary_hover)))
        .text_size(TEXT_MD)
        .text_color(rgb(t.button_primary_fg))
        .child(label.into())
}
