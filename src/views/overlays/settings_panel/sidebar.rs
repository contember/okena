use crate::theme::theme;
use gpui::*;
use gpui::prelude::*;
use okena_extensions::ExtensionRegistry;

use super::categories::SettingsCategory;
use super::SettingsPanel;

impl SettingsPanel {
    pub(super) fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let static_categories: Vec<SettingsCategory> = if self.selected_project_id.is_some() {
            SettingsCategory::project_categories().to_vec()
        } else {
            SettingsCategory::all().to_vec()
        };

        // Collect extension categories (extensions with settings_view that are enabled)
        let ext_categories: Vec<(SettingsCategory, String)> = if self.selected_project_id.is_none() {
            cx.try_global::<ExtensionRegistry>()
                .map(|registry| {
                    let settings = crate::settings::settings_entity(cx).read(cx).settings.clone();
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
            .w(px(120.0))
            .flex_shrink_0()
            .border_r_1()
            .border_color(rgb(t.border))
            .py(px(8.0))
            .flex()
            .flex_col()
            .gap(px(2.0))
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
                        this.active_category = category.clone();
                        this.close_all_dropdowns();
                        cx.notify();
                    }),
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
                        this.active_category = category.clone();
                        this.close_all_dropdowns();
                        cx.notify();
                    }),
                )
            }))
    }

    fn render_sidebar_item<T: Fn(&MouseDownEvent, &mut Window, &mut App) + 'static>(
        label: &str,
        is_active: bool,
        t: &okena_core::theme::ThemeColors,
        on_click: T,
    ) -> impl IntoElement + use<T> {
        div()
            .id(ElementId::Name(format!("sidebar-{}", label).into()))
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
            .child(label.to_string())
            .on_mouse_down(MouseButton::Left, on_click)
    }
}
