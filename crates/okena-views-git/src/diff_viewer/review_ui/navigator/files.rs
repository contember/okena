//! Files mode: the directory tree — spec §7.

use super::super::super::DiffViewer;
use gpui::prelude::*;
use gpui::*;
use okena_core::theme::ThemeColors;

impl DiffViewer {
    pub(crate) fn render_files_tree(
        &mut self,
        _t: &ThemeColors,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        // wave-0 stub — implemented by unit N
        div().child("files").into_any_element()
    }
}
