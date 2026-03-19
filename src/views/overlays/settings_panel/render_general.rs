use crate::settings::settings_entity;
use crate::theme::theme;
use crate::views::components::simple_input::SimpleInput;
use gpui::*;
use gpui::prelude::*;
use gpui_component::v_flex;
use okena_extensions::ExtensionRegistry;

use super::components::*;
use super::SettingsPanel;

impl SettingsPanel {
    pub(super) fn render_general(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let s = settings_entity(cx).read(cx).settings.clone();

        let mut section = section_container(&t)
            .child(self.render_toggle(
                "focus-border", "Show Focus Border", s.show_focused_border, true,
                |state, val, cx| state.set_show_focused_border(val, cx), cx,
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
                                        .text_size(px(13.0))
                                        .text_color(rgb(t.text_primary))
                                        .child("Listen Address"),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(rgb(t.text_muted))
                                        .child("IP address to bind the remote server (e.g. 0.0.0.0 for all interfaces)"),
                                ),
                        )
                        .child(
                            div()
                                .bg(rgb(t.bg_secondary))
                                .border_1()
                                .border_color(rgb(t.border))
                                .rounded(px(4.0))
                                .child(SimpleInput::new(&self.listen_address_input).text_size(px(12.0))),
                        ),
                )
            });

        // Dynamic extension toggles from the registry.
        // Collect metadata first to avoid holding an immutable borrow on cx while calling render_toggle.
        let ext_infos: Vec<(String, String)> = cx.try_global::<ExtensionRegistry>()
            .map(|registry| {
                registry.extensions().iter().map(|ext| {
                    (ext.manifest.id.to_string(), ext.manifest.name.to_string())
                }).collect()
            })
            .unwrap_or_default();

        for (ext_id, ext_name) in ext_infos {
            let enabled = s.enabled_extensions.contains(&ext_id);
            let toggle_id = format!("ext-{}", ext_id);
            let label = format!("{} Status", ext_name);
            let ext_id_for_closure = ext_id.clone();
            section = section.child(self.render_toggle(
                &toggle_id, &label, enabled, true,
                move |state, val, cx| state.set_extension_enabled(&ext_id_for_closure, val, cx), cx,
            ));
        }

        section = section.child(self.render_number_stepper(
            "min-col-width", "Min Column Width", s.min_column_width,
            "{}px", 50.0, 60.0, false,
            |state, val, cx| state.set_min_column_width(val, cx), cx,
        ));

        div()
            .child(section_header("Appearance", &t))
            .child(section)
            .child(section_header("File Opener", &t))
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
                                            .text_size(px(13.0))
                                            .text_color(rgb(t.text_primary))
                                            .child("Editor Command"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.0))
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
                                    .child(SimpleInput::new(&self.file_opener_input).text_size(px(12.0))),
                            ),
                    ),
            )
    }
}
