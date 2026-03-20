//! Reusable list row component.
//!
//! A standard row for sidebar lists, file trees, etc.

use crate::theme::ThemeColors;
use gpui::*;

/// Standard list row (24px height, flex, items_center, gap(4), hover bg).
///
/// `left_padding` controls indentation (e.g., 4.0 for top-level, 20.0 for nested).
///
/// Returns a Stateful<Div> ready for `.on_click()`, `.child()`, etc.
///
/// # Example
///
/// ```rust,ignore
/// list_row("project-row-1", 4.0, &t)
///     .child(color_dot(0x4EC9B0, false))
///     .child(div().child("My Project"))
/// ```
pub fn list_row(
    id: impl Into<ElementId>,
    left_padding: f32,
    t: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(24.0))
        .pl(px(left_padding))
        .pr(px(8.0))
        .flex()
        .items_center()
        .gap(px(4.0))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(t.bg_hover)))
}
