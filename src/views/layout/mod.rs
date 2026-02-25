//! Layout management views.
//!
//! This module contains views for managing terminal layouts:
//! - Layout containers (tabs, splits)
//! - Split panes with resize handles
//! - Individual terminal panes
//! - Focus navigation between panes

pub mod app_entity_registry;
pub mod app_pane;
pub mod app_registry;
pub mod kruh_pane;
pub mod layout_container;
pub mod navigation;
pub mod pane_drag;
pub mod remote_app_pane;
pub mod split_pane;
mod tabs;
pub mod terminal_pane;
