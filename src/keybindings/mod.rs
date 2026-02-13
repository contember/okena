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

// Define actions
actions!(
    okena,
    [
        Quit,
        About,
        Cancel,
        SendEscape,
        ToggleSidebar,
        ToggleSidebarAutoHide,
        ToggleFullscreen,
        FullscreenNextTerminal,
        FullscreenPrevTerminal,
        SplitVertical,
        SplitHorizontal,
        AddTab,
        CreateGrid,
        CloseTerminal,
        MinimizeTerminal,
        FocusNextTerminal,
        FocusPrevTerminal,
        FocusLeft,
        FocusRight,
        FocusUp,
        FocusDown,
        NewProject,
        CreateWorktree,
        ClearFocus,
        Copy,
        Paste,
        ScrollUp,
        ScrollDown,
        Search,
        SearchNext,
        SearchPrev,
        CloseSearch,
        ShowKeybindings,
        ShowSessionManager,
        ShowThemeSelector,
        ShowCommandPalette,
        ShowSettings,
        OpenSettingsFile,
        ShowFileSearch,
        ShowProjectSwitcher,
        ShowDiffViewer,
        SendTab,
        SendBacktab,
        ZoomIn,
        ZoomOut,
        ResetZoom,
        CheckForUpdates,
        InstallUpdate,
        FocusSidebar,
        SidebarUp,
        SidebarDown,
        SidebarConfirm,
        SidebarToggleExpand,
        SidebarEscape,
        ShowPairingDialog,
    ]
);

/// Global keybinding configuration (thread-safe)
static KEYBINDING_CONFIG: RwLock<Option<KeybindingConfig>> = RwLock::new(None);

/// Get a read guard to the current keybinding configuration
///
/// Returns a guard that dereferences to KeybindingConfig.
/// The guard must be held for the duration of access.
pub fn get_config() -> impl std::ops::Deref<Target = KeybindingConfig> {
    parking_lot::RwLockReadGuard::map(KEYBINDING_CONFIG.read(), |opt| {
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
        KeyBinding::new("escape", Cancel, Some("TerminalRename")),
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
        "CreateGrid" => Some(KeyBinding::new(keystroke, CreateGrid, context)),
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
        "Copy" => Some(KeyBinding::new(keystroke, Copy, context)),
        "Paste" => Some(KeyBinding::new(keystroke, Paste, context)),
        "ScrollUp" => Some(KeyBinding::new(keystroke, ScrollUp, context)),
        "ScrollDown" => Some(KeyBinding::new(keystroke, ScrollDown, context)),
        "Search" => Some(KeyBinding::new(keystroke, Search, context)),
        "SearchNext" => Some(KeyBinding::new(keystroke, SearchNext, context)),
        "SearchPrev" => Some(KeyBinding::new(keystroke, SearchPrev, context)),
        "CloseSearch" => Some(KeyBinding::new(keystroke, CloseSearch, context)),
        "ShowKeybindings" => Some(KeyBinding::new(keystroke, ShowKeybindings, context)),
        "ShowSessionManager" => Some(KeyBinding::new(keystroke, ShowSessionManager, context)),
        "ShowThemeSelector" => Some(KeyBinding::new(keystroke, ShowThemeSelector, context)),
        "ShowCommandPalette" => Some(KeyBinding::new(keystroke, ShowCommandPalette, context)),
        "ShowSettings" => Some(KeyBinding::new(keystroke, ShowSettings, context)),
        "OpenSettingsFile" => Some(KeyBinding::new(keystroke, OpenSettingsFile, context)),
        "ShowFileSearch" => Some(KeyBinding::new(keystroke, ShowFileSearch, context)),
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
