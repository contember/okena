use okena_ui::shortcuts::ShortcutPlatform;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::types::{KeybindingConflict, KeybindingEntry};

fn cmd_default(keystroke: &str, context: Option<&str>) -> KeybindingEntry {
    non_windows_default(keystroke, context)
}

fn ctrl_default(keystroke: &str, context: Option<&str>) -> KeybindingEntry {
    KeybindingEntry::new(keystroke, context)
}

// Preserve existing Mac/Linux navigation without claiming Windows system shortcuts.
fn non_windows_default(keystroke: &str, context: Option<&str>) -> KeybindingEntry {
    KeybindingEntry::for_platforms(
        keystroke,
        context,
        &[ShortcutPlatform::Mac, ShortcutPlatform::Linux],
    )
}

/// Complete keybinding configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeybindingConfig {
    /// Version for config migration
    #[serde(default = "default_version")]
    pub version: u32,
    /// Map from action name to list of keybindings
    pub bindings: HashMap<String, Vec<KeybindingEntry>>,
}

fn default_version() -> u32 {
    1
}

impl Default for KeybindingConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

impl KeybindingConfig {
    /// Create default keybinding configuration
    pub fn defaults() -> Self {
        let mut bindings = HashMap::new();

        // Global keybindings
        bindings.insert(
            "Quit".to_string(),
            vec![cmd_default("cmd-q", None), ctrl_default("ctrl-q", None)],
        );
        bindings.insert(
            "ToggleSidebar".to_string(),
            vec![cmd_default("cmd-b", None), ctrl_default("ctrl-b", None)],
        );
        bindings.insert(
            "ToggleSidebarAutoHide".to_string(),
            vec![
                cmd_default("cmd-shift-b", None),
                ctrl_default("ctrl-shift-b", None),
            ],
        );
        bindings.insert(
            "FocusSidebar".to_string(),
            vec![cmd_default("cmd-1", None), ctrl_default("ctrl-1", None)],
        );
        bindings.insert(
            "ClearFocus".to_string(),
            vec![cmd_default("cmd-0", None), ctrl_default("ctrl-0", None)],
        );
        bindings.insert(
            "FocusActiveProject".to_string(),
            vec![
                cmd_default("cmd-shift-0", None),
                ctrl_default("ctrl-shift-0", None),
            ],
        );
        bindings.insert(
            "ShowKeybindings".to_string(),
            vec![
                cmd_default("cmd-k cmd-s", None),
                ctrl_default("ctrl-k ctrl-s", None),
            ],
        );
        bindings.insert(
            "ShowSessionManager".to_string(),
            vec![
                cmd_default("cmd-k cmd-w", None),
                ctrl_default("ctrl-k ctrl-w", None),
            ],
        );
        bindings.insert(
            "ShowThemeSelector".to_string(),
            vec![
                cmd_default("cmd-k cmd-t", None),
                ctrl_default("ctrl-k ctrl-t", None),
            ],
        );
        bindings.insert(
            "ShowCommandPalette".to_string(),
            vec![
                cmd_default("cmd-shift-p", None),
                ctrl_default("ctrl-shift-p", None),
            ],
        );
        bindings.insert(
            "ShowSettings".to_string(),
            vec![cmd_default("cmd-,", None), ctrl_default("ctrl-,", None)],
        );
        bindings.insert(
            "ToggleStatusLanes".to_string(),
            vec![
                cmd_default("cmd-shift-k", None),
                ctrl_default("ctrl-shift-k", None),
            ],
        );
        bindings.insert(
            "OpenSettingsFile".to_string(),
            vec![
                cmd_default("cmd-alt-,", None),
                ctrl_default("ctrl-alt-,", None),
            ],
        );
        bindings.insert(
            "ShowFileSearch".to_string(),
            vec![cmd_default("cmd-p", None), ctrl_default("ctrl-p", None)],
        );
        bindings.insert(
            "ShowContentSearch".to_string(),
            vec![
                cmd_default("cmd-shift-f", None),
                ctrl_default("ctrl-shift-f", None),
            ],
        );
        bindings.insert(
            "ShowProjectSwitcher".to_string(),
            vec![cmd_default("cmd-e", None), ctrl_default("ctrl-e", None)],
        );

        // Fullscreen keybindings
        bindings.insert(
            "ToggleFullscreen".to_string(),
            vec![KeybindingEntry::new("shift-escape", Some("TerminalPane"))],
        );
        bindings.insert(
            "FullscreenNextTerminal".to_string(),
            vec![
                cmd_default("cmd-]", Some("TerminalPane")),
                ctrl_default("ctrl-]", Some("TerminalPane")),
            ],
        );
        bindings.insert(
            "FullscreenPrevTerminal".to_string(),
            vec![
                cmd_default("cmd-[", Some("TerminalPane")),
                ctrl_default("ctrl-[", Some("TerminalPane")),
            ],
        );

        // Terminal pane keybindings
        bindings.insert(
            "SplitVertical".to_string(),
            vec![
                cmd_default("cmd-d", Some("TerminalPane")),
                ctrl_default("ctrl-shift-d", Some("TerminalPane")),
            ],
        );
        bindings.insert(
            "SplitHorizontal".to_string(),
            vec![
                cmd_default("cmd-shift-d", Some("TerminalPane")),
                ctrl_default("ctrl-d", Some("TerminalPane")),
            ],
        );
        // Also bound globally: a project with no terminals has no TerminalPane
        // to dispatch through, so the pane-scoped binding alone leaves "Start
        // Terminal" reachable only by mouse. The window-level handler only acts
        // when there is genuinely no pane to take it (see `render.rs`).
        bindings.insert(
            "AddTab".to_string(),
            vec![
                cmd_default("cmd-t", Some("TerminalPane")),
                ctrl_default("ctrl-shift-t", Some("TerminalPane")),
                cmd_default("cmd-t", None),
                ctrl_default("ctrl-shift-t", None),
            ],
        );
        bindings.insert(
            "CloseTerminal".to_string(),
            vec![
                cmd_default("cmd-w", Some("TerminalPane")),
                ctrl_default("ctrl-shift-w", Some("TerminalPane")),
            ],
        );
        bindings.insert(
            "MinimizeTerminal".to_string(),
            vec![
                cmd_default("cmd-m", Some("TerminalPane")),
                ctrl_default("ctrl-shift-m", Some("TerminalPane")),
            ],
        );
        bindings.insert(
            "ToggleUnread".to_string(),
            vec![
                cmd_default("cmd-u", Some("TerminalPane")),
                ctrl_default("ctrl-shift-u", Some("TerminalPane")),
            ],
        );
        bindings.insert(
            "Copy".to_string(),
            vec![
                cmd_default("cmd-c", Some("TerminalPane")),
                ctrl_default("ctrl-shift-c", Some("TerminalPane")),
            ],
        );
        bindings.insert(
            "Paste".to_string(),
            vec![
                cmd_default("cmd-v", Some("TerminalPane")),
                ctrl_default("ctrl-shift-v", Some("TerminalPane")),
            ],
        );
        bindings.insert(
            "AnnotateSelection".to_string(),
            vec![
                cmd_default("cmd-shift-a", Some("TerminalPane")),
                ctrl_default("ctrl-shift-a", Some("TerminalPane")),
            ],
        );
        bindings.insert(
            "ScrollUp".to_string(),
            vec![KeybindingEntry::new("shift-pageup", Some("TerminalPane"))],
        );
        bindings.insert(
            "ScrollDown".to_string(),
            vec![KeybindingEntry::new("shift-pagedown", Some("TerminalPane"))],
        );
        bindings.insert(
            "Search".to_string(),
            vec![
                cmd_default("cmd-f", Some("TerminalPane")),
                ctrl_default("ctrl-f", Some("TerminalPane")),
            ],
        );

        // Zoom keybindings
        bindings.insert(
            "ZoomIn".to_string(),
            vec![
                cmd_default("cmd-=", Some("TerminalPane")),
                ctrl_default("ctrl-=", Some("TerminalPane")),
            ],
        );
        bindings.insert(
            "ZoomOut".to_string(),
            vec![
                cmd_default("cmd--", Some("TerminalPane")),
                ctrl_default("ctrl--", Some("TerminalPane")),
            ],
        );
        bindings.insert(
            "ResetZoom".to_string(),
            vec![
                cmd_default("cmd-0", Some("TerminalPane")),
                ctrl_default("ctrl-0", Some("TerminalPane")),
            ],
        );

        // Navigation keybindings
        bindings.insert(
            "FocusLeft".to_string(),
            vec![non_windows_default("cmd-alt-left", None)],
        );
        bindings.insert(
            "FocusRight".to_string(),
            vec![non_windows_default("cmd-alt-right", None)],
        );
        bindings.insert(
            "FocusUp".to_string(),
            vec![non_windows_default("cmd-alt-up", None)],
        );
        bindings.insert(
            "FocusDown".to_string(),
            vec![non_windows_default("cmd-alt-down", None)],
        );
        bindings.insert(
            "FocusNextTerminal".to_string(),
            vec![
                cmd_default("cmd-shift-]", Some("TerminalPane")),
                ctrl_default("ctrl-tab", Some("TerminalPane")),
            ],
        );
        bindings.insert(
            "FocusPrevTerminal".to_string(),
            vec![
                cmd_default("cmd-shift-[", Some("TerminalPane")),
                ctrl_default("ctrl-shift-tab", Some("TerminalPane")),
            ],
        );

        bindings.insert(
            "JumpToPreviousPrompt".to_string(),
            vec![non_windows_default("cmd-up", Some("TerminalPane"))],
        );
        bindings.insert(
            "JumpToNextPrompt".to_string(),
            vec![non_windows_default("cmd-down", Some("TerminalPane"))],
        );

        bindings.insert(
            "JumpToPreviousFailedCommand".to_string(),
            vec![non_windows_default("cmd-shift-up", Some("TerminalPane"))],
        );
        bindings.insert(
            "JumpToNextFailedCommand".to_string(),
            vec![non_windows_default("cmd-shift-down", Some("TerminalPane"))],
        );

        bindings.insert(
            "TogglePaneSwitcher".to_string(),
            vec![cmd_default("cmd-`", None), ctrl_default("ctrl-`", None)],
        );

        bindings.insert(
            "EqualizeLayout".to_string(),
            vec![
                cmd_default("cmd-shift-e", None),
                ctrl_default("ctrl-shift-e", None),
            ],
        );

        bindings.insert(
            "ToggleProjectLayout".to_string(),
            vec![
                cmd_default("cmd-shift-l", None),
                ctrl_default("ctrl-shift-l", None),
            ],
        );

        bindings.insert(
            "ToggleProjectVisibility".to_string(),
            vec![
                cmd_default("cmd-shift-h", None),
                ctrl_default("ctrl-shift-h", None),
            ],
        );

        bindings.insert(
            "ShowBranchSwitcher".to_string(),
            vec![
                cmd_default("cmd-alt-b", None),
                ctrl_default("ctrl-alt-b", None),
            ],
        );

        bindings.insert(
            "NewProject".to_string(),
            vec![cmd_default("cmd-n", None), ctrl_default("ctrl-n", None)],
        );

        bindings.insert(
            "NewWindow".to_string(),
            vec![
                cmd_default("cmd-shift-n", None),
                ctrl_default("ctrl-shift-n", None),
            ],
        );

        Self {
            version: 1,
            bindings,
        }
    }

