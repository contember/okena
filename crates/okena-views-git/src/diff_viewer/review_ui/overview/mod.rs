//! Overview: change at a glance, the facts, and "Start here" — spec §8.

use super::super::DiffViewer;
use gpui::prelude::*;
use gpui::*;
use okena_core::theme::ThemeColors;
use okena_ui::tokens::ui_text_ms;

impl DiffViewer {
    pub(crate) fn render_overview(&mut self, t: &ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        // wave-0 stub — implemented by unit O
        div()
            .flex_1()
            .min_h_0()
            .p(px(16.0))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child("overview")
            .into_any_element()
    }
}
