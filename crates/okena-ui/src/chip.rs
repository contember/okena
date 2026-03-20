//! Chip indicator components.

use crate::theme::ThemeColors;
use crate::tokens::*;
use gpui::*;
use gpui_component::h_flex;

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
        .px(SPACE_SM)
        .h(HEIGHT_CHIP)
        .flex()
        .items_center()
        .justify_center()
        .rounded(RADIUS_STD)
        .bg(rgb(t.bg_secondary))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .child(
            h_flex()
                .gap(SPACE_XS)
                .child(
                    div()
                        .text_size(TEXT_SM)
                        .text_color(rgb(t.text_secondary))
                        .child(shell_name.into()),
                )
                .child(
                    svg()
                        .path("icons/chevron-down.svg")
                        .size(ICON_SM)
                        .text_color(rgb(t.text_secondary)),
                ),
        )
}
