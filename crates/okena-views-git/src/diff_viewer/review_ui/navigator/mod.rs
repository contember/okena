//! Navigator column: Files tree and Attention list — spec §7.

mod attention;
mod files;
mod roles_menu;

use super::super::DiffViewer;
use super::state::{NavRowId, NavigatorMode};
use gpui::prelude::*;
use gpui::*;
use okena_core::theme::ThemeColors;
use okena_ui::tokens::ui_text_ms;

impl DiffViewer {
    pub(crate) fn render_navigator(
        &mut self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // wave-0 stub — implemented by unit N
        let rows = self.navigator_row_ids().len();
        let body = match self.review_ui.navigator {
            NavigatorMode::Files => self.render_files_tree(t, cx),
            NavigatorMode::Attention => self.render_attention_list(t, cx),
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .p(px(12.0))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child(format!("navigator \u{00B7} {rows} rows"))
            .child(body)
            .into_any_element()
    }

    /// Visible rows of the current navigator mode, in display order.
    pub(crate) fn navigator_row_ids(&self) -> Vec<NavRowId> {
        // wave-0 stub — implemented by unit N
        Vec::new()
    }
}
