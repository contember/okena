mod config;
mod descriptions;
mod types;

use gpui::*;
use parking_lot::RwLock;

pub use config::{
    get_keybindings_path, load_keybindings, save_keybindings,
    KeybindingConfig,
};
pub use descriptions::get_action_descriptions;
#[allow(unused_imports)]
pub use types::{ActionDescription, KeybindingConflict, KeybindingEntry};

// App-level actions (handled by root view, overlay manager, sidebar)
actions!(
    okena,
    [
        Quit,
        About,
        Cancel,
        ReloadKeybindings,
        ToggleSidebar,
        ToggleSidebarAutoHide,
        NewProject,
        CreateWorktree,
        ClearFocus,
        FocusActiveProject,
        ScrollUp,
        ScrollDown,
        ShowKeybindings,
        ShowSessionManager,
        ShowThemeSelector,
        ShowCommandPalette,
        ShowSettings,
        OpenSettingsFile,
        ShowFileSearch,
        ShowContentSearch,
        ShowProjectSwitcher,
        ShowDiffViewer,
        CheckForUpdates,
        InstallUpdate,
        FocusSidebar,
        ShowPairingDialog,
        TogglePaneSwitcher,
        StartAllServices,
        StopAllServices,
        ShowHookLog,
        EqualizeLayout,
    ]
);

// Terminal-specific actions (defined in okena-views-terminal crate)
pub use okena_views_terminal::actions::{
    SendEscape, SplitVertical, SplitHorizontal, AddTab, CloseTerminal,
    MinimizeTerminal, FocusNextTerminal, FocusPrevTerminal,
    FocusLeft, FocusRight, FocusUp, FocusDown,
    Copy, Paste, Search, SearchNext, SearchPrev, CloseSearch,
    SendTab, SendBacktab, ZoomIn, ZoomOut, ResetZoom,
    ToggleFullscreen, FullscreenNextTerminal, FullscreenPrevTerminal,
    JumpToPreviousPrompt, JumpToNextPrompt,
};

// Sidebar-specific actions (defined in okena-views-sidebar crate)
pub use okena_views_sidebar::{
    SidebarUp, SidebarDown, SidebarConfirm, SidebarToggleExpand, SidebarEscape,
};

/// Global keybinding configuration (thread-safe)
static KEYBINDING_CONFIG: RwLock<Option<KeybindingConfig>> = RwLock::new(None);

/// Get a read guard to the current keybinding configuration
///
/// Returns a guard that dereferences to KeybindingConfig.
/// The guard must be held for the duration of access.
pub fn get_config() -> impl std::ops::Deref<Target = KeybindingConfig> {
    parking_lot::RwLockReadGuard::map(KEYBINDING_CONFIG.read(), |opt| {
        #[allow(
            clippy::expect_used,
            reason = "init_keybindings() runs at startup before any caller reaches get_config()"
        )]
        opt.as_ref().expect("Keybinding config not initialized")
    })
}

/// Reset keybindings to defaults and save
pub fn reset_to_defaults() -> anyhow::Result<()> {
    let config = KeybindingConfig::defaults();
    save_keybindings(&config)?;
    *KEYBINDING_CONFIG.write() = Some(config);
    Ok(())
}

/// Convert a GPUI Keystroke to the config string format (e.g., "cmd-shift-d")
/// GPUI's unparse() uses "super-" on Linux for the platform modifier,
/// but our config format uses "cmd-" for cross-platform consistency.
pub fn keystroke_to_config_string(keystroke: &gpui::Keystroke) -> String {
    let unparsed = keystroke.unparse();
    // Normalize platform modifier names to "cmd-" for config consistency
    unparsed
        .replace("super-", "cmd-")
        .replace("win-", "cmd-")
}

