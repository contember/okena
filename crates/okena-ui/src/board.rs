//! Status board columns, read by views in any crate.
//!
//! The columns live in the app settings (`okena-workspace`), which this crate
//! does not depend on. The host registers a reader at startup, the way it
//! does for the status bar style in [`crate::metrics`]; unregistered (tests,
//! headless) falls back to the built-in default columns.

use gpui::*;
pub use okena_core::types::{BoardColumn, default_board_columns};

/// Global function pointer that reads the current board columns from the
/// host app's settings.
pub struct GlobalBoardColumns(pub fn(&App) -> Vec<BoardColumn>);

impl Global for GlobalBoardColumns {}

/// Global function pointer that reads whether the status board feature is
/// switched on in settings. Unregistered falls back to off, so nothing
/// status-related renders in tests or headless builds.
pub struct GlobalStatusBoardEnabled(pub fn(&App) -> bool);

impl Global for GlobalStatusBoardEnabled {}

pub fn status_board_enabled(cx: &App) -> bool {
    cx.try_global::<GlobalStatusBoardEnabled>()
        .is_some_and(|g| (g.0)(cx))
}

/// The colour a project's status lends it: the column's colour when the
/// board is enabled, the status resolves to a column and that column has a
/// colour. Views use it in place of the project's own colour so a status
/// reads at a glance everywhere the project is drawn.
pub fn status_color(status: Option<&str>, cx: &App) -> Option<okena_core::theme::FolderColor> {
    let status = status?;
    if !status_board_enabled(cx) {
        return None;
    }
    board_columns(cx)
        .into_iter()
        .find(|c| c.id == status)
        .map(|c| c.color)
        .filter(|c| *c != okena_core::theme::FolderColor::Default)
}

/// Sidebar presentation choices read by the sidebar crate.
#[derive(Clone, Copy, Debug)]
pub struct SidebarLook {
    pub worktree_marker: okena_core::types::WorktreeMarker,
    pub worktree_guide: bool,
}

impl Default for SidebarLook {
    fn default() -> Self {
        Self {
            worktree_marker: okena_core::types::WorktreeMarker::Project,
            worktree_guide: false,
        }
    }
}

/// Global function pointer that reads [`SidebarLook`] from the host app's
/// settings. Unregistered falls back to the defaults.
pub struct GlobalSidebarLook(pub fn(&App) -> SidebarLook);

impl Global for GlobalSidebarLook {}

pub fn sidebar_look(cx: &App) -> SidebarLook {
    cx.try_global::<GlobalSidebarLook>()
        .map(|g| (g.0)(cx))
        .unwrap_or_default()
}

pub fn board_columns(cx: &App) -> Vec<BoardColumn> {
    cx.try_global::<GlobalBoardColumns>()
        .map(|g| (g.0)(cx))
        .unwrap_or_else(default_board_columns)
}
