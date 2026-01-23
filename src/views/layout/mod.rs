//! Layout management views.
//!
//! This module contains views for managing terminal layouts:
//! - Layout containers (tabs, splits)
//! - Split panes with resize handles
//! - Individual terminal panes
//! - Focus navigation between panes

pub mod layout_container;
pub mod navigation;
pub mod split_pane;
mod tabs;
pub mod terminal_pane;

pub use layout_container::LayoutContainer;
pub use navigation::{get_pane_map, register_pane_bounds, NavigationDirection};
pub use split_pane::init_split_drag_context;
pub use terminal_pane::TerminalPane;