/// Reload keybindings: update global config, save to disk, and re-register with GPUI.
/// Call this after modifying the config via get_config_mut() or update_config().
pub fn reload_keybindings(cx: &mut App) {
    let config = {
        KEYBINDING_CONFIG.read().as_ref().cloned().unwrap_or_default()
    };

    // Save to disk
    if let Err(e) = save_keybindings(&config) {
        log::error!("Failed to save keybindings: {}", e);
    }

    // Clear existing bindings and re-register everything
    cx.clear_key_bindings();
    register_bindings_from_config(cx, &config);

    // Re-register essential non-overridable bindings
    cx.bind_keys([
        KeyBinding::new("tab", SendTab, Some("TerminalPane")),
        KeyBinding::new("shift-tab", SendBacktab, Some("TerminalPane")),
    ]);

    cx.bind_keys([
        KeyBinding::new("up", SidebarUp, Some("Sidebar")),
        KeyBinding::new("down", SidebarDown, Some("Sidebar")),
        KeyBinding::new("enter", SidebarConfirm, Some("Sidebar")),
        KeyBinding::new("space", SidebarToggleExpand, Some("Sidebar")),
        KeyBinding::new("left", SidebarToggleExpand, Some("Sidebar")),
        KeyBinding::new("right", SidebarToggleExpand, Some("Sidebar")),
        KeyBinding::new("escape", SidebarEscape, Some("Sidebar")),
    ]);

    cx.bind_keys([
        KeyBinding::new("escape", Cancel, None),
        KeyBinding::new("escape", SendEscape, Some("TerminalPane")),
        KeyBinding::new("escape", CloseSearch, Some("SearchBar")),
        KeyBinding::new("escape", okena_views_terminal::actions::Cancel, Some("TerminalRename")),
        KeyBinding::new("escape", okena_files::file_search::Cancel, Some("FileSearchDialog")),
        KeyBinding::new("escape", okena_files::file_search::Cancel, Some("FileViewer")),
        KeyBinding::new("escape", okena_views_git::Cancel, Some("WorktreeDialog")),
        KeyBinding::new("escape", okena_views_git::Cancel, Some("CloseWorktreeDialog")),
        KeyBinding::new("escape", okena_views_git::diff_viewer::Cancel, Some("DiffViewer")),
        KeyBinding::new("escape", okena_views_sidebar::Cancel, Some("ContextMenu")),
        KeyBinding::new("escape", okena_views_sidebar::Cancel, Some("FolderContextMenu")),
        KeyBinding::new("escape", okena_views_sidebar::Cancel, Some("RenameDirectoryDialog")),
        KeyBinding::new("escape", okena_views_sidebar::Cancel, Some("HookLog")),
        KeyBinding::new("escape", okena_views_terminal::actions::Cancel, Some("ShellSelectorOverlay")),
        KeyBinding::new("escape", okena_views_remote::Cancel, Some("RemoteConnectDialog")),
        KeyBinding::new("escape", okena_views_remote::Cancel, Some("RemotePairDialog")),
        KeyBinding::new("escape", okena_views_remote::Cancel, Some("RemoteContextMenu")),
    ]);
}

/// Get a mutable reference to the global keybinding configuration.
/// After modifying, call reload_keybindings(cx) to apply changes.
pub fn update_config(f: impl FnOnce(&mut KeybindingConfig)) {
    let mut guard = KEYBINDING_CONFIG.write();
    if let Some(config) = guard.as_mut() {
        f(config);
    }
}

/// Register keybindings for the application from configuration
pub fn register_keybindings(cx: &mut App) {
    // Load configuration
    let config = load_keybindings();

    // Check for conflicts and warn
    let conflicts = config.detect_conflicts();
    for conflict in &conflicts {
        log::warn!("Keybinding conflict detected: {}", conflict);
    }

    // Store config globally (thread-safe)
    *KEYBINDING_CONFIG.write() = Some(config.clone());

    // Register bindings from config
    register_bindings_from_config(cx, &config);

    // Register essential terminal keybindings that should not be overridden
    // Tab/Shift+Tab must be captured to prevent GPUI's focus navigation from consuming them
    cx.bind_keys([
        KeyBinding::new("tab", SendTab, Some("TerminalPane")),
        KeyBinding::new("shift-tab", SendBacktab, Some("TerminalPane")),
    ]);

    // Register sidebar navigation keybindings (not user-configurable)
    cx.bind_keys([
        KeyBinding::new("up", SidebarUp, Some("Sidebar")),
        KeyBinding::new("down", SidebarDown, Some("Sidebar")),
        KeyBinding::new("enter", SidebarConfirm, Some("Sidebar")),
        KeyBinding::new("space", SidebarToggleExpand, Some("Sidebar")),
        KeyBinding::new("left", SidebarToggleExpand, Some("Sidebar")),
        KeyBinding::new("right", SidebarToggleExpand, Some("Sidebar")),
        KeyBinding::new("escape", SidebarEscape, Some("Sidebar")),
    ]);

    // Register escape keybindings with context-based precedence:
    //   Global:             escape → Cancel        (overlays, sidebar rename)
    //   TerminalPane:       escape → SendEscape    (send 0x1b to PTY)
    //   SearchBar:          escape → CloseSearch   (close search, deeper than TerminalPane)
    //   TerminalRename:     escape → Cancel        (cancel rename, deeper than TerminalPane)
    cx.bind_keys([
        KeyBinding::new("escape", Cancel, None),
        KeyBinding::new("escape", SendEscape, Some("TerminalPane")),
        KeyBinding::new("escape", CloseSearch, Some("SearchBar")),
        // Terminal rename uses the crate's Cancel action
        KeyBinding::new("escape", okena_views_terminal::actions::Cancel, Some("TerminalRename")),
        // okena-files crate Cancel action for file search/viewer
        KeyBinding::new("escape", okena_files::file_search::Cancel, Some("FileSearchDialog")),
        KeyBinding::new("escape", okena_files::file_search::Cancel, Some("FileViewer")),
        // okena-views-git crate Cancel actions for git overlays
        KeyBinding::new("escape", okena_views_git::Cancel, Some("WorktreeDialog")),
        KeyBinding::new("escape", okena_views_git::Cancel, Some("CloseWorktreeDialog")),
        KeyBinding::new("escape", okena_views_git::diff_viewer::Cancel, Some("DiffViewer")),
        // okena-views-sidebar crate Cancel actions for context menus
        KeyBinding::new("escape", okena_views_sidebar::Cancel, Some("ContextMenu")),
        KeyBinding::new("escape", okena_views_sidebar::Cancel, Some("FolderContextMenu")),
        KeyBinding::new("escape", okena_views_sidebar::Cancel, Some("RenameDirectoryDialog")),
        KeyBinding::new("escape", okena_views_sidebar::Cancel, Some("HookLog")),
        // okena-views-terminal crate Cancel for shell selector
        KeyBinding::new("escape", okena_views_terminal::actions::Cancel, Some("ShellSelectorOverlay")),
        // okena-views-remote crate Cancel actions
        KeyBinding::new("escape", okena_views_remote::Cancel, Some("RemoteConnectDialog")),
        KeyBinding::new("escape", okena_views_remote::Cancel, Some("RemotePairDialog")),
        KeyBinding::new("escape", okena_views_remote::Cancel, Some("RemoteContextMenu")),
    ]);
}

