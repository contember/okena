//! Status board settings, read by views in any crate.
//!
//! The board lives in the app settings (`okena-workspace`), which this crate
//! does not depend on. The host registers a reader at startup, the way it
//! does for the status bar style in [`crate::metrics`]; unregistered (tests,
//! headless) reads as switched off with the built-in default columns.

use gpui::*;
use okena_core::theme::FolderColor;
pub use okena_core::types::{BoardColumn, default_board_columns};
use std::sync::LazyLock;

/// The board as the settings hold it, borrowed for one read.
#[derive(Clone, Copy, Debug)]
pub struct StatusBoard<'a> {
    /// Master switch; off hides every status surface.
    pub enabled: bool,
    pub columns: &'a [BoardColumn],
    /// Colour of the "No status" group; `Default` draws it grey.
    pub no_status_color: FolderColor,
    /// The number of columns before the "No status" group in the board
    /// order (`columns.len()` = last).
    pub no_status_at: usize,
}

impl StatusBoard<'_> {
    /// The colour a project's status lends it: the column's colour when the
    /// board is enabled, the status resolves to a column and that column has
    /// a colour. Views use it in place of the project's own colour so a
    /// status reads at a glance everywhere the project is drawn.
    pub fn status_color(&self, status: Option<&str>) -> Option<FolderColor> {
        let status = status?;
        if !self.enabled {
            return None;
        }
        self.columns
            .iter()
            .find(|c| c.id == status)
            .map(|c| c.color)
            .filter(|c| *c != FolderColor::Default)
    }

    /// The colour of a group caption: the column's own colour, or the
    /// "No status" colour for the unassigned group (`None`).
    pub fn group_color(&self, column: Option<&BoardColumn>) -> FolderColor {
        column.map_or(self.no_status_color, |c| c.color)
    }
}

/// Global function pointer that borrows the board from the host app's
/// settings.
pub struct GlobalStatusBoard(pub for<'a> fn(&'a App) -> StatusBoard<'a>);

impl Global for GlobalStatusBoard {}

static DEFAULT_COLUMNS: LazyLock<Vec<BoardColumn>> = LazyLock::new(default_board_columns);

pub fn status_board(cx: &App) -> StatusBoard<'_> {
    match cx.try_global::<GlobalStatusBoard>() {
        Some(reader) => (reader.0)(cx),
        None => StatusBoard {
            enabled: false,
            columns: &DEFAULT_COLUMNS,
            no_status_color: FolderColor::Default,
            no_status_at: DEFAULT_COLUMNS.len(),
        },
    }
}

pub fn status_board_enabled(cx: &App) -> bool {
    status_board(cx).enabled
}

pub fn board_columns(cx: &App) -> &[BoardColumn] {
    status_board(cx).columns
}

/// See [`StatusBoard::status_color`].
pub fn status_color(status: Option<&str>, cx: &App) -> Option<FolderColor> {
    status_board(cx).status_color(status)
}
