//! Reusable popover components for anchored floating panels.
//!
//! Provides anchored positioning and styled panel container.
//! Uses GPUI's `deferred(anchored().position().snap_to_window())` pattern.

use crate::theme::ThemeColors;
use gpui::*;

/// Styled popover panel container with standard look: bg, border, rounded corners, shadow.
///
/// Stops mouse-down and scroll-wheel propagation to prevent interaction with elements underneath.
pub fn popover_panel(id: impl Into<SharedString>, t: &ThemeColors) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .occlude()
        .bg(rgb(t.bg_primary))
        .border_1()
        .border_color(rgb(t.border))
        .rounded(px(6.0))
        .shadow_lg()
        .p(px(8.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_scroll_wheel(|_, _, cx| {
            cx.stop_propagation();
        })
}
