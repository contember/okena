//! Hook terminal list rendering for the sidebar

use crate::theme::theme;
use crate::workspace::state::HookTerminalStatus;
use gpui::*;
use gpui::prelude::*;

use super::{Sidebar, SidebarProjectInfo, SidebarHookInfo, GroupKind};
use super::item_widgets::sidebar_group_header;

impl Sidebar {
    /// Render the "Hooks" group header with collapse chevron.
    pub(super) fn render_hooks_group_header(
        &self,
        project: &SidebarProjectInfo,
        is_collapsed: bool,
        is_cursor: bool,
        left_padding: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let project_id = project.id.clone();

        sidebar_group_header(
            ElementId::Name(format!("hook-group-{}", project_id).into()),
            GroupKind::Hooks.label(),
            project.hook_terminals.len(),
            is_collapsed,
            is_cursor,
            left_padding,
            &t,
        )
        .on_click(cx.listener(move |this, _, _window, cx| {
            this.toggle_group(&project_id, GroupKind::Hooks);
            cx.notify();
        }))
    }

    /// Render a single hook terminal item row with status icon, label, and click to focus.
    pub(super) fn render_hook_item(
        &self,
        project: &SidebarProjectInfo,
        hook: &SidebarHookInfo,
        left_padding: f32,
        is_cursor: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let project_id = project.id.clone();
        let terminal_id = hook.terminal_id.clone();

        let (status_color, status_icon) = match &hook.status {
            HookTerminalStatus::Running => (t.term_yellow, "icons/terminal.svg"),
            HookTerminalStatus::Succeeded => (t.success, "icons/check.svg"),
            HookTerminalStatus::Failed { .. } => (t.error, "icons/close.svg"),
        };

        div()
            .id(ElementId::Name(format!("hook-item-{}-{}", project_id, terminal_id).into()))
            .group("hook-item")
            .h(px(22.0))
            .pl(px(left_padding))
            .pr(px(8.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .when(is_cursor, |d| d.border_l_2().border_color(rgb(t.border_active)))
            .on_click(cx.listener({
                let project_id = project_id.clone();
                move |this, _, _window, cx| {
                    this.cursor_index = None;
                    this.workspace.update(cx, |ws, cx| {
                        ws.set_focused_project(Some(project_id.clone()), cx);
                    });
                }
            }))
            .child(
                // Status icon
                div()
                    .flex_shrink_0()
                    .w(px(14.0))
                    .h(px(14.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        svg()
                            .path(status_icon)
                            .size(px(12.0))
                            .text_color(rgb(status_color)),
                    ),
            )
            .child(
                // Hook label
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_size(px(12.0))
                    .text_color(rgb(t.text_primary))
                    .text_ellipsis()
                    .child(hook.label.clone()),
            )
            .child(
                // Dismiss button on hover (for failed hooks)
                div()
                    .flex()
                    .flex_shrink_0()
                    .opacity(0.0)
                    .group_hover("hook-item", |s| s.opacity(1.0))
                    .child(
                        div()
                            .id(ElementId::Name(format!("hook-dismiss-{}", terminal_id).into()))
                            .cursor_pointer()
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .child(
                                svg()
                                    .path("icons/close.svg")
                                    .size(px(10.0))
                                    .text_color(rgb(t.text_muted)),
                            )
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener({
                                let terminal_id = terminal_id.clone();
                                move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    this.workspace.update(cx, |ws, cx| {
                                        ws.remove_hook_terminal(&terminal_id, cx);
                                    });
                                    let terminals = this.terminals.clone();
                                    terminals.lock().remove(&terminal_id);
                                }
                            })),
                    ),
            )
    }
}
