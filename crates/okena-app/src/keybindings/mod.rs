mod config;
mod descriptions;
mod types;

use gpui::*;
use parking_lot::RwLock;

pub use config::{
    KeybindingConfig, get_keybindings_path, load_keybindings, parse_keybindings,
    save_keybindings_if_unchanged,
};
pub use descriptions::get_action_descriptions;
pub use okena_ui::shortcuts::ShortcutPlatform;
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
        ExpandActiveProjects,
        CollapseActiveProjects,
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
pub fn reset_to_defaults(expected: Option<&str>) -> anyhow::Result<String> {
    let config = KeybindingConfig::defaults();
    let snapshot = config::save_keybindings_if_unchanged(&config, expected)?;
    *KEYBINDING_CONFIG.write() = Some(config);
    Ok(snapshot)
}

/// Convert a GPUI Keystroke to the config string format (e.g., "cmd-shift-d")
/// GPUI's unparse() uses "super-" on Linux for the platform modifier,
/// but our config format uses "cmd-" for cross-platform consistency.
pub fn keystroke_to_config_string(keystroke: &gpui::Keystroke) -> String {
    let unparsed = keystroke.unparse();
    // Normalize platform modifier names to "cmd-" for config consistency
    unparsed.replace("super-", "cmd-").replace("win-", "cmd-")
}

/// Reload a valid on-disk configuration without writing or reformatting the file.
/// Failures preserve the current configuration and live keymap.
pub fn refresh_keybindings_from_disk(cx: &mut App) -> anyhow::Result<Option<String>> {
    let path = get_keybindings_path();
    let (config, snapshot) = if path.exists() {
        let (config, snapshot) = config::load_keybindings_with_snapshot(&path)?;
        (config, Some(snapshot))
    } else {
        (KeybindingConfig::defaults(), None)
    };
    *KEYBINDING_CONFIG.write() = Some(config.clone());
    apply_keybindings(cx, &config);
    Ok(snapshot)
}

/// Apply already-saved inline changes without writing the file a second time.
pub fn apply_current_keybindings(cx: &mut App) {
    let config = get_config().clone();
    apply_keybindings(cx, &config);
}

// Metadata tags identify only our registrations so reloading preserves library keymaps.
const MANAGED_BINDING: KeyBindingMetaIndex = KeyBindingMetaIndex(u32::MAX);
fn bind_managed_keys(cx: &mut App, bindings: impl IntoIterator<Item = KeyBinding>) {
    cx.bind_keys(
        bindings
            .into_iter()
            .map(|binding| binding.with_meta(MANAGED_BINDING)),
    );
}

fn apply_keybindings(cx: &mut App, config: &KeybindingConfig) {
    // Clear existing bindings and re-register everything
    let preserved: Vec<_> = cx
        .key_bindings()
        .borrow()
        .bindings()
        .filter(|binding| binding.meta() != Some(MANAGED_BINDING))
        .cloned()
        .collect();
    cx.clear_key_bindings();
    cx.bind_keys(preserved);
    register_bindings_from_config(cx, config);

    // Re-register essential non-overridable bindings
    bind_managed_keys(
        cx,
        [
            KeyBinding::new("tab", SendTab, Some("TerminalPane")),
            KeyBinding::new("shift-tab", SendBacktab, Some("TerminalPane")),
            // Override gpui-component Root's global `tab` → focus_next so the
            // project switcher can use Tab to jump into a project's terminal.
            KeyBinding::new("tab", JumpToProjectTerminal, Some("ProjectSwitcher")),
        ],
    );

    bind_managed_keys(
        cx,
        [
            KeyBinding::new("up", SidebarUp, Some("Sidebar")),
            KeyBinding::new("down", SidebarDown, Some("Sidebar")),
            KeyBinding::new("enter", SidebarConfirm, Some("Sidebar")),
            KeyBinding::new("space", SidebarToggleExpand, Some("Sidebar")),
            KeyBinding::new("left", SidebarToggleExpand, Some("Sidebar")),
            KeyBinding::new("right", SidebarToggleExpand, Some("Sidebar")),
            KeyBinding::new("escape", SidebarEscape, Some("Sidebar")),
        ],
    );

    bind_managed_keys(
        cx,
        [
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
        ],
    );
}

