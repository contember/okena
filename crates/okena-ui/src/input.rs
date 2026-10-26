//! Input field components.

use crate::theme::ThemeColors;
use crate::tokens::*;
use gpui::*;
use gpui_component::v_flex;

/// Styled container for text inputs.
///
/// Provides consistent styling for input fields with optional focus highlight.
/// Pass `Some(true)` for focused state, `Some(false)` for unfocused, or `None` for no focus tracking.
pub fn input_container(t: &ThemeColors, focused: Option<bool>) -> Div {
    let border_color = match focused {
        Some(true) => t.border_active,
        _ => t.border,
    };
    div()
        .bg(rgb(t.bg_secondary))
        .border_1()
        .border_color(rgb(border_color))
        .rounded(RADIUS_STD)
}

/// Labeled input field with a label above the input container.
pub fn labeled_input(label: impl Into<SharedString>, t: &ThemeColors) -> Div {
    v_flex().gap(SPACE_XS).child(
        div()
            .text_size(TEXT_MS)
            .text_color(rgb(t.text_muted))
            .child(label.into()),
    )
}
