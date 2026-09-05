use crate::settings::settings_entity;
use crate::theme::theme;
use crate::ui::tokens::{ui_text_md, ui_text_sm};
use crate::views::components::simple_input::SimpleInput;
use crate::workspace::settings::HeaderDensity;
use gpui::prelude::*;
use gpui::*;
use okena_transport::client::tls::format_fingerprint;

use super::SettingsPanel;
use super::components::*;

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
                    settings_input_row(
                        "remote-listen-address",
                        "Listen Address",
                        "IP address to bind the remote server. Binding beyond 127.0.0.1 exposes it UNENCRYPTED on the network (token + terminal I/O in cleartext) — only use on a trusted network or behind an SSH/WireGuard tunnel.",
                        &t,
                        cx,
                        true,
                    )
                    .child(input_box(&t).child(
                        SimpleInput::new(&self.listen_address_input).text_size(ui_text_md(cx)),
                    )),
                )
                .child(self.render_toggle(
                    "remote-tls", "Encrypt with TLS", s.remote_tls_enabled, true,
                    |state, val, cx| state.set_remote_tls_enabled(val, cx), cx,
                ))
                .when(s.remote_tls_enabled, |d| {
                    // Show the server cert fingerprint so the user can verify it
                    // against the value the client pinned during pairing. Read
                    // from disk: the server runs in the daemon process.
                    let fingerprint = crate::remote::tls::read_fingerprint(
                        &crate::workspace::persistence::config_dir(),
                    );
                    d.child(
                        settings_input_row(
                            "remote-tls-fingerprint",
                            "Certificate fingerprint (SHA-256)",
                            "When pairing a new device, verify this matches the fingerprint shown on the client before trusting it.",
                            &t,
                            cx,
                            true,
                        )
                        .child(
                            input_box(&t)
                                .font_family("monospace")
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
                section_container(&t).child(
                    settings_input_row(
                        "file-opener-command",
                        "Editor Command",
                        "Command to open file paths (empty = system default)",
                        &t,
                        cx,
                        false,
                    )
                    .child(
                        input_box(&t).child(
                            SimpleInput::new(&self.file_opener_input).text_size(ui_text_md(cx)),
                        ),
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
            .child(section_header("Clipboard", &t, cx))
            .child(section_container(&t).child(self.render_toggle(
                "clipboard-read",
                "Allow Clipboard Read (OSC 52)",
                s.allow_clipboard_read,
                false,
                |state, val, cx| state.set_allow_clipboard_read(val, cx),
                cx,
            )))
    }

    fn render_header_density_row(
        &self,
        current: HeaderDensity,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let variants = HeaderDensity::all_variants();
        let segments: Vec<Segment<'_>> = variants
            .iter()
            .map(|density| Segment {
                id: format!("{:?}", density).into(),
                label: density.display_name(),
                selected: *density == current,
            })
            .collect();

        settings_row(
            "header-density".to_string(),
            "Project Header Density",
            &t,
            cx,
            true,
        )
        .child(segmented_control(
            "header-density",
            &segments,
            &t,
            cx,
            move |i, _, cx| {
                let density = variants[i];
                settings_entity(cx).update(cx, |state, cx| {
                    state.set_header_density(density, cx);
                });
            },
        ))
    }
}
