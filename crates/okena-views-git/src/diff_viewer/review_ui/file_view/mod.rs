//! File view: header, symbol bar, details, outline — spec §9.

use super::super::DiffViewer;
use super::super::review::ReviewFileKey;
use gpui::prelude::*;
use gpui::*;
use okena_core::theme::ThemeColors;
use okena_ui::tokens::ui_text_ms;

impl DiffViewer {
    pub(crate) fn render_file_header(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // wave-0 stub — implemented by unit F
        let path = self
            .smart_review
            .selected_file
            .as_ref()
            .map_or_else(|| "No file selected".to_string(), ReviewFileKey::display);
        div()
            .h(px(40.0))
            .px(px(16.0))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(t.border))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_secondary))
            .child(path)
            .into_any_element()
    }

    pub(crate) fn render_symbol_bar(
        &mut self,
        _t: &ThemeColors,
        _cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // wave-0 stub — implemented by unit F
        None
    }

    pub(crate) fn render_outline_popover(
        &self,
        _t: &ThemeColors,
        _cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // wave-0 stub — implemented by unit F
        None
    }
}
