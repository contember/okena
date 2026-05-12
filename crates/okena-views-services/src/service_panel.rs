//! ServicePanel — self-contained GPUI entity for the per-project service
//! log panel in the project column.
//!
//! Extracted from `ProjectColumn` to keep that view thin. Manages service
//! panel state (open/closed, active service, terminal pane, panel height)
//! and delegates rendering to the pure functions in `panel.rs`.

use crate::panel;
use crate::types::ServiceSnapshot;
use okena_core::api::ActionRequest;
use okena_core::process::open_url;
use okena_services::manager::{ServiceKind, ServiceManager, ServiceStatus};
use okena_terminal::backend::TerminalBackend;
use okena_terminal::TerminalsRegistry;
use okena_views_terminal::layout::split_pane::{ActiveDrag, DragState};
use okena_views_terminal::layout::terminal_pane::TerminalPane;
use okena_views_terminal::elements::resize_handle::ResizeHandle;
use okena_views_terminal::ActionDispatch;
use okena_workspace::request_broker::RequestBroker;
use okena_workspace::state::{WindowId, Workspace};

use gpui::prelude::*;
use gpui::*;
use okena_ui::theme::ThemeColors;
use std::sync::Arc;

/// Per-project service log panel entity.
///
/// Generic over `D: ActionDispatch` so it can dispatch service
/// start/stop/restart actions through either a local or remote dispatcher.
pub struct ServicePanel<D: ActionDispatch + Send + Sync> {
    project_id: String,
    workspace: Entity<Workspace>,
    focus_manager: Entity<okena_workspace::focus::FocusManager>,
    request_broker: Entity<RequestBroker>,
    window_id: WindowId,
    backend: Arc<dyn TerminalBackend>,
    terminals: TerminalsRegistry,
    active_drag: ActiveDrag,

    /// Action dispatcher for routing service actions (local or remote).
    action_dispatcher: Option<D>,
    /// Service manager reference (set after creation for local projects).
    service_manager: Option<Entity<ServiceManager>>,
    /// Whether the per-project service log panel is open.
    service_panel_open: bool,
    /// Currently active service name in the service panel.
    active_service_name: Option<String>,
    /// Terminal pane showing the active service's log output.
    service_terminal_pane: Option<Entity<TerminalPane<D>>>,
    /// Height of the service panel in pixels.
    service_panel_height: f32,
}

