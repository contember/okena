//! Workspace actions module
//!
//! This module contains all workspace mutation methods organized by domain:
//! - `focus`: Focus and fullscreen management
//! - `layout`: Split, tabs, and close operations
//! - `project`: Project CRUD and properties
//! - `terminal`: Terminal-specific actions
//! - `worktree`: Worktree lifecycle (create, register, discover, remove)

pub mod focus;
pub mod folder;
pub mod layout;
pub mod project;
pub mod soft_close;
pub mod terminal;
pub mod worktree;

// All impl blocks are on Workspace, so no re-exports needed.
// The methods are available directly on the Workspace type.
