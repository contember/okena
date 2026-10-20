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

pub fn board_columns(cx: &App) -> Vec<BoardColumn> {
    cx.try_global::<GlobalBoardColumns>()
        .map(|g| (g.0)(cx))
        .unwrap_or_else(default_board_columns)
}
