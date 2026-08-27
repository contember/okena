use crate::theme::{theme, with_alpha};
use crate::ui::tokens::ui_text_sm;
use crate::workspace::persistence::get_settings_path;
use gpui::*;

use super::SettingsPanel;

impl SettingsPanel {
    pub(super) fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let config_path = get_settings_path();

        div()
            .flex_shrink_0()
            .px(px(16.0))
            .py(px(7.0))
            .bg(with_alpha(t.bg_secondary, 0.4))
            .border_t_1()
            .border_color(rgb(t.border))
            .child(
                div()
                    .text_size(ui_text_sm(cx))
                    .font_family("monospace")
                    .text_color(rgb(t.text_muted))
                    .child(format!("Config: {}", config_path.display())),
            )
    }
}
