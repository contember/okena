#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

mod status;
mod settings;
mod usage;
mod ui_helpers;

use gpui::{AppContext as _, AnyView};
use okena_extensions::{ExtensionInstance, ExtensionManifest, ExtensionRegistration};
use std::sync::Arc;

pub fn register() -> ExtensionRegistration {
    ExtensionRegistration {
        manifest: ExtensionManifest {
            id: "codex",
            name: "Codex",
            default_enabled: false,
        },
        activate: Arc::new(|app| {
            let status = app.new(status::CodexStatus::new);
            let usage = app.new(usage::CodexUsage::new);
            ExtensionInstance {
                status_bar_widgets: vec![status.into(), usage.into()],
                status_bar_right_widgets: vec![],
            }
        }),
        settings_view: Some(Arc::new(|app| {
            AnyView::from(app.new(settings::CodexSettingsView::new))
        })),
    }
}
