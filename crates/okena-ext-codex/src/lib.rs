mod status;
mod usage;
mod ui_helpers;

use gpui::AppContext as _;
use okena_extensions::{ExtensionManifest, ExtensionRegistration};
use std::sync::Arc;

pub fn register() -> ExtensionRegistration {
    ExtensionRegistration {
        manifest: ExtensionManifest {
            id: "codex",
            name: "Codex",
            default_enabled: false,
        },
        status_bar_widgets: Some(Arc::new(|app| {
            let status = app.new(|cx| status::CodexStatus::new(cx));
            let usage = app.new(|cx| usage::CodexUsage::new(cx));
            vec![status.into(), usage.into()]
        })),
        status_bar_right_widgets: None,
    }
}
