use crate::settings::settings_entity;
use crate::theme::theme;
use gpui::*;
use okena_extensions::ExtensionRegistry;

use super::components::*;
use super::SettingsPanel;

impl SettingsPanel {
    pub(super) fn render_extensions(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let s = settings_entity(cx).read(cx).settings.clone();

        let ext_infos: Vec<(String, String)> = cx
            .try_global::<ExtensionRegistry>()
            .map(|registry| {
                registry
                    .extensions()
                    .iter()
                    .map(|ext| (ext.manifest.id.to_string(), ext.manifest.name.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let mut section = section_container(&t);

        for (i, (ext_id, ext_name)) in ext_infos.iter().enumerate() {
            let enabled = s.enabled_extensions.contains(ext_id);
            let toggle_id = format!("ext-{}", ext_id);
            let has_border = i + 1 < ext_infos.len();
            let ext_id_for_closure = ext_id.clone();
            section = section.child(self.render_toggle(
                &toggle_id,
                ext_name,
                enabled,
                has_border,
                move |state, val, cx| state.set_extension_enabled(&ext_id_for_closure, val, cx),
                cx,
            ));
        }

        div()
            .child(section_header("Extensions", &t, cx))
            .child(section)
    }
}
