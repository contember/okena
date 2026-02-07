use crate::remote_client::config::RemoteConnectionConfig;
use crate::remote_client::connection::ConnectionStatus;
use crate::remote::types::ApiProject;
use crate::theme::theme;
use crate::workspace::requests::OverlayRequest;
use gpui::*;
use gpui::prelude::*;

use super::Sidebar;

/// Owned snapshot of a single connection for rendering.
/// Avoids holding borrows into the RemoteConnectionManager entity across render calls.
struct ConnectionSnapshot {
    config: RemoteConnectionConfig,
    status: ConnectionStatus,
    projects: Vec<ApiProject>,
}

impl Sidebar {
    /// Render the entire REMOTE section (header + connections + add button)
    pub(super) fn render_remote_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(ref remote_manager) = self.remote_manager else {
            return div().into_any_element();
        };

        // Snapshot connection data so we release the immutable borrow on cx before rendering
        let snapshots: Vec<ConnectionSnapshot> = remote_manager.read(cx).connections().iter().map(|(config, status, state)| {
            ConnectionSnapshot {
                config: (*config).clone(),
                status: (*status).clone(),
                projects: state.map(|s| s.projects.clone()).unwrap_or_default(),
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

        for snap in &snapshots {
            let connection_id = snap.config.id.clone();
            let is_collapsed = self
                .collapsed_connections
                .get(&connection_id)
                .copied()
                .unwrap_or(false);

            // Connection header
            children.push(
                self.render_connection_header(&snap.config, &snap.status, is_collapsed, cx)
                    .into_any_element(),
            );

            // Connection's projects (if expanded and connected)
            if !is_collapsed {
                for project in &snap.projects {
                    children.push(
                        self.render_remote_project(
                            &connection_id,
                            &project.id,
                            &project.name,
                            cx,
                        )
                        .into_any_element(),
                    );
                }
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
        is_collapsed: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let connection_id = config.id.clone();
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

        let arrow = if is_collapsed { "\u{25B8}" } else { "\u{25BE}" };

        let conn_id_for_ctx = config.id.clone();
        let conn_name_for_ctx = config.name.clone();

        div()
            .id(ElementId::Name(
                format!("remote-conn-{}", connection_id).into(),
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
            .on_click(cx.listener(move |this, _, _window, cx| {
                let collapsed = this
                    .collapsed_connections
                    .get(&connection_id)
                    .copied()
                    .unwrap_or(false);
                this.collapsed_connections
                    .insert(connection_id.clone(), !collapsed);
                cx.notify();
            }))
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(rgb(t.text_muted))
                    .w(px(10.0))
                    .child(arrow),
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
                    .child(host_port),
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
    }

    fn render_remote_project(
        &self,
        connection_id: &str,
        project_id: &str,
        project_name: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let conn_id = connection_id.to_string();
        let proj_id = project_id.to_string();
        let name = project_name.to_string();

        // Check if this is the focused remote project
        let is_focused = self.remote_manager.as_ref().map_or(false, |rm| {
            rm.read(cx)
                .focused_remote()
                .map_or(false, |(c, p)| c == conn_id && p == proj_id)
        });

        div()
            .id(ElementId::Name(
                format!("remote-proj-{}-{}", conn_id, proj_id).into(),
            ))
            .h(px(26.0))
            .pl(px(28.0))
            .pr(px(12.0))
            .flex()
            .items_center()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .when(is_focused, |d| d.bg(rgb(t.bg_selection)))
            .on_click(cx.listener(move |this, _, _window, cx| {
                // Clear local focus
                this.workspace.update(cx, |ws, cx| {
                    ws.set_focused_project(None, cx);
                });
                // Set remote focus
                if let Some(ref rm) = this.remote_manager {
                    rm.update(cx, |rm, cx| {
                        rm.set_focused_remote(Some((conn_id.clone(), proj_id.clone())), cx);
                    });
                }
            }))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(t.text_primary))
                    .child(name),
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
