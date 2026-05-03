//! Modal backdrop component for overlay dialogs.
//!
//! Provides reusable builders for modal dialogs with standard styling.

use crate::theme::ThemeColors;
use crate::tokens::{ui_text, ui_text_md, ui_text_ms};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{h_flex, v_flex};

/// Create a fullscreen overlay that fills the entire window.
///
/// Used for content-heavy views (diff viewer, file viewer) that benefit
/// from maximum screen real estate. No backdrop, no rounded corners.
pub fn fullscreen_overlay(id: impl Into<SharedString>, t: &ThemeColors) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .occlude()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        .bg(rgb(t.bg_primary))
        .flex()
        .flex_col()
}

/// Like `fullscreen_overlay`, but sized via `size_full()` instead of
/// absolute positioning. Use when the overlay is hosted inside a parent
/// that owns the layout (e.g. a detached window's content area), where
/// absolute positioning does not interact correctly with flex sizing.
pub fn fullscreen_panel(id: impl Into<SharedString>, t: &ThemeColors) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .occlude()
        .size_full()
        .bg(rgb(t.bg_primary))
        .flex()
        .flex_col()
}

/// A spacer that always fills remaining flex space. When `enabled` is true
/// the spacer also acts as a drag handle for window move, mirroring the
/// main app titlebar: `WindowControlArea::Drag` (HTCAPTION on Windows,
/// platform-native drag on macOS) plus a Linux mouse-move fallback since
/// `WindowControlArea::Drag` is a no-op there.
pub fn window_drag_spacer(enabled: bool) -> Stateful<Div> {
    div()
        .id("window-drag-spacer")
        .flex_1()
        .h_full()
        .when(enabled, |d| {
            d.window_control_area(WindowControlArea::Drag)
                .when(cfg!(target_os = "linux"), |d| {
                    d.on_mouse_down(MouseButton::Left, |_, window, _cx| {
                        window.start_window_move();
                    })
                })
        })
}

/// Whether a detached overlay window should draw its own min/max chrome.
/// Mirrors the rule used by the main app titlebar so detached windows stay
/// consistent: always on Windows, never on macOS (native traffic lights),
/// runtime-determined on Linux.
pub fn detached_needs_controls(window: &Window) -> bool {
    if cfg!(target_os = "windows") {
        true
    } else if cfg!(target_os = "macos") {
        false
    } else {
        matches!(window.window_decorations(), Decorations::Client { .. })
    }
}

/// Render minimize + maximize buttons for a detached overlay window.
/// Returns an empty container when `needs_controls` is false (the OS draws
/// the controls itself, e.g. macOS server-side decorations).
///
/// The close button is intentionally omitted — the host overlay already has
/// its own close button which closes the detached window via `Close` event.
pub fn window_min_max_controls(
    needs_controls: bool,
    is_maximized: bool,
    t: &ThemeColors,
    cx: &App,
) -> Div {
    let t = t.clone();
    div().when(needs_controls, move |d| {
        d.child(
            h_flex()
                .gap(px(2.0))
                .child(window_chrome_button(
                    "dw-min",
                    "\u{2014}",
                    WindowControlArea::Min,
                    &t,
                    cx,
                    |window| window.minimize_window(),
                ))
                .child(window_chrome_button(
                    "dw-max",
                    if is_maximized { "\u{2750}" } else { "\u{25A1}" },
                    WindowControlArea::Max,
                    &t,
                    cx,
                    |window| window.zoom_window(),
                )),
        )
    })
}

/// Build a single chrome button. On Windows we mark the area with
/// `WindowControlArea` so the OS handles the click natively (matches the
/// main titlebar's behavior); on other platforms we wire a normal click
/// handler.
fn window_chrome_button(
    id: &'static str,
    label: &'static str,
    area: WindowControlArea,
    t: &ThemeColors,
    cx: &App,
    on_activate: fn(&mut Window),
) -> Stateful<Div> {
    let use_native = cfg!(target_os = "windows");
    div()
        .id(id)
        .cursor_pointer()
        .w(px(28.0))
        .h(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .text_size(ui_text_md(cx))
        .text_color(rgb(t.text_secondary))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .child(label)
        .when(use_native, |d| d.occlude().window_control_area(area))
        .when(!use_native, |d| {
            d.on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                on_activate(window);
            })
        })
}

/// Create a modal backdrop with click-to-close functionality.
///
/// Returns a positioned div that covers the screen with a semi-transparent overlay.
/// The backdrop handles clicks to close the modal.
///
/// # Example
///
/// ```rust,ignore
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
pub fn modal_header<F>(
    title: impl Into<SharedString>,
    subtitle: Option<impl Into<SharedString>>,
    t: &ThemeColors,
    cx: &App,
    on_close: F,
) -> Stateful<Div>
where
    F: Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
{
    let title = title.into();
    let subtitle = subtitle.map(|s| s.into());

    let mut title_section = v_flex().gap(px(2.0)).child(
        div()
            .text_size(ui_text(16.0, cx))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(t.text_primary))
            .child(title),
    );

    if let Some(subtitle) = subtitle {
        title_section = title_section.child(
            div()
                .text_size(ui_text_ms(cx))
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
                .text_size(ui_text(16.0, cx))
                .text_color(rgb(t.text_secondary))
                .child("✕")
                .on_mouse_down(MouseButton::Left, on_close),
        )
}