/// Register keybindings from a configuration
fn register_bindings_from_config(cx: &mut App, config: &KeybindingConfig) {
    // Collect all keybindings
    let mut bindings: Vec<KeyBinding> = Vec::new();

    for (action, entries) in &config.bindings {
        for entry in entries {
            if !entry.enabled {
                continue;
            }

            let context = entry.context.as_deref();

            // Map action name to action type
            if let Some(binding) = create_keybinding(action, &entry.keystroke, context) {
                bindings.push(binding);
            }
        }
    }

    // Register all bindings
    cx.bind_keys(bindings);
}

/// Create a KeyBinding from action name, keystroke, and context
fn create_keybinding(action: &str, keystroke: &str, context: Option<&str>) -> Option<KeyBinding> {
    // Map action names to actual actions
    match action {
        "Quit" => Some(KeyBinding::new(keystroke, Quit, context)),
        "Cancel" => Some(KeyBinding::new(keystroke, Cancel, context)),
        "SendEscape" => Some(KeyBinding::new(keystroke, SendEscape, context)),
        "ToggleSidebar" => Some(KeyBinding::new(keystroke, ToggleSidebar, context)),
        "ToggleSidebarAutoHide" => Some(KeyBinding::new(keystroke, ToggleSidebarAutoHide, context)),
        "ToggleFullscreen" => Some(KeyBinding::new(keystroke, ToggleFullscreen, context)),
        "FullscreenNextTerminal" => Some(KeyBinding::new(keystroke, FullscreenNextTerminal, context)),
        "FullscreenPrevTerminal" => Some(KeyBinding::new(keystroke, FullscreenPrevTerminal, context)),
        "SplitVertical" => Some(KeyBinding::new(keystroke, SplitVertical, context)),
        "SplitHorizontal" => Some(KeyBinding::new(keystroke, SplitHorizontal, context)),
        "AddTab" => Some(KeyBinding::new(keystroke, AddTab, context)),
        "CloseTerminal" => Some(KeyBinding::new(keystroke, CloseTerminal, context)),
        "MinimizeTerminal" => Some(KeyBinding::new(keystroke, MinimizeTerminal, context)),
        "FocusNextTerminal" => Some(KeyBinding::new(keystroke, FocusNextTerminal, context)),
        "FocusPrevTerminal" => Some(KeyBinding::new(keystroke, FocusPrevTerminal, context)),
        "FocusLeft" => Some(KeyBinding::new(keystroke, FocusLeft, context)),
        "FocusRight" => Some(KeyBinding::new(keystroke, FocusRight, context)),
        "FocusUp" => Some(KeyBinding::new(keystroke, FocusUp, context)),
        "FocusDown" => Some(KeyBinding::new(keystroke, FocusDown, context)),
        "NewProject" => Some(KeyBinding::new(keystroke, NewProject, context)),
        "CreateWorktree" => Some(KeyBinding::new(keystroke, CreateWorktree, context)),
        "ClearFocus" => Some(KeyBinding::new(keystroke, ClearFocus, context)),
        "FocusActiveProject" => Some(KeyBinding::new(keystroke, FocusActiveProject, context)),
        "Copy" => Some(KeyBinding::new(keystroke, Copy, context)),
        "Paste" => Some(KeyBinding::new(keystroke, Paste, context)),
        "ScrollUp" => Some(KeyBinding::new(keystroke, ScrollUp, context)),
        "ScrollDown" => Some(KeyBinding::new(keystroke, ScrollDown, context)),
        "Search" => Some(KeyBinding::new(keystroke, Search, context)),
        "SearchNext" => Some(KeyBinding::new(keystroke, SearchNext, context)),
        "SearchPrev" => Some(KeyBinding::new(keystroke, SearchPrev, context)),
        "JumpToPreviousPrompt" => Some(KeyBinding::new(keystroke, JumpToPreviousPrompt, context)),
        "JumpToNextPrompt" => Some(KeyBinding::new(keystroke, JumpToNextPrompt, context)),
        "CloseSearch" => Some(KeyBinding::new(keystroke, CloseSearch, context)),
        "ShowKeybindings" => Some(KeyBinding::new(keystroke, ShowKeybindings, context)),
        "ShowSessionManager" => Some(KeyBinding::new(keystroke, ShowSessionManager, context)),
        "ShowThemeSelector" => Some(KeyBinding::new(keystroke, ShowThemeSelector, context)),
        "ShowCommandPalette" => Some(KeyBinding::new(keystroke, ShowCommandPalette, context)),
        "ShowSettings" => Some(KeyBinding::new(keystroke, ShowSettings, context)),
        "OpenSettingsFile" => Some(KeyBinding::new(keystroke, OpenSettingsFile, context)),
        "ShowFileSearch" => Some(KeyBinding::new(keystroke, ShowFileSearch, context)),
        "ShowContentSearch" => Some(KeyBinding::new(keystroke, ShowContentSearch, context)),
        "ShowProjectSwitcher" => Some(KeyBinding::new(keystroke, ShowProjectSwitcher, context)),
        "ShowDiffViewer" => Some(KeyBinding::new(keystroke, ShowDiffViewer, context)),
        "SendTab" => Some(KeyBinding::new(keystroke, SendTab, context)),
        "ZoomIn" => Some(KeyBinding::new(keystroke, ZoomIn, context)),
        "ZoomOut" => Some(KeyBinding::new(keystroke, ZoomOut, context)),
        "ResetZoom" => Some(KeyBinding::new(keystroke, ResetZoom, context)),
        "CheckForUpdates" => Some(KeyBinding::new(keystroke, CheckForUpdates, context)),
        "InstallUpdate" => Some(KeyBinding::new(keystroke, InstallUpdate, context)),
        "FocusSidebar" => Some(KeyBinding::new(keystroke, FocusSidebar, context)),
        "SidebarUp" => Some(KeyBinding::new(keystroke, SidebarUp, context)),
        "SidebarDown" => Some(KeyBinding::new(keystroke, SidebarDown, context)),
        "SidebarConfirm" => Some(KeyBinding::new(keystroke, SidebarConfirm, context)),
        "SidebarToggleExpand" => Some(KeyBinding::new(keystroke, SidebarToggleExpand, context)),
        "SidebarEscape" => Some(KeyBinding::new(keystroke, SidebarEscape, context)),
        "ShowPairingDialog" => Some(KeyBinding::new(keystroke, ShowPairingDialog, context)),
        "TogglePaneSwitcher" => Some(KeyBinding::new(keystroke, TogglePaneSwitcher, context)),
        "StartAllServices" => Some(KeyBinding::new(keystroke, StartAllServices, context)),
        "StopAllServices" => Some(KeyBinding::new(keystroke, StopAllServices, context)),
        "EqualizeLayout" => Some(KeyBinding::new(keystroke, EqualizeLayout, context)),
        _ => {
            log::warn!("Unknown action in keybinding config: {}", action);
            None
        }
    }
}

/// Format a keystroke for display (convert to human-readable format)
pub fn format_keystroke(keystroke: &str) -> String {
    keystroke
        .replace("cmd", "⌘")
        .replace("ctrl", "Ctrl")
        .replace("alt", "Alt")
        .replace("shift", "⇧")
        .replace("-", "+")
        .replace("pageup", "PgUp")
        .replace("pagedown", "PgDn")
        .replace("escape", "Esc")
        .replace("left", "←")
        .replace("right", "→")
        .replace("up", "↑")
        .replace("down", "↓")
}
