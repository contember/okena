use gpui::prelude::FluentBuilder;
use gpui::*;
use okena_transport::client::{
    ConnectionStatus, LOCAL_DAEMON_CONNECTION_ID, RemoteConnectionConfig,
};
use okena_ui::theme::theme;
use okena_ui::tokens::{ui_text_md, ui_text_ms, ui_text_sm, ui_text_xl};
use okena_workspace::requests::OverlayRequest;

use crate::sidebar::Sidebar;
use crate::sidebar::sections::{format_host_stats, host_stats_widget, status_label};

/// Owned snapshot of a single connection for rendering.
struct ConnectionSnapshot {
    config: RemoteConnectionConfig,
    status: ConnectionStatus,
    /// Host CPU / memory, present only while the sidebar setting is on.
    stats: Option<okena_core::api::ApiSystemStats>,
}

impl Sidebar {
    /// Render the REMOTE section at the bottom of the sidebar: connection
    /// management (status rows + "Add Connection"). Remote projects render as
    /// ordinary workspace projects above. In the Separate remote layout the
    /// hosts already have their own rows up there (status, stats and the
    /// connection menu included), so only the "Add Connection" button is left.
    pub fn render_remote_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let has_remote = self.get_remote_connections.is_some();

        if !has_remote {
            return div().into_any_element();
        }

        let settings = self.sidebar_settings(cx);
        if settings.remote_layout.is_separate() {
            return div()
                .mt(px(4.0))
                .child(self.render_add_connection_button(cx))
                .into_any_element();
        }

        // Snapshot connection data via callback. The implicit loopback
        // connection to the local daemon (`--daemon-client` mode) is not a
        // user-managed remote — its projects already render as ordinary
        // workspace projects — so it is hidden from this REMOTE management
        // section. Real remote daemons still appear here.
        let stats_style = settings.host_stats;
        let snapshots: Vec<ConnectionSnapshot> =
            if let Some(ref get_connections) = self.get_remote_connections {
                (get_connections)(cx)
                    .into_iter()
                    .filter(|s| s.config.id != LOCAL_DAEMON_CONNECTION_ID)
                    .map(|s| ConnectionSnapshot {
                        config: s.config,
                        status: s.status,
                        stats: s.system_stats.filter(|_| stats_style.is_on()),
                    })
                    .collect()
            } else {
                Vec::new()
            };

        if snapshots.is_empty() {
            return div()
                .child(self.render_remote_header(cx))
                .child(self.render_add_connection_button(cx))
                .into_any_element();
        }

        let mut children: Vec<AnyElement> = Vec::new();
        children.push(self.render_remote_header(cx).into_any_element());

        for snap in &snapshots {
            children.push(
                self.render_connection_header(
                    &snap.config,
                    &snap.status,
                    snap.stats.as_ref(),
                    stats_style,
                    cx,
                )
                .into_any_element(),
            );
        }

        children.push(self.render_add_connection_button(cx).into_any_element());

        div().children(children).into_any_element()
    }

    fn render_remote_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        div()
            .h(px(28.0))
            .px(px(12.0))
            .mt(px(8.0))
            .flex()
            .items_center()
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_secondary))
                    .child("REMOTE"),
            )
    }

    fn render_connection_header(
        &self,
        config: &RemoteConnectionConfig,
        status: &ConnectionStatus,
        stats: Option<&okena_core::api::ApiSystemStats>,
        stats_style: okena_workspace::settings::HostStatsStyle,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let name = config.name.clone();
        let host_port = format!("{}:{}", config.host, config.port);
        let stats_widget = stats.and_then(|stats| host_stats_widget(stats_style, stats, &t, cx));
        let stats_tooltip = stats.map(format_host_stats);

        // Status dot color
        let status_color = match status {
            ConnectionStatus::Connected => t.term_green,
            ConnectionStatus::Connecting
            | ConnectionStatus::Pairing
            | ConnectionStatus::Reconnecting { .. } => t.term_yellow,
            ConnectionStatus::Disconnected => t.text_muted,
            ConnectionStatus::Error(_) => t.term_red,
        };
        let status_text = status_label(status);

        let conn_id_for_ctx = config.id.clone();
        let conn_name_for_ctx = config.name.clone();
        let is_pairing = matches!(
            status,
            ConnectionStatus::Pairing | ConnectionStatus::Error(_) | ConnectionStatus::Disconnected
        );
        // Flag plain-http connections so the user can spot (and upgrade) them.
        let insecure = !config.tls;
        let conn_tls = config.tls;

        div()
            .id(ElementId::Name(format!("remote-conn-{}", config.id).into()))
            .h(px(28.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .when_some(stats_tooltip, |d, tooltip| {
                d.tooltip(move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                })
            })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    this.request_broker.update(cx, |broker, cx| {
                        broker.push_overlay_request(
                            okena_workspace::requests::OverlayRequest::RemoteConnectionContextMenu {
                                connection_id: conn_id_for_ctx.clone(),
                                connection_name: conn_name_for_ctx.clone(),
                                is_pairing,
                                tls: conn_tls,
                                position: event.position,
                            },
                            cx,
                        );
                    });
                    cx.stop_propagation();
                }),
            )
            .child(
                // Status dot
                div()
                    .w(px(8.0))
                    .h(px(8.0))
                    .rounded_full()
                    .bg(rgb(status_color))
                    .flex_shrink_0(),
            )
            .child(
                div()
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(t.text_primary))
                    .child(name),
            )
            .child(
                div()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(format!("{} — {}", host_port, status_text)),
            )
            .children(stats_widget)
            // Warning badge for unencrypted connections (right-click → Upgrade).
            .children(insecure.then(|| {
                div()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.term_yellow))
                    .child("⚠ no TLS")
            }))
    }

    fn render_add_connection_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        div()
            .id("add-remote-connection-btn")
            .h(px(26.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .on_click(cx.listener(|this, _, _window, cx| {
                this.request_broker.update(cx, |broker, cx| {
                    broker.push_overlay_request(OverlayRequest::RemoteConnect, cx);
                });
            }))
            .child(
                div()
                    .text_size(ui_text_xl(cx))
                    .text_color(rgb(t.text_secondary))
                    .child("+"),
            )
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child("Add Connection"),
            )
    }
}
