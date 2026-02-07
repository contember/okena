//! Modal backdrop component for overlay dialogs.
//!
//! Provides reusable builders for modal dialogs with standard styling.

use crate::theme::ThemeColors;
use gpui::*;

/// Create a modal backdrop with click-to-close functionality.
///
/// Returns a positioned div that covers the screen with a semi-transparent overlay.
/// The backdrop handles clicks to close the modal.
///
/// # Example
///
/// ```rust
/// modal_backdrop("my-modal-backdrop", &t)
///     .items_center() // or .items_start().pt(px(80.0)) for top positioning
///     .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.close(cx)))
///     .child(modal_content("my-modal", &t).child(...))
/// ```
pub fn modal_backdrop(id: impl Into<SharedString>, _t: &ThemeColors) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .occlude()
        .absolute()
        .inset_0()
        .bg(hsla(0.0, 0.0, 0.0, 0.5))
        .flex()
        .justify_center()
}

/// Create a modal content container with standard styling.
///
/// Returns a styled div with background, border, shadow, and rounded corners.
/// Includes a mouse handler that prevents clicks from propagating to the backdrop.
///
/// # Example
///
/// ```rust
/// modal_content("my-modal", &t)
///     .w(px(500.0))
///     .max_h(px(600.0))
///     .child(modal_header("Title", Some("Subtitle"), &t, on_close))
///     .child(body_content)
/// ```
pub fn modal_content(id: impl Into<SharedString>, t: &ThemeColors) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .bg(rgb(t.bg_primary))
        .rounded(px(8.0))
        .border_1()
        .border_color(rgb(t.border))
        .shadow_xl()
        .flex()
        .flex_col()
        // Prevent clicks on content from closing modal
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        // Prevent scroll events from propagating to terminal underneath
        .on_scroll_wheel(|_, _, cx| {
            cx.stop_propagation();
        })
}

/// Create a modal header with title, optional subtitle, and close button.
///
/// # Arguments
///
/// * `title` - The main title text
/// * `subtitle` - Optional subtitle/description text
/// * `t` - Theme colors
/// * `on_close` - Callback when close button is clicked
///
/// # Example
///
/// ```rust
/// modal_header(
///     "Session Manager",
///     Some("Save and restore workspace sessions"),
///     &t,
///     cx.listener(|this, _, _, cx| this.close(cx)),
/// )
/// ```
pub fn modal_header<F>(
    title: impl Into<SharedString>,
    subtitle: Option<impl Into<SharedString>>,
    t: &ThemeColors,
    on_close: F,
) -> Stateful<Div>
where
    F: Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
{
    let title = title.into();
    let subtitle = subtitle.map(|s| s.into());

    let mut title_section = div().flex().flex_col().gap(px(2.0)).child(
        div()
            .text_size(px(16.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(t.text_primary))
            .child(title),
    );

    if let Some(subtitle) = subtitle {
        title_section = title_section.child(
            div()
                .text_size(px(11.0))
                .text_color(rgb(t.text_muted))
                .child(subtitle),
        );
    }

    div()
        .id("modal-header")
        .px(px(16.0))
        .py(px(12.0))
        .flex()
        .items_center()
        .justify_between()
        .border_b_1()
        .border_color(rgb(t.border))
        .child(title_section)
        .child(
            div()
                .id("modal-close-btn")
                .cursor_pointer()
                .w(px(28.0))
                .h(px(28.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.0))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .text_size(px(16.0))
                .text_color(rgb(t.text_secondary))
                .child("✕")
                .on_mouse_down(MouseButton::Left, on_close),
        )
}
