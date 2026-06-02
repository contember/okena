use crate::remote::GlobalRemoteInfo;
use crate::settings::settings_entity;
use crate::theme::theme;
use crate::ui::tokens::{ui_text, ui_text_sm, ui_text_md};
use crate::views::components::simple_input::SimpleInput;
use crate::workspace::settings::HeaderDensity;
use gpui::*;
use gpui::prelude::*;
use gpui_component::{h_flex, v_flex};

use super::components::*;
use super::SettingsPanel;

/// Format a 64-char hex fingerprint as colon-separated byte pairs for readable
/// out-of-band comparison (e.g. `ab:cd:ef:…`).
fn format_fingerprint(fp: &str) -> String {
    fp.as_bytes()
        .chunks(2)
        .map(|pair| std::str::from_utf8(pair).unwrap_or("??"))
        .collect::<Vec<_>>()
        .join(":")
}

impl SettingsPanel {
    pub(super) fn render_general(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let s = settings_entity(cx).read(cx).settings.clone();

        let section = section_container(&t)
            .child(self.render_toggle(
                "focus-border", "Show Focus Border", s.show_focused_border, true,
                |state, val, cx| state.set_show_focused_border(val, cx), cx,
            ))
            .child(self.render_toggle(
                "color-tinted-bg", "Color Tinted Background", s.color_tinted_background, true,
                |state, val, cx| state.set_color_tinted_background(val, cx), cx,
            ))
            .child(self.render_header_density_row(s.header_density, cx))
            .child(self.render_toggle(
                "detached-by-default", "Detached Overlays by Default", s.detached_overlays_by_default, true,
                |state, val, cx| state.set_detached_overlays_by_default(val, cx), cx,
            ))
            .child(self.render_toggle(
                "remote-server", "Remote Server", s.remote_server_enabled, true,
                |state, val, cx| state.set_remote_server_enabled(val, cx), cx,
            ))
            .when(s.remote_server_enabled, |d| {
                d.child(
                    div()
                        .px(px(12.0))
                        .py(px(8.0))
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(
                            v_flex()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .text_size(ui_text(13.0, cx))
                                        .text_color(rgb(t.text_primary))
                                        .child("Listen Address"),
                                )
                                .child(
                                    div()
                                        .text_size(ui_text_sm(cx))
                                        .text_color(rgb(t.text_muted))
                                        .child("IP address to bind the remote server. Binding beyond 127.0.0.1 exposes it UNENCRYPTED on the network (token + terminal I/O in cleartext) — only use on a trusted network or behind an SSH/WireGuard tunnel."),
                                ),
                        )
                        .child(
                            div()
                                .bg(rgb(t.bg_secondary))
                                .border_1()
                                .border_color(rgb(t.border))
                                .rounded(px(4.0))
                                .child(SimpleInput::new(&self.listen_address_input).text_size(ui_text_md(cx))),
                        ),
                )
                .child(self.render_toggle(
                    "remote-tls", "Encrypt with TLS", s.remote_tls_enabled, true,
                    |state, val, cx| state.set_remote_tls_enabled(val, cx), cx,
                ))
                .when(s.remote_tls_enabled, |d| {
                    // Show the server cert fingerprint so the user can verify it
                    // against the value the client pinned during pairing.
                    let fingerprint = cx
                        .try_global::<GlobalRemoteInfo>()
                        .and_then(|info| info.0.cert_fingerprint());
                    d.child(
                        div()
                            .px(px(12.0))
                            .py(px(8.0))
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_size(ui_text(13.0, cx))
                                    .text_color(rgb(t.text_primary))
                                    .child("Certificate fingerprint (SHA-256)"),
                            )
                            .child(
                                div()
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child("When pairing a new device, verify this matches the fingerprint shown on the client before trusting it."),
                            )
                            .child(
                                div()
                                    .bg(rgb(t.bg_secondary))
                                    .border_1()
                                    .border_color(rgb(t.border))
                                    .rounded(px(4.0))
                                    .px(px(8.0))
                                    .py(px(6.0))
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(t.text_primary))
                                    .child(match fingerprint {
                                        Some(fp) => format_fingerprint(&fp),
                                        None => "(server not running — start it to view)".to_string(),
                                    }),
                            ),
                    )
                })
            })
            .child(self.render_number_stepper(
                "min-col-width", "Min Column Width", s.min_column_width,
                "{}px", 50.0, 60.0, false,
                |state, val, cx| state.set_min_column_width(val, cx), cx,
            ));

        div()
            .child(section_header("Appearance", &t, cx))
            .child(section)
            .child(section_header("File Opener", &t, cx))
            .child(
                section_container(&t)
                    .child(
                        div()
                            .px(px(12.0))
                            .py(px(8.0))
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(
                                v_flex()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(ui_text(13.0, cx))
                                            .text_color(rgb(t.text_primary))
                                            .child("Editor Command"),
                                    )
                                    .child(
                                        div()
                                            .text_size(ui_text_sm(cx))
                                            .text_color(rgb(t.text_muted))
                                            .child("Command to open file paths (empty = system default)"),
                                    ),
                            )
                            .child(
                                div()
                                    .bg(rgb(t.bg_secondary))
                                    .border_1()
                                    .border_color(rgb(t.border))
                                    .rounded(px(4.0))
                                    .child(SimpleInput::new(&self.file_opener_input).text_size(ui_text_md(cx))),
                            ),
                    ),
            )
            .child(section_header("Notifications", &t, cx))
            .child({
                let n = s.notifications.clone();
                section_container(&t)
                    .child(self.render_toggle(
                        "desktop-notifications",
                        "Desktop Notifications",
                        n.enabled,
                        // Border only when the sub-toggles follow below.
                        n.enabled,
                        |state, val, cx| state.set_notifications_enabled(val, cx),
                        cx,
                    ))
                    .when(n.enabled, |d| {
                        d.child(self.render_toggle(
                            "notify-osc",
                            "Terminal Alerts (OSC 9 / 777)",
                            n.osc,
                            true,
                            |state, val, cx| state.set_notifications_osc(val, cx),
                            cx,
                        ))
                        .child(self.render_toggle(
                            "notify-bell",
                            "Terminal Bell",
                            n.bell,
                            false,
                            |state, val, cx| state.set_notifications_bell(val, cx),
                            cx,
                        ))
                    })
            })
    }

    fn render_header_density_row(
        &self,
        current: HeaderDensity,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);

        settings_row(
            "header-density".to_string(),
            "Project Header Density",
            &t,
            cx,
            true,
        )
        .child(
            h_flex()
                .gap(px(2.0))
                .rounded(px(4.0))
                .bg(rgb(t.bg_secondary))
                .p(px(2.0))
                .children(HeaderDensity::all_variants().iter().map(|&density: &HeaderDensity| {
                    let is_selected = density == current;
                    let hover_bg = t.bg_hover;
                    div()
                        .id(ElementId::Name(
                            format!("header-density-{:?}", density).into(),
                        ))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(3.0))
                        .text_size(ui_text_md(cx))
                        .when(is_selected, |el: Stateful<Div>| {
                            el.bg(rgb(t.border_active))
                                .text_color(rgb(t.text_primary))
                        })
                        .when(!is_selected, |el: Stateful<Div>| {
                            el.text_color(rgb(t.text_muted))
                                .hover(|s: StyleRefinement| s.bg(rgb(hover_bg)))
                        })
                        .child(density.display_name().to_string())
                        .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, _, cx| {
                            settings_entity(cx).update(cx, |state, cx| {
                                state.set_header_density(density, cx);
                            });
                        }))
                })),
        )
    }
}