    /// Check for keybinding conflicts
    /// Returns a list of conflicts found
    pub fn detect_conflicts(&self) -> Vec<KeybindingConflict> {
        let mut conflicts = Vec::new();
        type BindingScope<'a> = (&'a str, Option<&'a str>);
        type SeenBindings<'a> = Vec<(&'a str, &'a KeybindingEntry)>;
        let mut seen: HashMap<BindingScope<'_>, SeenBindings<'_>> = HashMap::new();
        let mut actions: Vec<_> = self.bindings.iter().collect();
        actions.sort_by_key(|(action, _)| *action);

        for (action, entries) in actions {
            for entry in entries.iter().filter(|entry| entry.enabled) {
                let key = (entry.keystroke.as_str(), entry.context.as_deref());
                let previous = seen.entry(key).or_default();
                for &(existing_action, existing_entry) in previous.iter() {
                    if existing_action != action
                        && [
                            ShortcutPlatform::Mac,
                            ShortcutPlatform::Windows,
                            ShortcutPlatform::Linux,
                        ]
                        .into_iter()
                        .any(|platform| {
                            entry.is_supported_on(platform)
                                && existing_entry.is_supported_on(platform)
                        })
                    {
                        conflicts.push(KeybindingConflict {
                            keystroke: entry.keystroke.clone(),
                            context: entry.context.clone(),
                            action1: existing_action.to_owned(),
                            action2: action.clone(),
                        });
                    }
                }
                previous.push((action.as_str(), entry));
            }
        }

        conflicts
    }

