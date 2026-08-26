//! Terminal overlay views.
//!
//! Contains overlay views for terminal-related functionality:
//! - Detached terminal windows
//! - Adaptive terminal menu (right-click and header triggers)
//! - Tab context menu (right-click on tab)
//! - Send composer (annotate a selection, paste it back)
//! - Shared terminal overlay utilities

pub mod detached_terminal;
pub mod rename_terminal_dialog;
pub mod send_composer;
pub mod tab_context_menu;
pub mod terminal_menu;
pub mod terminal_overlay_utils;
