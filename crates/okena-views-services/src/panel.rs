//! Pure render functions for the per-project service panel in ProjectColumn.

use crate::types::{ServiceSnapshot, status_color, status_label};
use gpui::*;
use gpui::prelude::*;
use gpui_component::tooltip::Tooltip;
use okena_services::manager::ServiceStatus;
use okena_ui::icon_action_button::icon_action_button;
use okena_ui::theme::ThemeColors;

/// Render the tab header row for the service panel.
///
/// Contains the Overview tab, per-service tabs, contextual action buttons, and close button.
/// Event handlers are passed as closures so the caller retains state control.
pub fn render_service_panel_header(
    services: &[ServiceSnapshot],
    active_service_name: Option<&str>,
    t: &ThemeColors,
    on_overview_click: impl Fn(&mut Window, &mut App) + 'static,
    on_tab_click: impl Fn(String, &mut Window, &mut App) + 'static,
    on_start_all: impl Fn(&mut Window, &mut App) + 'static,
    on_stop_all: impl Fn(&mut Window, &mut App) + 'static,
    on_reload: impl Fn(&mut Window, &mut App) + 'static,
    on_start: impl Fn(&mut Window, &mut App) + 'static,
    on_stop: impl Fn(&mut Window, &mut App) + 'static,
    on_restart: impl Fn(&mut Window, &mut App) + 'static,
    on_close: impl Fn(&mut Window, &mut App) + 'static,
    active_status: Option<&ServiceStatus>,
) -> Stateful<Div> {
    let is_overview = active_service_name.is_none();

    let active_is_running = matches!(active_status, Some(ServiceStatus::Running));
    let active_is_starting = matches!(
        active_status,
        Some(ServiceStatus::Starting | ServiceStatus::Restarting)
    );
    let active_is_stopped = !active_is_running && !active_is_starting;
    let active_exit_code = match active_status {
        Some(ServiceStatus::Crashed { exit_code }) => *exit_code,
        _ => None,
    };
    let active_is_crashed = matches!(active_status, Some(ServiceStatus::Crashed { .. }));

    let on_tab_click = std::sync::Arc::new(on_tab_click);

    div()
        .id("service-panel-header")
        .flex_shrink_0()
        .bg(rgb(t.bg_header))
        .border_b_1()
        .border_color(rgb(t.border))
        .flex()
        .items_center()
        .child(
            // Tabs area (overview + service tabs)
            div()
                .id("service-tabs-scroll")
                .flex_1()
                .min_w_0()
                .flex()
                .overflow_x_scroll()
                // Overview tab
                .child(
                    div()
                        .id("svc-tab-overview")
                        .cursor_pointer()
                        .h(px(34.0))
                        .px(px(12.0))
                        .flex()
                        .items_center()
                        .flex_shrink_0()
                        .text_size(px(12.0))
                        .when(is_overview, |d| {
                            d.bg(rgb(t.bg_primary))
                                .text_color(rgb(t.text_primary))
                        })
                        .when(!is_overview, |d| {
                            d.text_color(rgb(t.text_secondary))
                                .hover(|s| s.bg(rgb(t.bg_hover)))
                        })
                        .child("Overview")
                        .on_click(move |_, window, cx| {
                            on_overview_click(window, cx);
                        }),
                )
                // Service tabs (exclude extra Docker services unless active)
                .children(
                    services
                        .iter()
                        .filter(|svc| {
                            !svc.is_extra || active_service_name == Some(&svc.name)
                        })
                        .map(|svc| {
                            let name = svc.name.clone();
                            let is_active = active_service_name == Some(&name);
                            let sc = status_color(&svc.status, t);
                            let on_tab_click = on_tab_click.clone();

                            div()
                                .id(ElementId::Name(
                                    format!("svc-tab-{}", name).into(),
                                ))
                                .cursor_pointer()
                                .h(px(34.0))
                                .px(px(12.0))
                                .flex()
                                .items_center()
                                .flex_shrink_0()
                                .gap(px(6.0))
                                .text_size(px(12.0))
                                .when(is_active, |d| {
                                    d.bg(rgb(t.bg_primary))
                                        .text_color(rgb(t.text_primary))
                                })
                                .when(!is_active, |d| {
                                    d.text_color(rgb(t.text_secondary))
                                        .hover(|s| s.bg(rgb(t.bg_hover)))
                                })
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .w(px(7.0))
                                        .h(px(7.0))
                                        .rounded(px(3.5))
                                        .bg(rgb(sc)),
                                )
                                .child(name.clone())
                                .on_click(move |_, window, cx| {
                                    on_tab_click(name.clone(), window, cx);
                                })
                        }),
                ),
        )
        // Contextual action buttons
        .child(
            div()
                .flex()
                .flex_shrink_0()
                .h(px(34.0))
                .items_center()
                .gap(px(2.0))
                .mr(px(4.0))
                .border_l_1()
                .border_color(rgb(t.border))
                .pl(px(6.0))
                // --- Overview actions ---
                .when(is_overview, |d| {
                    d
                        // Start All
                        .child(
                            icon_action_button("svc-panel-start-all", "\u{25B6}\u{25B6}", t.term_green, t)
                                .on_click(move |_, window, cx| {
                                    cx.stop_propagation();
                                    on_start_all(window, cx);
                                })
                                .tooltip(|_window, cx| {
                                    Tooltip::new("Start All").build(_window, cx)
                                }),
                        )
                        // Stop All
                        .child(
                            icon_action_button("svc-panel-stop-all", "\u{25A0}\u{25A0}", t.term_red, t)
                                .on_click(move |_, window, cx| {
                                    cx.stop_propagation();
                                    on_stop_all(window, cx);
                                })
                                .tooltip(|_window, cx| {
                                    Tooltip::new("Stop All").build(_window, cx)
                                }),
                        )
                        // Reload
                        .child(
                            icon_action_button("svc-panel-reload", "\u{27F3}", t.text_secondary, t)
                                .on_click(move |_, window, cx| {
                                    cx.stop_propagation();
                                    on_reload(window, cx);
                                })
                                .tooltip(|_window, cx| {
                                    Tooltip::new("Reload Services").build(_window, cx)
                                }),
                        )
                })
                // --- Detail tab actions ---
                .when(!is_overview, |d| {
                    d
                        // Exit code label (when crashed)
                        .when(active_is_crashed, |d| {
                            let label = match active_exit_code {
                                Some(code) => format!("exit {}", code),
                                None => "crashed".to_string(),
                            };
                            d.child(
                                div()
                                    .px(px(5.0))
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    .text_size(px(11.0))
                                    .text_color(rgb(t.term_red))
                                    .child(label),
                            )
                        })
                        // Start button (when stopped/crashed)
                        .when(active_is_stopped, |d| {
                            d.child(
                                icon_action_button("svc-panel-start", "\u{25B6}", t.term_green, t)
                                    .on_click(move |_, window, cx| {
                                        cx.stop_propagation();
                                        on_start(window, cx);
                                    })
                                    .tooltip(|_window, cx| {
                                        Tooltip::new("Start").build(_window, cx)
                                    }),
                            )
                        })
                        // Restart button (when running)
                        .when(active_is_running, |d| {
                            d.child(
                                icon_action_button("svc-panel-restart", "\u{27F3}", t.text_secondary, t)
                                    .on_click(move |_, window, cx| {
                                        cx.stop_propagation();
                                        on_restart(window, cx);
                                    })
                                    .tooltip(|_window, cx| {
                                        Tooltip::new("Restart").build(_window, cx)
                                    }),
                            )
                        })
                        // Stop button (when running)
                        .when(active_is_running, |d| {
                            d.child(
                                icon_action_button("svc-panel-stop", "\u{25A0}", t.term_red, t)
                                    .on_click(move |_, window, cx| {
                                        cx.stop_propagation();
                                        on_stop(window, cx);
                                    })
                                    .tooltip(|_window, cx| {
                                        Tooltip::new("Stop").build(_window, cx)
                                    }),
                            )
                        })
                }),
        )
        .child(
            // Close button
            div()
                .flex_shrink_0()
                .h(px(34.0))
                .flex()
                .items_center()
                .child(
                    div()
                        .id("service-panel-close")
                        .cursor_pointer()
                        .w(px(26.0))
                        .h(px(26.0))
                        .mx(px(4.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(3.0))
                        .hover(|s| s.bg(rgb(t.bg_hover)))
                        .text_size(px(12.0))
                        .text_color(rgb(t.text_secondary))
                        .child("\u{2715}")
                        .on_click(move |_, window, cx| {
                            on_close(window, cx);
                        }),
                ),
        )
}

