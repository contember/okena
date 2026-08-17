//! Attention mode: the ordered list — spec §7.

use super::super::super::DiffViewer;
use gpui::prelude::*;
use gpui::*;
use okena_core::theme::ThemeColors;

impl DiffViewer {
    pub(crate) fn render_attention_list(
        &mut self,
        _t: &ThemeColors,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        // wave-0 stub — implemented by unit N
        div().child("attention").into_any_element()
    }
}