/// Get a mutable reference to the global keybinding configuration.
/// Persist with a checked snapshot, then call apply_current_keybindings(cx).
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
    bind_managed_keys(
        cx,
        [
            KeyBinding::new("tab", SendTab, Some("TerminalPane")),
            KeyBinding::new("shift-tab", SendBacktab, Some("TerminalPane")),
            // Override gpui-component Root's global `tab` → focus_next so the
            // project switcher can use Tab to jump into a project's terminal.
            KeyBinding::new("tab", JumpToProjectTerminal, Some("ProjectSwitcher")),
        ],
    );

    // Register sidebar navigation keybindings (not user-configurable)
    bind_managed_keys(
        cx,
        [
            KeyBinding::new("up", SidebarUp, Some("Sidebar")),
            KeyBinding::new("down", SidebarDown, Some("Sidebar")),
            KeyBinding::new("enter", SidebarConfirm, Some("Sidebar")),
            KeyBinding::new("space", SidebarToggleExpand, Some("Sidebar")),
            KeyBinding::new("left", SidebarToggleExpand, Some("Sidebar")),
            KeyBinding::new("right", SidebarToggleExpand, Some("Sidebar")),
            KeyBinding::new("escape", SidebarEscape, Some("Sidebar")),
        ],
    );

    // Register escape keybindings with context-based precedence:
    //   Global:             escape → Cancel        (overlays, sidebar rename)
    //   TerminalPane:       escape → SendEscape    (send 0x1b to PTY)
    //   SearchBar:          escape → CloseSearch   (close search, deeper than TerminalPane)
    //   TerminalRename:     escape → Cancel        (cancel rename, deeper than TerminalPane)
    //   TerminalMenu:       escape → Cancel        (close the flyout, then the menu)
    bind_managed_keys(
        cx,
        [
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
        ],
    );
}

/// Register keybindings from a configuration
fn register_bindings_from_config(cx: &mut App, config: &KeybindingConfig) {
    bind_managed_keys(
        cx,
        bindings_for_platform(config, ShortcutPlatform::current()),
    );
}

fn bindings_for_platform(config: &KeybindingConfig, platform: ShortcutPlatform) -> Vec<KeyBinding> {
    let mut bindings = Vec::new();
    for (action, entries) in &config.bindings {
        for entry in entries {
            if !entry.enabled || !entry.is_supported_on(platform) {
                continue;
            }
            if let Some(binding) =
                create_keybinding(action, &entry.keystroke, entry.context.as_deref())
            {
                bindings.push(binding);
            }
        }
    }
    bindings
}

