//! Application views.
//!
//! This module contains all UI views organized into submodules:
//! - `layout` - Layout management (containers, splits, terminal panes)
//! - `panels` - Side panels (sidebar, project columns, status bar)
//! - `overlays` - Modal overlays (fullscreen, command palette, settings, etc.)
//! - `chrome` - Window chrome (title bar, header buttons)
//! - `components` - Reusable UI components (inputs, etc.)
//!
//! The root view is in this module as `root.rs`.

// Submodules
pub mod chrome;
pub mod components;
pub mod layout;
pub mod overlay_manager;
pub mod overlays;
pub mod panels;
pub mod root;
pub mod sidebar_controller;
