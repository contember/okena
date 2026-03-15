//! Claude Code status indicator for the status bar.

use gpui::prelude::*;
use gpui::*;

pub struct ClaudeStatus {
    _focus_handle: FocusHandle,
}

impl ClaudeStatus {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self { _focus_handle: cx.focus_handle() }
    }
}

impl Render for ClaudeStatus {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
