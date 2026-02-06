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

// Re-export everything for backward compatibility
// These allow existing code to use `crate::views::Sidebar` instead of `crate::views::panels::Sidebar`
#[allow(unused_imports)]
pub use layout::layout_container;
#[allow(unused_imports)]
pub use layout::navigation;
#[allow(unused_imports)]
pub use layout::split_pane;
#[allow(unused_imports)]
pub use layout::terminal_pane;
#[allow(unused_imports)]
pub use layout::{get_pane_map, register_pane_bounds, NavigationDirection};
#[allow(unused_imports)]
pub use layout::{LayoutContainer, TerminalPane};

#[allow(unused_imports)]
pub use panels::project_column;
#[allow(unused_imports)]
pub use panels::sidebar;
#[allow(unused_imports)]
pub use panels::status_bar;
#[allow(unused_imports)]
pub use panels::{ProjectColumn, Sidebar, StatusBar};

#[allow(unused_imports)]
pub use overlays::command_palette;
#[allow(unused_imports)]
pub use overlays::detached_terminal;
#[allow(unused_imports)]
pub use overlays::keybindings_help;
#[allow(unused_imports)]
pub use overlays::session_manager;
#[allow(unused_imports)]
pub use overlays::settings_panel;
#[allow(unused_imports)]
pub use overlays::theme_selector;
#[allow(unused_imports)]
pub use overlays::worktree_dialog;
#[allow(unused_imports)]
pub use overlays::{
    CommandPalette, DetachedTerminalView, KeybindingsHelp, SessionManager,
    SettingsPanel, ThemeSelector, WorktreeDialog,
};

#[allow(unused_imports)]
pub use chrome::header_buttons;
#[allow(unused_imports)]
pub use chrome::title_bar;
#[allow(unused_imports)]
pub use chrome::{header_button_base, ButtonSize, HeaderAction, TitleBar};

#[allow(unused_imports)]
pub use components::simple_input;
#[allow(unused_imports)]
pub use components::{SimpleInput, SimpleInputState};

#[allow(unused_imports)]
pub use overlay_manager::{CloseEvent, OverlayManager, OverlayManagerEvent, OverlaySlot};

#[allow(unused_imports)]
pub use sidebar_controller::{SidebarController, AnimationTarget};
