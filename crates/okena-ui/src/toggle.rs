//! Toggle components.

use crate::theme::ThemeColors;
use crate::tokens::ui_text_md;
use gpui::*;

/// Segmented toggle button for switching between options.
///
/// `options` is a slice of `(label, is_active)` pairs.
pub fn segmented_toggle(options: &[(&str, bool)], t: &ThemeColors, cx: &App) -> Div {
    let mut container = div()
        .flex()
        .rounded(px(6.0))
        .bg(rgb(t.bg_secondary))
        .p(px(3.0));

    for (i, &(label, is_active)) in options.iter().enumerate() {
        let mut button = div()
            .px(px(10.0))
            .py(px(4.0))
            .rounded(px(4.0))
            .text_size(ui_text_md(cx))
            .cursor_pointer();

        if is_active {
            button = button
                .bg(rgb(t.bg_primary))
                .text_color(rgb(t.text_primary))
                .shadow_sm();
        } else {
            button = button
                .text_color(rgb(t.text_muted))
                .hover(|s| s.text_color(rgb(t.text_secondary)));
        }

        // Add small gap between buttons
        if i > 0 {
            container = container.child(div().w(px(2.0)));
        }

        container = container.child(button.child(label.to_string()));
    }

    container
}

/// Render a toggle switch.
pub fn toggle_switch(id: impl Into<SharedString>, enabled: bool, t: &ThemeColors) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .cursor_pointer()
        .w(px(40.0))
        .h(px(22.0))
        .rounded(px(11.0))
        .bg(if enabled { rgb(t.border_active) } else { rgb(t.bg_secondary) })
        .flex()
        .items_center()
        .child(
            div()
                .w(px(18.0))
                .h(px(18.0))
                .rounded_full()
                .bg(rgb(t.text_primary))
                .ml(if enabled { px(20.0) } else { px(2.0) }),
        )
}
