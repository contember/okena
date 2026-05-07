#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

//! okena-state — Pure data types for workspace state.
//!
//! Holds the serializable data structures that describe a workspace:
//! projects, folders, layouts (re-exported from `okena-layout`), worktree
//! metadata, hook terminals, and lifecycle hooks configuration. No GPUI,
//! no behavior beyond a few pure helpers.

mod hooks_config;
mod toast;
mod transient;
mod window_state;
mod workspace_data;

pub use hooks_config::{HooksConfig, ProjectHooks, TerminalHooks, WorktreeHooks};
pub use okena_layout::{LayoutNode, SplitDirection};
pub use toast::{Toast, ToastLevel};
pub use transient::{DropZone, FocusedTerminalState, PendingWorktreeClose};
pub use window_state::{WindowBounds, WindowState};
pub use workspace_data::{
    FolderData, HookTerminalEntry, HookTerminalStatus, ProjectData, WorkspaceData,
    WorktreeMetadata, is_bash_prompt_title,
};
