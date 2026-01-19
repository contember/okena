//! Context menu rendering for terminal pane.
//!
//! Standalone rendering function for the right-click context menu.

use crate::theme::ThemeColors;
use gpui::prelude::FluentBuilder;
use gpui::*;

/// Render the context menu at the given position.
/// Note: This creates the visual elements only. Event handlers must be added
/// by the caller since we cannot capture mutable callbacks in Fn closures.
pub fn render_context_menu(
    position: Point<Pixels>,
    element_bounds: Option<Bounds<Pixels>>,
    has_selection: bool,
    t: &ThemeColors,
) -> impl IntoElement {
    // Calculate menu height for positioning
    let menu_height = 9.0 * 26.0 + 3.0 * 9.0 + 8.0;

    // Calculate relative position and direction
    let (relative_pos, open_upward) = if let Some(bounds) = element_bounds {
        let rel_x = position.x - bounds.origin.x;
        let rel_y = position.y - bounds.origin.y;
        let space_below = f32::from(bounds.size.height) - f32::from(rel_y);
        let should_open_up = space_below < menu_height;
        (Point { x: rel_x, y: rel_y }, should_open_up)
    } else {
        (position, false)
    };

    let menu = div()
        .id("terminal-context-menu")
        .absolute()
        .left(relative_pos.x)
        .bg(rgb(t.bg_secondary))
        .border_1()
        .border_color(rgb(t.border))
        .rounded(px(4.0))
        .shadow_lg()
        .py(px(4.0))
        .min_w(px(120.0))
        // Copy
        .child(
            div()
                .id("context-menu-copy")
                .px(px(12.0))
                .py(px(6.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_size(px(13.0))
                .text_color(if has_selection {
                    rgb(t.text_primary)
                } else {
                    rgb(t.text_muted)
                })
                .cursor(if has_selection {
                    CursorStyle::PointingHand
                } else {
                    CursorStyle::Arrow
                })
                .when(has_selection, |el| el.hover(|s| s.bg(rgb(t.bg_hover))))
                .child(
                    svg()
                        .path("icons/copy.svg")
                        .size(px(14.0))
                        .text_color(if has_selection {
                            rgb(t.text_secondary)
                        } else {
                            rgb(t.text_muted)
                        }),
                )
                .child("Copy"),
        )
        // Paste
        .child(
            div()
                .id("context-menu-paste")
                .px(px(12.0))
                .py(px(6.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_size(px(13.0))
                .text_color(rgb(t.text_primary))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .cursor_pointer()
                .child(
                    svg()
                        .path("icons/clipboard-paste.svg")
                        .size(px(14.0))
                        .text_color(rgb(t.text_secondary)),
                )
                .child("Paste"),
        )
        // Separator
        .child(div().h(px(1.0)).mx(px(8.0)).my(px(4.0)).bg(rgb(t.border)))
        // Clear
        .child(
            div()
                .id("context-menu-clear")
                .px(px(12.0))
                .py(px(6.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_size(px(13.0))
                .text_color(rgb(t.text_primary))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .cursor_pointer()
                .child(
                    svg()
                        .path("icons/eraser.svg")
                        .size(px(14.0))
                        .text_color(rgb(t.text_secondary)),
                )
                .child("Clear"),
        )
        // Select All
        .child(
            div()
                .id("context-menu-select-all")
                .px(px(12.0))
                .py(px(6.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_size(px(13.0))
                .text_color(rgb(t.text_primary))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .cursor_pointer()
                .child(
                    svg()
                        .path("icons/select-all.svg")
                        .size(px(14.0))
                        .text_color(rgb(t.text_secondary)),
                )
                .child("Select All"),
        )
        // Separator
        .child(div().h(px(1.0)).mx(px(8.0)).my(px(4.0)).bg(rgb(t.border)))
        // Split Horizontal
        .child(
            div()
                .id("context-menu-split-h")
                .px(px(12.0))
                .py(px(6.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_size(px(13.0))
                .text_color(rgb(t.text_primary))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .cursor_pointer()
                .child(
                    svg()
                        .path("icons/split-horizontal.svg")
                        .size(px(14.0))
                        .text_color(rgb(t.text_secondary)),
                )
                .child("Split Horizontal"),
        )
        // Split Vertical
        .child(
            div()
                .id("context-menu-split-v")
                .px(px(12.0))
                .py(px(6.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_size(px(13.0))
                .text_color(rgb(t.text_primary))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .cursor_pointer()
                .child(
                    svg()
                        .path("icons/split-vertical.svg")
                        .size(px(14.0))
                        .text_color(rgb(t.text_secondary)),
                )
                .child("Split Vertical"),
        )
        // Separator
        .child(div().h(px(1.0)).mx(px(8.0)).my(px(4.0)).bg(rgb(t.border)))
        // Close
        .child(
            div()
                .id("context-menu-close")
                .px(px(12.0))
                .py(px(6.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_size(px(13.0))
                .text_color(rgb(t.error))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .cursor_pointer()
                .child(
                    svg()
                        .path("icons/close.svg")
                        .size(px(14.0))
                        .text_color(rgb(t.error)),
                )
                .child("Close"),
        );

    // Position menu
    if open_upward {
        let bottom_offset = if let Some(bounds) = element_bounds {
            f32::from(bounds.size.height) - f32::from(relative_pos.y)
        } else {
            0.0
        };
        menu.bottom(px(bottom_offset))
    } else {
        menu.top(relative_pos.y)
    }
}
