//! Shell selector for tab groups

use crate::ActionDispatch;
use crate::layout::layout_container::LayoutContainer;
use gpui::prelude::*;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use okena_files::theme::theme;
use okena_terminal::shell_config::ShellType;
use okena_ui::chip::shell_indicator_chip;
use okena_workspace::state::LayoutNode;

impl<D: ActionDispatch + Send + Sync> LayoutContainer<D> {
    pub(super) fn get_active_terminal_id(
        &self,
        active_tab: usize,
        cx: &Context<Self>,
    ) -> Option<String> {
        let ws = self.workspace.read(cx);
        match self.get_layout(ws) {
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

    fn get_active_shell_type(&self, active_tab: usize, cx: &Context<Self>) -> ShellType {
        let ws = self.workspace.read(cx);
        match self.get_layout(ws) {
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

    pub(super) fn render_shell_indicator(
        &self,
        active_tab: usize,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        render_shell_indicator(
            &format!("tabs-{:?}", self.layout_path),
            self.get_active_shell_type(active_tab, cx),
            self.get_active_terminal_id(active_tab, cx),
            self.project_id.clone(),
            self.request_broker.clone(),
            cx,
        )
    }
}

pub(crate) fn render_shell_indicator(
    id_suffix: &str,
    shell_type: ShellType,
    terminal_id: Option<String>,
    project_id: String,
    request_broker: Entity<okena_workspace::request_broker::RequestBroker>,
    cx: &App,
) -> Stateful<Div> {
    let t = theme(cx);
    let shell_name = shell_type.short_display_name().to_string();
    shell_indicator_chip(format!("shell-indicator-{}", id_suffix), &shell_name, &t)
        .flex_shrink_0()
        .whitespace_nowrap()
        .overflow_hidden()
        .when_some(terminal_id, |el, tid| {
            el.on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                cx.stop_propagation();
                request_broker.update(cx, |broker, cx| {
                    broker.push_overlay_request(
                        okena_workspace::requests::OverlayRequest::Project(
                            okena_workspace::requests::ProjectOverlay {
                                project_id: project_id.clone(),
                                kind:
                                    okena_workspace::requests::ProjectOverlayKind::ShellSelector {
                                        terminal_id: tid.clone(),
                                        current_shell: shell_type.clone(),
                                    },
                            },
                        ),
                        cx,
                    );
                });
            })
        })
        .tooltip(|_window, cx| Tooltip::new("Switch Shell").build(_window, cx))
}
