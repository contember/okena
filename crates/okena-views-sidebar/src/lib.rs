pub mod sidebar;
pub mod project_list;
pub mod folder_list;
pub mod worktree_list;
pub mod hook_list;
pub mod remote_list;
pub mod service_list;
pub mod item_widgets;
pub mod color_picker;
pub mod drag;

pub use sidebar::Sidebar;

// Re-export settings types
pub use sidebar::{DispatchActionFn, GetSettingsFn, SidebarSettings};

// Re-export remote manager callback types
pub use sidebar::{RemoteConnectionSnapshot, GetRemoteConnectionsFn, SendRemoteActionFn, GetRemoteFolderFn};

gpui::actions!(okena_views_sidebar, [
    SidebarUp,
    SidebarDown,
    SidebarConfirm,
    SidebarToggleExpand,
    SidebarEscape,
]);
