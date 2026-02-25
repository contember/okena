//! Service list rendering for the sidebar

use crate::process;
use crate::services::manager::ServiceStatus;
use crate::theme::theme;
use gpui::*;
use gpui::prelude::*;
use gpui_component::tooltip::Tooltip;
use okena_core::api::ActionRequest;

use super::{Sidebar, SidebarProjectInfo, SidebarServiceInfo};

impl Sidebar {
    /// Dispatch a service action for a project.
    /// Routes through ActionDispatcher (works for both local and remote).
    /// Falls back to ServiceManager for local projects if no dispatcher is available.
    fn dispatch_service_action(&self, project_id: &str, action: ActionRequest, cx: &mut Context<Self>) {
        if let Some(dispatcher) = self.dispatcher_for_project(project_id, cx) {
            dispatcher.dispatch(action, cx);
        }
    }

    /// Render the "Services" separator header with Start All / Stop All / Reload buttons.
    pub(super) fn render_services_header(
        &self,
        project: &SidebarProjectInfo,
        left_padding: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let project_id = project.id.clone();

        div()
            .h(px(20.0))
            .pl(px(left_padding))
            .pr(px(8.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .group("services-header")
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(rgb(t.text_muted))
                    .child("Services"),
            )
            .child(
                // Separator line
                div()
                    .flex_1()
                    .h(px(1.0))
                    .bg(rgb(t.border)),
            )
            .child(
                // Action buttons - visible on hover
                div()
                    .flex()
                    .flex_shrink_0()
                    .gap(px(2.0))
                    .opacity(0.0)
                    .group_hover("services-header", |s| s.opacity(1.0))
                    .child(
                        // Start All button
                        div()
                            .id(ElementId::Name(format!("svc-start-all-{}", project_id).into()))
                            .cursor_pointer()
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .text_size(px(10.0))
                            .text_color(rgb(t.text_secondary))
                            .child("▶")
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener({
                                let project_id = project_id.clone();
                                move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    this.dispatch_service_action(&project_id, ActionRequest::StartAllServices {
                                        project_id: project_id.clone(),
                                    }, cx);
                                }
                            }))
                            .tooltip(|_window, cx| Tooltip::new("Start All").build(_window, cx)),
                    )
                    .child(
                        // Stop All button
                        div()
                            .id(ElementId::Name(format!("svc-stop-all-{}", project_id).into()))
                            .cursor_pointer()
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .text_size(px(10.0))
                            .text_color(rgb(t.text_secondary))
                            .child("■")
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener({
                                let project_id = project_id.clone();
                                move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    this.dispatch_service_action(&project_id, ActionRequest::StopAllServices {
                                        project_id: project_id.clone(),
                                    }, cx);
                                }
                            }))
                            .tooltip(|_window, cx| Tooltip::new("Stop All").build(_window, cx)),
                    )
                    .child(
                        // Reload button
                        div()
                            .id(ElementId::Name(format!("svc-reload-{}", project_id).into()))
                            .cursor_pointer()
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .text_size(px(10.0))
                            .text_color(rgb(t.text_secondary))
                            .child("⟳")
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener({
                                let project_id = project_id.clone();
                                move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    this.dispatch_service_action(&project_id, ActionRequest::ReloadServices {
                                        project_id: project_id.clone(),
                                    }, cx);
                                }
                            }))
                            .tooltip(|_window, cx| Tooltip::new("Reload Services").build(_window, cx)),
                    ),
            )
    }

    /// Render a single service item row with status dot, name, and action buttons.
    pub(super) fn render_service_item(
        &self,
        project: &SidebarProjectInfo,
        service: &SidebarServiceInfo,
        left_padding: f32,
        is_cursor: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let project_id = project.id.clone();
        let service_name = service.name.clone();
        let status = service.status.clone();

        let is_running = matches!(status, ServiceStatus::Running);
        let is_starting = matches!(status, ServiceStatus::Starting | ServiceStatus::Restarting);

        let status_color = match &status {
            ServiceStatus::Running => t.term_green,
            ServiceStatus::Crashed { .. } => t.term_red,
            ServiceStatus::Stopped => t.text_muted,
            ServiceStatus::Starting | ServiceStatus::Restarting => t.term_yellow,
        };

        div()
            .id(ElementId::Name(format!("svc-item-{}-{}", project_id, service_name).into()))
            .group("service-item")
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
                let service_name = service_name.clone();
                move |this, _, _window, cx| {
                    this.cursor_index = None;
                    // Focus the project when clicking a service
                    this.workspace.update(cx, |ws, cx| {
                        ws.set_focused_project(Some(project_id.clone()), cx);
                    });
                    // Open/toggle the service log panel
                    this.request_broker.update(cx, |broker, cx| {
                        broker.push_overlay_request(
                            crate::workspace::requests::OverlayRequest::ShowServiceLog {
                                project_id: project_id.clone(),
                                service_name: service_name.clone(),
                            },
                            cx,
                        );
                    });
                }
            }))
            .child({
                // Status indicator (rounded square to distinguish from project's circle)
                let dot = div()
                    .id(ElementId::Name(format!("svc-dot-{}-{}", project_id, service_name).into()))
                    .flex_shrink_0()
                    .w(px(6.0))
                    .h(px(6.0))
                    .rounded(px(1.5))
                    .bg(rgb(status_color));
                if let ServiceStatus::Crashed { exit_code } = &status {
                    let tip = match exit_code {
                        Some(code) => format!("Exited with code {}", code),
                        None => "Crashed".to_string(),
                    };
                    dot.tooltip(move |_window, cx| Tooltip::new(tip.clone()).build(_window, cx))
                } else {
                    dot
                }
            })
            .child(
                // Service name
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_size(px(12.0))
                    .text_color(rgb(t.text_primary))
                    .text_ellipsis()
                    .child(service_name.clone()),
            )
            .children(
                // Port badges — host baked into SidebarServiceInfo
                service.ports.iter().map({
                    let project_id = project_id.clone();
                    let service_name = service_name.clone();
                    let port_host = service.port_host.clone();
                    move |port| {
                        let port = *port;
                        let url = format!("http://{}:{}", port_host, port);
                        let tooltip_url = url.clone();
                        div()
                            .id(ElementId::Name(format!("svc-port-{}-{}-{}", project_id, service_name, port).into()))
                            .flex_shrink_0()
                            .cursor_pointer()
                            .px(px(4.0))
                            .h(px(16.0))
                            .flex()
                            .items_center()
                            .rounded(px(3.0))
                            .bg(rgb(t.bg_secondary))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .text_size(px(10.0))
                            .text_color(rgb(t.text_muted))
                            .child(format!(":{}", port))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(move |_, _, _cx| {
                                process::open_url(&url);
                            })
                            .tooltip(move |_window, cx| {
                                Tooltip::new(tooltip_url.clone()).build(_window, cx)
                            })
                    }
                })
            )
            .child(
                // Action buttons - show on hover
                div()
                    .flex()
                    .flex_shrink_0()
                    .gap(px(2.0))
                    .opacity(0.0)
                    .group_hover("service-item", |s| s.opacity(1.0))
                    .when(!is_running && !is_starting, |d| {
                        // Play button for stopped/crashed services
                        d.child(
                            div()
                                .id(ElementId::Name(format!("svc-play-{}-{}", project_id, service_name).into()))
                                .cursor_pointer()
                                .w(px(18.0))
                                .h(px(18.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(3.0))
                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                .text_size(px(10.0))
                                .text_color(rgb(t.term_green))
                                .child("▶")
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener({
                                    let project_id = project_id.clone();
                                    let service_name = service_name.clone();
                                    move |this, _, _window, cx| {
                                        cx.stop_propagation();
                                        this.dispatch_service_action(&project_id, ActionRequest::StartService {
                                            project_id: project_id.clone(),
                                            service_name: service_name.clone(),
                                        }, cx);
                                    }
                                }))
                                .tooltip(|_window, cx| Tooltip::new("Start").build(_window, cx)),
                        )
                    })
                    .when(is_running, |d| {
                        d
                            .child(
                                // Restart button
                                div()
                                    .id(ElementId::Name(format!("svc-restart-{}-{}", project_id, service_name).into()))
                                    .cursor_pointer()
                                    .w(px(18.0))
                                    .h(px(18.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(3.0))
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .text_size(px(10.0))
                                    .text_color(rgb(t.text_secondary))
                                    .child("⟳")
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                    .on_click(cx.listener({
                                        let project_id = project_id.clone();
                                        let service_name = service_name.clone();
                                        move |this, _, _window, cx| {
                                            cx.stop_propagation();
                                            this.dispatch_service_action(&project_id, ActionRequest::RestartService {
                                                project_id: project_id.clone(),
                                                service_name: service_name.clone(),
                                            }, cx);
                                        }
                                    }))
                                    .tooltip(|_window, cx| Tooltip::new("Restart").build(_window, cx)),
                            )
                            .child(
                                // Stop button
                                div()
                                    .id(ElementId::Name(format!("svc-stop-{}-{}", project_id, service_name).into()))
                                    .cursor_pointer()
                                    .w(px(18.0))
                                    .h(px(18.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(3.0))
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .text_size(px(10.0))
                                    .text_color(rgb(t.term_red))
                                    .child("■")
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                    .on_click(cx.listener({
                                        let project_id = project_id.clone();
                                        let service_name = service_name.clone();
                                        move |this, _, _window, cx| {
                                            cx.stop_propagation();
                                            this.dispatch_service_action(&project_id, ActionRequest::StopService {
                                                project_id: project_id.clone(),
                                                service_name: service_name.clone(),
                                            }, cx);
                                        }
                                    }))
                                    .tooltip(|_window, cx| Tooltip::new("Stop").build(_window, cx)),
                            )
                    }),
            )
    }
}
