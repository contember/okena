//! Workspace actions module
//!
//! This module contains all workspace mutation methods organized by domain:
//! - `focus`: Focus and fullscreen management
//! - `layout`: Split, tabs, and close operations
//! - `project`: Project CRUD and properties
//! - `terminal`: Terminal-specific actions

pub mod execute;
mod focus;
mod folder;
mod grid;
mod layout;
mod project;
mod terminal;

// All impl blocks are on Workspace, so no re-exports needed.
// The methods are available directly on the Workspace type.
