use crate::settings::settings_entity;
use crate::theme::theme;
use crate::ui::tokens::{ui_text, ui_text_sm, ui_text_md};
use crate::views::components::simple_input::SimpleInput;
use gpui::*;
use gpui::prelude::*;
use gpui_component::v_flex;

use super::components::*;
use super::SettingsPanel;

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
                                        .child("IP address to bind the remote server (e.g. 0.0.0.0 for all interfaces)"),
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
    }
}
