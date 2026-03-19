pub mod checker;
pub mod downloader;
pub mod installer;
mod process;
mod status;
mod update_checker;

use gpui::AppContext as _;
use okena_extensions::{ExtensionManifest, ExtensionRegistration};
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
        status_bar_widgets: Some(Arc::new(|app| {
            let widget = app.new(|cx| crate::status::UpdateStatusWidget::new(cx));
            vec![widget.into()]
        })),
    }
}

/// Initialize the updater: set GlobalUpdateInfo global, clean up old binary,
/// start background checker. Called by the host app at startup.
pub fn init(cx: &mut gpui::App) {
    installer::cleanup_old_binary();

    let update_info = UpdateInfo::new();
    cx.set_global(GlobalUpdateInfo(update_info));
}
