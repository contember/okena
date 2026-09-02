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

pub mod about;
pub mod add_project_dialog;
pub mod change_path_dialog;
pub mod close_worktree_dialog;
pub mod command_palette;
pub mod content_search;
pub mod context_menu;
pub mod detached_overlay;
pub mod detached_terminal;
pub mod diff_viewer;
pub mod file_search;
pub mod file_viewer;
pub mod folder_context_menu;
pub mod hook_log;
pub mod keybindings_help;
pub mod log_console;
pub mod pairing_dialog;
pub mod profile_manager;
pub mod project_inspector;
pub mod project_switcher;
pub mod remote_connect_dialog;
pub mod remote_context_menu;
pub mod remote_pair_dialog;
pub mod rename_directory_dialog;
pub mod rename_terminal_dialog;
pub mod send_composer;
pub mod session_manager;
pub mod settings_panel;
pub mod shell_selector_overlay;
pub mod tab_context_menu;
pub mod terminal_menu;
pub mod terminal_overlay_utils;
pub mod theme_selector;
pub mod worktree_dialog;

pub use project_switcher::{ProjectSwitcher, ProjectSwitcherEvent};
pub use shell_selector_overlay::{ShellSelectorOverlay, ShellSelectorOverlayEvent};
