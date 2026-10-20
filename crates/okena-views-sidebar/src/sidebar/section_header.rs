//! Section headers for the Separate remote layout:
//!
//! - `LOCAL` — label, the local machine's load (when enabled) and a `+` for a
//!   new local project;
//! - `REMOTE` — label only; connections are added from the button at the
//!   bottom of the sidebar, as in the mixed layout;
//! - one row per remote host — status dot, name, its load or its status when
//!   not connected, and a `+` for a new project on that host. Right-click
//!   opens the connection menu. Endpoint and exact memory figures are in
//!   the tooltip.

use gpui::prelude::FluentBuilder;
use gpui::*;
use okena_transport::client::ConnectionStatus;
use okena_ui::theme::theme;
use okena_ui::tokens::{ui_text_md, ui_text_ms, ui_text_sm, ui_text_xl};
use okena_workspace::requests::OverlayRequest;

use super::Sidebar;
use super::sections::{
    SectionKind, SidebarSectionInfo, format_host_stats, host_stats_widget, status_label,
};

impl Sidebar {
    pub(crate) fn render_section_header(
        &self,
        section: &SidebarSectionInfo,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match &section.kind {
            SectionKind::Local => self.render_local_heading(section, cx).into_any_element(),
            SectionKind::RemoteGroup => self.render_remote_heading(cx).into_any_element(),
            SectionKind::Host(conn_id) => self
                .render_host_row(conn_id, section, cx)
                .into_any_element(),
        }
    }

    fn heading_label(&self, text: &'static str, cx: &mut Context<Self>) -> Div {
        let t = theme(cx);
        div()
            .text_size(ui_text_ms(cx))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(t.text_secondary))
            .whitespace_nowrap()
            .child(text)
    }

    /// `+` that opens the New project dialog with `target_connection_id`
    /// preselected (`None` = local).
    fn add_project_button(
        &self,
        id: String,
        target_connection_id: Option<String>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let t = theme(cx);
        div()
            .id(ElementId::Name(id.into()))
            .flex_shrink_0()
            .w(px(20.0))
            .h(px(20.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .text_size(ui_text_xl(cx))
            .text_color(rgb(t.text_secondary))
            .child("+")
            .on_click(cx.listener(move |this, _, _window, cx| {
                let target_connection_id = target_connection_id.clone();
                this.request_broker.update(cx, |broker, cx| {
                    broker.push_overlay_request(
                        OverlayRequest::AddProjectDialog {
                            target_connection_id,
                        },
                        cx,
                    );
                });
                cx.stop_propagation();
            }))
    }

    fn render_local_heading(
        &self,
        section: &SidebarSectionInfo,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let stats = section
            .stats
            .as_ref()
            .and_then(|stats| host_stats_widget(section.stats_style, stats, &t, cx));
        let tooltip = section.stats.as_ref().map(format_host_stats);

        div()
            .id("host-section-local")
            .h(px(28.0))
            .px(px(12.0))
            .mt(px(8.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .when_some(tooltip, |d, tooltip| {
                d.tooltip(move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                })
            })
            .child(self.heading_label("LOCAL", cx))
            .child(div().flex_1())
            .children(stats)
            .child(self.add_project_button("host-section-add-local".to_string(), None, cx))
    }

    fn render_remote_heading(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(28.0))
            .px(px(12.0))
            .mt(px(12.0))
            .flex()
            .items_center()
            .child(self.heading_label("REMOTE", cx))
    }

    fn render_host_row(
        &self,
        conn_id: &str,
        section: &SidebarSectionInfo,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let connected = matches!(section.status, Some(ConnectionStatus::Connected));
        let status_color = match &section.status {
            Some(ConnectionStatus::Connected) => t.term_green,
            Some(
                ConnectionStatus::Connecting
                | ConnectionStatus::Pairing
                | ConnectionStatus::Reconnecting { .. },
            ) => t.term_yellow,
            Some(ConnectionStatus::Error(_)) => t.term_red,
            Some(ConnectionStatus::Disconnected) | None => t.text_muted,
        };
        // Connected: the load (when enabled). Otherwise: what it is doing.
        let stats = if connected {
            section
                .stats
                .as_ref()
                .and_then(|stats| host_stats_widget(section.stats_style, stats, &t, cx))
        } else {
            None
        };
        let status_text = (!connected)
            .then(|| section.status.as_ref().map(status_label))
            .flatten();
        let tooltip = match (
            &section.endpoint,
            section.stats.as_ref().filter(|_| connected),
        ) {
            (Some(endpoint), Some(stats)) => format!("{endpoint}\n{}", format_host_stats(stats)),
            (Some(endpoint), None) => endpoint.clone(),
            (None, Some(stats)) => format_host_stats(stats),
            (None, None) => String::new(),
        };
        let is_pairing = matches!(
            section.status,
            Some(
                ConnectionStatus::Pairing
                    | ConnectionStatus::Error(_)
                    | ConnectionStatus::Disconnected
            )
        );
        let menu_conn_id = conn_id.to_string();
        let menu_conn_name = section.name.clone();
        let tls = section.tls;

        div()
            .id(ElementId::Name(format!("host-section-{conn_id}").into()))
            .h(px(24.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .min_w_0()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .when(!tooltip.is_empty(), |d| {
                d.tooltip(move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                })
            })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    this.request_broker.update(cx, |broker, cx| {
                        broker.push_overlay_request(
                            OverlayRequest::RemoteConnectionContextMenu {
                                connection_id: menu_conn_id.clone(),
                                connection_name: menu_conn_name.clone(),
                                is_pairing,
                                tls,
                                position: event.position,
                            },
                            cx,
                        );
                    });
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .size(px(7.0))
                    .rounded_full()
                    .bg(rgb(status_color))
                    .flex_shrink_0(),
            )
            .child(
                // The name takes the slack and truncates first, so the stats
                // and the `+` never fall off a narrow sidebar.
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(ui_text_md(cx))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(if connected {
                        t.text_primary
                    } else {
                        t.text_muted
                    }))
                    .child(section.name.clone()),
            )
            .children(stats)
            .children(status_text.map(|text| {
                div()
                    .flex_shrink_0()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(text)
            }))
            .child(self.add_project_button(
                format!("host-section-add-{conn_id}"),
                Some(conn_id.to_string()),
                cx,
            ))
    }
}
