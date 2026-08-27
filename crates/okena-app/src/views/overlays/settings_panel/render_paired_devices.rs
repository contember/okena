use crate::theme::theme;
use crate::ui::tokens::ui_text_ms;
use gpui::*;
use okena_ui::empty_state::empty_state;

use super::components::*;
use super::{PairedDevices, SettingsPanel};

impl SettingsPanel {
    pub(super) fn render_paired_devices(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let content = div();

        let devices =
            match &self.paired_devices {
                PairedDevices::Unavailable => {
                    return content
                        .child(section_header("Paired Devices", &t, cx))
                        .child(section_container(&t).child(
                            empty_state("Local daemon is not connected", &t, cx).py(px(16.0)),
                        ));
                }
                PairedDevices::Loading => {
                    return content
                        .child(section_header("Paired Devices", &t, cx))
                        .child(
                            section_container(&t)
                                .child(empty_state("Loading devices...", &t, cx).py(px(16.0))),
                        );
                }
                PairedDevices::Failed(error) => {
                    return content
                        .child(section_header("Paired Devices", &t, cx))
                        .child(
                            section_container(&t).child(
                                empty_state(error.clone(), &t, cx)
                                    .py(px(16.0))
                                    .text_color(rgb(t.term_red)),
                            ),
                        );
                }
                PairedDevices::Loaded(devices) => devices,
            };

        if devices.is_empty() {
            return content
                .child(section_header("Paired Devices", &t, cx))
                .child(
                    section_container(&t)
                        .child(empty_state("No devices are currently paired", &t, cx).py(px(16.0))),
                );
        }

        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let device_count = devices.len();
        let items: Vec<_> = devices
            .iter()
            .enumerate()
            .map(|(i, info)| {
                let is_last = i == device_count - 1;
                let id_str = info.id.clone();
                let display_name = info
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("Device {}", &info.id[..8.min(info.id.len())]));

                let created = format_relative_time(now_secs, info.created_at);
                let last_used = format_relative_time(now_secs, info.last_used_at);
                let expires = if info.expires_at > now_secs {
                    format_duration(info.expires_at - now_secs)
                } else {
                    "expired".to_string()
                };

                let row = settings_row_with_desc(
                    format!("device-{}", i),
                    &display_name,
                    &format!(
                        "Created {} \u{2022} Last used {} \u{2022} Expires in {}",
                        created, last_used, expires,
                    ),
                    &t,
                    cx,
                    !is_last,
                )
                .child(
                    div()
                        .id(ElementId::Name(format!("revoke-{}", i).into()))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_secondary))
                        .hover(|s| s.bg(rgb(t.bg_hover)).text_color(rgb(0xE06C75)))
                        .child("Revoke")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.revoke_paired_device(id_str.clone(), cx);
                            }),
                        ),
                );

                row.into_any_element()
            })
            .collect();

        content
            .child(section_header("Paired Devices", &t, cx))
            .child(section_container(&t).children(items))
    }
}

fn format_relative_time(now_secs: u64, timestamp: u64) -> String {
    if timestamp > now_secs {
        return "just now".to_string();
    }
    let diff = now_secs - timestamp;
    if diff < 60 {
        "just now".to_string()
    } else if diff < 3600 {
        let mins = diff / 60;
        format!("{}m ago", mins)
    } else if diff < 86400 {
        let hours = diff / 3600;
        format!("{}h ago", hours)
    } else {
        let days = diff / 86400;
        format!("{}d ago", days)
    }
}

fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}
