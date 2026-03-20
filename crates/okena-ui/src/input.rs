//! Input field components.

use crate::theme::{with_alpha, ThemeColors};
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
    v_flex()
        .gap(SPACE_XS)
        .child(
            div()
                .text_size(TEXT_MS)
                .text_color(rgb(t.text_muted))
                .child(label.into()),
        )
}

/// Search input area with ">" prefix prompt and query/placeholder display.
pub fn search_input_area(query: &str, placeholder: &str, t: &ThemeColors) -> Div {
    search_input_area_impl(query, placeholder, false, t)
}

/// Search input area with optional text selection highlight.
pub fn search_input_area_selected(query: &str, placeholder: &str, selected: bool, t: &ThemeColors) -> Div {
    search_input_area_impl(query, placeholder, selected, t)
}

fn search_input_area_impl(query: &str, placeholder: &str, selected: bool, t: &ThemeColors) -> Div {
    let query_element: AnyElement = if !query.is_empty() && selected {
        div()
            .flex_1()
            .text_size(TEXT_XL)
            .child(
                div()
                    .bg(with_alpha(t.border_active, 0.3))
                    .rounded(px(2.0))
                    .text_color(rgb(t.text_primary))
                    .child(query.to_string()),
            )
            .into_any_element()
    } else {
        div()
            .flex_1()
            .text_size(TEXT_XL)
            .text_color(if query.is_empty() {
                rgb(t.text_muted)
            } else {
                rgb(t.text_primary)
            })
            .child(if query.is_empty() {
                placeholder.to_string()
            } else {
                query.to_string()
            })
            .into_any_element()
    };

    div()
        .px(SPACE_LG)
        .py(px(10.0))
        .flex()
        .items_center()
        .gap(SPACE_MD)
        .border_b_1()
        .border_color(rgb(t.border))
        .child(
            div()
                .text_size(TEXT_XL)
                .text_color(rgb(t.text_muted))
                .child(">"),
        )
        .child(query_element)
}
