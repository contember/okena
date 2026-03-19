mod status;
mod usage;
mod ui_helpers;

use gpui::AppContext as _;
use okena_extensions::{ExtensionManifest, ExtensionRegistration};
use std::sync::Arc;

pub fn register() -> ExtensionRegistration {
    ExtensionRegistration {
        manifest: ExtensionManifest {
            id: "claude-code",
            name: "Claude Code",
            default_enabled: false,
        },
        status_bar_widgets: Some(Arc::new(|app| {
            let status = app.new(|cx| status::ClaudeStatus::new(cx));
            let usage = app.new(|cx| usage::ClaudeUsage::new(cx));
            vec![status.into(), usage.into()]
        })),
    }
}
