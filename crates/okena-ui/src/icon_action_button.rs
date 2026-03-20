//! Icon action button for service controls.
//!
//! A 22x22 square button with a centered icon character (e.g., "▶", "■", "↻").

use crate::theme::ThemeColors;
use gpui::*;

/// 22×22 icon action button with text icon character.
///
/// Used for service Start/Stop/Restart controls. The icon is a text character, not an SVG.
/// Returns a Stateful<Div> ready for `.on_click()`, `.tooltip()`.
pub fn icon_action_button(
    id: impl Into<ElementId>,
    icon_char: impl Into<SharedString>,
    icon_color: u32,
    t: &ThemeColors,
) -> Stateful<Div> {
    icon_action_button_sized(id, icon_char, icon_color, 22.0, t)
}

/// Icon action button with custom size.
pub fn icon_action_button_sized(
    id: impl Into<ElementId>,
    icon_char: impl Into<SharedString>,
    icon_color: u32,
    size: f32,
    t: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .w(px(size))
        .h(px(size))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.0))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .child(
            div()
                .text_size(px(10.0))
                .text_color(rgb(icon_color))
                .child(icon_char.into()),
        )
}
