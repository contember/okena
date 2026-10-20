//! Terminal status board columns, read by views in any crate.
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

pub fn board_columns(cx: &App) -> Vec<BoardColumn> {
    cx.try_global::<GlobalBoardColumns>()
        .map(|g| (g.0)(cx))
        .unwrap_or_else(default_board_columns)
}
