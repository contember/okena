//! Selectable list item component for overlay lists.

use crate::theme::{ThemeColors, with_alpha};
use gpui::prelude::FluentBuilder;
use gpui::*;

/// A list item row with selection highlight and hover state.
///
/// Used in command palette, project switcher, file search, theme selector, etc.
/// Returns a Stateful<Div> ready for `.on_mouse_down()`, `.child()`, etc.
pub fn selectable_list_item(
    id: impl Into<ElementId>,
    is_selected: bool,
    t: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .flex()
        .items_center()
        .px(px(12.0))
        .py(px(8.0))
        .when(is_selected, |d| d.bg(with_alpha(t.border_active, 0.15)))
        .hover(|s| s.bg(rgb(t.bg_hover)))
}
