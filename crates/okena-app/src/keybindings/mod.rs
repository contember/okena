mod config;
mod descriptions;
mod types;

use gpui::*;
use parking_lot::RwLock;
use std::rc::Rc;

pub use config::{KeybindingConfig, get_keybindings_path, load_keybindings, save_keybindings};
pub use descriptions::get_action_descriptions;
#[allow(unused_imports)]
pub use types::{ActionDescription, KeybindingConflict, KeybindingEntry};

// App-level actions (handled by window view, overlay manager, sidebar)
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
        CloseWindow,
        FocusActiveProject,
        ShowKeybindings,
        ShowWhatsNew,
        ShowSessionManager,
        ShowThemeSelector,
        ShowCommandPalette,
        ShowSettings,
        OpenSettingsFile,
        ShowFileSearch,
        ShowContentSearch,
        ShowProjectSwitcher,
        JumpToProjectTerminal,
        ShowDiffViewer,
        ReviewChanges,
        CheckForUpdates,
        InstallUpdate,
        FocusSidebar,
        ShowPairingDialog,
        TogglePaneSwitcher,
        StartAllServices,
        StopAllServices,
        ShowHookLog,
        ShowLogConsole,
        ShowAiProxyActivity,
        ToggleStatusLanes,
        EqualizeLayout,
        ToggleProjectLayout,
        ToggleProjectVisibility,
        ShowBranchSwitcher,
        ShowProfileManager,
        NewWindow,
        RestartDaemon,
    ]
);

// Terminal-specific actions (defined in okena-views-terminal crate)
pub use okena_views_terminal::actions::{
    AddTab, CloseSearch, CloseTerminal, Copy, DetachTerminal, ExportTerminalBuffer, FocusDown,
    FocusLeft, FocusNextTerminal, FocusPrevTerminal, FocusRight, FocusUp, FullscreenNextTerminal,
    FullscreenPrevTerminal, JumpToNextFailedCommand, JumpToNextPrompt, JumpToPreviousFailedCommand,
    JumpToPreviousPrompt, MinimizeTerminal, Paste, ResetZoom, ScrollDown, ScrollUp, Search,
    SearchNext, SearchPrev, SendBacktab, SendEscape, SendTab, SplitHorizontal, SplitVertical,
    ToggleFullscreen, ToggleUnread, ZoomIn, ZoomOut,
};

