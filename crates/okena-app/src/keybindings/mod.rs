mod config;
mod descriptions;
mod start_page;
mod types;

use gpui::*;
use parking_lot::RwLock;
use std::rc::Rc;

pub use config::{
    KeybindingConfig, get_keybindings_path, load_keybindings, parse_keybindings,
    save_keybindings_if_unchanged,
};
pub use descriptions::get_action_descriptions;
pub use okena_ui::shortcuts::ShortcutPlatform;
pub use start_page::observe_start_page_shortcuts;
pub(crate) use start_page::{
    RunStartPageCommand, active_start_page_shortcut, start_page_button_for_shortcut,
    start_page_shortcut_error, start_page_shortcut_label, validate_start_page_shortcut,
};
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

pub use okena_views_git::{ToggleDiffIgnoreWhitespace, ToggleDiffLineWrap, ToggleDiffViewMode};

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
pub fn keystroke_to_config_string(keystroke: &gpui::Keystroke, os: OperatingSystem) -> String {
    let unparsed = keystroke.unparse(os);
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

    // Essential non-overridable bindings. Tab/Shift+Tab must be captured to
    // prevent GPUI's focus navigation from consuming them.
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

    // Escape keybindings with context-based precedence:
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
                Some(okena_views_git::DIFF_VIEWER_KEY_CONTEXT),
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
    start_page::register_start_page_shortcuts(cx);
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

    apply_keybindings(cx, &config);
}

/// Register keybindings from a configuration
fn register_bindings_from_config(cx: &mut App, config: &KeybindingConfig) {
    let bindings = bindings_for_platform(config, ShortcutPlatform::current(), cx);
    bind_managed_keys(cx, bindings);
}

