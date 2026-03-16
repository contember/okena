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
                    this.workspace.update(cx, |ws, cx| {
                        ws.focus_terminal_by_id(&project_id, &terminal_id, cx);
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
                    .text_size(px(12.0))
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
                        let command = hook.command.clone();
                        let cwd = hook.cwd.clone();
                        |el| el.child(
                            div()
                                .id(ElementId::Name(format!("hook-rerun-{}", terminal_id).into()))
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
                                        .path("icons/refresh.svg")
                                        .size(px(10.0))
                                        .text_color(rgb(t.text_muted)),
                                )
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    this.rerun_hook_terminal(
                                        &project_id,
                                        &terminal_id,
                                        &command,
                                        &cwd,
                                        cx,
                                    );
                                })),
                        )
                    })
                    // Dismiss button
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
                                    if let Some(monitor) = cx.try_global::<crate::workspace::hook_monitor::HookMonitor>() {
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

    /// Rerun a hook by killing the old PTY and creating a new one with the same command.
    pub(super) fn rerun_hook_terminal(
        &self,
        project_id: &str,
        terminal_id: &str,
        command: &str,
        cwd: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(runner) = cx.try_global::<crate::workspace::hooks::HookRunner>().cloned() else {
            log::warn!("Cannot rerun hook: no HookRunner available");
            return;
        };

        // Kill old PTY
        runner.backend.kill(terminal_id);

        // Create new PTY with a live shell, then type the command into it
        match runner.backend.create_terminal(cwd, None) {
            Ok(new_terminal_id) => {
                let transport = runner.backend.transport();
                let terminal = std::sync::Arc::new(crate::terminal::terminal::Terminal::new(
                    new_terminal_id.clone(),
                    crate::terminal::terminal::TerminalSize::default(),
                    transport.clone(),
                    cwd.to_string(),
                ));

                // Replace in TerminalsRegistry: remove old, insert new
                let terminals = self.terminals.clone();
                terminals.lock().remove(terminal_id);
                terminals.lock().insert(new_terminal_id.clone(), terminal);

                // Update workspace: swap terminal ID in hook_terminals and layout
                self.workspace.update(cx, |ws, cx| {
                    ws.swap_hook_terminal_id(project_id, terminal_id, &new_terminal_id, cx);
                });

                // Type the command into the new shell
                let cmd_with_newline = format!("{}\n", command);
                transport.send_input(&new_terminal_id, cmd_with_newline.as_bytes());

                log::info!("Hook rerun: replaced {} with {}", terminal_id, new_terminal_id);
            }
            Err(e) => {
                log::error!("Failed to rerun hook terminal: {}", e);
            }
        }
    }
}
