//! Analysis status pill and its details popover — spec §10.

use super::super::DiffViewer;
use gpui::prelude::*;
use gpui::*;
use okena_core::theme::ThemeColors;
use okena_ui::tokens::ui_text_ms;

impl DiffViewer {
    pub(crate) fn render_status_pill(&self, t: &ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        // wave-0 stub — implemented by unit S
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child("status")
            .into_any_element()
    }

    pub(crate) fn render_status_popover(
        &self,
        _t: &ThemeColors,
        _cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // wave-0 stub — implemented by unit S
        None
    }
}
