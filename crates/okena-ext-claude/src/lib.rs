#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

mod status;
pub mod usage;
mod ui_helpers;

pub use usage::resolve_claude_dir;

use gpui::AppContext as _;
use okena_extensions::{ExtensionInstance, ExtensionManifest, ExtensionRegistration};
use std::sync::Arc;

pub fn register() -> ExtensionRegistration {
    ExtensionRegistration {
        manifest: ExtensionManifest {
            id: "claude-code",
            name: "Claude Code",
            default_enabled: false,
        },
        activate: Arc::new(|app| {
            let status = app.new(|cx| status::ClaudeStatus::new(cx));
            let usage = app.new(|cx| usage::ClaudeUsage::new(cx));
            ExtensionInstance {
                status_bar_widgets: vec![status.into(), usage.into()],
                status_bar_right_widgets: vec![],
            }
        }),
        settings_view: None,
    }
}
