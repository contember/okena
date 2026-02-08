use crate::theme::theme;
use gpui::*;
use gpui::prelude::*;

use super::categories::SettingsCategory;
use super::SettingsPanel;

impl SettingsPanel {
    pub(super) fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let categories = if self.selected_project_id.is_some() {
            SettingsCategory::project_categories()
        } else {
            SettingsCategory::all()
        };

        div()
            .id("settings-sidebar")
            .w(px(120.0))
            .flex_shrink_0()
            .border_r_1()
            .border_color(rgb(t.border))
            .py(px(8.0))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .children(categories.iter().map(|cat| {
                let is_active = *cat == self.active_category;
                let category = *cat;

                div()
                    .id(ElementId::Name(format!("sidebar-{}", cat.label()).into()))
                    .cursor_pointer()
                    .mx(px(6.0))
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(4.0))
                    .text_size(px(12.0))
                    .when(is_active, |d| {
                        d.bg(rgb(t.bg_secondary))
                            .text_color(rgb(t.text_primary))
                            .font_weight(FontWeight::MEDIUM)
                    })
                    .when(!is_active, |d| {
                        d.text_color(rgb(t.text_secondary))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                    })
                    .child(cat.label().to_string())
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        this.active_category = category;
                        this.close_all_dropdowns();
                        cx.notify();
                    }))
            }))
    }
}
