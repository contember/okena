//! Shell selector for tab groups
//!
//! This module contains shell indicator functionality for the tab bar.
//! The actual shell switching is handled by the ShellSelectorOverlay.

use crate::terminal::shell_config::ShellType;
use crate::theme::theme;
use crate::views::components::shell_indicator_chip;
use crate::views::layout::layout_container::LayoutContainer;
use crate::workspace::state::LayoutNode;
use gpui::prelude::*;
use gpui::*;
use gpui_component::tooltip::Tooltip;

impl LayoutContainer {
    /// Get the terminal_id for the active tab in this container.
    /// Works for both Tabs containers (looks up child by index) and standalone terminals.
    pub(super) fn get_active_terminal_id(&self, active_tab: usize, cx: &Context<Self>) -> Option<String> {
        let ws = self.workspace.read(cx);
        match self.get_layout(&ws) {
            Some(LayoutNode::Tabs { children, .. }) => {
                if let Some(LayoutNode::Terminal { terminal_id, .. }) = children.get(active_tab) {
                    return terminal_id.clone();
                }
            }
            Some(LayoutNode::Terminal { terminal_id, .. }) => {
                return terminal_id.clone();
            }
            _ => {}
        }
        None
    }

    /// Get the shell type for the active tab in this container.
    /// Works for both Tabs containers and standalone terminals.
    fn get_active_shell_type(&self, active_tab: usize, cx: &Context<Self>) -> ShellType {
        let ws = self.workspace.read(cx);
        match self.get_layout(&ws) {
            Some(LayoutNode::Tabs { children, .. }) => {
                if let Some(LayoutNode::Terminal { shell_type, .. }) = children.get(active_tab) {
                    return shell_type.clone();
                }
            }
            Some(LayoutNode::Terminal { shell_type, .. }) => {
                return shell_type.clone();
            }
            _ => {}
        }
        ShellType::Default
    }

    /// Render the shell indicator button.
    /// Clicking opens the shell selector overlay.
    pub(super) fn render_shell_indicator(&self, active_tab: usize, cx: &Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let shell_type = self.get_active_shell_type(active_tab, cx);
        let shell_name = shell_type.short_display_name().to_string();
        let id_suffix = format!("tabs-{:?}", self.layout_path);
        let terminal_id = self.get_active_terminal_id(active_tab, cx);
        let project_id = self.project_id.clone();
        let request_broker = self.request_broker.clone();

        shell_indicator_chip(format!("shell-indicator-{}", id_suffix), &shell_name, &t)
            .when_some(terminal_id, |el, tid| {
                el.on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                    cx.stop_propagation();
                    request_broker.update(cx, |broker, cx| {
                        broker.push_overlay_request(crate::workspace::requests::OverlayRequest::ShellSelector {
                            project_id: project_id.clone(),
                            terminal_id: tid.clone(),
                            current_shell: shell_type.clone(),
                        }, cx);
                    });
                })
            })
            .tooltip(|_window, cx| Tooltip::new("Switch Shell").build(_window, cx))
    }
}
