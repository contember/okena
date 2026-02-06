use crate::settings::settings_entity;
use crate::theme::theme;
use gpui::*;

use super::components::*;
use super::SettingsPanel;

impl SettingsPanel {
    pub(super) fn render_font(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let s = settings_entity(cx).read(cx).settings.clone();

        div()
            .child(section_header("Font", &t))
            .child(
                section_container(&t)
                    .child(self.render_number_stepper(
                        "font-size", "Font Size", s.font_size, "{}", 1.0, 50.0, true,
                        |state, val, cx| state.set_font_size(val, cx), cx,
                    ))
                    .child(self.render_font_dropdown_row(&s.font_family, cx))
                    .child(self.render_number_stepper(
                        "line-height", "Line Height", s.line_height, "{}", 0.1, 50.0, true,
                        |state, val, cx| state.set_line_height(val, cx), cx,
                    ))
                    .child(self.render_number_stepper(
                        "ui-font-size", "UI Font Size", s.ui_font_size, "{}", 1.0, 50.0, true,
                        |state, val, cx| state.set_ui_font_size(val, cx), cx,
                    ))
                    .child(self.render_number_stepper(
                        "file-font-size", "File Font Size", s.file_font_size, "{}", 1.0, 50.0, false,
                        |state, val, cx| state.set_file_font_size(val, cx), cx,
                    )),
            )
    }
}
