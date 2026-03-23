pub mod sidebar;
pub mod project_list;
pub mod folder_list;
pub mod worktree_list;
pub mod hook_list;
pub mod remote_list;
pub mod service_list;
pub mod item_widgets;
pub mod color_picker;
pub mod context_menu;
pub mod folder_context_menu;
pub mod rename_directory_dialog;
pub mod hook_log;
pub mod drag;

pub use sidebar::Sidebar;

// Re-export settings types
pub use sidebar::{DispatchActionFn, GetSettingsFn, SidebarSettings};

// Re-export remote manager callback types
pub use sidebar::{RemoteConnectionSnapshot, GetRemoteConnectionsFn, SendRemoteActionFn, GetRemoteFolderFn};

// Re-export context menu types
pub use context_menu::{ContextMenu, ContextMenuEvent};
pub use folder_context_menu::{FolderContextMenu, FolderContextMenuEvent};
pub use rename_directory_dialog::{RenameDirectoryDialog, RenameDirectoryDialogEvent};
pub use hook_log::{HookLog, HookLogEvent};

gpui::actions!(okena_views_sidebar, [
    SidebarUp,
    SidebarDown,
    SidebarConfirm,
    SidebarToggleExpand,
    SidebarEscape,
    Cancel,
]);