/// Create a KeyBinding from action name, keystroke, and context
pub(crate) fn create_keybinding(
    action: &str,
    keystroke: &str,
    context: Option<&str>,
) -> Option<KeyBinding> {
    if keystroke.trim().is_empty()
        || keystroke == "unset"
        || keystroke
            .split_whitespace()
            .any(|key| Keystroke::parse(key).is_err())
        || context.is_some_and(|context| KeyBindingContextPredicate::parse(context).is_err())
    {
        log::warn!("Ignoring invalid keybinding for {action}");
        return None;
    }

    // Map action names to actual actions
    match action {
        "Quit" => Some(KeyBinding::new(keystroke, Quit, context)),
        "Cancel" => Some(KeyBinding::new(keystroke, Cancel, context)),
        "SendEscape" => Some(KeyBinding::new(keystroke, SendEscape, context)),
        "ToggleSidebar" => Some(KeyBinding::new(keystroke, ToggleSidebar, context)),
        "ToggleSidebarAutoHide" => Some(KeyBinding::new(keystroke, ToggleSidebarAutoHide, context)),
        "ToggleFullscreen" => Some(KeyBinding::new(keystroke, ToggleFullscreen, context)),
        "FullscreenNextTerminal" => {
            Some(KeyBinding::new(keystroke, FullscreenNextTerminal, context))
        }
        "FullscreenPrevTerminal" => {
            Some(KeyBinding::new(keystroke, FullscreenPrevTerminal, context))
        }
        "SplitVertical" => Some(KeyBinding::new(keystroke, SplitVertical, context)),
        "SplitHorizontal" => Some(KeyBinding::new(keystroke, SplitHorizontal, context)),
        "AddTab" => Some(KeyBinding::new(keystroke, AddTab, context)),
        "CloseTerminal" => Some(KeyBinding::new(keystroke, CloseTerminal, context)),
        "MinimizeTerminal" => Some(KeyBinding::new(keystroke, MinimizeTerminal, context)),
        "ExportTerminalBuffer" => Some(KeyBinding::new(keystroke, ExportTerminalBuffer, context)),
        "DetachTerminal" => Some(KeyBinding::new(keystroke, DetachTerminal, context)),
        "ToggleUnread" => Some(KeyBinding::new(keystroke, ToggleUnread, context)),
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
        "AnnotateSelection" => Some(KeyBinding::new(
            keystroke,
            okena_views_terminal::actions::AnnotateSelection,
            context,
        )),
        "Paste" => Some(KeyBinding::new(keystroke, Paste, context)),
        "ScrollUp" => Some(KeyBinding::new(keystroke, ScrollUp, context)),
        "ScrollDown" => Some(KeyBinding::new(keystroke, ScrollDown, context)),
        "Search" => Some(KeyBinding::new(keystroke, Search, context)),
        "SearchNext" => Some(KeyBinding::new(keystroke, SearchNext, context)),
        "SearchPrev" => Some(KeyBinding::new(keystroke, SearchPrev, context)),
        "JumpToPreviousPrompt" => Some(KeyBinding::new(keystroke, JumpToPreviousPrompt, context)),
        "JumpToNextPrompt" => Some(KeyBinding::new(keystroke, JumpToNextPrompt, context)),
        "JumpToPreviousFailedCommand" => Some(KeyBinding::new(
            keystroke,
            JumpToPreviousFailedCommand,
            context,
        )),
        "JumpToNextFailedCommand" => {
            Some(KeyBinding::new(keystroke, JumpToNextFailedCommand, context))
        }
        "CloseSearch" => Some(KeyBinding::new(keystroke, CloseSearch, context)),
        "ShowKeybindings" => Some(KeyBinding::new(keystroke, ShowKeybindings, context)),
        "ShowWhatsNew" => Some(KeyBinding::new(keystroke, ShowWhatsNew, context)),
        "ShowSessionManager" => Some(KeyBinding::new(keystroke, ShowSessionManager, context)),
        "ShowThemeSelector" => Some(KeyBinding::new(keystroke, ShowThemeSelector, context)),
        "ShowCommandPalette" => Some(KeyBinding::new(keystroke, ShowCommandPalette, context)),
        "ShowSettings" => Some(KeyBinding::new(keystroke, ShowSettings, context)),
        "OpenSettingsFile" => Some(KeyBinding::new(keystroke, OpenSettingsFile, context)),
        "ShowFileSearch" => Some(KeyBinding::new(keystroke, ShowFileSearch, context)),
        "ShowContentSearch" => Some(KeyBinding::new(keystroke, ShowContentSearch, context)),
        "ShowProjectSwitcher" => Some(KeyBinding::new(keystroke, ShowProjectSwitcher, context)),
        "ShowDiffViewer" => Some(KeyBinding::new(keystroke, ShowDiffViewer, context)),
        "ReviewChanges" => Some(KeyBinding::new(keystroke, ReviewChanges, context)),
        "SendTab" => Some(KeyBinding::new(keystroke, SendTab, context)),
        "ZoomIn" => Some(KeyBinding::new(keystroke, ZoomIn, context)),
        "ZoomOut" => Some(KeyBinding::new(keystroke, ZoomOut, context)),
        "ResetZoom" => Some(KeyBinding::new(keystroke, ResetZoom, context)),
        "CheckForUpdates" => Some(KeyBinding::new(keystroke, CheckForUpdates, context)),
        "InstallUpdate" => Some(KeyBinding::new(keystroke, InstallUpdate, context)),
        "ExpandActiveProjects" => Some(KeyBinding::new(keystroke, ExpandActiveProjects, context)),
        "CollapseActiveProjects" => {
            Some(KeyBinding::new(keystroke, CollapseActiveProjects, context))
        }
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
        "ToggleProjectLayout" => Some(KeyBinding::new(keystroke, ToggleProjectLayout, context)),
        "ToggleProjectVisibility" => {
            Some(KeyBinding::new(keystroke, ToggleProjectVisibility, context))
        }
        "ShowBranchSwitcher" => Some(KeyBinding::new(keystroke, ShowBranchSwitcher, context)),
        "ShowProfileManager" => Some(KeyBinding::new(keystroke, ShowProfileManager, context)),
        "ShowLogConsole" => Some(KeyBinding::new(keystroke, ShowLogConsole, context)),
        "ShowAiProxyActivity" => Some(KeyBinding::new(keystroke, ShowAiProxyActivity, context)),
        "ToggleStatusLanes" => Some(KeyBinding::new(keystroke, ToggleStatusLanes, context)),
        "NewWindow" => Some(KeyBinding::new(keystroke, NewWindow, context)),
        "ShowHookLog" => Some(KeyBinding::new(keystroke, ShowHookLog, context)),
        "RestartDaemon" => Some(KeyBinding::new(keystroke, RestartDaemon, context)),
        _ => {
            log::warn!("Unknown action in keybinding config: {}", action);
            None
        }
    }
}

