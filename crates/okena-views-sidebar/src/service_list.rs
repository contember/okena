//! Service list rendering for the sidebar

use okena_ui::theme::theme;
use gpui::*;
use okena_core::api::ActionRequest;
use okena_views_services::types::ServiceSnapshot;

use crate::sidebar::{Sidebar, SidebarProjectInfo, SidebarServiceInfo, GroupKind};
use crate::item_widgets::sidebar_group_header;

impl Sidebar {
    /// Render the "Services" group header with collapse chevron + Start All / Stop All / Reload buttons.
    pub fn render_services_group_header(
        &self,
        project: &SidebarProjectInfo,
        is_collapsed: bool,
        is_cursor: bool,
        left_padding: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let project_id = project.id.clone();
        let entity = cx.entity().downgrade();

        sidebar_group_header(
            ElementId::Name(format!("svc-group-{}", project_id).into()),
            GroupKind::Services.label(),
            project.services.len(),
            is_collapsed,
            is_cursor,
            left_padding,
            &t,
            cx,
        )
        .group("services-header")
        .child(
            // Spacer to push action buttons to the right
            div().flex_1(),
        )
        .child(
            okena_views_services::sidebar::render_service_group_actions(
                &project_id,
                &t,
                cx,
                {
                    let entity = entity.clone();
                    let project_id = project_id.clone();
                    move |_window, cx| {
                        if let Some(entity) = entity.upgrade() {
                            entity.update(cx, |this, cx| {
                                this.dispatch_action_for_project(&project_id, ActionRequest::StartAllServices {
                                    project_id: project_id.clone(),
                                }, cx);
                            });
                        }
                    }
                },
                {
                    let entity = entity.clone();
                    let project_id = project_id.clone();
                    move |_window, cx| {
                        if let Some(entity) = entity.upgrade() {
                            entity.update(cx, |this, cx| {
                                this.dispatch_action_for_project(&project_id, ActionRequest::StopAllServices {
                                    project_id: project_id.clone(),
                                }, cx);
                            });
                        }
                    }
                },
                {
                    let entity = entity.clone();
                    let project_id = project_id.clone();
                    move |_window, cx| {
                        if let Some(entity) = entity.upgrade() {
                            entity.update(cx, |this, cx| {
                                this.dispatch_action_for_project(&project_id, ActionRequest::ReloadServices {
                                    project_id: project_id.clone(),
                                }, cx);
                            });
                        }
                    }
                },
            ),
        )
        .on_click(cx.listener({
            let project_id = project_id.clone();
            move |this, _, _window, cx| {
                this.toggle_group(&project_id, GroupKind::Services);
                cx.notify();
            }
        }))
    }

    /// Render a single service item row with status dot, name, and action buttons.
    pub fn render_service_item(
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
        let port_host = service.port_host.clone();
        let entity = cx.entity().downgrade();

        let snapshot = ServiceSnapshot {
            name: service.name.clone(),
            status: service.status.clone(),
            terminal_id: None,
            ports: service.ports.clone(),
            is_docker: service.is_docker,
            is_extra: false,
        };

        okena_views_services::sidebar::render_service_item(
            &snapshot,
            &project_id,
            is_cursor,
            left_padding,
            &port_host,
            &t,
            cx,
            // on_start
            {
                let entity = entity.clone();
                let project_id = project_id.clone();
                let service_name = service_name.clone();
                move |_window, cx| {
                    if let Some(entity) = entity.upgrade() {
                        entity.update(cx, |this, cx| {
                            this.dispatch_action_for_project(&project_id, ActionRequest::StartService {
                                project_id: project_id.clone(),
                                service_name: service_name.clone(),
                            }, cx);
                        });
                    }
                }
            },
            // on_stop
            {
                let entity = entity.clone();
                let project_id = project_id.clone();
                let service_name = service_name.clone();
                move |_window, cx| {
                    if let Some(entity) = entity.upgrade() {
                        entity.update(cx, |this, cx| {
                            this.dispatch_action_for_project(&project_id, ActionRequest::StopService {
                                project_id: project_id.clone(),
                                service_name: service_name.clone(),
                            }, cx);
                        });
                    }
                }
            },
            // on_restart
            {
                let entity = entity.clone();
                let project_id = project_id.clone();
                let service_name = service_name.clone();
                move |_window, cx| {
                    if let Some(entity) = entity.upgrade() {
                        entity.update(cx, |this, cx| {
                            this.dispatch_action_for_project(&project_id, ActionRequest::RestartService {
                                project_id: project_id.clone(),
                                service_name: service_name.clone(),
                            }, cx);
                        });
                    }
                }
            },
            // on_click
            {
                let entity = entity.clone();
                let project_id = project_id.clone();
                let service_name = service_name.clone();
                move |_window, cx| {
                    if let Some(entity) = entity.upgrade() {
                        entity.update(cx, |this, cx| {
                            this.cursor_index = None;
                            let workspace = this.workspace.clone();
                            let pid = project_id.clone();
                            this.focus_manager.update(cx, |fm, cx| {
                                workspace.update(cx, |ws, cx| {
                                    ws.set_focused_project_individual(fm, Some(pid), cx);
                                });
                                cx.notify();
                            });
                            this.request_broker.update(cx, |broker, cx| {
                                broker.push_overlay_request(
                                    okena_workspace::requests::OverlayRequest::Project(okena_workspace::requests::ProjectOverlay {
                                        project_id: project_id.clone(),
                                        kind: okena_workspace::requests::ProjectOverlayKind::ShowServiceLog {
                                            service_name: service_name.clone(),
                                        },
                                    }),
                                    cx,
                                );
                            });
                        });
                    }
                }
            },
            // on_port_click
            {
                let port_host = port_host.clone();
                move |port: u16| {
                    let url = format!("http://{}:{}", port_host, port);
                    okena_core::process::open_url(&url);
                }
            },
        )
    }
}