    /// Update the keystroke for a specific binding entry
    pub fn update_binding(&mut self, action: &str, entry_index: usize, new_keystroke: String) {
        if let Some(entries) = self.bindings.get_mut(action)
            && let Some(entry) = entries.get_mut(entry_index)
        {
            entry.keystroke = new_keystroke;
        }
    }

    /// Reset a single action's bindings back to defaults
    pub fn reset_single_action(&mut self, action: &str) {
        let defaults = Self::defaults();
        if let Some(default_entries) = defaults.bindings.get(action) {
            self.bindings
                .insert(action.to_string(), default_entries.clone());
        } else {
            // Action doesn't exist in defaults — remove it
            self.bindings.remove(action);
        }
    }

    /// Add a new binding entry for an action
    pub fn add_binding(&mut self, action: &str, entry: KeybindingEntry) {
        self.bindings
            .entry(action.to_string())
            .or_default()
            .push(entry);
    }

    /// Remove a specific binding entry by index
    /// Returns true if the entry was removed
    pub fn remove_binding(&mut self, action: &str, entry_index: usize) -> bool {
        if let Some(entries) = self.bindings.get_mut(action)
            && entry_index < entries.len()
        {
            entries.remove(entry_index);
            return true;
        }
        false
    }

    /// Toggle the enabled state of a specific binding entry
    pub fn toggle_binding(&mut self, action: &str, entry_index: usize) {
        if let Some(entries) = self.bindings.get_mut(action)
            && let Some(entry) = entries.get_mut(entry_index)
        {
            entry.enabled = !entry.enabled;
        }
    }

