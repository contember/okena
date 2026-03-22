//! Git view settings — GPUI Global for diff viewer preferences.
//!
//! The host app registers this at startup and syncs it with persistent storage.
//! Crate views read/write directly without knowing about the app's settings system.

use okena_core::types::DiffViewMode;

/// Settings for git-related views.
#[derive(Clone, Debug)]
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

/// GPUI Global holding git view settings.
///
/// The host app initializes this at startup from persistent settings
/// and syncs changes back to disk.
pub struct GlobalGitViewSettings {
    pub current: GitViewSettings,
}

impl gpui::Global for GlobalGitViewSettings {}

impl GlobalGitViewSettings {
    pub fn new(initial: GitViewSettings) -> Self {
        Self { current: initial }
    }

    /// Update settings in-place.
    pub fn update(&mut self, f: impl FnOnce(&mut GitViewSettings)) {
        f(&mut self.current);
    }
}

/// Read current git view settings from the GPUI global.
pub fn git_settings(cx: &gpui::App) -> GitViewSettings {
    cx.global::<GlobalGitViewSettings>().current.clone()
}
