//! Window chrome views.
//!
//! This module contains views for window decorations:
//! - Title bar (custom window chrome)
//! - Header buttons for terminal panes
//! - Repaint observers for chrome rendered as `.cached()` views

pub mod header_buttons;
pub(crate) mod repaint;
pub mod title_bar;