/// Render the overview content showing all services in a table layout.
///
/// Contains column headers and data rows. The caller passes closures for
/// service name clicks, port clicks, and action button clicks.
pub fn render_service_overview(
    services: &[ServiceSnapshot],
    project_id: &str,
    remote_host: Option<&str>,
    t: &ThemeColors,
    on_service_click: impl Fn(String, &mut Window, &mut App) + 'static,
    on_start: impl Fn(String, &mut Window, &mut App) + 'static,
    on_stop: impl Fn(String, &mut Window, &mut App) + 'static,
    on_restart: impl Fn(String, &mut Window, &mut App) + 'static,
    on_port_click: impl Fn(u16) + 'static,
) -> Stateful<Div> {
    let has_docker = services.iter().any(|s| s.is_docker);
    let has_ports = services.iter().any(|s| !s.ports.is_empty());

    let on_service_click = std::sync::Arc::new(on_service_click);
    let on_start = std::sync::Arc::new(on_start);
    let on_stop = std::sync::Arc::new(on_stop);
    let on_restart = std::sync::Arc::new(on_restart);
    let on_port_click = std::sync::Arc::new(on_port_click);

    div()
        .id("service-overview-content")
        .flex_1()
        .min_h_0()
        .min_w_0()
        .overflow_y_scroll()
        .bg(rgb(t.bg_primary))
        .flex()
        .flex_col()
        // Column header
        .child(
            div()
                .flex_shrink_0()
                .h(px(28.0))
                .px(px(12.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .border_b_1()
                .border_color(rgb(t.border))
                .text_size(px(10.0))
                .text_color(rgb(t.text_muted))
                // Status column (dot width)
                .child(div().flex_shrink_0().w(px(7.0)))
                // Name column
                .child(div().flex_1().min_w(px(80.0)).child("NAME"))
                // Status text column
                .child(div().flex_shrink_0().w(px(70.0)).child("STATUS"))
                // Type column (only if any docker)
                .when(has_docker, |d| {
                    d.child(div().flex_shrink_0().w(px(56.0)).child("TYPE"))
                })
                // Ports column (only if any ports)
                .when(has_ports, |d| {
                    d.child(div().flex_shrink_0().w(px(100.0)).child("PORTS"))
                })
                // Actions column
                .child(div().flex_shrink_0().w(px(52.0))),
        )
        // Data rows
        .child(
            div()
                .id("service-overview-rows")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .children({
                    let has_extras = services.iter().any(|s| s.is_extra);
                    let mut rows: Vec<gpui::AnyElement> = Vec::new();
                    let mut separator_added = false;

                    for (idx, svc) in services.iter().enumerate() {
                        // Insert separator before the first extra service
                        if has_extras && svc.is_extra && !separator_added {
                            separator_added = true;
                            rows.push(
                                div()
                                    .id("svc-overview-extra-separator")
                                    .h(px(24.0))
                                    .px(px(12.0))
                                    .mt(px(4.0))
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(div().flex_1().h(px(1.0)).bg(rgb(t.border)))
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .text_size(px(9.0))
                                            .text_color(rgb(t.text_muted))
                                            .child("OTHER DOCKER SERVICES"),
                                    )
                                    .child(div().flex_1().h(px(1.0)).bg(rgb(t.border)))
                                    .into_any_element(),
                            );
                        }

                        rows.push(render_overview_row(
                            idx,
                            svc,
                            project_id,
                            has_docker,
                            has_ports,
                            remote_host,
                            t,
                            on_service_click.clone(),
                            on_start.clone(),
                            on_stop.clone(),
                            on_restart.clone(),
                            on_port_click.clone(),
                        ));
                    }
                    rows
                }),
        )
}

/// Render a single service row in the overview table.
fn render_overview_row(
    idx: usize,
    svc: &ServiceSnapshot,
    project_id: &str,
    has_docker: bool,
    has_ports: bool,
    remote_host: Option<&str>,
    t: &ThemeColors,
    on_service_click: std::sync::Arc<dyn Fn(String, &mut Window, &mut App) + 'static>,
    on_start: std::sync::Arc<dyn Fn(String, &mut Window, &mut App) + 'static>,
    on_stop: std::sync::Arc<dyn Fn(String, &mut Window, &mut App) + 'static>,
    on_restart: std::sync::Arc<dyn Fn(String, &mut Window, &mut App) + 'static>,
    on_port_click: std::sync::Arc<dyn Fn(u16) + 'static>,
) -> gpui::AnyElement {
    let name = svc.name.clone();
    let status = svc.status.clone();
    let is_docker = svc.is_docker;
    let is_extra = svc.is_extra;
    let ports = svc.ports.clone();
    let remote_host = remote_host.map(|s| s.to_string());

    let is_running = matches!(status, ServiceStatus::Running);
    let is_starting = matches!(status, ServiceStatus::Starting | ServiceStatus::Restarting);

    let sc = status_color(&status, t);
    let sl = status_label(&status);
    let name_color = if is_extra { t.text_muted } else { t.text_primary };
    let project_id = project_id.to_string();

    div()
        .id(ElementId::Name(format!("svc-overview-{}", idx).into()))
        .group(SharedString::from(format!("svc-row-{}", idx)))
        .h(px(32.0))
        .px(px(12.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .when(is_extra, |d| d.opacity(0.55))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        // Status dot
        .child(
            div()
                .flex_shrink_0()
                .w(px(7.0))
                .h(px(7.0))
                .rounded(px(3.5))
                .bg(rgb(sc)),
        )
        // Service name (clickable)
        .child({
            let on_service_click = on_service_click.clone();
            let name = name.clone();
            div()
                .id(ElementId::Name(
                    format!("svc-overview-name-{}", idx).into(),
                ))
                .cursor_pointer()
                .flex_1()
                .min_w(px(80.0))
                .text_size(px(12.0))
                .text_color(rgb(name_color))
                .text_ellipsis()
                .overflow_hidden()
                .hover(|s| s.text_color(rgb(t.border_active)))
                .child(name.clone())
                .on_click(move |_, window, cx| {
                    on_service_click(name.clone(), window, cx);
                })
        })
        // Status text
        .child(
            div()
                .flex_shrink_0()
                .w(px(70.0))
                .text_size(px(11.0))
                .text_color(rgb(sc))
                .child(sl),
        )
        // Type column
        .when(has_docker, |d| {
            d.child(
                div()
                    .flex_shrink_0()
                    .w(px(56.0))
                    .when(is_docker, |d| {
                        d.child(
                            div()
                                .px(px(3.0))
                                .h(px(14.0))
                                .flex()
                                .items_center()
                                .rounded(px(2.0))
                                .bg(rgb(t.bg_secondary))
                                .text_size(px(9.0))
                                .text_color(rgb(t.text_muted))
                                .child("docker"),
                        )
                    }),
            )
        })
        // Ports column
        .when(has_ports, |d| {
            d.child(
                div()
                    .flex_shrink_0()
                    .w(px(100.0))
                    .flex()
                    .gap(px(4.0))
                    .overflow_hidden()
                    .children(ports.iter().map({
                        let name = name.clone();
                        let project_id = project_id.clone();
                        let remote_host = remote_host.clone();
                        let on_port_click = on_port_click.clone();
                        move |port| {
                            let port = *port;
                            let host = remote_host.as_deref().unwrap_or("localhost");
                            let url = format!("http://{}:{}", host, port);
                            let tooltip_url = url.clone();
                            let on_port_click = on_port_click.clone();
                            div()
                                .id(ElementId::Name(
                                    format!(
                                        "svc-overview-port-{}-{}-{}",
                                        project_id, name, port
                                    )
                                    .into(),
                                ))
                                .flex_shrink_0()
                                .cursor_pointer()
                                .px(px(4.0))
                                .h(px(16.0))
                                .flex()
                                .items_center()
                                .rounded(px(3.0))
                                .bg(rgb(t.bg_secondary))
                                .hover(|s| s.bg(rgb(t.bg_hover)).underline())
                                .text_size(px(10.0))
                                .text_color(rgb(t.text_muted))
                                .child(format!(":{}", port))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation()
                                })
                                .on_click(move |_, _, _cx| {
                                    on_port_click(port);
                                })
                                .tooltip(move |_window, cx| {
                                    Tooltip::new(tooltip_url.clone()).build(_window, cx)
                                })
                        }
                    })),
            )
        })
        // Action buttons (show on hover)
        .child({
            let group_name = SharedString::from(format!("svc-row-{}", idx));
            let name_for_start = name.clone();
            let name_for_restart = name.clone();
            let name_for_stop = name.clone();
            div()
                .flex()
                .flex_shrink_0()
                .w(px(52.0))
                .justify_end()
                .gap(px(2.0))
                .opacity(0.0)
                .group_hover(group_name, |s| s.opacity(1.0))
                .when(!is_running && !is_starting, |d| {
                    let on_start = on_start.clone();
                    d.child(
                        icon_action_button(
                            ElementId::Name(
                                format!("svc-overview-play-{}", idx).into(),
                            ),
                            "\u{25B6}",
                            t.term_green,
                            t,
                        )
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            on_start(name_for_start.clone(), window, cx);
                        })
                        .tooltip(|_window, cx| Tooltip::new("Start").build(_window, cx)),
                    )
                })
                .when(is_running, |d| {
                    let on_restart = on_restart.clone();
                    let on_stop = on_stop.clone();
                    d.child(
                        icon_action_button(
                            ElementId::Name(
                                format!("svc-overview-restart-{}", idx).into(),
                            ),
                            "\u{27F3}",
                            t.text_secondary,
                            t,
                        )
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            on_restart(name_for_restart.clone(), window, cx);
                        })
                        .tooltip(|_window, cx| {
                            Tooltip::new("Restart").build(_window, cx)
                        }),
                    )
                    .child(
                        icon_action_button(
                            ElementId::Name(
                                format!("svc-overview-stop-{}", idx).into(),
                            ),
                            "\u{25A0}",
                            t.term_red,
                            t,
                        )
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            on_stop(name_for_stop.clone(), window, cx);
                        })
                        .tooltip(|_window, cx| {
                            Tooltip::new("Stop").build(_window, cx)
                        }),
                    )
                })
        })
        .into_any_element()
}

