//! Chrome shared by every harness view, and the per-section dispatch.
//!
//! Tasks, Specs and Knowledge live in their own modules; this file holds the
//! toolbar, banners and buttons they all wear.

use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms, ui_text_sm};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};

use super::{HarnessPane, HarnessSection};

impl HarnessPane {
    /// Translate a client-side project/terminal id into the id the daemon knows.
    ///
    /// Harness panes post straight through `RemoteActionClient` rather than the
    /// `ActionDispatcher`, so they never pass through `strip_remote_ids`. The
    /// client mirror prefixes every id as `remote:<connection>:<uuid>` while the
    /// daemon only knows the bare uuid — sending the prefixed form gets a
    /// "project not found" back. Any harness view putting an id in an action
    /// must route it through here.
    pub(super) fn daemon_id(&self, id: &str) -> String {
        okena_transport::client::strip_prefix(id, self.client.connection_id())
    }

    /// A small coloured label.
    pub(super) fn chip(&self, text: String, color: u32, cx: &Context<Self>) -> AnyElement {
        div()
            .px(px(6.0))
            .py(px(1.0))
            .rounded(px(3.0))
            .bg(with_alpha(color, 0.15))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(color))
            .child(text)
            .into_any_element()
    }

    /// Show what changed in a worktree.
    pub(super) fn open_diff(&self, project_id: &str, cx: &mut App) {
        crate::views::components::project_nav::open_diff(&self.request_broker, project_id, cx);
    }

    /// A small secondary button, used across the harness views.
    pub(super) fn small_button(
        &self,
        id: &'static str,
        label: &str,
        on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
        cx: &Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        div()
            .id(id)
            .cursor_pointer()
            .flex_shrink_0()
            .px(px(10.0))
            .py(px(3.0))
            .rounded(px(4.0))
            .bg(rgb(t.bg_secondary))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .text_size(ui_text_md(cx))
            .text_color(rgb(t.text_primary))
            .child(label.to_string())
            .on_mouse_down(MouseButton::Left, on_click)
            .into_any_element()
    }

    pub(super) fn info_banner(&self, message: String, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .px(px(12.0))
            .py(px(6.0))
            .text_size(ui_text_sm(cx))
            .text_color(rgb(t.text_secondary))
            .child(message)
            .into_any_element()
    }

    pub(super) fn error_banner(&self, message: String, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .px(px(12.0))
            .py(px(6.0))
            .bg(with_alpha(t.error, 0.1))
            .text_size(ui_text_sm(cx))
            .text_color(rgb(t.error))
            .child(message)
            .into_any_element()
    }

    /// The toolbar every harness view wears.
    ///
    /// One shape for all of them — the view's name on the left, its own
    /// controls on the right — so moving between Tasks, Specs and Knowledge
    /// does not mean relearning where things are. Views differ only in what
    /// they put in `actions`.
    pub(super) fn render_toolbar(
        &self,
        actions: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        h_flex()
            .w_full()
            .flex_shrink_0()
            .items_center()
            .gap(px(8.0))
            .px(px(12.0))
            .py(px(6.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(ui_text(13.0, cx))
                    .text_color(rgb(t.text_primary))
                    .child(self.section.label()),
            )
            .child(div().flex_1().min_w_0())
            .children(actions)
            .into_any_element()
    }

    /// A square icon button for the toolbar.
    pub(super) fn toolbar_icon(
        &self,
        id: &'static str,
        icon: &'static str,
        tooltip: &'static str,
        on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
        cx: &Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        div()
            .id(id)
            .cursor_pointer()
            .flex_shrink_0()
            .size(px(24.0))
            .rounded(px(4.0))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .path(icon)
                    .size(px(13.0))
                    .text_color(rgb(t.text_secondary)),
            )
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(tooltip).build(window, cx)
            })
            .on_mouse_down(MouseButton::Left, on_click)
            .into_any_element()
    }

    /// Open the settings modal on `page`.
    pub(super) fn open_settings(&self, page: &'static str, cx: &mut App) {
        self.request_broker.update(cx, |broker, cx| {
            broker.push_overlay_request(
                okena_workspace::requests::OverlayRequest::Settings {
                    page: Some(page.to_string()),
                },
                cx,
            );
        });
    }
}

impl Render for HarnessPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        // Exhaustive on purpose: a new section must choose its view here rather
        // than silently falling through to a placeholder.
        let body = match self.section {
            HarnessSection::Tasks => self.render_tasks_view(cx),
            HarnessSection::Specs => self.render_specs_view(cx),
            HarnessSection::Knowledge => self.render_knowledge_view(cx),
        };

        // No title bar and no close button: the sidebar's HARNESS nav already
        // shows which view is open, and every view carries its own toolbar. A
        // second bar on top of that read as a window pasted over the app rather
        // than part of it. Leaving happens by selecting a project or an
        // overview, the same way every other view is left.
        v_flex()
            .size_full()
            .bg(rgb(t.bg_primary))
            .child(div().flex_1().min_h_0().child(body))
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use okena_transport::client::strip_prefix;

    const LOCAL: &str = okena_transport::client::LOCAL_DAEMON_CONNECTION_ID;

    #[test]
    fn client_ids_are_stripped_to_the_daemons_form() {
        // What the client mirror holds vs. what the daemon knows. Sending the
        // prefixed form is exactly the "project not found" bug.
        let client_id = format!("remote:{LOCAL}:8d173ae8-a7f4-4598-b488-5aee654d471e");
        assert_eq!(
            strip_prefix(&client_id, LOCAL),
            "8d173ae8-a7f4-4598-b488-5aee654d471e"
        );
    }

    #[test]
    fn an_already_bare_id_is_unchanged() {
        // Ids that came back from the daemon (e.g. via the MCP path) must
        // survive a second stripping untouched.
        let bare = "8d173ae8-a7f4-4598-b488-5aee654d471e";
        assert_eq!(strip_prefix(bare, LOCAL), bare);
    }

    #[test]
    fn a_different_connections_prefix_is_left_alone() {
        // Stripping must be scoped to this connection, not any `remote:` prefix.
        let other = "remote:some-other-daemon:abc";
        assert_eq!(strip_prefix(other, LOCAL), other);
    }
}
