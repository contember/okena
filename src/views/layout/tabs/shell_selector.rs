//! Shell selector for tab groups
//!
//! This module contains shell switching functionality for the tab bar:
//! - Shell indicator button showing current shell
//! - Shell dropdown modal for switching shells

use crate::terminal::shell_config::ShellType;
use crate::terminal::terminal::{Terminal, TerminalSize};
use crate::theme::theme;
use crate::views::layout::layout_container::LayoutContainer;
use crate::workspace::state::LayoutNode;
use gpui::*;
use gpui::prelude::*;
use gpui_component::tooltip::Tooltip;
use std::sync::Arc;

impl LayoutContainer {
    /// Get the terminal_id for the active tab in this Tabs container.
    pub(super) fn get_active_terminal_id(&self, active_tab: usize, cx: &Context<Self>) -> Option<String> {
        let ws = self.workspace.read(cx);
        if let Some(LayoutNode::Tabs { children, .. }) = self.get_layout(&ws) {
            if let Some(LayoutNode::Terminal { terminal_id, .. }) = children.get(active_tab) {
                return terminal_id.clone();
            }
        }
        None
    }

    /// Get the shell type for the active tab in this Tabs container.
    fn get_active_shell_type(&self, active_tab: usize, cx: &Context<Self>) -> ShellType {
        let ws = self.workspace.read(cx);
        if let Some(LayoutNode::Tabs { children, .. }) = self.get_layout(&ws) {
            if let Some(LayoutNode::Terminal { shell_type, .. }) = children.get(active_tab) {
                return shell_type.clone();
            }
        }
        ShellType::Default
    }

    /// Get the display name for a shell type.
    fn get_shell_display_name(&self, shell_type: &ShellType) -> String {
        shell_type.display_name()
    }

    /// Toggle the shell dropdown for tab groups.
    fn toggle_shell_dropdown(&mut self, cx: &mut Context<Self>) {
        self.shell_dropdown_open = !self.shell_dropdown_open;
        cx.notify();
    }

    /// Switch the shell for the active tab.
    fn switch_shell(&mut self, active_tab: usize, shell_type: ShellType, cx: &mut Context<Self>) {
        self.shell_dropdown_open = false;

        // Get the terminal_id for the active tab
        let terminal_id = self.get_active_terminal_id(active_tab, cx);

        // Get the current shell type
        let current_shell = self.get_active_shell_type(active_tab, cx);
        if current_shell == shell_type {
            cx.notify();
            return;
        }

        // Kill the old terminal if it exists
        if let Some(ref tid) = terminal_id {
            self.pty_manager.kill(tid);
        }

        // Update the shell type in workspace state
        let mut full_path = self.layout_path.clone();
        full_path.push(active_tab);
        let project_id = self.project_id.clone();
        let shell_for_save = shell_type.clone();
        self.workspace.update(cx, |ws, cx| {
            ws.set_terminal_shell(&project_id, &full_path, shell_for_save, cx);
        });

        // Create a new terminal with the new shell
        match self.pty_manager.create_terminal_with_shell(&self.project_path, Some(&shell_type)) {
            Ok(new_terminal_id) => {
                // Update the terminal_id in workspace state
                let new_id = new_terminal_id.clone();
                self.workspace.update(cx, |ws, cx| {
                    ws.set_terminal_id(&project_id, &full_path, new_id.clone(), cx);
                });

                // Create Terminal wrapper and register it
                let size = TerminalSize::default();
                let terminal = Arc::new(Terminal::new(new_terminal_id.clone(), size, self.pty_manager.clone()));
                self.terminals.lock().insert(new_terminal_id.clone(), terminal);

                log::info!("Switched tab {} to shell {:?}, new terminal_id: {}", active_tab, shell_type, new_terminal_id);
            }
            Err(e) => {
                log::error!("Failed to create terminal with new shell: {}", e);
            }
        }

        cx.notify();
    }

    /// Render the shell indicator button for tab groups.
    pub(super) fn render_shell_indicator(&mut self, active_tab: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let shell_type = self.get_active_shell_type(active_tab, cx);
        let shell_name = self.get_shell_display_name(&shell_type);
        let id_suffix = format!("tabs-{:?}", self.layout_path);

        div()
            .id(format!("shell-indicator-{}", id_suffix))
            .cursor_pointer()
            .px(px(6.0))
            .h(px(18.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .bg(rgb(t.bg_secondary))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                cx.stop_propagation();
                this.toggle_shell_dropdown(cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(rgb(t.text_secondary))
                            .child(shell_name)
                    )
                    .child(
                        svg()
                            .path("icons/chevron-down.svg")
                            .size(px(10.0))
                            .text_color(rgb(t.text_secondary))
                    )
            )
            .tooltip(|_window, cx| Tooltip::new("Switch Shell").build(_window, cx))
    }

    /// Render the shell dropdown modal for tab groups.
    pub(super) fn render_shell_dropdown(&mut self, active_tab: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        if !self.shell_dropdown_open {
            return div().into_any_element();
        }

        let shells = self.available_shells.clone();
        let current_shell = self.get_active_shell_type(active_tab, cx);

        // Full-screen backdrop + centered modal
        div()
            .id("shell-modal-backdrop-tabs")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000088))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                this.shell_dropdown_open = false;
                cx.notify();
            }))
            .child(
                div()
                    .id("shell-modal-tabs")
                    .w(px(200.0))
                    .bg(rgb(t.bg_secondary))
                    .border_1()
                    .border_color(rgb(t.border))
                    .rounded(px(8.0))
                    .shadow_lg()
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        // Modal header
                        div()
                            .px(px(12.0))
                            .py(px(8.0))
                            .border_b_1()
                            .border_color(rgb(t.border))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(t.text_primary))
                                    .child("Switch Shell")
                            )
                    )
                    .child(
                        // Shell list
                        div()
                            .py(px(2.0))
                            .children(shells.into_iter().filter(|s| s.available).map(|shell| {
                                let shell_type = shell.shell_type.clone();
                                let is_current = shell_type == current_shell;
                                let name = shell.name.clone();

                                div()
                                    .id(format!("shell-option-tabs-{}", name.replace(" ", "-").to_lowercase()))
                                    .w_full()
                                    .px(px(12.0))
                                    .py(px(6.0))
                                    .cursor_pointer()
                                    .bg(if is_current { rgb(t.bg_hover) } else { rgb(t.bg_secondary) })
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _window, cx| {
                                        this.switch_shell(active_tab, shell_type.clone(), cx);
                                    }))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.0))
                                            .child(
                                                div()
                                                    .text_size(px(12.0))
                                                    .text_color(rgb(t.text_primary))
                                                    .child(name)
                                            )
                                            .when(is_current, |d| {
                                                d.child(
                                                    svg()
                                                        .path("icons/check.svg")
                                                        .size(px(12.0))
                                                        .text_color(rgb(t.success))
                                                )
                                            })
                                    )
                            }))
                    )
            )
            .into_any_element()
    }
}
