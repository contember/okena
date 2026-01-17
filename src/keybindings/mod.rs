#![allow(static_mut_refs)]

mod config;

use gpui::*;

pub use config::{
    get_action_descriptions, get_keybindings_path, load_keybindings, save_keybindings,
    KeybindingConfig,
};

// Define actions
actions!(
    term_manager,
    [
        Quit,
        About,
        ToggleSidebar,
        ToggleSidebarAutoHide,
        ExitFullscreen,
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
        SendTab,
        SendBacktab,
    ]
);

/// Global keybinding configuration
static mut KEYBINDING_CONFIG: Option<KeybindingConfig> = None;

/// Get the current keybinding configuration
pub fn get_config() -> &'static KeybindingConfig {
    unsafe {
        KEYBINDING_CONFIG
            .as_ref()
            .expect("Keybinding config not initialized")
    }
}

/// Get the current keybinding configuration mutably
#[allow(dead_code)]
pub fn get_config_mut() -> &'static mut KeybindingConfig {
    unsafe {
        KEYBINDING_CONFIG
            .as_mut()
            .expect("Keybinding config not initialized")
    }
}

/// Reset keybindings to defaults and save
pub fn reset_to_defaults() -> anyhow::Result<()> {
    unsafe {
        KEYBINDING_CONFIG = Some(KeybindingConfig::defaults());
        save_keybindings(KEYBINDING_CONFIG.as_ref().unwrap())?;
    }
    Ok(())
}

/// Reload keybindings from disk
#[allow(dead_code)]
pub fn reload_keybindings() {
    let config = load_keybindings();
    unsafe {
        KEYBINDING_CONFIG = Some(config);
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

    // Store config globally
    unsafe {
        KEYBINDING_CONFIG = Some(config.clone());
    }

    // Register bindings from config
    register_bindings_from_config(cx, &config);

    // Register essential terminal keybindings that should not be overridden
    // Tab/Shift+Tab must be captured to prevent GPUI's focus navigation from consuming them
    cx.bind_keys([
        KeyBinding::new("tab", SendTab, Some("TerminalPane")),
        KeyBinding::new("tab", SendTab, Some("FullscreenTerminal")),
        KeyBinding::new("shift-tab", SendBacktab, Some("TerminalPane")),
        KeyBinding::new("shift-tab", SendBacktab, Some("FullscreenTerminal")),
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
        "ExitFullscreen" => Some(KeyBinding::new(keystroke, ExitFullscreen, context)),
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
        "SendTab" => Some(KeyBinding::new(keystroke, SendTab, context)),
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
