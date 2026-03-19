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

/// Factory that creates status bar widgets. Called once during StatusBar init.
/// Receives `&mut App` which allows creating entities via `app.new(|cx| ...)`.
/// Uses `Arc` so the factory can be cloned out of the global registry without holding borrows.
pub type StatusBarWidgetFactory = Arc<dyn Fn(&mut App) -> Vec<AnyView>>;

/// A registered extension with its capabilities.
pub struct ExtensionRegistration {
    pub manifest: ExtensionManifest,
    pub status_bar_widgets: Option<StatusBarWidgetFactory>,
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

#[cfg(test)]
mod tests {
    use super::{ExtensionManifest, ExtensionRegistration, ExtensionRegistry};

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
            status_bar_widgets: None,
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
            status_bar_widgets: None,
        });
        registry.register(ExtensionRegistration {
            manifest: ExtensionManifest {
                id: "disabled-ext",
                name: "Disabled",
                default_enabled: false,
            },
            status_bar_widgets: None,
        });

        let defaults = registry.default_enabled_ids();
        assert!(defaults.contains("enabled-ext"));
        assert!(!defaults.contains("disabled-ext"));
    }
}
