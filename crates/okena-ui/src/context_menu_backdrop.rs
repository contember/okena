//! Context menu backdrop with anchored positioning.

use gpui::*;

/// Backdrop for context menus — absolute, inset-0, transparent.
///
/// Closes on left-click and right-click. Caller adds `.child(deferred(anchored()...))`.
pub fn context_menu_backdrop<F>(
    id: impl Into<SharedString>,
    on_close: F,
) -> Stateful<Div>
where
    F: Fn(&MouseDownEvent, &mut Window, &mut App) + Clone + 'static,
{
    let on_close2 = on_close.clone();
    div()
        .id(ElementId::Name(id.into()))
        .absolute()
        .inset_0()
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            on_close(ev, window, cx);
        })
        .on_mouse_down(MouseButton::Right, move |ev, window, cx| {
            on_close2(ev, window, cx);
        })
}
