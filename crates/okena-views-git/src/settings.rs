//! Git view settings — read/written through ExtensionSettingsStore.
//!
//! The host app registers getter/setter callbacks that map these fields
//! to/from persistent AppSettings. Crate views use `git_settings(cx)` and
//! `set_git_settings()` without knowing about the app's settings system.

use okena_core::types::DiffViewMode;

/// Settings namespace used in ExtensionSettingsStore.
const SETTINGS_ID: &str = "git";

/// Settings for git-related views.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GitViewSettings {
    pub diff_view_mode: DiffViewMode,
    pub diff_ignore_whitespace: bool,
    pub file_font_size: f32,
    pub is_dark: bool,
}

impl Default for GitViewSettings {
    fn default() -> Self {
        Self {
            diff_view_mode: DiffViewMode::default(),
            diff_ignore_whitespace: false,
            file_font_size: 13.0,
            is_dark: true,
        }
    }
}

/// Read current git view settings from ExtensionSettingsStore.
pub fn git_settings(cx: &gpui::App) -> GitViewSettings {
    let store = cx.global::<okena_extensions::ExtensionSettingsStore>();
    store
        .get(SETTINGS_ID, cx)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

/// Write git view settings to ExtensionSettingsStore.
pub fn set_git_settings(settings: &GitViewSettings, cx: &mut gpui::App) {
    if let Ok(value) = serde_json::to_value(settings) {
        okena_extensions::ExtensionSettingsStore::update(SETTINGS_ID, value, cx);
    }
}
