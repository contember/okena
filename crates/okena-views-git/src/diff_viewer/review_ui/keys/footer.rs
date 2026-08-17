//! Footer hints — only keys that work on the current screen — spec §11.

use super::super::super::DiffViewer;
use gpui::prelude::*;
use gpui::*;
use okena_core::theme::ThemeColors;

impl DiffViewer {
    pub(crate) fn render_review_footer(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // wave-0 stub — implemented by unit K
        self.render_footer(t, cx).into_any_element()
    }
}
