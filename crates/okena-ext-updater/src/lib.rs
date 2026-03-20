pub mod checker;
pub mod downloader;
pub mod installer;
mod process;
mod status;
mod update_checker;

use gpui::AppContext as _;
use okena_extensions::{ExtensionInstance, ExtensionManifest, ExtensionRegistration};
use std::sync::Arc;

// Re-export public types used by the host app
pub use status::{GlobalUpdateInfo, UpdateInfo, UpdateStatus};
pub use installer::restart_app;

pub fn register() -> ExtensionRegistration {
    ExtensionRegistration {
        manifest: ExtensionManifest {
            id: "updater",
            name: "Auto Update",
            default_enabled: true,
        },
        activate: Arc::new(|app| {
            let widget = app.new(|cx| crate::status::UpdateStatusWidget::new(cx));
            ExtensionInstance {
                status_bar_widgets: vec![],
                status_bar_right_widgets: vec![widget.into()],
            }
        }),
        settings_view: None,
    }
}

/// Initialize the updater: set GlobalUpdateInfo global, clean up old binary,
/// start background checker. Called by the host app at startup.
/// `app_version` should be the host application's version (from root Cargo.toml).
pub fn init(app_version: &str, cx: &mut gpui::App) {
    installer::cleanup_old_binary();

    let update_info = UpdateInfo::new(app_version.to_string());
    cx.set_global(GlobalUpdateInfo(update_info));
}
