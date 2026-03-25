//! Title + subtitle text pair component.

use crate::theme::ThemeColors;
use crate::tokens::{ui_text, ui_text_ms};
use gpui::*;
use gpui_component::v_flex;

/// Title and subtitle text pair (e.g., command name + description).
///
/// Title is 13px primary, subtitle is 11px muted. Stacked with 2px gap.
pub fn title_subtitle(
    title: impl Into<SharedString>,
    subtitle: impl Into<SharedString>,
    t: &ThemeColors,
    cx: &App,
) -> Div {
    v_flex()
        .gap(px(2.0))
        .child(
            div()
                .text_size(ui_text(13.0, cx))
                .text_color(rgb(t.text_primary))
                .child(title.into()),
        )
        .child(
            div()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child(subtitle.into()),
        )
}