/// Render the service indicator button for the project header.
///
/// Shows an aggregate status dot. The caller handles the toggle logic via `on_click`.
pub fn render_service_indicator(
    services: &[ServiceSnapshot],
    t: &ThemeColors,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> gpui::AnyElement {
    if services.is_empty() {
        return div().into_any_element();
    }

    // Compute aggregate status color
    let has_running = services
        .iter()
        .any(|s| s.status == ServiceStatus::Running);
    let has_crashed = services
        .iter()
        .any(|s| matches!(s.status, ServiceStatus::Crashed { .. }));
    let has_starting = services
        .iter()
        .any(|s| matches!(s.status, ServiceStatus::Starting | ServiceStatus::Restarting));

    let dot_color = if has_crashed {
        t.term_red
    } else if has_starting {
        t.term_yellow
    } else if has_running {
        t.term_green
    } else {
        t.text_muted
    };

    let running_count = services
        .iter()
        .filter(|s| s.status == ServiceStatus::Running)
        .count();
    let total_count = services.len();
    let tooltip_text = format!("{}/{} services running", running_count, total_count);

    div()
        .id("service-indicator-btn")
        .cursor_pointer()
        .w(px(24.0))
        .h(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            on_click(window, cx);
        })
        .child(
            div()
                .w(px(7.0))
                .h(px(7.0))
                .rounded(px(4.0))
                .bg(rgb(dot_color)),
        )
        .tooltip(move |_window, cx| Tooltip::new(tooltip_text.clone()).build(_window, cx))
        .into_any_element()
}

/// Render the "not running" placeholder with a Start button.
///
/// Used in the service panel content area when a service tab is selected but not running.
pub fn render_not_running_placeholder(
    t: &ThemeColors,
    on_start: impl Fn(&mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .overflow_hidden()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(10.0))
        .bg(rgb(t.bg_primary))
        .child(
            div()
                .text_size(px(13.0))
                .text_color(rgb(t.text_muted))
                .child("Service not running"),
        )
        .child(
            div()
                .id("svc-panel-start-placeholder")
                .cursor_pointer()
                .px(px(14.0))
                .py(px(6.0))
                .rounded(px(4.0))
                .bg(rgb(t.bg_secondary))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(t.term_green))
                        .child("\u{25B6}"),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(t.text_secondary))
                        .child("Start"),
                )
                .on_click(move |_, window, cx| {
                    on_start(window, cx);
                }),
        )
}
