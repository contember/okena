use crate::theme::ThemeColors;
use crate::views::components::simple_input::{SimpleInput, SimpleInputState};
use gpui::*;
use gpui_component::v_flex;

// Re-export from okena-ui
pub use okena_ui::settings::{
    section_container, section_header, settings_row, settings_row_with_desc, stepper_button,
    value_display,
};
pub use okena_ui::toggle::toggle_switch;

/// Available monospace font families
pub(super) const FONT_FAMILIES: &[&str] = &[
    "JetBrains Mono",
    "Menlo",
    "SF Mono",
    "Monaco",
    "Fira Code",
    "Source Code Pro",
    "Consolas",
    "DejaVu Sans Mono",
    "Ubuntu Mono",
    "Hack",
];

/// Render a hook input row with label, description, and text input
pub(super) fn hook_input_row(
    id: impl Into<SharedString>,
    label: &str,
    desc: &str,
    input: &Entity<SimpleInputState>,
    placeholder: &str,
    t: &ThemeColors,
    has_border: bool,
) -> Stateful<Div> {
    let _ = placeholder; // placeholder is set on the entity itself
    let row = div()
        .id(ElementId::Name(id.into()))
        .px(px(12.0))
        .py(px(8.0))
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            v_flex()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(rgb(t.text_primary))
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(rgb(t.text_muted))
                        .child(desc.to_string()),
                ),
        )
        .child(
            div()
                .bg(rgb(t.bg_secondary))
                .border_1()
                .border_color(rgb(t.border))
                .rounded(px(4.0))
                .child(SimpleInput::new(input).text_size(px(12.0))),
        );

    if has_border {
        row.border_b_1().border_color(rgb(t.border))
    } else {
        row
    }
}

/// Convert empty string to None, non-empty to Some
pub(super) fn opt_string(s: &str) -> Option<String> {
    if s.is_empty() { None } else { Some(s.to_string()) }
}
