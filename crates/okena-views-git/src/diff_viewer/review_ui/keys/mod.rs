//! Keyboard handling for the review workspace — spec §11.

mod footer;
mod help;

use super::super::DiffViewer;
use gpui::{Context, KeyDownEvent, Window};

impl DiffViewer {
    /// Returns true when the review handled the key and the legacy path must not run.
    pub(crate) fn handle_review_key(
        &mut self,
        _event: &KeyDownEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> bool {
        // wave-0 stub — implemented by unit K
        false
    }

    /// The Esc ladder. Returns true when it consumed the key.
    pub(crate) fn handle_review_cancel(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> bool {
        // wave-0 stub — implemented by unit K
        false
    }
}
