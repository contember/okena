mod config;

use gpui::*;
use parking_lot::RwLock;

pub use config::{
    get_action_descriptions, get_keybindings_path, load_keybindings, save_keybindings,
    KeybindingConfig,
};

// Define actions
actions!(
    okena,
    [
        Quit,
        About,
        ToggleSidebar,
        ToggleSidebarAutoHide,
        ToggleFullscreen,
        FullscreenNextTerminal,
        FullscreenPrevTerminal,
        SplitVertical,
        SplitHorizontal,
        AddTab,
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
    ]
);

/// Pending diff viewer path (set before dispatching ShowDiffViewer from project header).
static PENDING_DIFF_PATH: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);

/// Pending diff file to select (set before dispatching ShowDiffViewer).
static PENDING_DIFF_FILE: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);

/// Set a pending diff path to be used by the next ShowDiffViewer action.
pub fn set_pending_diff_path(path: String) {
    *PENDING_DIFF_PATH.lock() = Some(path);
}

/// Set a pending diff file to select in the diff viewer.
pub fn set_pending_diff_file(file: String) {
    *PENDING_DIFF_FILE.lock() = Some(file);
}

/// Take the pending diff path (returns and clears it).
pub fn take_pending_diff_path() -> Option<String> {
    PENDING_DIFF_PATH.lock().take()
}

/// Take the pending diff file (returns and clears it).
pub fn take_pending_diff_file() -> Option<String> {
    PENDING_DIFF_FILE.lock().take()
}

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

/// Get a write guard to the current keybinding configuration
#[allow(dead_code)]
pub fn get_config_mut() -> impl std::ops::DerefMut<Target = KeybindingConfig> {
    parking_lot::RwLockWriteGuard::map(KEYBINDING_CONFIG.write(), |opt| {
        opt.as_mut().expect("Keybinding config not initialized")
    })
}

/// Reset keybindings to defaults and save
pub fn reset_to_defaults() -> anyhow::Result<()> {
    let config = KeybindingConfig::defaults();
    save_keybindings(&config)?;
    *KEYBINDING_CONFIG.write() = Some(config);
    Ok(())
}

/// Reload keybindings from disk
#[allow(dead_code)]
pub fn reload_keybindings() {
    let config = load_keybindings();
    *KEYBINDING_CONFIG.write() = Some(config);
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

/// Get the primary (first enabled) keybinding for an action
#[allow(dead_code)]
pub fn get_primary_binding(action: &str) -> Option<String> {
    let config = get_config();
    config
        .get_enabled_bindings(action)
        .first()
        .map(|e| format_keystroke(&e.keystroke))
}
