use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::types::{KeybindingConflict, KeybindingEntry};

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
            "ToggleSidebar".to_string(),
            vec![
                KeybindingEntry::new("cmd-b", None),
                KeybindingEntry::new("ctrl-b", None),
            ],
        );
        bindings.insert(
            "ToggleSidebarAutoHide".to_string(),
            vec![
                KeybindingEntry::new("cmd-shift-b", None),
                KeybindingEntry::new("ctrl-shift-b", None),
            ],
        );
        bindings.insert(
            "FocusSidebar".to_string(),
            vec![
                KeybindingEntry::new("cmd-1", None),
                KeybindingEntry::new("ctrl-1", None),
            ],
        );
        bindings.insert(
            "ClearFocus".to_string(),
            vec![
                KeybindingEntry::new("cmd-0", None),
                KeybindingEntry::new("ctrl-0", None),
            ],
        );
        bindings.insert(
            "ShowKeybindings".to_string(),
            vec![
                KeybindingEntry::new("cmd-k cmd-s", None),
                KeybindingEntry::new("ctrl-k ctrl-s", None),
            ],
        );
        bindings.insert(
            "ShowSessionManager".to_string(),
            vec![
                KeybindingEntry::new("cmd-k cmd-w", None),
                KeybindingEntry::new("ctrl-k ctrl-w", None),
            ],
        );
        bindings.insert(
            "ShowThemeSelector".to_string(),
            vec![
                KeybindingEntry::new("cmd-k cmd-t", None),
                KeybindingEntry::new("ctrl-k ctrl-t", None),
            ],
        );
        bindings.insert(
            "ShowCommandPalette".to_string(),
            vec![
                KeybindingEntry::new("cmd-shift-p", None),
                KeybindingEntry::new("ctrl-shift-p", None),
            ],
        );
        bindings.insert(
            "ShowSettings".to_string(),
            vec![
                KeybindingEntry::new("cmd-,", None),
                KeybindingEntry::new("ctrl-,", None),
            ],
        );
        bindings.insert(
            "OpenSettingsFile".to_string(),
            vec![
                KeybindingEntry::new("cmd-alt-,", None),
                KeybindingEntry::new("ctrl-alt-,", None),
            ],
        );
        bindings.insert(
            "ShowFileSearch".to_string(),
            vec![
                KeybindingEntry::new("cmd-p", None),
                KeybindingEntry::new("ctrl-p", None),
            ],
        );
        bindings.insert(
            "ShowProjectSwitcher".to_string(),
            vec![
                KeybindingEntry::new("cmd-e", None),
                KeybindingEntry::new("ctrl-e", None),
            ],
        );

        // Fullscreen keybindings
        bindings.insert(
            "ToggleFullscreen".to_string(),
            vec![
                KeybindingEntry::new("shift-escape", Some("TerminalPane")),
            ],
        );
        bindings.insert(
            "FullscreenNextTerminal".to_string(),
            vec![
                KeybindingEntry::new("cmd-]", Some("TerminalPane")),
                KeybindingEntry::new("ctrl-]", Some("TerminalPane")),
            ],
        );
        bindings.insert(
            "FullscreenPrevTerminal".to_string(),
            vec![
                KeybindingEntry::new("cmd-[", Some("TerminalPane")),
                KeybindingEntry::new("ctrl-[", Some("TerminalPane")),
            ],
        );

        // Terminal pane keybindings
        bindings.insert(
            "SplitVertical".to_string(),
            vec![
                KeybindingEntry::new("cmd-d", Some("TerminalPane")),
                KeybindingEntry::new("ctrl-shift-d", Some("TerminalPane")),
            ],
        );
        bindings.insert(
            "SplitHorizontal".to_string(),
            vec![
                KeybindingEntry::new("cmd-shift-d", Some("TerminalPane")),
                KeybindingEntry::new("ctrl-d", Some("TerminalPane")),
            ],
        );
        bindings.insert(
            "AddTab".to_string(),
            vec![
                KeybindingEntry::new("cmd-t", Some("TerminalPane")),
                KeybindingEntry::new("ctrl-shift-t", Some("TerminalPane")),
            ],
        );
        bindings.insert(
            "CloseTerminal".to_string(),
            vec![
                KeybindingEntry::new("cmd-w", Some("TerminalPane")),
                KeybindingEntry::new("ctrl-shift-w", Some("TerminalPane")),
            ],
        );
        bindings.insert(
            "MinimizeTerminal".to_string(),
            vec![
                KeybindingEntry::new("cmd-m", Some("TerminalPane")),
                KeybindingEntry::new("ctrl-shift-m", Some("TerminalPane")),
            ],
        );
        bindings.insert(
            "Copy".to_string(),
            vec![
                KeybindingEntry::new("cmd-c", Some("TerminalPane")),
                KeybindingEntry::new("ctrl-shift-c", Some("TerminalPane")),
            ],
        );
        bindings.insert(
            "Paste".to_string(),
            vec![
                KeybindingEntry::new("cmd-v", Some("TerminalPane")),
                KeybindingEntry::new("ctrl-shift-v", Some("TerminalPane")),
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
                KeybindingEntry::new("cmd-f", Some("TerminalPane")),
                KeybindingEntry::new("ctrl-f", Some("TerminalPane")),
            ],
        );

        // Zoom keybindings
        bindings.insert(
            "ZoomIn".to_string(),
            vec![
                KeybindingEntry::new("cmd-=", Some("TerminalPane")),
                KeybindingEntry::new("ctrl-=", Some("TerminalPane")),
            ],
        );
        bindings.insert(
            "ZoomOut".to_string(),
            vec![
                KeybindingEntry::new("cmd--", Some("TerminalPane")),
                KeybindingEntry::new("ctrl--", Some("TerminalPane")),
            ],
        );
        bindings.insert(
            "ResetZoom".to_string(),
            vec![
                KeybindingEntry::new("cmd-0", Some("TerminalPane")),
                KeybindingEntry::new("ctrl-0", Some("TerminalPane")),
            ],
        );

        // Navigation keybindings
        bindings.insert(
            "FocusLeft".to_string(),
            vec![KeybindingEntry::new("cmd-alt-left", None)],
        );
        bindings.insert(
            "FocusRight".to_string(),
            vec![KeybindingEntry::new("cmd-alt-right", None)],
        );
        bindings.insert(
            "FocusUp".to_string(),
            vec![KeybindingEntry::new("cmd-alt-up", None)],
        );
        bindings.insert(
            "FocusDown".to_string(),
            vec![KeybindingEntry::new("cmd-alt-down", None)],
        );
        bindings.insert(
            "FocusNextTerminal".to_string(),
            vec![
                KeybindingEntry::new("cmd-shift-]", Some("TerminalPane")),
                KeybindingEntry::new("ctrl-tab", Some("TerminalPane")),
            ],
        );
        bindings.insert(
            "FocusPrevTerminal".to_string(),
            vec![
                KeybindingEntry::new("cmd-shift-[", Some("TerminalPane")),
                KeybindingEntry::new("ctrl-shift-tab", Some("TerminalPane")),
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
        let mut seen: HashMap<(String, Option<String>), String> = HashMap::new();

        for (action, entries) in &self.bindings {
            for entry in entries {
                if !entry.enabled {
                    continue;
                }

                let key = (entry.keystroke.clone(), entry.context.clone());

                if let Some(existing_action) = seen.get(&key) {
                    if existing_action != action {
                        conflicts.push(KeybindingConflict {
                            keystroke: entry.keystroke.clone(),
                            context: entry.context.clone(),
                            action1: existing_action.clone(),
                            action2: action.clone(),
                        });
                    }
                } else {
                    seen.insert(key, action.clone());
                }
            }
        }

        conflicts
    }

    /// Get all actions that have custom (non-default) bindings
    pub fn get_customized_actions(&self) -> HashSet<String> {
        let defaults = Self::defaults();
        let mut customized = HashSet::new();

        for (action, entries) in &self.bindings {
            if let Some(default_entries) = defaults.bindings.get(action) {
                if entries != default_entries {
                    customized.insert(action.clone());
                }
            } else {
                // Action exists in config but not in defaults
                customized.insert(action.clone());
            }
        }

        // Also check for actions in defaults that are missing from config
        for action in defaults.bindings.keys() {
            if !self.bindings.contains_key(action) {
                customized.insert(action.clone());
            }
        }

        customized
    }

}

/// Get the keybindings configuration file path
pub fn get_keybindings_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("okena")
        .join("keybindings.json")
}

/// Load keybinding configuration from disk
pub fn load_keybindings() -> KeybindingConfig {
    let path = get_keybindings_path();
    if path.exists() {
        if let Ok(content) = std::fs::read_to_string(&path) {
            match serde_json::from_str::<KeybindingConfig>(&content) {
                Ok(config) => {
                    // Check for conflicts and log warnings
                    let conflicts = config.detect_conflicts();
                    for conflict in &conflicts {
                        log::warn!("Keybinding conflict: {}", conflict);
                    }
                    return config;
                }
                Err(e) => {
                    log::warn!("Failed to parse keybindings config: {}, using defaults", e);
                }
            }
        }
    }
    KeybindingConfig::defaults()
}

/// Save keybinding configuration to disk
pub fn save_keybindings(config: &KeybindingConfig) -> anyhow::Result<()> {
    let path = get_keybindings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(config)?;
    std::fs::write(&path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_has_no_conflicts() {
        let config = KeybindingConfig::defaults();
        let conflicts = config.detect_conflicts();
        assert!(conflicts.is_empty(), "Default config should have no conflicts");
    }

}
