//! Simple root wrapper without window_border (fixes Linux/Wayland maximize issue)

use gpui::{
    AnyView, Context, InteractiveElement, IntoElement, ParentElement, Render, Styled, Window,
    div,
};

/// Simple root view wrapper that doesn't use window_border
/// This avoids the buggy resize edge detection in gpui_component's window_border
pub struct SimpleRoot {
    view: AnyView,
}

impl SimpleRoot {
    pub fn new(view: impl Into<AnyView>, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {
            view: view.into(),
        }
    }
}

impl Render for SimpleRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("simple-root")
            .size_full()
            .child(self.view.clone())
    }
}
