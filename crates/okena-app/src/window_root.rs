//! Shared window root and component dialog layer for every desktop window.

use gpui::{
    AnyView, AppContext as _, Context, IntoElement, ParentElement, Render, Styled, Window, div,
};
use gpui_component::Root;

struct WindowContent {
    view: AnyView,
}

impl Render for WindowContent {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .child(self.view.clone())
            .children(Root::render_dialog_layer(window, cx))
    }
}

/// Keep the component root outermost for `WindowExt` on all platforms.
/// Linux retains its existing resize wrapper without enabling `window_border`.
pub fn new_root(view: impl Into<AnyView>, window: &mut Window, cx: &mut Context<Root>) -> Root {
    let view = view.into();
    #[cfg(target_os = "linux")]
    let view: AnyView = cx
        .new(|cx| crate::simple_root::SimpleRoot::new(view, window, cx))
        .into();
    let content = cx.new(|_| WindowContent { view });
    let root = Root::new(content, window, cx);
    #[cfg(target_os = "linux")]
    let root = root.bordered(false);
    root
}

#[cfg(test)]
mod tests {
    use super::new_root;
    use gpui::{AppContext as _, Context, IntoElement, Render, TestAppContext, Window, div};
    use gpui_component::{Root, WindowExt as _};
    use std::{cell::Cell, rc::Rc};

    struct TestView;

    impl Render for TestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[gpui::test]
    fn shared_root_hosts_component_alert_dialogs(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| TestView);
            new_root(view, window, cx)
        });
        let renders = Rc::new(Cell::new(0));
        let dialog_renders = renders.clone();
        cx.update(|window, cx| {
            assert!(window.root::<Root>().flatten().is_some());
            assert!(!window.has_active_dialog(cx));
            window.open_alert_dialog(cx, move |alert, _, _| {
                dialog_renders.set(dialog_renders.get() + 1);
                alert
                    .title("Confirm action")
                    .description("Shared component dialog")
                    .show_cancel(true)
            });
            assert!(window.has_active_dialog(cx));
        });
        cx.run_until_parked();
        assert!(
            renders.get() > 0,
            "the content host must render the dialog layer"
        );
        cx.update(|window, cx| {
            window.close_all_dialogs(cx);
            assert!(!window.has_active_dialog(cx));
        });
        cx.run_until_parked();
    }
}
