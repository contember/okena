#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

pub mod remote_connect_dialog;
pub mod remote_pair_dialog;
pub mod remote_context_menu;

pub use remote_connect_dialog::{RemoteConnectDialog, RemoteConnectDialogEvent};
pub use remote_pair_dialog::{RemotePairDialog, RemotePairDialogEvent};
pub use remote_context_menu::{RemoteContextMenu, RemoteContextMenuEvent};

gpui::actions!(okena_views_remote, [Cancel]);
