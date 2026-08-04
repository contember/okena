#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

mod settings;
mod status;
mod ui_helpers;
pub mod usage;

pub use usage::resolve_claude_dir;

use gpui::{AnyView, AppContext as _};
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
            let status = app.new(status::ClaudeStatus::new);
            let usage = app.new(usage::ClaudeUsage::new);
            ExtensionInstance {
                status_bar_widgets: vec![status.into(), usage.into()],
                status_bar_right_widgets: vec![],
            }
        }),
        settings_view: Some(Arc::new(|app| {
            AnyView::from(app.new(settings::ClaudeSettingsView::new))
        })),
    }
}
