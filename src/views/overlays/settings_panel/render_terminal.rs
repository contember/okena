use crate::settings::settings_entity;
use crate::theme::theme;
use gpui::*;

use super::components::*;
use super::SettingsPanel;

impl SettingsPanel {
    pub(super) fn render_terminal(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let s = settings_entity(cx).read(cx).settings.clone();

        div()
            .child(section_header("Terminal", &t))
            .child(
                section_container(&t)
                    .child(self.render_shell_dropdown_row(&s.default_shell, cx))
                    .child(self.render_session_backend_dropdown_row(&s.session_backend, cx))
                    .child(self.render_toggle(
                        "show-shell-selector", "Show Shell Selector", s.show_shell_selector, true,
                        |state, val, cx| state.set_show_shell_selector(val, cx), cx,
                    ))
                    .child(self.render_toggle(
                        "cursor-blink", "Cursor Blink", s.cursor_blink, true,
                        |state, val, cx| state.set_cursor_blink(val, cx), cx,
                    ))
                    .child(self.render_integer_stepper(
                        "scrollback", "Scrollback Lines", s.scrollback_lines, 1000, 70.0, false,
                        |state, val, cx| state.set_scrollback_lines(val, cx), cx,
                    )),
            )
    }
}
