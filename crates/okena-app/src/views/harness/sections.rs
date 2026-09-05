//! Shared chrome plus the still-stubbed views.
//!
//! Tasks, Projects, Agents and Specs are implemented in their own modules;
//! Knowledge remains a placeholder that states what will live there rather than
//! inventing content that looks real.

use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_sm};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};

use super::{HarnessPane, HarnessPaneEvent, HarnessSection};

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

    /// Placeholder body for a view that isn't built yet.
    fn render_stub(&self, section: HarnessSection, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        let lines: Vec<&str> = match section {
            HarnessSection::Knowledge => vec![
                "Collections of skills, technical designs and feature docs.",
                "Git-backed, with PR / branch support for changes.",
            ],
            // These are real views; they never reach here.
            HarnessSection::Tasks
            | HarnessSection::Projects
            | HarnessSection::Agents
            | HarnessSection::Specs => vec![],
        };

        v_flex()
            .p(px(16.0))
            .gap(px(6.0))
            .child(
                div()
                    .text_size(ui_text(13.0, cx))
                    .text_color(rgb(t.text_muted))
                    .child(section.blurb()),
            )
            .children(lines.into_iter().map(|l| {
                div()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(format!("• {l}"))
                    .into_any_element()
            }))
            .into_any_element()
    }

    /// Header: the view's name and the way back to the terminal workspace.
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx);
        let section = self.section;
        h_flex()
            .h(crate::ui::tokens::HEADER_HEIGHT)
            .w_full()
            .items_center()
            .justify_between()
            .px(px(12.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_header))
            .child(
                v_flex().child(
                    div()
                        .text_size(ui_text(13.0, cx))
                        .text_color(rgb(t.text_primary))
                        .child(section.label()),
                ),
            )
            .child(
                div()
                    .id(SharedString::from(format!(
                        "harness-close-{}",
                        section.slug()
                    )))
                    .cursor_pointer()
                    .px(px(6.0))
                    .rounded(px(3.0))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child("×")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |_this, _, _window, cx| {
                            cx.emit(HarnessPaneEvent::Close(section));
                        }),
                    ),
            )
    }
}

impl Render for HarnessPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let body = match self.section {
            HarnessSection::Tasks => self.render_tasks_view(cx),
            HarnessSection::Projects => self.render_projects_view(cx),
            HarnessSection::Agents => self.render_agents_view(cx),
            HarnessSection::Specs => self.render_specs_view(cx),
            other => self.render_stub(other, cx),
        };

        v_flex()
            .size_full()
            .bg(rgb(t.bg_primary))
            .child(self.render_header(cx))
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