impl<D: ActionDispatch + Send + Sync> ServicePanel<D> {
    pub fn new(
        project_id: String,
        workspace: Entity<Workspace>,
        focus_manager: Entity<okena_workspace::focus::FocusManager>,
        request_broker: Entity<RequestBroker>,
        backend: Arc<dyn TerminalBackend>,
        terminals: TerminalsRegistry,
        active_drag: ActiveDrag,
        window_id: WindowId,
        initial_height: f32,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            project_id,
            workspace,
            focus_manager,
            request_broker,
            window_id,
            backend,
            terminals,
            active_drag,
            action_dispatcher: None,
            service_manager: None,
            service_panel_open: false,
            active_service_name: None,
            service_terminal_pane: None,
            service_panel_height: initial_height,
        }
    }

    /// Set the action dispatcher.
    pub fn set_action_dispatcher(&mut self, dispatcher: Option<D>) {
        self.action_dispatcher = dispatcher;
    }

    /// Whether the service panel is currently open.
    pub fn is_open(&self) -> bool {
        self.service_panel_open
    }

    /// Set the service manager and observe it for changes.
    pub fn set_service_manager(&mut self, manager: Entity<ServiceManager>, cx: &mut Context<Self>) {
        let project_id = self.project_id.clone();
        cx.observe(&manager, move |this, sm, cx| {
            let Some(ref active_name) = this.active_service_name else { return };
            let current_tid = sm.read(cx)
                .terminal_id_for(&project_id, active_name)
                .cloned();

            match current_tid {
                Some(new_tid) => {
                    let pane_tid = this.service_terminal_pane.as_ref()
                        .and_then(|p| p.read(cx).terminal_id());
                    if pane_tid.as_deref() != Some(&new_tid) {
                        let name = active_name.clone();
                        this.show_service(&name, cx);
                    }
                }
                None => {
                    let is_active_docker = sm.read(cx)
                        .instances()
                        .get(&(project_id.clone(), active_name.clone()))
                        .is_some_and(|i| {
                            matches!(i.kind, ServiceKind::DockerCompose { .. })
                                && matches!(i.status, ServiceStatus::Running | ServiceStatus::Restarting)
                        });

                    if is_active_docker {
                        let name = active_name.clone();
                        this.show_service(&name, cx);
                    } else {
                        this.service_terminal_pane = None;
                        cx.notify();
                    }
                }
            }
        }).detach();

        self.service_manager = Some(manager);
    }

    /// Get the service manager reference (if set).
    pub fn service_manager(&self) -> Option<&Entity<ServiceManager>> {
        self.service_manager.as_ref()
    }

    /// Show a service's log output in the per-project panel.
    pub fn show_service(&mut self, service_name: &str, cx: &mut Context<Self>) {
        // For Docker services with no terminal_id, spawn a log viewer PTY on demand
        if let Some(ref sm) = self.service_manager {
            let is_docker = sm.read(cx).instances()
                .get(&(self.project_id.clone(), service_name.to_string()))
                .is_some_and(|i| matches!(i.kind, ServiceKind::DockerCompose { .. }));
            let has_terminal = sm.read(cx).terminal_id_for(&self.project_id, service_name).is_some();
            if is_docker && !has_terminal {
                let pid = self.project_id.clone();
                let name = service_name.to_string();
                sm.update(cx, |sm, cx| {
                    sm.open_docker_logs(&pid, &name, cx);
                });
            }
        }

        // Look up terminal_id from either ServiceManager or remote services
        let terminal_id = if let Some(ref sm) = self.service_manager {
            sm.read(cx).terminal_id_for(&self.project_id, service_name).cloned()
        } else {
            self.workspace.read(cx).remote_snapshot(&self.project_id)
                .and_then(|snap| {
                    snap.services.iter()
                        .find(|s| s.name == service_name)
                        .and_then(|s| s.terminal_id.clone())
                })
        };

        self.active_service_name = Some(service_name.to_string());
        self.service_panel_open = true;

        if let Some(tid) = terminal_id {
            let project_path = self.service_manager.as_ref()
                .and_then(|sm| sm.read(cx).project_path(&self.project_id).cloned())
                .or_else(|| {
                    self.workspace.read(cx).project(&self.project_id)
                        .map(|p| p.path.clone())
                })
                .unwrap_or_default();

            let ws = self.workspace.clone();
            let fm = self.focus_manager.clone();
            let rb = self.request_broker.clone();
            let window_id = self.window_id;
            let backend = self.backend.clone();
            let terminals = self.terminals.clone();
            let pid = self.project_id.clone();

            let pane = cx.new(move |cx| {
                TerminalPane::new(
                    ws,
                    fm,
                    rb,
                    window_id,
                    pid,
                    project_path,
                    vec![usize::MAX],
                    Some(tid),
                    false,
                    false,
                    backend,
                    terminals,
                    None,
                    cx,
                )
            });

            self.service_terminal_pane = Some(pane);
        } else {
            self.service_terminal_pane = None;
        }

        cx.notify();
    }

    /// Set the service panel height (called during drag resize).
    pub fn set_service_panel_height(&mut self, height: f32, cx: &mut Context<Self>) {
        self.service_panel_height = height.clamp(80.0, 600.0);
        let project_id = self.project_id.clone();
        let h = self.service_panel_height;
        self.workspace.update(cx, |ws, cx| {
            ws.update_service_panel_height(&project_id, h, cx);
        });
        cx.notify();
    }

    /// Show the service overview tab (no specific service selected).
    pub fn show_overview(&mut self, cx: &mut Context<Self>) {
        self.active_service_name = None;
        self.service_terminal_pane = None;
        self.service_panel_open = true;
        cx.notify();
    }

    /// Close the per-project service log panel.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.service_panel_open = false;
        self.service_terminal_pane = None;
        self.active_service_name = None;
        cx.notify();
    }

    /// Observe workspace for remote service state changes (used for remote project columns).
    pub fn observe_remote_services(&mut self, workspace: Entity<Workspace>, cx: &mut Context<Self>) {
        let project_id = self.project_id.clone();
        cx.observe(&workspace, move |this, ws, cx| {
            let Some(ref active_name) = this.active_service_name else { return };

            let current_tid = ws.read(cx).remote_snapshot(&project_id)
                .and_then(|snap| {
                    snap.services.iter()
                        .find(|s| s.name == *active_name)
                        .and_then(|s| s.terminal_id.clone())
                });

            match current_tid {
                Some(new_tid) => {
                    let pane_tid = this.service_terminal_pane.as_ref()
                        .and_then(|p| p.read(cx).terminal_id());
                    if pane_tid.as_deref() != Some(&new_tid) {
                        let name = active_name.clone();
                        this.show_service(&name, cx);
                    }
                }
                None => {
                    this.service_terminal_pane = None;
                    cx.notify();
                }
            }
        }).detach();
    }

    /// Get the list of services for this project, from either ServiceManager (local)
    /// or the remote snapshot (remote).
    fn get_service_list(&self, cx: &Context<Self>) -> Vec<ServiceSnapshot> {
        if let Some(ref sm) = self.service_manager {
            let services = sm.read(cx).services_for_project(&self.project_id);
            if !services.is_empty() {
                return services.iter().map(|inst| ServiceSnapshot {
                    name: inst.definition.name.clone(),
                    status: inst.status.clone(),
                    terminal_id: inst.terminal_id.clone(),
                    ports: inst.detected_ports.clone(),
                    is_docker: matches!(inst.kind, ServiceKind::DockerCompose { .. }),
                    is_extra: inst.is_extra,
                }).collect();
            }
        }
        let ws = self.workspace.read(cx);
        ws.remote_snapshot(&self.project_id)
            .map(|snap| snap.services.iter().map(|api_svc| ServiceSnapshot {
                name: api_svc.name.clone(),
                status: ServiceStatus::from_api(&api_svc.status, api_svc.exit_code),
                terminal_id: api_svc.terminal_id.clone(),
                ports: api_svc.ports.clone(),
                is_docker: api_svc.kind == "docker_compose",
                is_extra: api_svc.is_extra,
            }).collect())
            .unwrap_or_default()
    }

    /// Dispatch a service action through ActionDispatcher.
    fn dispatch_service_action(&self, action: ActionRequest, cx: &mut Context<Self>) {
        if let Some(ref dispatcher) = self.action_dispatcher {
            dispatcher.dispatch(action, cx);
        }
    }

    // ── Rendering ───────────────────────────────────────────────────

    /// Render the service indicator button for the project header.
    pub fn render_service_indicator(&self, t: &ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        let services = self.get_service_list(cx);
        let entity = cx.entity().downgrade();

        panel::render_service_indicator(
            &services,
            t,
            move |_window, cx| {
                if let Some(e) = entity.upgrade() {
                    e.update(cx, |this, cx| {
                        if this.service_panel_open {
                            this.close(cx);
                        } else {
                            this.show_overview(cx);
                        }
                    });
                }
            },
        )
    }

    /// Render the per-project service log panel (resize handle + tab header + terminal pane).
    pub fn render_panel(&self, t: &ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        if !self.service_panel_open {
            return div().into_any_element();
        }
        let services = self.get_service_list(cx);

        if services.is_empty() {
            return div().into_any_element();
        }

        let active_name = self.active_service_name.clone();
        let is_overview = active_name.is_none();

        let active_status = active_name.as_ref().and_then(|name| {
            services.iter()
                .find(|s| s.name == *name)
                .map(|s| s.status.clone())
        });

        let project_id = self.project_id.clone();
        let active_drag = self.active_drag.clone();
        let panel_height = self.service_panel_height;
        let entity = cx.entity().downgrade();

        div()
            .id("service-panel")
            .flex()
            .flex_col()
            .h(px(panel_height))
            .flex_shrink_0()
            .child(
                ResizeHandle::new(
                    true,
                    t.border,
                    t.border_active,
                    move |mouse_pos, _cx| {
                        *active_drag.borrow_mut() = Some(DragState::ServicePanel {
                            project_id: project_id.clone(),
                            initial_mouse_y: f32::from(mouse_pos.y),
                            initial_height: panel_height,
                        });
                    },
                ),
            )
            .child(
                panel::render_service_panel_header(
                    &services,
                    active_name.as_deref(),
                    &t,
                    cx,
                    // on_overview_click
                    {
                        let entity = entity.clone();
                        move |_window, cx| {
                            if let Some(e) = entity.upgrade() {
                                e.update(cx, |this, cx| this.show_overview(cx));
                            }
                        }
                    },
                    // on_tab_click
                    {
                        let entity = entity.clone();
                        move |name: String, _window, cx| {
                            if let Some(e) = entity.upgrade() {
                                e.update(cx, |this, cx| this.show_service(&name, cx));
                            }
                        }
                    },
                    // on_start_all
                    {
                        let entity = entity.clone();
                        move |_window, cx| {
                            if let Some(e) = entity.upgrade() {
                                e.update(cx, |this, cx| {
                                    this.dispatch_service_action(ActionRequest::StartAllServices {
                                        project_id: this.project_id.clone(),
                                    }, cx);
                                });
                            }
                        }
                    },
                    // on_stop_all
                    {
                        let entity = entity.clone();
                        move |_window, cx| {
                            if let Some(e) = entity.upgrade() {
                                e.update(cx, |this, cx| {
                                    this.dispatch_service_action(ActionRequest::StopAllServices {
                                        project_id: this.project_id.clone(),
                                    }, cx);
                                });
                            }
                        }
                    },
                    // on_reload
                    {
                        let entity = entity.clone();
                        move |_window, cx| {
                            if let Some(e) = entity.upgrade() {
                                e.update(cx, |this, cx| {
                                    this.dispatch_service_action(ActionRequest::ReloadServices {
                                        project_id: this.project_id.clone(),
                                    }, cx);
                                });
                            }
                        }
                    },
                    // on_start (active service)
                    {
                        let entity = entity.clone();
                        move |_window, cx| {
                            if let Some(e) = entity.upgrade() {
                                e.update(cx, |this, cx| {
                                    if let Some(name) = this.active_service_name.clone() {
                                        this.dispatch_service_action(ActionRequest::StartService {
                                            project_id: this.project_id.clone(),
                                            service_name: name,
                                        }, cx);
                                    }
                                });
                            }
                        }
                    },
                    // on_stop (active service)
                    {
                        let entity = entity.clone();
                        move |_window, cx| {
                            if let Some(e) = entity.upgrade() {
                                e.update(cx, |this, cx| {
                                    if let Some(name) = this.active_service_name.clone() {
                                        this.dispatch_service_action(ActionRequest::StopService {
                                            project_id: this.project_id.clone(),
                                            service_name: name,
                                        }, cx);
                                    }
                                });
                            }
                        }
                    },
                    // on_restart (active service)
                    {
                        let entity = entity.clone();
                        move |_window, cx| {
                            if let Some(e) = entity.upgrade() {
                                e.update(cx, |this, cx| {
                                    if let Some(name) = this.active_service_name.clone() {
                                        this.dispatch_service_action(ActionRequest::RestartService {
                                            project_id: this.project_id.clone(),
                                            service_name: name,
                                        }, cx);
                                    }
                                });
                            }
                        }
                    },
                    // on_close
                    {
                        let entity = entity.clone();
                        move |_window, cx| {
                            if let Some(e) = entity.upgrade() {
                                e.update(cx, |this, cx| this.close(cx));
                            }
                        }
                    },
                    active_status.as_ref(),
                ),
            )
            .child(
                // Content area
                if is_overview {
                    self.render_overview_content(t, &services, cx).into_any_element()
                } else if self.service_terminal_pane.is_some() {
                    div()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .overflow_hidden()
                        .children(self.service_terminal_pane.clone())
                        .into_any_element()
                } else {
                    let entity = entity.clone();
                    panel::render_not_running_placeholder(
                        t,
                        cx,
                        move |_window, cx| {
                            if let Some(e) = entity.upgrade() {
                                e.update(cx, |this, cx| {
                                    if let Some(name) = this.active_service_name.clone() {
                                        this.dispatch_service_action(ActionRequest::StartService {
                                            project_id: this.project_id.clone(),
                                            service_name: name,
                                        }, cx);
                                    }
                                });
                            }
                        },
                    ).into_any_element()
                },
            )
            .into_any_element()
    }

    /// Render the overview content showing all services in a table layout.
    fn render_overview_content(&self, t: &ThemeColors, services: &[ServiceSnapshot], cx: &mut Context<Self>) -> impl IntoElement {

        let remote_host = self.workspace.read(cx).remote_snapshot(&self.project_id)
            .and_then(|snap| snap.host.clone());

        let project_id = self.project_id.clone();
        let entity = cx.entity().downgrade();

        panel::render_service_overview(
            services,
            &project_id,
            remote_host.as_deref(),
            &t,
            cx,
            // on_service_click
            {
                let entity = entity.clone();
                move |name: String, _window, cx| {
                    if let Some(e) = entity.upgrade() {
                        e.update(cx, |this, cx| this.show_service(&name, cx));
                    }
                }
            },
            // on_start
            {
                let entity = entity.clone();
                move |name: String, _window, cx| {
                    if let Some(e) = entity.upgrade() {
                        e.update(cx, |this, cx| {
                            this.dispatch_service_action(ActionRequest::StartService {
                                project_id: this.project_id.clone(),
                                service_name: name.clone(),
                            }, cx);
                        });
                    }
                }
            },
            // on_stop
            {
                let entity = entity.clone();
                move |name: String, _window, cx| {
                    if let Some(e) = entity.upgrade() {
                        e.update(cx, |this, cx| {
                            this.dispatch_service_action(ActionRequest::StopService {
                                project_id: this.project_id.clone(),
                                service_name: name.clone(),
                            }, cx);
                        });
                    }
                }
            },
            // on_restart
            {
                let entity = entity.clone();
                move |name: String, _window, cx| {
                    if let Some(e) = entity.upgrade() {
                        e.update(cx, |this, cx| {
                            this.dispatch_service_action(ActionRequest::RestartService {
                                project_id: this.project_id.clone(),
                                service_name: name.clone(),
                            }, cx);
                        });
                    }
                }
            },
            // on_port_click
            |port: u16| {
                let url = format!("http://localhost:{}", port);
                open_url(&url);
            },
        )
    }
}
