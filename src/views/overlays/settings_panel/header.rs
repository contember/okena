use crate::settings::open_settings_file;
use crate::theme::theme;
use crate::views::components::{dropdown_button, dropdown_option, dropdown_overlay};
use gpui::*;

use super::SettingsPanel;

impl SettingsPanel {
    pub(super) fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        div()
            .px(px(16.0))
            .py(px(10.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .flex()
            .items_center()
            .justify_between()
            .child(
                // Left: Project selector
                self.render_project_selector(cx),
            )
            .child(
                // Right: Edit in settings.json button
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .id("edit-settings-file-btn")
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .bg(rgb(t.bg_secondary))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .text_size(px(11.0))
                            .text_color(rgb(t.text_secondary))
                            .child("Edit in settings.json")
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                open_settings_file();
                                this.close(cx);
                            })),
                    )
                    .child(
                        div()
                            .id("settings-close-btn")
                            .cursor_pointer()
                            .w(px(24.0))
                            .h(px(24.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .text_size(px(14.0))
                            .text_color(rgb(t.text_muted))
                            .child("\u{2715}")
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                this.close(cx);
                            })),
                    ),
            )
    }

    pub(super) fn render_project_selector(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let label = match &self.selected_project_id {
            None => "User".to_string(),
            Some(pid) => {
                self.workspace.read(cx).project(pid)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "Unknown".to_string())
            }
        };

        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(
                div()
                    .text_size(px(14.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_primary))
                    .child("Settings"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(t.text_muted))
                    .child("\u{2014}"),
            )
            .child(
                dropdown_button("project-selector-btn", &label, self.project_dropdown_open, &t)
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        this.project_dropdown_open = !this.project_dropdown_open;
                        this.font_dropdown_open = false;
                        this.shell_dropdown_open = false;
                        this.session_backend_dropdown_open = false;
                        cx.notify();
                    })),
            )
    }

    pub(super) fn render_project_dropdown_overlay(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let projects: Vec<(String, String)> = self.workspace.read(cx).projects()
            .iter()
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect();

        let is_user_selected = self.selected_project_id.is_none();

        dropdown_overlay("project-selector-dropdown", 44.0, 32.0, &t)
            .left(px(16.0))
            .right_auto()
            .min_w(px(180.0))
            .max_h(px(250.0))
            .child(
                dropdown_option("project-opt-user", "User (Global)", is_user_selected, &t)
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        this.select_project(None, cx);
                    }))
            )
            .children(projects.into_iter().map(|(id, name)| {
                let is_selected = self.selected_project_id.as_deref() == Some(&id);
                dropdown_option(format!("project-opt-{}", id), &name, is_selected, &t)
                    .on_mouse_down(MouseButton::Left, cx.listener({
                        let id = id.clone();
                        move |this, _, _, cx| {
                            this.select_project(Some(id.clone()), cx);
                        }
                    }))
            }))
    }
}
