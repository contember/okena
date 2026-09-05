use crate::theme::{theme, with_alpha};
use crate::ui::tokens::ui_text_md;
use gpui::prelude::*;
use gpui::*;
use okena_extensions::ExtensionRegistry;

use super::SettingsPanel;
use super::categories::SettingsCategory;

impl SettingsPanel {
    pub(super) fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let static_categories: Vec<SettingsCategory> = if self.selected_project_id.is_some() {
            SettingsCategory::project_categories().to_vec()
        } else {
            SettingsCategory::all().to_vec()
        };

        // Collect extension categories (extensions with settings_view that are enabled)
        let ext_categories: Vec<(SettingsCategory, String)> = if self.selected_project_id.is_none()
        {
            cx.try_global::<ExtensionRegistry>()
                .map(|registry| {
                    let settings = crate::settings::settings_entity(cx)
                        .read(cx)
                        .settings
                        .clone();
                    registry
                        .extensions()
                        .iter()
                        .filter(|ext| {
                            ext.settings_view.is_some()
                                && settings.enabled_extensions.contains(ext.manifest.id)
                        })
                        .map(|ext| {
                            (
                                SettingsCategory::Extension(ext.manifest.id.to_string()),
                                ext.manifest.name.to_string(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        div()
            .id("settings-sidebar")
            .w(px(148.0))
            .flex_shrink_0()
            .bg(with_alpha(t.bg_secondary, 0.4))
            .border_r_1()
            .border_color(rgb(t.border))
            .py(px(10.0))
            .flex()
            .flex_col()
            .gap(px(1.0))
            // Static categories
            .children(static_categories.iter().map(|cat| {
                let is_active = *cat == self.active_category;
                let category = cat.clone();
                let label = cat.label().to_string();

                Self::render_sidebar_item(
                    &label,
                    is_active,
                    &t,
                    cx.listener(move |this, _, _, cx| {
                        this.set_category(category.clone(), cx);
                    }),
                    cx,
                )
            }))
            // Extension categories
            .children(ext_categories.into_iter().map(|(cat, name)| {
                let is_active = self.active_category == cat;
                let category = cat;

                Self::render_sidebar_item(
                    &name,
                    is_active,
                    &t,
                    cx.listener(move |this, _, _, cx| {
                        this.set_category(category.clone(), cx);
                    }),
                    cx,
                )
            }))
    }

    fn render_sidebar_item<T: Fn(&MouseDownEvent, &mut Window, &mut App) + 'static>(
        label: &str,
        is_active: bool,
        t: &okena_core::theme::ThemeColors,
        on_click: T,
        cx: &App,
    ) -> impl IntoElement + use<T> {
        div()
            .id(ElementId::Name(format!("sidebar-{}", label).into()))
            .cursor_pointer()
            .mx(px(8.0))
            .px(px(10.0))
            .py(px(6.0))
            .rounded(px(5.0))
            .flex()
            .items_center()
            .text_size(ui_text_md(cx))
            .when(is_active, |d| {
                d.bg(with_alpha(t.border_active, 0.18))
                    .text_color(rgb(t.text_primary))
                    .font_weight(FontWeight::MEDIUM)
            })
            .when(!is_active, |d| {
                d.text_color(rgb(t.text_secondary))
                    .hover(|s| s.bg(rgb(t.bg_hover)).text_color(rgb(t.text_primary)))
            })
            .child(label.to_string())
            .on_mouse_down(MouseButton::Left, on_click)
    }
}
