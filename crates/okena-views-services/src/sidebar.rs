//! Pure render functions for service items in the sidebar.

use crate::types::{ServiceSnapshot, status_color};
use gpui::*;
use gpui::prelude::*;
use gpui_component::tooltip::Tooltip;
use okena_services::manager::ServiceStatus;
use okena_ui::icon_action_button::icon_action_button_sized;
use okena_ui::theme::ThemeColors;
use okena_ui::tokens::{ui_text_xs, ui_text_sm, ui_text_md};

/// Render the action buttons for the services group header.
///
/// Returns a `Div` containing Start All / Stop All / Reload buttons.
/// The caller is responsible for:
/// - Creating the group header frame (via `sidebar_group_header`)
/// - Adding the `.on_click()` for collapse/expand
/// - Wrapping this in the appropriate group container
pub fn render_service_group_actions(
    project_id: &str,
    t: &ThemeColors,
    cx: &App,
    on_start_all: impl Fn(&mut Window, &mut App) + 'static,
    on_stop_all: impl Fn(&mut Window, &mut App) + 'static,
    on_reload: impl Fn(&mut Window, &mut App) + 'static,
) -> Div {
    let pid = project_id.to_string();
    div()
        .flex()
        .flex_shrink_0()
        .gap(px(2.0))
        .opacity(0.0)
        .group_hover("services-header", |s| s.opacity(1.0))
        .child(
            icon_action_button_sized(
                ElementId::Name(format!("svc-start-all-{}", pid).into()),
                "\u{25B6}",
                t.text_secondary,
                18.0,
                t,
                cx,
            )
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                on_start_all(window, cx);
            })
            .tooltip(|_window, cx| Tooltip::new("Start All").build(_window, cx)),
        )
        .child({
            let pid = pid.clone();
            icon_action_button_sized(
                ElementId::Name(format!("svc-stop-all-{}", pid).into()),
                "\u{25A0}",
                t.text_secondary,
                18.0,
                t,
                cx,
            )
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                on_stop_all(window, cx);
            })
            .tooltip(|_window, cx| Tooltip::new("Stop All").build(_window, cx))
        })
        .child({
            icon_action_button_sized(
                ElementId::Name(format!("svc-reload-{}", pid).into()),
                "\u{27F3}",
                t.text_secondary,
                18.0,
                t,
                cx,
            )
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                on_reload(window, cx);
            })
            .tooltip(|_window, cx| Tooltip::new("Reload Services").build(_window, cx))
        })
}

/// Render a single service item row with status dot, name, ports, and action buttons.
///
/// Returns a `Div` ready to be placed in the sidebar. Event handlers are passed as closures
/// so the caller retains control over state mutation.
pub fn render_service_item(
    service: &ServiceSnapshot,
    project_id: &str,
    is_cursor: bool,
    left_padding: f32,
    port_host: &str,
    t: &ThemeColors,
    cx: &App,
    on_start: impl Fn(&mut Window, &mut App) + 'static,
    on_stop: impl Fn(&mut Window, &mut App) + 'static,
    on_restart: impl Fn(&mut Window, &mut App) + 'static,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
    on_port_click: impl Fn(u16) + 'static,
) -> Stateful<Div> {
    let pid = project_id.to_string();
    let service_name = service.name.clone();
    let status = service.status.clone();
    let is_docker = service.is_docker;
    let ports = service.ports.clone();
    let port_host = port_host.to_string();

    let is_running = matches!(status, ServiceStatus::Running);
    let is_starting = matches!(status, ServiceStatus::Starting | ServiceStatus::Restarting);
    let sc = status_color(&status, t);

    div()
        .id(ElementId::Name(
            format!("svc-item-{}-{}", pid, service_name).into(),
        ))
        .group("service-item")
        .h(px(22.0))
        .pl(px(left_padding))
        .pr(px(8.0))
        .flex()
        .items_center()
        .gap(px(4.0))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .when(is_cursor, |d| {
            d.border_l_2().border_color(rgb(t.border_active))
        })
        .on_click(move |_, window, cx| {
            on_click(window, cx);
        })
        .child({
            // Status indicator (rounded square to distinguish from project's circle)
            let dot = div()
                .id(ElementId::Name(
                    format!("svc-dot-{}-{}", pid, service_name).into(),
                ))
                .flex_shrink_0()
                .w(px(6.0))
                .h(px(6.0))
                .rounded(px(1.5))
                .bg(rgb(sc));
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
        .when(is_docker, |d| {
            d.child(
                div()
                    .flex_shrink_0()
                    .px(px(3.0))
                    .h(px(14.0))
                    .flex()
                    .items_center()
                    .rounded(px(2.0))
                    .bg(rgb(t.bg_secondary))
                    .text_size(ui_text_xs(cx))
                    .text_color(rgb(t.text_muted))
                    .child("docker"),
            )
        })
        .child(
            // Service name
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_size(ui_text_md(cx))
                .text_color(rgb(t.text_primary))
                .text_ellipsis()
                .child(service_name.clone()),
        )
        .children(
            // Port badges
            ports.iter().map({
                let pid = pid.clone();
                let service_name = service_name.clone();
                let port_host = port_host.clone();
                let on_port_click = std::sync::Arc::new(on_port_click);
                move |port| {
                    let port = *port;
                    let url = format!("http://{}:{}", port_host, port);
                    let tooltip_url = url.clone();
                    let on_port_click = on_port_click.clone();
                    div()
                        .id(ElementId::Name(
                            format!("svc-port-{}-{}-{}", pid, service_name, port).into(),
                        ))
                        .flex_shrink_0()
                        .cursor_pointer()
                        .px(px(4.0))
                        .h(px(16.0))
                        .flex()
                        .items_center()
                        .rounded(px(3.0))
                        .bg(rgb(t.bg_secondary))
                        .hover(|s| s.bg(rgb(t.bg_hover)))
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(t.text_muted))
                        .child(format!(":{}", port))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, _, _cx| {
                            on_port_click(port);
                        })
                        .tooltip(move |_window, cx| {
                            Tooltip::new(tooltip_url.clone()).build(_window, cx)
                        })
                }
            }),
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
                    d.child(
                        icon_action_button_sized(
                            ElementId::Name(
                                format!("svc-play-{}-{}", pid, service_name).into(),
                            ),
                            "\u{25B6}",
                            t.term_green,
                            18.0,
                            t,
                            cx,
                        )
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            on_start(window, cx);
                        })
                        .tooltip(|_window, cx| Tooltip::new("Start").build(_window, cx)),
                    )
                })
                .when(is_running, |d| {
                    d.child(
                        icon_action_button_sized(
                            ElementId::Name(
                                format!("svc-restart-{}-{}", pid, service_name).into(),
                            ),
                            "\u{27F3}",
                            t.text_secondary,
                            18.0,
                            t,
                            cx,
                        )
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            on_restart(window, cx);
                        })
                        .tooltip(|_window, cx| Tooltip::new("Restart").build(_window, cx)),
                    )
                    .child(
                        icon_action_button_sized(
                            ElementId::Name(
                                format!("svc-stop-{}-{}", pid, service_name).into(),
                            ),
                            "\u{25A0}",
                            t.term_red,
                            18.0,
                            t,
                            cx,
                        )
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            on_stop(window, cx);
                        })
                        .tooltip(|_window, cx| Tooltip::new("Stop").build(_window, cx)),
                    )
                }),
        )
}