fn bindings_for_platform(
    config: &KeybindingConfig,
    platform: ShortcutPlatform,
    cx: &App,
) -> Vec<KeyBinding> {
    let mut bindings = Vec::new();
    for (action, entries) in &config.bindings {
        for entry in entries {
            if !entry.enabled || !entry.is_supported_on(platform) {
                continue;
            }
            if let Some(binding) =
                create_keybinding(action, &entry.keystroke, entry.context.as_deref(), cx)
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
    cx: &App,
) -> Option<KeyBinding> {
    if keystroke.trim().is_empty() || keystroke == "unset" {
        log::warn!("Ignoring invalid keybinding for {action}");
        return None;
    }
    let action_box = action_for_name(action)?;
    let predicate = match context.map(KeyBindingContextPredicate::parse) {
        None => None,
        Some(Ok(predicate)) => Some(Rc::new(predicate)),
        Some(Err(err)) => {
            log::warn!("Invalid context in keybinding for {action}: {err}");
            return None;
        }
    };
    match KeyBinding::load(keystroke, action_box, predicate, false, None, cx) {
        Ok(binding) => Some(binding),
        Err(err) => {
            log::warn!("Invalid keystroke in keybinding for {action}: {err}");
            None
        }
    }
}

/// The action a keybinding config entry names, or `None` for an unknown name.
pub(crate) fn action_for_name(action: &str) -> Option<Box<dyn Action>> {
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
        "ExpandActiveProjects" => Some(Box::new(ExpandActiveProjects)),
        "CollapseActiveProjects" => Some(Box::new(CollapseActiveProjects)),
        "FocusSidebar" => Some(Box::new(FocusSidebar)),
        "SidebarUp" => Some(Box::new(SidebarUp)),
        "SidebarDown" => Some(Box::new(SidebarDown)),
        "ToggleDiffLineWrap" => Some(Box::new(ToggleDiffLineWrap)),
        "ToggleDiffIgnoreWhitespace" => Some(Box::new(ToggleDiffIgnoreWhitespace)),
        "ToggleDiffViewMode" => Some(Box::new(ToggleDiffViewMode)),
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
        "ShowHookLog" => Some(Box::new(ShowHookLog)),
        "RestartDaemon" => Some(Box::new(RestartDaemon)),
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

    #[gpui::test]
    fn registration_filters_platforms(cx: &mut gpui::TestAppContext) {
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
            let bindings = cx.update(|cx| bindings_for_platform(&config, platform, cx));
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

    #[gpui::test]
    fn diff_toggles_bind_in_the_diff_by_default_and_can_be_reassigned_or_disabled(
        cx: &mut gpui::TestAppContext,
    ) {
        let defaults = KeybindingConfig::defaults();
        let descriptions = super::get_action_descriptions();
        let diff_context = Some(okena_views_git::DIFF_VIEWER_KEY_CONTEXT);
        for (action, default_key) in [
            ("ToggleDiffLineWrap", "alt-z"),
            ("ToggleDiffIgnoreWhitespace", "w"),
            ("ToggleDiffViewMode", "s"),
        ] {
            assert!(descriptions.contains_key(action));
            let mut config = KeybindingConfig {
                version: defaults.version,
                bindings: std::collections::HashMap::from([(
                    action.into(),
                    defaults.bindings[action].clone(),
                )]),
            };

            cx.update(|cx| {
                let assert_bound =
                    |config: &KeybindingConfig, key: &'static str, context: Option<&str>| {
                        let bindings = bindings_for_platform(config, ShortcutPlatform::Windows, cx);
                        assert_eq!(bindings.len(), 1);
                        assert_eq!(
                            bindings[0].keystrokes()[0].as_keystroke(),
                            &gpui::Keystroke::parse(key).unwrap()
                        );
                        assert_eq!(
                            bindings[0].predicate().map(|p| p.to_string()).as_deref(),
                            context
                        );
                    };
                assert_bound(&config, default_key, diff_context);

                config.bindings.insert(
                    action.into(),
                    vec![KeybindingEntry::new("ctrl-f12", diff_context)],
                );
                assert_bound(&config, "ctrl-f12", diff_context);

                // An explicit global entry stays global, as the dialog and conflict check show it.
                config
                    .bindings
                    .insert(action.into(), vec![KeybindingEntry::new("ctrl-f12", None)]);
                assert_bound(&config, "ctrl-f12", None);

                config.bindings.get_mut(action).unwrap().clear();
                assert!(bindings_for_platform(&config, ShortcutPlatform::Windows, cx).is_empty());
            });
        }
    }

    #[gpui::test]
    fn custom_win_bindings_and_chords_never_register_on_windows(cx: &mut gpui::TestAppContext) {
        for source in ["cmd-alt-left", "win-x", "super-x", "ctrl-k cmd-s"] {
            let entry = KeybindingEntry::new(source, None);
            let config = KeybindingConfig {
                version: 1,
                bindings: std::collections::HashMap::from([("Quit".into(), vec![entry])]),
            };
            cx.update(|cx| {
                assert!(bindings_for_platform(&config, ShortcutPlatform::Windows, cx).is_empty());
                assert_eq!(
                    bindings_for_platform(&config, ShortcutPlatform::Mac, cx).len(),
                    1
                );
                assert_eq!(
                    bindings_for_platform(&config, ShortcutPlatform::Linux, cx).len(),
                    1
                );
            });
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

    /// Every dismissable popup needs an `escape` binding, or its `Cancel`
    /// handler is dead code — which is what happened to the terminal menu
    /// until it grew a submenu.
    #[gpui::test]
    fn dismissable_overlays_bind_escape(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            super::apply_keybindings(cx, &KeybindingConfig::defaults());
            let escape = [gpui::Keystroke::parse("escape").unwrap()];
            for context in [
                "TerminalMenu",
                "TerminalRename",
                "ShellSelectorOverlay",
                "SendComposer",
                "ContextMenu",
                "FolderContextMenu",
                "RemoteContextMenu",
            ] {
                let contexts = [gpui::KeyContext::parse(context).unwrap()];
                let keymap = cx.key_bindings();
                let keymap = keymap.borrow();
                let (bindings, _) = keymap.bindings_for_input(&escape, &contexts);
                assert!(
                    bindings.iter().any(|binding| binding
                        .predicate()
                        .is_some_and(|predicate| { predicate.to_string() == context })),
                    "{context} must bind escape"
                );
            }
        });
    }
}
