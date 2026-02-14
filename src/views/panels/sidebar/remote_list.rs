use okena_core::client::{ConnectionStatus, RemoteConnectionConfig};
use crate::theme::theme;
use crate::workspace::requests::OverlayRequest;
use gpui::*;

use super::Sidebar;

/// Owned snapshot of a single connection for rendering.
struct ConnectionSnapshot {
    config: RemoteConnectionConfig,
    status: ConnectionStatus,
}

impl Sidebar {
    /// Render the REMOTE section (header + connection status headers + add button).
    /// Remote projects are now rendered as regular workspace projects inside auto-created folders,
    /// so this section only needs connection management UI.
    pub(super) fn render_remote_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(ref remote_manager) = self.remote_manager else {
            return div().into_any_element();
        };

        // Snapshot connection data
        let snapshots: Vec<ConnectionSnapshot> = remote_manager.read(cx).connections().iter().map(|(config, status, _state)| {
            ConnectionSnapshot {
                config: (*config).clone(),
                status: (*status).clone(),
            }
        }).collect();

        if snapshots.is_empty() {
            return div()
                .child(self.render_remote_header(cx))
                .child(self.render_add_connection_button(cx))
                .into_any_element();
        }

        let mut children: Vec<AnyElement> = Vec::new();
        children.push(self.render_remote_header(cx).into_any_element());

        // Only show connection headers for non-connected states (connecting, error, etc.)
        // Connected connections have their projects shown via the regular folder rendering
        for snap in &snapshots {
            if !matches!(snap.status, ConnectionStatus::Connected) {
                children.push(
                    self.render_connection_header(&snap.config, &snap.status, false, cx)
                        .into_any_element(),
                );
            }
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
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_secondary))
                    .child("REMOTE"),
            )
    }

    fn render_connection_header(
        &self,
        config: &RemoteConnectionConfig,
        status: &ConnectionStatus,
        _is_collapsed: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let name = config.name.clone();
        let host_port = format!("{}:{}", config.host, config.port);

        // Status dot color
        let status_color = match status {
            ConnectionStatus::Connected => t.term_green,
            ConnectionStatus::Connecting
            | ConnectionStatus::Pairing
            | ConnectionStatus::Reconnecting { .. } => t.term_yellow,
            ConnectionStatus::Disconnected => t.text_muted,
            ConnectionStatus::Error(_) => t.term_red,
        };

        let status_text = match status {
            ConnectionStatus::Connecting => "Connecting...",
            ConnectionStatus::Pairing => "Pairing...",
            ConnectionStatus::Reconnecting { .. } => {
                // We can't easily format with the attempt number in a static str,
                // so we'll just show "Reconnecting..."
                "Reconnecting..."
            }
            ConnectionStatus::Disconnected => "Disconnected",
            ConnectionStatus::Error(_) => "Error",
            ConnectionStatus::Connected => "Connected",
        };

        let conn_id_for_ctx = config.id.clone();
        let conn_name_for_ctx = config.name.clone();

        div()
            .id(ElementId::Name(
                format!("remote-conn-{}", config.id).into(),
            ))
            .h(px(28.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                this.request_broker.update(cx, |broker, cx| {
                    broker.push_overlay_request(
                        crate::workspace::requests::OverlayRequest::RemoteConnectionContextMenu {
                            connection_id: conn_id_for_ctx.clone(),
                            connection_name: conn_name_for_ctx.clone(),
                            position: event.position,
                        },
                        cx,
                    );
                });
                cx.stop_propagation();
            }))
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
                    .text_size(px(12.0))
                    .text_color(rgb(t.text_primary))
                    .child(name),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(rgb(t.text_muted))
                    .child(format!("{} — {}", host_port, status_text)),
            )
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
                    .text_size(px(14.0))
                    .text_color(rgb(t.text_secondary))
                    .child("+"),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(t.text_secondary))
                    .child("Add Connection"),
            )
    }
}