    /// Only whole-action overrides are custom; omitted actions inherit defaults.
    pub fn get_customized_actions(&self) -> HashSet<String> {
        self.overrides().bindings.into_keys().collect()
    }

    fn overrides(&self) -> Self {
        let defaults = Self::defaults();
        Self {
            version: self.version,
            bindings: self
                .bindings
                .iter()
                .filter(|(action, entries)| {
                    defaults
                        .bindings
                        .get(*action)
                        .is_none_or(|default| !equivalent_to_defaults(entries, default))
                })
                .map(|(action, entries)| (action.clone(), entries.clone()))
                .collect(),
        }
    }
}

/// Compare whole arrays, retaining entry order and explicit platform choices.
/// Old files wrote every default without platform metadata; recognize only that
/// complete original array, never a partial/custom list that happens to use cmd/ctrl.
fn equivalent_to_defaults(entries: &[KeybindingEntry], defaults: &[KeybindingEntry]) -> bool {
    let same_binding = |entry: &KeybindingEntry, default: &KeybindingEntry| {
        entry.keystroke == default.keystroke
            && entry.context == default.context
            && entry.enabled == default.enabled
    };
    entries.len() == defaults.len()
        && (entries.iter().zip(defaults).all(|(entry, default)| {
            same_binding(entry, default)
                && match (&entry.platforms, &default.platforms) {
                    (None, None) => true,
                    (Some(left), Some(right)) => {
                        left.iter().all(|p| right.contains(p))
                            && right.iter().all(|p| left.contains(p))
                    }
                    _ => false,
                }
        }) || entries.iter().all(|entry| entry.platforms.is_none())
            && entries
                .iter()
                .zip(defaults)
                .all(|(entry, default)| same_binding(entry, default)))
}

/// Get the keybindings configuration file path
pub fn get_keybindings_path() -> PathBuf {
    if let Some(p) = okena_core::profiles::try_current() {
        p.keybindings_json()
    } else {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("okena")
            .join("keybindings.json")
    }
}

/// Parse user configuration, preserving existing entries and adding missing default actions.
pub fn parse_keybindings(content: &str) -> anyhow::Result<KeybindingConfig> {
    let mut config: KeybindingConfig = serde_json::from_str(content)?;
    for entries in config.bindings.values() {
        for entry in entries {
            anyhow::ensure!(
                !entry.keystroke.trim().is_empty() && entry.keystroke != "unset",
                "Empty or unfinished shortcut"
            );
            for stroke in entry.keystroke.split_whitespace() {
                gpui::Keystroke::parse_with_os(stroke, ShortcutPlatform::current().os())?;
            }
            if let Some(context) = &entry.context {
                gpui::KeyBindingContextPredicate::parse(context)?;
            }
        }
    }

    for (action, entries) in KeybindingConfig::defaults().bindings {
        match config.bindings.entry(action) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(entries);
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                if equivalent_to_defaults(entry.get(), &entries) {
                    entry.insert(entries);
                }
            }
        }
    }
    Ok(config)
}

/// Read and validate a specific file without changing its contents.
pub fn load_keybindings_from_path(path: &Path) -> anyhow::Result<KeybindingConfig> {
    load_keybindings_with_snapshot(path).map(|(config, _)| config)
}

/// Normalize defaults in memory and return exactly the original on-disk bytes.
/// Loading never saves, cleans, or reformats the file.
pub(super) fn load_keybindings_with_snapshot(
    path: &Path,
) -> anyhow::Result<(KeybindingConfig, String)> {
    let original = std::fs::read(path)?;
    let content = std::str::from_utf8(&original)?;
    let config = parse_keybindings(content)?;
    Ok((config, content.to_owned()))
}

