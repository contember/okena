//! Pane drag-and-drop types for terminal rearrangement.

use okena_files::theme::theme;
use okena_ui::theme::with_alpha;
use okena_ui::tokens::ui_text_md;
use gpui::*;
use gpui_component::h_flex;

/// Drag payload emitted from a terminal header.
#[derive(Clone)]
pub struct PaneDrag {
    pub project_id: String,
    pub layout_path: Vec<usize>,
    pub terminal_id: String,
    pub terminal_name: String,
}

/// Ghost view rendered while dragging a terminal pane.
pub struct PaneDragView {
    label: String,
}

impl PaneDragView {
    pub fn new(label: String) -> Self {
        Self { label }
    }
}

impl Render for PaneDragView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        div()
            .px(px(12.0))
            .py(px(6.0))
            .bg(with_alpha(t.bg_primary, 0.95))
            .border_1()
            .border_color(rgb(t.border_active))
            .rounded(px(6.0))
            .shadow_xl()
            .text_size(ui_text_md(cx))
            .text_color(rgb(t.text_primary))
            .font_weight(FontWeight::MEDIUM)
            .child(
                h_flex()
                    .gap(px(6.0))
                    .child(
                        svg()
                            .path("icons/terminal.svg")
                            .size(px(12.0))
                            .text_color(rgb(t.success)),
                    )
                    .child(self.label.clone()),
            )
    }
}

/// Re-export DropZone from workspace state.
pub use okena_workspace::state::DropZone;
