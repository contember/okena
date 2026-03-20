//! Expand/collapse toggle component.
//!
//! A chevron arrow that rotates between right (collapsed) and down (expanded),
//! or renders as a spacer when there's no expandable content.

use crate::theme::ThemeColors;
use gpui::*;

/// Expand/collapse toggle arrow, or a spacer when `has_content` is false.
///
/// Returns an `AnyElement` — either a clickable chevron or an empty spacer of the same size.
///
/// # Example
///
/// ```rust,ignore
/// expand_toggle("expand-project-1", is_expanded, has_children, &t)
/// ```
pub fn expand_toggle(
    id: impl Into<ElementId>,
    is_expanded: bool,
    has_content: bool,
    t: &ThemeColors,
) -> AnyElement {
    if has_content {
        div()
            .id(id)
            .flex_shrink_0()
            .w(px(12.0))
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
            .into_any_element()
    } else {
        div()
            .flex_shrink_0()
            .w(px(12.0))
            .h(px(16.0))
            .into_any_element()
    }
}
