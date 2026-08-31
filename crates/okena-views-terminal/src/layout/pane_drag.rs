//! Pane drag-and-drop types for terminal rearrangement.

use gpui::*;
use gpui_component::h_flex;
use okena_files::theme::theme;
use okena_ui::theme::with_alpha;
use okena_ui::tokens::ui_text_md;

/// Drag payload emitted from a terminal header.
#[derive(Clone)]
pub struct PaneDrag {
    pub project_id: String,
    pub layout_path: Vec<usize>,
    pub terminal_id: String,
    pub terminal_name: String,
}

/// Per-window source selected by the explicit "Move Terminal" command.
#[derive(Default)]
pub struct PaneMoveState {
    source: Option<PaneDrag>,
}

impl PaneMoveState {
    pub fn source(&self) -> Option<&PaneDrag> {
        self.source.as_ref()
    }

    pub fn begin(&mut self, source: PaneDrag, cx: &mut Context<Self>) {
        self.source = Some(source);
        cx.notify();
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        self.source = None;
        cx.notify();
    }
}

pub(super) fn is_move_target(source: &PaneDrag, target_terminal_id: Option<&str>) -> bool {
    target_terminal_id.is_some_and(|target| target != source.terminal_id)
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

#[cfg(test)]
mod tests {
    use super::{PaneDrag, is_move_target};

    fn source() -> PaneDrag {
        PaneDrag {
            project_id: "project".to_string(),
            layout_path: vec![0],
            terminal_id: "source".to_string(),
            terminal_name: "Source".to_string(),
        }
    }

    #[test]
    fn explicit_move_requires_a_different_terminal() {
        let source = source();

        assert!(!is_move_target(&source, None));
        assert!(!is_move_target(&source, Some("source")));
        assert!(is_move_target(&source, Some("target")));
    }
}
