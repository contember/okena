#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

mod settings;
mod usage;

use gpui::{AnyView, AppContext as _};
use okena_extensions::{ExtensionInstance, ExtensionManifest, ExtensionRegistration};
use std::sync::Arc;

pub fn register() -> ExtensionRegistration {
    ExtensionRegistration {
        manifest: ExtensionManifest {
            id: "grok",
            name: "Grok",
            default_enabled: false,
        },
        activate: Arc::new(|app| {
            let usage = app.new(usage::GrokUsage::new);
            ExtensionInstance {
                status_bar_widgets: vec![usage.into()],
                status_bar_right_widgets: vec![],
            }
        }),
        settings_view: Some(Arc::new(|app| {
            AnyView::from(app.new(settings::GrokSettingsView::new))
        })),
    }
}
