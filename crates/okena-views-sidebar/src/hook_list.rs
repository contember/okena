//! Hook terminal list rendering for the sidebar

use okena_core::api::ActionRequest;
use okena_ui::theme::theme;
use okena_ui::tokens::ui_text_md;
use okena_workspace::state::HookTerminalStatus;
use gpui::*;
use gpui::prelude::*;
use okena_ui::icon_button::icon_button;

use crate::sidebar::{Sidebar, SidebarProjectInfo, SidebarHookInfo, GroupKind};
use crate::item_widgets::sidebar_group_header;

impl Sidebar {
    /// Render the "Hooks" group header with collapse chevron.
    pub fn render_hooks_group_header(
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
            cx,
        )
        .on_click(cx.listener(move |this, _, _window, cx| {
            this.toggle_group(&project_id, GroupKind::Hooks);
            cx.notify();
        }))
    }

    /// Render a single hook terminal item row with status icon, label, and click to focus.
    pub fn render_hook_item(
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
        let is_running = matches!(&hook.status, HookTerminalStatus::Running);

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
                let terminal_id = terminal_id.clone();
                move |this, _, _window, cx| {
                    this.cursor_index = None;
                    this.request_broker.update(cx, |broker, cx| {
                        broker.push_overlay_request(
                            okena_workspace::requests::OverlayRequest::Project(okena_workspace::requests::ProjectOverlay {
                                project_id: project_id.clone(),
                                kind: okena_workspace::requests::ProjectOverlayKind::ShowHookTerminal {
                                    terminal_id: terminal_id.clone(),
                                },
                            }),
                            cx,
                        );
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
                // Hook label (shows hook_type as the visible text)
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(t.text_primary))
                    .text_ellipsis()
                    .child(hook.label.clone()),
            )
            .child(
                // Hover action buttons
                div()
                    .flex()
                    .flex_shrink_0()
                    .gap(px(2.0))
                    .opacity(0.0)
                    .group_hover("hook-item", |s| s.opacity(1.0))
                    // Rerun button (only when not running)
                    .when(!is_running, {
                        let terminal_id = terminal_id.clone();
                        let project_id = project_id.clone();
                        |el| el.child(
                            icon_button(
                                ElementId::Name(format!("hook-rerun-{}", terminal_id).into()),
                                "icons/refresh.svg",
                                &t,
                            )
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    this.rerun_hook_terminal(&project_id, &terminal_id, cx);
                                })),
                        )
                    })
                    // Dismiss button
                    .child(
                        icon_button(
                            ElementId::Name(format!("hook-dismiss-{}", terminal_id).into()),
                            "icons/close.svg",
                            &t,
                        )
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener({
                                let terminal_id = terminal_id.clone();
                                move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    if let Some(monitor) = cx.try_global::<okena_workspace::hook_monitor::HookMonitor>() {
                                        monitor.notify_exit(&terminal_id, None);
                                    }
                                    this.workspace.update(cx, |ws, cx| {
                                        ws.cancel_pending_worktree_close(&terminal_id);
                                        ws.remove_hook_terminal(&terminal_id, cx);
                                    });
                                    let terminals = this.terminals.clone();
                                    terminals.lock().remove(&terminal_id);
                                }
                            })),
                    ),
            )
    }

    /// Rerun a hook terminal: dispatch to the daemon, which kills the old PTY,
    /// spawns a fresh shell at the hook's cwd, and re-types the stored command.
    /// The daemon owns the hook terminal's command + cwd, so the action carries
    /// only the ids.
    pub fn rerun_hook_terminal(&self, project_id: &str, terminal_id: &str, cx: &mut Context<Self>) {
        self.dispatch_action_for_project(
            project_id,
            ActionRequest::RerunHook {
                project_id: project_id.to_string(),
                terminal_id: terminal_id.to_string(),
            },
            cx,
        );
    }
}