// Sidebar-specific actions (defined in okena-views-sidebar crate)
pub use okena_views_sidebar::{
    SidebarConfirm, SidebarDown, SidebarEscape, SidebarToggleExpand, SidebarUp,
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
pub fn keystroke_to_config_string(keystroke: &gpui::Keystroke, os: OperatingSystem) -> String {
    let unparsed = keystroke.unparse(os);
    // Normalize platform modifier names to "cmd-" for config consistency
    unparsed.replace("super-", "cmd-").replace("win-", "cmd-")
}

/// Reload keybindings: update global config, save to disk, and re-register with GPUI.
/// Call this after modifying the config via get_config_mut() or update_config().
pub fn reload_keybindings(cx: &mut App) {
    let config = {
        KEYBINDING_CONFIG
            .read()
            .as_ref()
            .cloned()
            .unwrap_or_default()
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
        // Override gpui-component Root's global `tab` → focus_next so the
        // project switcher can use Tab to jump into a project's terminal.
        KeyBinding::new("tab", JumpToProjectTerminal, Some("ProjectSwitcher")),
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
        KeyBinding::new(
            "escape",
            okena_views_terminal::actions::Cancel,
            Some("TerminalRename"),
        ),
        KeyBinding::new(
            "escape",
            okena_files::file_search::Cancel,
            Some("FileSearchDialog"),
        ),
        KeyBinding::new(
            "escape",
            okena_files::file_search::Cancel,
            Some("FileViewer"),
        ),
        KeyBinding::new("escape", okena_views_git::Cancel, Some("WorktreeDialog")),
        KeyBinding::new(
            "escape",
            okena_views_git::Cancel,
            Some("CloseWorktreeDialog"),
        ),
        KeyBinding::new(
            "escape",
            okena_views_git::diff_viewer::Cancel,
            Some("DiffViewer"),
        ),
        KeyBinding::new("escape", okena_views_sidebar::Cancel, Some("ContextMenu")),
        KeyBinding::new(
            "escape",
            okena_views_sidebar::Cancel,
            Some("FolderContextMenu"),
        ),
        KeyBinding::new(
            "escape",
            okena_views_sidebar::Cancel,
            Some("RenameDirectoryDialog"),
        ),
        KeyBinding::new(
            "escape",
            okena_views_sidebar::Cancel,
            Some("ChangePathDialog"),
        ),
        KeyBinding::new("escape", okena_views_sidebar::Cancel, Some("HookLog")),
        KeyBinding::new(
            "escape",
            okena_views_terminal::actions::Cancel,
            Some("ShellSelectorOverlay"),
        ),
        // The terminal menu peels an open submenu flyout first, then closes itself.
        KeyBinding::new(
            "escape",
            okena_views_terminal::actions::Cancel,
            Some("TerminalMenu"),
        ),
        KeyBinding::new(
            "escape",
            okena_views_terminal::actions::Cancel,
            Some("SendComposer"),
        ),
        KeyBinding::new(
            "escape",
            okena_views_remote::Cancel,
            Some("RemoteConnectDialog"),
        ),
        KeyBinding::new(
            "escape",
            okena_views_remote::Cancel,
            Some("RemotePairDialog"),
        ),
        KeyBinding::new(
            "escape",
            okena_views_remote::Cancel,
            Some("RemoteContextMenu"),
        ),
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
        // Override gpui-component Root's global `tab` → focus_next so the
        // project switcher can use Tab to jump into a project's terminal.
        KeyBinding::new("tab", JumpToProjectTerminal, Some("ProjectSwitcher")),
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
    //   TerminalMenu:       escape → Cancel        (close the flyout, then the menu)
    cx.bind_keys([
        KeyBinding::new("escape", Cancel, None),
        KeyBinding::new("escape", SendEscape, Some("TerminalPane")),
        KeyBinding::new("escape", CloseSearch, Some("SearchBar")),
        // Terminal rename uses the crate's Cancel action
        KeyBinding::new(
            "escape",
            okena_views_terminal::actions::Cancel,
            Some("TerminalRename"),
        ),
        // okena-files crate Cancel action for file search/viewer
        KeyBinding::new(
            "escape",
            okena_files::file_search::Cancel,
            Some("FileSearchDialog"),
        ),
        KeyBinding::new(
            "escape",
            okena_files::file_search::Cancel,
            Some("FileViewer"),
        ),
        // okena-views-git crate Cancel actions for git overlays
        KeyBinding::new("escape", okena_views_git::Cancel, Some("WorktreeDialog")),
        KeyBinding::new(
            "escape",
            okena_views_git::Cancel,
            Some("CloseWorktreeDialog"),
        ),
        KeyBinding::new(
            "escape",
            okena_views_git::diff_viewer::Cancel,
            Some("DiffViewer"),
        ),
        // okena-views-sidebar crate Cancel actions for context menus
        KeyBinding::new("escape", okena_views_sidebar::Cancel, Some("ContextMenu")),
        KeyBinding::new(
            "escape",
            okena_views_sidebar::Cancel,
            Some("FolderContextMenu"),
        ),
        KeyBinding::new(
            "escape",
            okena_views_sidebar::Cancel,
            Some("RenameDirectoryDialog"),
        ),
        KeyBinding::new(
            "escape",
            okena_views_sidebar::Cancel,
            Some("ChangePathDialog"),
        ),
        KeyBinding::new("escape", okena_views_sidebar::Cancel, Some("HookLog")),
        // okena-views-terminal crate Cancel for shell selector + send composer
        KeyBinding::new(
            "escape",
            okena_views_terminal::actions::Cancel,
            Some("ShellSelectorOverlay"),
        ),
        // The terminal menu peels an open submenu flyout first, then closes itself.
        KeyBinding::new(
            "escape",
            okena_views_terminal::actions::Cancel,
            Some("TerminalMenu"),
        ),
        KeyBinding::new(
            "escape",
            okena_views_terminal::actions::Cancel,
            Some("SendComposer"),
        ),
        // okena-views-remote crate Cancel actions
        KeyBinding::new(
            "escape",
            okena_views_remote::Cancel,
            Some("RemoteConnectDialog"),
        ),
        KeyBinding::new(
            "escape",
            okena_views_remote::Cancel,
            Some("RemotePairDialog"),
        ),
        KeyBinding::new(
            "escape",
            okena_views_remote::Cancel,
            Some("RemoteContextMenu"),
        ),
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

            let Some(action_box) = action_for_name(action) else {
                continue;
            };
            let predicate = match entry
                .context
                .as_deref()
                .map(KeyBindingContextPredicate::parse)
            {
                None => None,
                Some(Ok(predicate)) => Some(Rc::new(predicate)),
                Some(Err(err)) => {
                    log::warn!("Invalid context in keybinding for {action}: {err}");
                    continue;
                }
            };
            match KeyBinding::load(&entry.keystroke, action_box, predicate, false, None, cx) {
                Ok(binding) => bindings.push(binding),
                Err(err) => log::warn!("Invalid keystroke in keybinding for {action}: {err}"),
            }
        }
    }

    // Register all bindings
    cx.bind_keys(bindings);
}

/// The action a keybinding config entry names, or `None` for an unknown name.
fn action_for_name(action: &str) -> Option<Box<dyn Action>> {
    match action {
        "Quit" => Some(Box::new(Quit)),
        "Cancel" => Some(Box::new(Cancel)),
        "SendEscape" => Some(Box::new(SendEscape)),
        "ToggleSidebar" => Some(Box::new(ToggleSidebar)),
        "ToggleSidebarAutoHide" => Some(Box::new(ToggleSidebarAutoHide)),
        "ToggleFullscreen" => Some(Box::new(ToggleFullscreen)),
        "FullscreenNextTerminal" => Some(Box::new(FullscreenNextTerminal)),
        "FullscreenPrevTerminal" => Some(Box::new(FullscreenPrevTerminal)),
        "SplitVertical" => Some(Box::new(SplitVertical)),
        "SplitHorizontal" => Some(Box::new(SplitHorizontal)),
        "AddTab" => Some(Box::new(AddTab)),
        "CloseTerminal" => Some(Box::new(CloseTerminal)),
        "MinimizeTerminal" => Some(Box::new(MinimizeTerminal)),
        "ExportTerminalBuffer" => Some(Box::new(ExportTerminalBuffer)),
        "DetachTerminal" => Some(Box::new(DetachTerminal)),
        "ToggleUnread" => Some(Box::new(ToggleUnread)),
        "FocusNextTerminal" => Some(Box::new(FocusNextTerminal)),
        "FocusPrevTerminal" => Some(Box::new(FocusPrevTerminal)),
        "FocusLeft" => Some(Box::new(FocusLeft)),
        "FocusRight" => Some(Box::new(FocusRight)),
        "FocusUp" => Some(Box::new(FocusUp)),
        "FocusDown" => Some(Box::new(FocusDown)),
        "NewProject" => Some(Box::new(NewProject)),
        "CreateWorktree" => Some(Box::new(CreateWorktree)),
        "ClearFocus" => Some(Box::new(ClearFocus)),
        "FocusActiveProject" => Some(Box::new(FocusActiveProject)),
        "Copy" => Some(Box::new(Copy)),
        "AnnotateSelection" => Some(Box::new(okena_views_terminal::actions::AnnotateSelection)),
        "Paste" => Some(Box::new(Paste)),
        "ScrollUp" => Some(Box::new(ScrollUp)),
        "ScrollDown" => Some(Box::new(ScrollDown)),
        "Search" => Some(Box::new(Search)),
        "SearchNext" => Some(Box::new(SearchNext)),
        "SearchPrev" => Some(Box::new(SearchPrev)),
        "JumpToPreviousPrompt" => Some(Box::new(JumpToPreviousPrompt)),
        "JumpToNextPrompt" => Some(Box::new(JumpToNextPrompt)),
        "JumpToPreviousFailedCommand" => Some(Box::new(JumpToPreviousFailedCommand)),
        "JumpToNextFailedCommand" => Some(Box::new(JumpToNextFailedCommand)),
        "CloseSearch" => Some(Box::new(CloseSearch)),
        "ShowKeybindings" => Some(Box::new(ShowKeybindings)),
        "ShowWhatsNew" => Some(Box::new(ShowWhatsNew)),
        "ShowSessionManager" => Some(Box::new(ShowSessionManager)),
        "ShowThemeSelector" => Some(Box::new(ShowThemeSelector)),
        "ShowCommandPalette" => Some(Box::new(ShowCommandPalette)),
        "ShowSettings" => Some(Box::new(ShowSettings)),
        "OpenSettingsFile" => Some(Box::new(OpenSettingsFile)),
        "ShowFileSearch" => Some(Box::new(ShowFileSearch)),
        "ShowContentSearch" => Some(Box::new(ShowContentSearch)),
        "ShowProjectSwitcher" => Some(Box::new(ShowProjectSwitcher)),
        "ShowDiffViewer" => Some(Box::new(ShowDiffViewer)),
        "ReviewChanges" => Some(Box::new(ReviewChanges)),
        "SendTab" => Some(Box::new(SendTab)),
        "ZoomIn" => Some(Box::new(ZoomIn)),
        "ZoomOut" => Some(Box::new(ZoomOut)),
        "ResetZoom" => Some(Box::new(ResetZoom)),
        "CheckForUpdates" => Some(Box::new(CheckForUpdates)),
        "InstallUpdate" => Some(Box::new(InstallUpdate)),
        "FocusSidebar" => Some(Box::new(FocusSidebar)),
        "SidebarUp" => Some(Box::new(SidebarUp)),
        "SidebarDown" => Some(Box::new(SidebarDown)),
        "SidebarConfirm" => Some(Box::new(SidebarConfirm)),
        "SidebarToggleExpand" => Some(Box::new(SidebarToggleExpand)),
        "SidebarEscape" => Some(Box::new(SidebarEscape)),
        "ShowPairingDialog" => Some(Box::new(ShowPairingDialog)),
        "TogglePaneSwitcher" => Some(Box::new(TogglePaneSwitcher)),
        "StartAllServices" => Some(Box::new(StartAllServices)),
        "StopAllServices" => Some(Box::new(StopAllServices)),
        "EqualizeLayout" => Some(Box::new(EqualizeLayout)),
        "ToggleProjectLayout" => Some(Box::new(ToggleProjectLayout)),
        "ToggleProjectVisibility" => Some(Box::new(ToggleProjectVisibility)),
        "ShowBranchSwitcher" => Some(Box::new(ShowBranchSwitcher)),
        "ShowProfileManager" => Some(Box::new(ShowProfileManager)),
        "ShowLogConsole" => Some(Box::new(ShowLogConsole)),
        "ShowAiProxyActivity" => Some(Box::new(ShowAiProxyActivity)),
        "ToggleStatusLanes" => Some(Box::new(ToggleStatusLanes)),
        "NewWindow" => Some(Box::new(NewWindow)),
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

/// The active shortcut for an action, formatted for the current platform.
pub fn shortcut_for_action(action: &str) -> Option<String> {
    let config = get_config();
    let entries = config.bindings.get(action)?;
    let chosen = if cfg!(target_os = "macos") {
        entries.iter().find(|entry| entry.enabled)
    } else {
        entries
            .iter()
            .find(|entry| entry.enabled && entry.keystroke.contains("ctrl"))
            .or_else(|| entries.iter().find(|entry| entry.enabled))
    }?;
    Some(format_keystroke(&chosen.keystroke))
}

#[cfg(test)]
mod escape_binding_tests {
    /// Every dismissable popup needs an `escape` binding in *both* the initial
    /// registration and the reload path, or its `Cancel` handler is dead code —
    /// which is exactly what happened to the terminal menu until it grew a submenu.
    #[test]
    fn dismissable_overlays_bind_escape_in_both_paths() {
        let src = include_str!("mod.rs");

        for context in [
            "TerminalMenu",
            "TerminalRename",
            "ShellSelectorOverlay",
            "SendComposer",
            "ContextMenu",
            "FolderContextMenu",
            "RemoteContextMenu",
        ] {
            let bound = src.matches(&format!("Some(\"{context}\")")).count();
            assert!(
                bound >= 2,
                "{context} must bind escape in register_keybindings and reload_keybindings \
                 (found {bound} occurrences)"
            );
        }
    }
}
