use crate::theme::theme;
use crate::workspace::persistence::get_settings_path;
use gpui::*;

use super::SettingsPanel;

impl SettingsPanel {
    pub(super) fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let config_path = get_settings_path();

        div()
            .px(px(16.0))
            .py(px(8.0))
            .border_t_1()
            .border_color(rgb(t.border))
            .child(
                div()
                    .text_size(px(10.0))
                    .font_family("monospace")
                    .text_color(rgb(t.text_muted))
                    .child(format!("Config: {}", config_path.display())),
            )
    }
}
