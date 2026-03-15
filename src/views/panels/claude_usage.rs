//! Claude Code usage indicator for the status bar.

use gpui::prelude::*;
use gpui::*;

pub struct ClaudeUsage {
    _focus_handle: FocusHandle,
}

impl ClaudeUsage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self { _focus_handle: cx.focus_handle() }
    }
}

impl Render for ClaudeUsage {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
