//! Modal overlay views.
//!
//! This module contains views for modal overlays:
//! - Fullscreen terminal mode
//! - Detached terminal windows
//! - Command palette
//! - Context menu
//! - Keybindings help
//! - Session manager
//! - Settings panel
//! - Shell selector
//! - Theme selector
//! - Worktree dialog

pub mod command_palette;
pub mod context_menu;
pub mod detached_terminal;
pub mod fullscreen_terminal;
pub mod keybindings_help;
pub mod session_manager;
pub mod settings_panel;
pub mod shell_selector_overlay;
pub mod theme_selector;
pub mod worktree_dialog;

pub use command_palette::CommandPalette;
pub use detached_terminal::DetachedTerminalView;
pub use fullscreen_terminal::FullscreenTerminal;
pub use keybindings_help::KeybindingsHelp;
pub use session_manager::SessionManager;
pub use settings_panel::SettingsPanel;
pub use shell_selector_overlay::{ShellSelectorOverlay, ShellSelectorOverlayEvent};
pub use theme_selector::ThemeSelector;
pub use worktree_dialog::WorktreeDialog;
