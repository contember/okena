use gpui::*;
use std::collections::HashSet;
use std::sync::Arc;

// Re-export theme types for extension use
pub use okena_core::theme::{ThemeColors, ThemeMode};

/// Global theme provider — a function pointer that reads the current theme colors.
/// The host app registers this at startup; extensions call `theme()` to read colors.
pub struct GlobalThemeProvider(pub fn(&App) -> ThemeColors);

impl Global for GlobalThemeProvider {}

/// Get current theme colors. Extensions should use this instead of accessing theme globals directly.
pub fn theme(cx: &App) -> ThemeColors {
    (cx.global::<GlobalThemeProvider>().0)(cx)
}

/// Extension metadata.
pub struct ExtensionManifest {
    pub id: &'static str,
    pub name: &'static str,
    pub default_enabled: bool,
}

/// Factory that creates a settings view for an extension.
/// Called by the host settings panel when the extension's settings category is selected.
pub type SettingsViewFactory = Arc<dyn Fn(&mut App) -> AnyView>;

/// An active extension instance. Holds all resources created during activation.
/// Dropping this deactivates the extension — views are released, and any `Task`
/// handles stored in the underlying entities are cancelled automatically.
pub struct ExtensionInstance {
    /// Widgets rendered on the left side of the status bar (after CPU/MEM).
    pub status_bar_widgets: Vec<AnyView>,
    /// Widgets rendered on the right side of the status bar (before version/time).
    pub status_bar_right_widgets: Vec<AnyView>,
}

/// Called when an extension is enabled. Creates views, entities, and background
/// tasks. The returned `ExtensionInstance` is dropped when the extension is
/// disabled, which triggers cleanup of all owned resources.
pub type ActivateFn = Arc<dyn Fn(&mut App) -> ExtensionInstance>;

/// A registered extension with its capabilities.
pub struct ExtensionRegistration {
    pub manifest: ExtensionManifest,
    /// Called when the extension is enabled. The returned instance is dropped on disable.
    pub activate: ActivateFn,
    /// Optional settings UI, rendered inside the host settings panel.
    pub settings_view: Option<SettingsViewFactory>,
}

/// Global registry of all extensions, set via `cx.set_global()`.
pub struct ExtensionRegistry {
    extensions: Vec<ExtensionRegistration>,
}

impl Global for ExtensionRegistry {}

impl ExtensionRegistry {
    pub fn new() -> Self {
        Self {
            extensions: Vec::new(),
        }
    }

    pub fn register(&mut self, registration: ExtensionRegistration) {
        self.extensions.push(registration);
    }

    pub fn extensions(&self) -> &[ExtensionRegistration] {
        &self.extensions
    }

    /// Get the default set of enabled extension IDs (for new users).
    pub fn default_enabled_ids(&self) -> HashSet<String> {
        self.extensions
            .iter()
            .filter(|e| e.manifest.default_enabled)
            .map(|e| e.manifest.id.to_string())
            .collect()
    }
}

/// Trait for checking extension enablement.
/// The host app implements this on its settings type and registers it as a global.
pub trait ExtensionSettings {
    fn is_extension_enabled(&self, extension_id: &str) -> bool;
}

/// Global bridge for extensions to read/write their persisted settings.
/// The host app registers an implementation at startup via `cx.set_global()`.
pub struct ExtensionSettingsStore {
    getter: Box<dyn Fn(&str, &App) -> Option<serde_json::Value>>,
    setter: Box<dyn Fn(&str, serde_json::Value, &mut App)>,
}

impl Global for ExtensionSettingsStore {}

impl ExtensionSettingsStore {
    pub fn new(
        getter: impl Fn(&str, &App) -> Option<serde_json::Value> + 'static,
        setter: impl Fn(&str, serde_json::Value, &mut App) + 'static,
    ) -> Self {
        Self {
            getter: Box::new(getter),
            setter: Box::new(setter),
        }
    }

    /// Read the extension's settings blob.
    pub fn get(&self, extension_id: &str, cx: &App) -> Option<serde_json::Value> {
        (self.getter)(extension_id, cx)
    }

    /// Write the extension's settings blob (triggers auto-save via the host).
    pub fn set(&self, extension_id: &str, value: serde_json::Value, cx: &mut App) {
        (self.setter)(extension_id, value, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_register_and_lookup() {
        let mut registry = ExtensionRegistry::new();
        assert_eq!(registry.extensions().len(), 0);

        registry.register(ExtensionRegistration {
            manifest: ExtensionManifest {
                id: "test-ext",
                name: "Test Extension",
                default_enabled: true,
            },
            activate: Arc::new(|_| ExtensionInstance {
                status_bar_widgets: vec![],
                status_bar_right_widgets: vec![],
            }),
            settings_view: None,
        });

        assert_eq!(registry.extensions().len(), 1);
        assert_eq!(registry.extensions()[0].manifest.id, "test-ext");
    }

    #[test]
    fn default_enabled_ids() {
        let mut registry = ExtensionRegistry::new();
        registry.register(ExtensionRegistration {
            manifest: ExtensionManifest {
                id: "enabled-ext",
                name: "Enabled",
                default_enabled: true,
            },
            activate: Arc::new(|_| ExtensionInstance {
                status_bar_widgets: vec![],
                status_bar_right_widgets: vec![],
            }),
            settings_view: None,
        });
        registry.register(ExtensionRegistration {
            manifest: ExtensionManifest {
                id: "disabled-ext",
                name: "Disabled",
                default_enabled: false,
            },
            activate: Arc::new(|_| ExtensionInstance {
                status_bar_widgets: vec![],
                status_bar_right_widgets: vec![],
            }),
            settings_view: None,
        });

        let defaults = registry.default_enabled_ids();
        assert!(defaults.contains("enabled-ext"));
        assert!(!defaults.contains("disabled-ext"));
    }
}
