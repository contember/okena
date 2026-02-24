//! Modal overlay views.
//!
//! This module contains views for modal overlays:
//! - Detached terminal windows
//! - Command palette
//! - Context menu
//! - Diff viewer
//! - File search
//! - File viewer
//! - Keybindings help
//! - Session manager
//! - Settings panel
//! - Shell selector
//! - Theme selector
//! - Worktree dialog

pub mod add_project_dialog;
pub mod command_palette;
pub mod context_menu;
pub mod folder_context_menu;
pub mod detached_terminal;
pub mod diff_viewer;
pub mod file_search;
pub mod file_viewer;
mod markdown_renderer;
pub mod keybindings_help;
pub mod project_switcher;
pub mod session_manager;
pub mod settings_panel;
pub mod shell_selector_overlay;
mod terminal_overlay_utils;
pub mod theme_selector;
pub mod remote_connect_dialog;
pub mod remote_context_menu;
pub mod tab_context_menu;
pub mod terminal_context_menu;
pub mod pairing_dialog;
pub mod close_worktree_dialog;
pub mod rename_directory_dialog;
pub mod worktree_dialog;

pub mod app_picker;

pub use app_picker::{AppPickerOverlay, AppPickerEvent};
pub use project_switcher::{ProjectSwitcher, ProjectSwitcherEvent};
pub use shell_selector_overlay::{ShellSelectorOverlay, ShellSelectorOverlayEvent};