pub use okena_ui::shortcuts::format_keystroke;

#[cfg(test)]
pub(crate) fn set_test_keybinding_config(config: KeybindingConfig) {
    *KEYBINDING_CONFIG.write() = Some(config);
}

#[cfg(test)]
mod escape_binding_tests {
    use super::{KeybindingConfig, KeybindingEntry, ShortcutPlatform, bindings_for_platform};
    use gpui::AsKeystroke;

    #[test]
    fn registration_filters_platforms() {
        for platform in [
            ShortcutPlatform::Mac,
            ShortcutPlatform::Windows,
            ShortcutPlatform::Linux,
        ] {
            let entries = vec![
                KeybindingEntry::for_platforms("cmd-d", None, &[ShortcutPlatform::Mac]),
                KeybindingEntry::for_platforms(
                    "ctrl-d",
                    None,
                    &[ShortcutPlatform::Windows, ShortcutPlatform::Linux],
                ),
                KeybindingEntry::for_platforms("alt-d", None, &[]),
            ];
            let config = KeybindingConfig {
                version: 1,
                bindings: std::collections::HashMap::from([(
                    "SplitVertical".into(),
                    entries.clone(),
                )]),
            };
            let bindings = bindings_for_platform(&config, platform);
            assert_eq!(bindings.len(), 1);
            let expected = if platform == ShortcutPlatform::Mac {
                "cmd-d"
            } else {
                "ctrl-d"
            };
            assert_eq!(
                bindings[0].keystrokes()[0].as_keystroke(),
                &gpui::Keystroke::parse(expected).unwrap()
            );
        }
    }

    #[test]
    fn custom_win_bindings_and_chords_never_register_on_windows() {
        for source in ["cmd-alt-left", "win-x", "super-x", "ctrl-k cmd-s"] {
            let entry = KeybindingEntry::new(source, None);
            let config = KeybindingConfig {
                version: 1,
                bindings: std::collections::HashMap::from([("Quit".into(), vec![entry])]),
            };
            assert!(bindings_for_platform(&config, ShortcutPlatform::Windows).is_empty());
            assert_eq!(
                bindings_for_platform(&config, ShortcutPlatform::Mac).len(),
                1
            );
            assert_eq!(
                bindings_for_platform(&config, ShortcutPlatform::Linux).len(),
                1
            );
        }
    }

    #[gpui::test]
    fn reload_preserves_other_components_bindings(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.bind_keys([gpui::KeyBinding::new("ctrl-f12", super::Quit, None)]);
            super::apply_keybindings(cx, &KeybindingConfig::defaults());
            super::apply_keybindings(cx, &KeybindingConfig::defaults());
            assert_eq!(
                cx.key_bindings()
                    .borrow()
                    .bindings()
                    .filter(|binding| binding.meta().is_none())
                    .count(),
                1
            );
        });
    }

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
