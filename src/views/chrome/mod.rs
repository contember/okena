//! Window chrome views.
//!
//! This module contains views for window decorations:
//! - Title bar (custom window chrome)
//! - Header buttons for terminal panes

pub mod header_buttons;
pub mod title_bar;

pub use header_buttons::{header_button_base, ButtonSize, HeaderAction};
pub use title_bar::TitleBar;
