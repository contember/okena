use crate::theme::theme;
use gpui::*;

use super::components::*;
use super::SettingsPanel;

impl SettingsPanel {
    pub(super) fn render_paired_devices(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let content = div();

        if self.auth_store.is_none() {
            return content
                .child(section_header("Paired Devices", &t))
                .child(
                    section_container(&t).child(
                        div()
                            .px(px(12.0))
                            .py(px(16.0))
                            .text_size(px(13.0))
                            .text_color(rgb(t.text_muted))
                            .child("Remote server is not running"),
                    ),
                );
        }

        if self.paired_devices.is_empty() {
            return content
                .child(section_header("Paired Devices", &t))
                .child(
                    section_container(&t).child(
                        div()
                            .px(px(12.0))
                            .py(px(16.0))
                            .text_size(px(13.0))
                            .text_color(rgb(t.text_muted))
                            .child("No devices are currently paired"),
                    ),
                );
        }

        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let device_count = self.paired_devices.len();
        let items: Vec<_> = self
            .paired_devices
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

                let row = div()
                    .id(ElementId::Name(format!("device-{}", i).into()))
                    .px(px(12.0))
                    .py(px(8.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(rgb(t.text_primary))
                                    .child(display_name),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(rgb(t.text_muted))
                                    .child(format!(
                                        "Created {} \u{2022} Last used {} \u{2022} Expires in {}",
                                        created, last_used, expires,
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .id(ElementId::Name(format!("revoke-{}", i).into()))
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .text_size(px(11.0))
                            .text_color(rgb(t.text_secondary))
                            .hover(|s| s.bg(rgb(t.bg_hover)).text_color(rgb(0xE06C75)))
                            .child("Revoke")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    if let Some(ref store) = this.auth_store {
                                        store.revoke_token(&id_str);
                                        this.paired_devices = store.list_tokens();
                                        cx.notify();
                                    }
                                }),
                            ),
                    );

                if is_last {
                    row.into_any_element()
                } else {
                    row.border_b_1()
                        .border_color(rgb(t.border))
                        .into_any_element()
                }
            })
            .collect();

        content
            .child(section_header("Paired Devices", &t))
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
