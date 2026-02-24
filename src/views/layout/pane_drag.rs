//! Pane drag-and-drop types for terminal rearrangement.
//!
//! Defines the drag payload, ghost view, and drop zone enum used
//! when dragging a terminal pane header onto another pane's edge zones.

use crate::theme::theme;
use gpui::*;
use gpui_component::h_flex;

/// Drag payload emitted from a pane header (terminal or app).
#[derive(Clone)]
pub struct PaneDrag {
    pub project_id: String,
    pub layout_path: Vec<usize>,
    pub pane_id: String,       // terminal_id or app_id
    pub pane_name: String,     // display name
    pub icon_path: String,     // "icons/terminal.svg" or "icons/kruh.svg"
}

/// Ghost view rendered while dragging a pane.
pub struct PaneDragView {
    label: String,
    icon_path: String,
}

impl PaneDragView {
    pub fn new(label: String, icon_path: String) -> Self {
        Self { label, icon_path }
    }
}

impl Render for PaneDragView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        div()
            .px(px(12.0))
            .py(px(6.0))
            .bg(crate::theme::with_alpha(t.bg_primary, 0.95))
            .border_1()
            .border_color(rgb(t.border_active))
            .rounded(px(6.0))
            .shadow_xl()
            .text_size(px(12.0))
            .text_color(rgb(t.text_primary))
            .font_weight(FontWeight::MEDIUM)
            .child(
                h_flex()
                    .gap(px(6.0))
                    .child(
                        svg()
                            .path(self.icon_path.clone())
                            .size(px(12.0))
                            .text_color(rgb(t.success)),
                    )
                    .child(self.label.clone()),
            )
    }
}

/// Which edge zone the user dropped onto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropZone {
    Top,
    Bottom,
    Left,
    Right,
    Center,
}
