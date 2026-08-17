//! Shortcut help overlay (`?`) — spec §11.

use super::super::super::DiffViewer;
use gpui::*;
use okena_core::theme::ThemeColors;

impl DiffViewer {
    pub(crate) fn render_help_overlay(
        &self,
        _t: &ThemeColors,
        _cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // wave-0 stub — implemented by unit K
        None
    }
}
