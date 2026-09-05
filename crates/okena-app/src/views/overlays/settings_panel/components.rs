use crate::theme::ThemeColors;
use crate::ui::tokens::ui_text_md;
use crate::views::components::simple_input::{SimpleInput, SimpleInputState};
use gpui::*;

// Re-export from okena-ui
pub use okena_ui::settings::{
    input_box, section_container, section_header, section_note, settings_input_row, settings_row,
    settings_row_with_desc, stepper,
};
pub use okena_ui::toggle::{Segment, segmented_control, toggle_switch};

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

/// Render a stacked row with label, description and a full-width text input.
pub(super) fn hook_input_row(
    id: impl Into<SharedString>,
    label: &str,
    desc: &str,
    input: &Entity<SimpleInputState>,
    t: &ThemeColors,
    has_border: bool,
    cx: &App,
) -> Stateful<Div> {
    settings_input_row(id, label, desc, t, cx, has_border)
        .child(input_box(t).child(SimpleInput::new(input).text_size(ui_text_md(cx))))
}

/// Convert empty string to None, non-empty to Some
pub(super) fn opt_string(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}