/// Load configuration at startup, retaining the historical fallback on failure.
pub fn load_keybindings() -> KeybindingConfig {
    let path = get_keybindings_path();
    if path.exists() {
        match load_keybindings_from_path(&path) {
            Ok(config) => {
                for conflict in config.detect_conflicts() {
                    log::warn!("Keybinding conflict: {}", conflict);
                }
                return config;
            }
            Err(error) => {
                log::warn!(
                    "Failed to load keybindings config: {}, using defaults",
                    error
                );
            }
        }
    }
    KeybindingConfig::defaults()
}

/// Persist only complete action overrides; empty arrays intentionally suppress defaults.
#[cfg(test)]
fn save_keybindings_to_path(config: &KeybindingConfig, path: &Path) -> anyhow::Result<String> {
    let content = serde_json::to_string_pretty(&config.overrides())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_keybindings_atomically(path, &content, None)?;
    Ok(content)
}

/// Replace only after the complete new JSON has been written successfully.
#[cfg(test)]
fn write_keybindings_atomically(
    path: &Path,
    content: &str,
    expected: Option<&[u8]>,
) -> anyhow::Result<()> {
    write_conditionally(path, content, expected.map(Some))
}

fn write_conditionally(
    path: &Path,
    content: &str,
    expected: Option<Option<&[u8]>>,
) -> anyhow::Result<()> {
    use std::io::Write;
    let temporary = path.with_extension(format!("json.{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> anyhow::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        drop(file);
        if let Some(expected) = expected {
            anyhow::ensure!(
                match expected {
                    Some(bytes) => std::fs::read(path)? == bytes,
                    None => !path.exists(),
                },
                "Keybindings file changed before saving"
            );
        }
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// The comparison is repeated after writing the temporary file, just before replacement.
pub fn save_keybindings_if_unchanged(
    config: &KeybindingConfig,
    expected: Option<&str>,
) -> anyhow::Result<String> {
    let path = get_keybindings_path();
    let content = serde_json::to_string_pretty(&config.overrides())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_conditionally(&path, &content, Some(expected.map(str::as_bytes)))?;
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ConfigFixture(PathBuf);

    impl ConfigFixture {
        fn new(content: &str) -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let root = std::env::temp_dir().join("okena-keybindings-test-fixtures");
            std::fs::create_dir_all(&root).unwrap();
            let path = root.join(format!(
                "{}-{}.json",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::write(&path, content).unwrap();
            Self(path)
        }
    }

    impl Drop for ConfigFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn loading_preserves_legacy_bytes_and_only_explicit_save_cleans_defaults() {
        let mut legacy = KeybindingConfig::defaults();
        for entries in legacy.bindings.values_mut() {
            for entry in entries {
                entry.platforms = None;
            }
        }
        assert!(legacy.get_customized_actions().is_empty());
        let original = format!("  {}\n", serde_json::to_string(&legacy).unwrap());
        let fixture = ConfigFixture::new(&original);
        let (loaded, snapshot) = load_keybindings_with_snapshot(&fixture.0).unwrap();
        assert_eq!(snapshot, original);
        assert_eq!(std::fs::read_to_string(&fixture.0).unwrap(), original);
        assert_eq!(loaded.bindings, KeybindingConfig::defaults().bindings);
        assert!(loaded.get_customized_actions().is_empty());
        load_keybindings_from_path(&fixture.0).unwrap();
        assert_eq!(std::fs::read_to_string(&fixture.0).unwrap(), original);

        save_keybindings_to_path(&loaded, &fixture.0).unwrap();
        let cleaned = std::fs::read(&fixture.0).unwrap();
        let raw: KeybindingConfig = serde_json::from_slice(&cleaned).unwrap();
        assert_eq!(raw.version, 1);
        assert!(raw.bindings.is_empty());
        load_keybindings_from_path(&fixture.0).unwrap();
        assert_eq!(std::fs::read(&fixture.0).unwrap(), cleaned);
    }

    #[test]
    fn loading_read_only_legacy_file_keeps_overrides_and_creates_no_files() {
        let mut config = KeybindingConfig::defaults();
        config.bindings.insert(
            "FutureAction".into(),
            vec![KeybindingEntry::new("alt-f12", None)],
        );
        let original = serde_json::to_string_pretty(&config).unwrap();
        let fixture = ConfigFixture::new(&original);
        let permissions = std::fs::metadata(&fixture.0).unwrap().permissions();
        let mut read_only = permissions.clone();
        read_only.set_readonly(true);
        std::fs::set_permissions(&fixture.0, read_only).unwrap();
        // Restore permissions before assertions so a failure cannot leave fixture litter.
        let result = load_keybindings_with_snapshot(&fixture.0);
        std::fs::set_permissions(&fixture.0, permissions).unwrap();
        let (loaded, snapshot) = result.unwrap();
        assert_eq!(snapshot, original);
        assert_eq!(std::fs::read_to_string(&fixture.0).unwrap(), original);
        assert_eq!(
            loaded.bindings["FutureAction"],
            config.bindings["FutureAction"]
        );
    }

    #[test]
    fn whole_action_customizations_and_unknown_actions_round_trip() {
        let defaults = KeybindingConfig::defaults();
        let original = &defaults.bindings["SplitVertical"];
        let mut disabled = original.clone();
        disabled[0].enabled = false;
        let mut context = original.clone();
        context[0].context = Some("Sidebar".into());
        let mut platform = original.clone();
        platform[0].platforms = Some(vec![ShortcutPlatform::Linux]);
        let mut mixed = original.clone();
        mixed[0].platforms = None;
        mixed[1].platforms = Some(vec![ShortcutPlatform::Windows, ShortcutPlatform::Linux]);
        let mut changed = original.clone();
        changed[0].keystroke = "alt-f12".into();
        for entries in [
            vec![],
            vec![original[0].clone()],
            disabled,
            context,
            platform,
            mixed,
            changed,
        ] {
            let mut config = defaults.clone();
            config
                .bindings
                .insert("SplitVertical".into(), entries.clone());
            config.bindings.insert("FutureAction".into(), vec![]);
            assert_eq!(
                config.get_customized_actions(),
                HashSet::from(["SplitVertical".into(), "FutureAction".into()])
            );
            let fixture = ConfigFixture::new("{}");
            save_keybindings_to_path(&config, &fixture.0).unwrap();
            let before = std::fs::read(&fixture.0).unwrap();
            let loaded = load_keybindings_from_path(&fixture.0).unwrap();
            assert_eq!(loaded.bindings["SplitVertical"], entries);
            assert_eq!(loaded.bindings["FutureAction"], vec![]);
            assert_eq!(std::fs::read(&fixture.0).unwrap(), before);
        }
    }

    #[test]
    fn platform_order_is_equivalent_but_partial_legacy_arrays_are_custom() {
        let mut config = KeybindingConfig::defaults();
        config.bindings.get_mut("SplitVertical").unwrap()[0]
            .platforms
            .as_mut()
            .unwrap()
            .reverse();
        assert!(config.get_customized_actions().is_empty());
        config.bindings.insert(
            "SplitVertical".into(),
            vec![KeybindingEntry::new("cmd-d", Some("TerminalPane"))],
        );
        assert!(config.get_customized_actions().contains("SplitVertical"));
        let loaded = parse_keybindings(&serde_json::to_string(&config).unwrap()).unwrap();
        assert_eq!(loaded.bindings["SplitVertical"][0].platforms, None);
    }

    #[test]
    fn corrupt_loads_and_stale_explicit_writes_preserve_original_files() {
        for content in [
            "invalid",
            r#"{"bindings":{"Quit":[{"keystroke":"unset"}]}}"#,
        ] {
            let fixture = ConfigFixture::new(content);
            assert!(load_keybindings_from_path(&fixture.0).is_err());
            assert_eq!(std::fs::read_to_string(&fixture.0).unwrap(), content);
        }
        let fixture = ConfigFixture::new("external editor bytes");
        assert!(
            write_keybindings_atomically(&fixture.0, "cleaned", Some(b"original bytes")).is_err()
        );
        assert_eq!(
            std::fs::read_to_string(&fixture.0).unwrap(),
            "external editor bytes"
        );
    }

    #[test]
    fn defaults_have_explicit_native_pairs_and_keep_shared_bindings() {
        let config = KeybindingConfig::defaults();
        for platform in [
            ShortcutPlatform::Mac,
            ShortcutPlatform::Windows,
            ShortcutPlatform::Linux,
        ] {
            let entries = &config.bindings["SplitVertical"];
            let active: Vec<_> = entries
                .iter()
                .filter(|entry| entry.applies_to(platform))
                .collect();
            assert_eq!(
                active.len(),
                if platform == ShortcutPlatform::Windows {
                    1
                } else {
                    2
                }
            );
            assert!(active.iter().any(|entry| entry.keystroke == "ctrl-shift-d"));
            assert_eq!(
                active.iter().any(|entry| entry.keystroke == "cmd-d"),
                platform != ShortcutPlatform::Windows
            );
            assert!(config.bindings["ToggleFullscreen"][0].applies_to(platform));
            assert_eq!(
                config.bindings["FocusLeft"][0].applies_to(platform),
                platform != ShortcutPlatform::Windows
            );
        }
    }

    #[test]
    fn mac_and_linux_defaults_keep_every_original_cmd_and_ctrl_alternative() {
        let defaults = KeybindingConfig::defaults();
        for platform in [ShortcutPlatform::Mac, ShortcutPlatform::Linux] {
            for entries in defaults.bindings.values() {
                for entry in entries {
                    assert!(
                        entry.is_supported_on(platform),
                        "{} must remain supported on {platform:?}",
                        entry.keystroke
                    );
                }
            }
            let split = &defaults.bindings["SplitVertical"];
            assert!(
                split
                    .iter()
                    .any(|entry| entry.keystroke == "cmd-d" && entry.is_supported_on(platform))
            );
            assert!(
                split
                    .iter()
                    .any(|entry| entry.keystroke == "ctrl-shift-d"
                        && entry.is_supported_on(platform))
            );
        }
    }

    #[test]
    fn windows_defaults_never_claim_the_windows_key_but_custom_bindings_survive() {
        let defaults = KeybindingConfig::defaults();
        for entries in defaults.bindings.values() {
            for entry in entries
                .iter()
                .filter(|e| e.applies_to(ShortcutPlatform::Windows))
            {
                assert!(entry.keystroke.split_whitespace().all(|s| {
                    !gpui::Keystroke::parse_with_os(s, gpui::OperatingSystem::Windows)
                        .unwrap()
                        .modifiers
                        .platform
                }));
            }
        }
        let actions = [
            "FocusLeft",
            "FocusRight",
            "FocusUp",
            "FocusDown",
            "JumpToPreviousPrompt",
            "JumpToNextPrompt",
            "JumpToPreviousFailedCommand",
            "JumpToNextFailedCommand",
        ];
        let mut legacy = defaults.clone();
        for action in actions {
            for entry in legacy.bindings.get_mut(action).unwrap() {
                entry.platforms = None;
            }
        }
        let loaded = parse_keybindings(&serde_json::to_string(&legacy).unwrap()).unwrap();
        for action in actions {
            assert!(
                loaded.bindings[action]
                    .iter()
                    .all(|e| !e.applies_to(ShortcutPlatform::Windows))
            );
            for platform in [ShortcutPlatform::Mac, ShortcutPlatform::Linux] {
                assert!(
                    loaded.bindings[action]
                        .iter()
                        .any(|e| e.applies_to(platform))
                );
            }
        }
        let custom = vec![KeybindingEntry::for_platforms(
            "cmd-alt-left",
            None,
            &[ShortcutPlatform::Windows],
        )];
        legacy.bindings.insert("FocusLeft".into(), custom.clone());
        let loaded = parse_keybindings(&serde_json::to_string(&legacy).unwrap()).unwrap();
        assert_eq!(loaded.bindings["FocusLeft"], custom);
        assert!(loaded.get_customized_actions().contains("FocusLeft"));
    }

    #[test]
    fn conflicts_require_overlapping_platforms_including_later_entries() {
        let mac = KeybindingEntry::for_platforms("ctrl-x", None, &[ShortcutPlatform::Mac]);
        let windows = KeybindingEntry::for_platforms("ctrl-x", None, &[ShortcutPlatform::Windows]);
        let linux = KeybindingEntry::for_platforms("ctrl-x", None, &[ShortcutPlatform::Linux]);
        let mut config = KeybindingConfig {
            version: 1,
            bindings: HashMap::from([
                ("Action1".into(), vec![mac]),
                ("Action2".into(), vec![windows.clone(), linux]),
            ]),
        };
        assert!(config.detect_conflicts().is_empty());
        config.bindings.insert("Action3".into(), vec![windows]);
        let conflicts = config.detect_conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].action1, "Action2");
        assert_eq!(conflicts[0].action2, "Action3");
        config.bindings.insert(
            "Action4".into(),
            vec![KeybindingEntry::for_platforms("ctrl-x", None, &[])],
        );
        assert_eq!(config.detect_conflicts().len(), 1);
        config
            .bindings
            .insert("Action5".into(), vec![KeybindingEntry::new("ctrl-x", None)]);
        assert_eq!(config.detect_conflicts().len(), 5);
    }

    #[test]
    fn parsing_adds_missing_actions_but_preserves_legacy_and_empty_overrides() {
        let config = parse_keybindings(
            r#"{"bindings":{"SplitVertical":[{"keystroke":"cmd-d"}],"Quit":[]}}"#,
        )
        .unwrap();
        assert_eq!(config.bindings["SplitVertical"][0].platforms, None);
        assert!(config.bindings["Quit"].is_empty());
        assert!(config.bindings.contains_key("ToggleSidebar"));
        assert!(parse_keybindings("invalid").is_err());
    }

    #[test]
    fn test_default_config_has_no_conflicts() {
        let config = KeybindingConfig::defaults();
        let conflicts = config.detect_conflicts();
        assert!(
            conflicts.is_empty(),
            "Default config should have no conflicts"
        );
    }

    /// An unwired action name, or a keystroke or context that doesn't parse,
    /// is not a compile error — the binding is dropped with a warning and the
    /// shortcut is simply dead, while a missing description hides it from the
    /// command palette and the keybindings overlay. All are silent at runtime,
    /// so pin them here.
    #[test]
    fn every_default_action_is_bindable_and_described() {
        let config = KeybindingConfig::defaults();
        let descriptions = crate::keybindings::get_action_descriptions();

        for (action, entries) in &config.bindings {
            assert!(
                descriptions.contains_key(action.as_str()),
                "{action} has default bindings but no description"
            );
            assert!(
                crate::keybindings::action_for_name(action).is_some(),
                "{action} is not wired into action_for_name — its shortcut would do nothing"
            );
            for entry in entries {
                for keystroke in entry.keystroke.split_whitespace() {
                    assert!(
                        gpui::Keystroke::parse_with_os(keystroke, gpui::OperatingSystem::Linux)
                            .is_ok(),
                        "{action}: default keystroke {keystroke:?} does not parse"
                    );
                }
                if let Some(context) = entry.context.as_deref() {
                    assert!(
                        gpui::KeyBindingContextPredicate::parse(context).is_ok(),
                        "{action}: default context {context:?} does not parse"
                    );
                }
            }
        }
    }

    #[test]
    fn header_fallback_commands_are_bindable_and_described() {
        let descriptions = crate::keybindings::get_action_descriptions();

        for action in ["ExportTerminalBuffer", "DetachTerminal"] {
            assert!(
                descriptions.contains_key(action),
                "{action} must be available in the command palette"
            );
            assert!(
                crate::keybindings::action_for_name(action).is_some(),
                "{action} must remain bindable"
            );
        }
    }

    #[test]
    fn test_duplicate_keybinding_detected() {
        let mut config = KeybindingConfig::defaults();
        // Add a binding that conflicts with an existing one
        config.bindings.insert(
            "CustomAction".to_string(),
            vec![KeybindingEntry::new("cmd-b", None)], // conflicts with ToggleSidebar
        );
        let conflicts = config.detect_conflicts();
        assert!(!conflicts.is_empty(), "Should detect conflict on cmd-b");
        assert!(conflicts.iter().any(|c| c.keystroke == "cmd-b"));
    }

    #[test]
    fn test_same_key_different_context_no_conflict() {
        let mut bindings = HashMap::new();
        bindings.insert(
            "Action1".to_string(),
            vec![KeybindingEntry::new("cmd-d", None)], // global
        );
        bindings.insert(
            "Action2".to_string(),
            vec![KeybindingEntry::new("cmd-d", Some("TerminalPane"))], // scoped
        );
        let config = KeybindingConfig {
            version: 1,
            bindings,
        };
        let conflicts = config.detect_conflicts();
        assert!(
            conflicts.is_empty(),
            "Different contexts should not conflict"
        );
    }

    #[test]
    fn test_disabled_binding_no_conflict() {
        let mut bindings = HashMap::new();
        bindings.insert(
            "Action1".to_string(),
            vec![KeybindingEntry::new("cmd-x", None)],
        );
        let mut disabled = KeybindingEntry::new("cmd-x", None);
        disabled.enabled = false;
        bindings.insert("Action2".to_string(), vec![disabled]);
        let config = KeybindingConfig {
            version: 1,
            bindings,
        };
        let conflicts = config.detect_conflicts();
        assert!(conflicts.is_empty(), "Disabled binding should not conflict");
    }

    #[test]
    fn test_customized_actions_detected() {
        let mut config = KeybindingConfig::defaults();
        // Modify an existing action's binding
        config.bindings.insert(
            "ToggleSidebar".to_string(),
            vec![KeybindingEntry::new("ctrl-shift-b", None)], // changed from default
        );
        let customized = config.get_customized_actions();
        assert!(
            customized.contains("ToggleSidebar"),
            "Modified action should be detected"
        );
    }

    #[test]
    fn test_serialization_round_trip() {
        let config = KeybindingConfig::defaults();
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: KeybindingConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.version, config.version);
        assert_eq!(deserialized.bindings.len(), config.bindings.len());
    }
}
