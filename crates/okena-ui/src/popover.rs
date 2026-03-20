//! Reusable popover components for anchored floating panels.
//!
//! Provides backdrop, anchored positioning, and styled panel container.
//! Use with GPUI's `deferred(anchored().position().snap_to_window())` pattern.

use crate::theme::ThemeColors;
use gpui::*;

/// Backdrop for popovers — absolute, inset-0, transparent.
///
/// Closes on left-click. Stops scroll-wheel propagation.
/// Caller chains `.on_mouse_down(MouseButton::Left, cx.listener(...))` for close behavior,
/// then adds `.child(popover_anchored(...))` for the content.
pub fn popover_backdrop(id: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .absolute()
        .inset_0()
        .on_scroll_wheel(|_, _, cx| {
            cx.stop_propagation();
        })
}

/// Position a popover anchored at the given point, auto-flipping to stay within window bounds.
pub fn popover_anchored(position: Point<Pixels>, child: impl IntoElement) -> Deferred {
    deferred(
        anchored()
            .position(position)
            .snap_to_window()
            .child(child),
    )
}

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
