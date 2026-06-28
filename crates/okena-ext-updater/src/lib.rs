#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

pub mod checker;
pub mod daemon_client;
pub mod downloader;
pub mod installer;
pub mod manager;
#[cfg(feature = "gpui-ui")]
pub mod orchestrator;
mod process;
mod status;
#[cfg(feature = "gpui-ui")]
mod update_checker;

#[cfg(feature = "gpui-ui")]
use gpui::AppContext as _;
#[cfg(feature = "gpui-ui")]
use okena_extensions::{ExtensionInstance, ExtensionManifest, ExtensionRegistration};
#[cfg(feature = "gpui-ui")]
use std::sync::Arc;

// Re-export public types used by the host app
#[cfg(feature = "gpui-ui")]
pub use installer::restart_app;
pub use status::{GlobalUpdateInfo, UpdateInfo, UpdateStatus, UpdateStatusSnapshot};

#[cfg(feature = "gpui-ui")]
pub fn register() -> ExtensionRegistration {
    ExtensionRegistration {
        manifest: ExtensionManifest {
            id: "updater",
            name: "Auto Update",
            default_enabled: true,
        },
        activate: Arc::new(|app| {
            let widget = app.new(crate::status::UpdateStatusWidget::new);
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
#[cfg(feature = "gpui-ui")]
pub fn init(app_version: &str, cx: &mut gpui::App) {
    installer::cleanup_old_binary();

    let update_info = UpdateInfo::new(app_version.to_string());
    cx.set_global(GlobalUpdateInfo(update_info));
}
