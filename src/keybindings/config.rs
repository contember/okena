use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Represents a single keybinding configuration
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeybindingEntry {
    /// The keystroke string (e.g., "cmd-b", "ctrl-shift-d")
    pub keystroke: String,
    /// Optional context for the keybinding (e.g., "TerminalPane", "FullscreenTerminal")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Whether this binding is enabled
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

impl KeybindingEntry {
    pub fn new(keystroke: impl Into<String>, context: Option<&str>) -> Self {
        Self {
            keystroke: keystroke.into(),
            context: context.map(String::from),
            enabled: true,
        }
    }

    #[allow(dead_code)]
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// Represents a conflict between two keybindings
#[derive(Clone, Debug)]
pub struct KeybindingConflict {
    pub keystroke: String,
    pub context: Option<String>,
    pub action1: String,
    pub action2: String,
}

impl std::fmt::Display for KeybindingConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ctx = self
            .context
            .as_ref()
            .map(|c| format!(" (in {})", c))
            .unwrap_or_default();
        write!(
            f,
            "'{}'{} conflicts: {} vs {}",
            self.keystroke, ctx, self.action1, self.action2
        )
    }
}

/// Human-readable description of an action
#[derive(Clone, Debug)]
pub struct ActionDescription {
    pub name: &'static str,
    pub description: &'static str,
    pub category: &'static str,
}

/// Get human-readable descriptions for all actions
pub fn get_action_descriptions() -> HashMap<&'static str, ActionDescription> {
    let mut map = HashMap::new();

    // Global actions
    map.insert(
        "ToggleSidebar",
        ActionDescription {
            name: "Toggle Sidebar",
            description: "Show or hide the sidebar",
            category: "Global",
        },
    );
    map.insert(
        "ToggleSidebarAutoHide",
        ActionDescription {
            name: "Toggle Auto-Hide",
            description: "Enable or disable sidebar auto-hide mode",
            category: "Global",
        },
    );
    map.insert(
        "ClearFocus",
        ActionDescription {
            name: "Clear Focus",
            description: "Clear focus and show all projects",
            category: "Global",
        },
    );

    // Fullscreen actions
    map.insert(
        "ExitFullscreen",
        ActionDescription {
            name: "Exit Fullscreen",
            description: "Exit fullscreen mode",
            category: "Fullscreen",
        },
    );
    map.insert(
        "ToggleFullscreen",
        ActionDescription {
            name: "Toggle Fullscreen",
            description: "Toggle fullscreen mode for focused terminal",
            category: "Fullscreen",
        },
    );
    map.insert(
        "FullscreenNextTerminal",
        ActionDescription {
            name: "Next Terminal",
            description: "Switch to next terminal in fullscreen",
            category: "Fullscreen",
        },
    );
    map.insert(
        "FullscreenPrevTerminal",
        ActionDescription {
            name: "Previous Terminal",
            description: "Switch to previous terminal in fullscreen",
            category: "Fullscreen",
        },
    );

    // Terminal pane actions
    map.insert(
        "SplitVertical",
        ActionDescription {
            name: "Split Vertical",
            description: "Split the terminal vertically",
            category: "Terminal",
        },
    );
    map.insert(
        "SplitHorizontal",
        ActionDescription {
            name: "Split Horizontal",
            description: "Split the terminal horizontally",
            category: "Terminal",
        },
    );
    map.insert(
        "AddTab",
        ActionDescription {
            name: "Add Tab",
            description: "Add a new tab (creates tab group if needed)",
            category: "Terminal",
        },
    );
    map.insert(
        "CloseTerminal",
        ActionDescription {
            name: "Close Terminal",
            description: "Close the current terminal",
            category: "Terminal",
        },
    );
    map.insert(
        "MinimizeTerminal",
        ActionDescription {
            name: "Minimize Terminal",
            description: "Minimize/detach the terminal",
            category: "Terminal",
        },
    );
    map.insert(
        "Copy",
        ActionDescription {
            name: "Copy",
            description: "Copy selected text",
            category: "Terminal",
        },
    );
    map.insert(
        "Paste",
        ActionDescription {
            name: "Paste",
            description: "Paste from clipboard",
            category: "Terminal",
        },
    );
    map.insert(
        "ScrollUp",
        ActionDescription {
            name: "Scroll Up",
            description: "Scroll terminal output up",
            category: "Terminal",
        },
    );
    map.insert(
        "ScrollDown",
        ActionDescription {
            name: "Scroll Down",
            description: "Scroll terminal output down",
            category: "Terminal",
        },
    );
    map.insert(
        "Search",
        ActionDescription {
            name: "Search",
            description: "Open search in terminal",
            category: "Terminal",
        },
    );
    map.insert(
        "SearchNext",
        ActionDescription {
            name: "Search Next",
            description: "Find next search match",
            category: "Search",
        },
    );
    map.insert(
        "SearchPrev",
        ActionDescription {
            name: "Search Previous",
            description: "Find previous search match",
            category: "Search",
        },
    );
    map.insert(
        "CloseSearch",
        ActionDescription {
            name: "Close Search",
            description: "Close search panel",
            category: "Search",
        },
    );

    // Navigation actions
    map.insert(
        "FocusLeft",
        ActionDescription {
            name: "Focus Left",
            description: "Move focus to the left terminal",
            category: "Navigation",
        },
    );
    map.insert(
        "FocusRight",
        ActionDescription {
            name: "Focus Right",
            description: "Move focus to the right terminal",
            category: "Navigation",
        },
    );
    map.insert(
        "FocusUp",
        ActionDescription {
            name: "Focus Up",
            description: "Move focus to the terminal above",
            category: "Navigation",
        },
    );
    map.insert(
        "FocusDown",
        ActionDescription {
            name: "Focus Down",
            description: "Move focus to the terminal below",
            category: "Navigation",
        },
    );
    map.insert(
        "FocusNextTerminal",
        ActionDescription {
            name: "Focus Next",
            description: "Move focus to the next terminal",
            category: "Navigation",
        },
    );
    map.insert(
        "FocusPrevTerminal",
        ActionDescription {
            name: "Focus Previous",
            description: "Move focus to the previous terminal",
            category: "Navigation",
        },
    );

    // Project actions
    map.insert(
        "NewProject",
        ActionDescription {
            name: "New Project",
            description: "Create a new project",
            category: "Project",
        },
    );
    map.insert(
        "CreateWorktree",
        ActionDescription {
            name: "Create Worktree",
            description: "Create a git worktree from the focused project",
            category: "Project",
        },
    );
    map.insert(
        "ShowKeybindings",
        ActionDescription {
            name: "Show Keybindings",
            description: "Display keybinding help",
            category: "Global",
        },
    );
    map.insert(
        "ShowSessionManager",
        ActionDescription {
            name: "Session Manager",
            description: "Open session manager to save/load workspaces",
            category: "Global",
        },
    );
    map.insert(
        "ShowThemeSelector",
        ActionDescription {
            name: "Theme Selector",
            description: "Open theme selector to change appearance",
            category: "Global",
        },
    );
    map.insert(
        "ShowCommandPalette",
        ActionDescription {
            name: "Command Palette",
            description: "Open command palette for quick access to all commands",
            category: "Global",
        },
    );
    map.insert(
        "ShowSettings",
        ActionDescription {
            name: "Settings",
            description: "Open settings panel",
            category: "Global",
        },
    );
    map.insert(
        "OpenSettingsFile",
        ActionDescription {
            name: "Open Settings File",
            description: "Open settings JSON file in default editor",
            category: "Global",
        },
    );

    map
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

        // Fullscreen keybindings
        bindings.insert(
            "ExitFullscreen".to_string(),
            vec![KeybindingEntry::new("escape", Some("FullscreenTerminal"))],
        );
        bindings.insert(
            "ToggleFullscreen".to_string(),
            vec![
                KeybindingEntry::new("shift-escape", Some("TerminalPane")),
                KeybindingEntry::new("shift-escape", Some("FullscreenTerminal")),
            ],
        );
        bindings.insert(
            "FullscreenNextTerminal".to_string(),
            vec![
                KeybindingEntry::new("cmd-]", Some("FullscreenTerminal")),
                KeybindingEntry::new("ctrl-]", Some("FullscreenTerminal")),
            ],
        );
        bindings.insert(
            "FullscreenPrevTerminal".to_string(),
            vec![
                KeybindingEntry::new("cmd-[", Some("FullscreenTerminal")),
                KeybindingEntry::new("ctrl-[", Some("FullscreenTerminal")),
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

    /// Reset a specific action to its default bindings
    #[allow(dead_code)]
    pub fn reset_action(&mut self, action: &str) {
        let defaults = Self::defaults();
        if let Some(default_bindings) = defaults.bindings.get(action) {
            self.bindings
                .insert(action.to_string(), default_bindings.clone());
        }
    }

    /// Reset all bindings to defaults
    #[allow(dead_code)]
    pub fn reset_all(&mut self) {
        *self = Self::defaults();
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

    /// Add a new keybinding for an action
    #[allow(dead_code)]
    pub fn add_binding(&mut self, action: &str, keystroke: &str, context: Option<&str>) {
        let entry = KeybindingEntry::new(keystroke, context);
        self.bindings
            .entry(action.to_string())
            .or_insert_with(Vec::new)
            .push(entry);
    }

    /// Remove a specific keybinding
    #[allow(dead_code)]
    pub fn remove_binding(&mut self, action: &str, keystroke: &str, context: Option<&str>) {
        if let Some(entries) = self.bindings.get_mut(action) {
            entries.retain(|e| !(e.keystroke == keystroke && e.context.as_deref() == context));
        }
    }

    /// Disable a specific keybinding without removing it
    #[allow(dead_code)]
    pub fn disable_binding(&mut self, action: &str, keystroke: &str, context: Option<&str>) {
        if let Some(entries) = self.bindings.get_mut(action) {
            for entry in entries {
                if entry.keystroke == keystroke && entry.context.as_deref() == context {
                    entry.enabled = false;
                }
            }
        }
    }

    /// Enable a specific keybinding
    #[allow(dead_code)]
    pub fn enable_binding(&mut self, action: &str, keystroke: &str, context: Option<&str>) {
        if let Some(entries) = self.bindings.get_mut(action) {
            for entry in entries {
                if entry.keystroke == keystroke && entry.context.as_deref() == context {
                    entry.enabled = true;
                }
            }
        }
    }

    /// Get enabled bindings for an action
    #[allow(dead_code)]
    pub fn get_enabled_bindings(&self, action: &str) -> Vec<&KeybindingEntry> {
        self.bindings
            .get(action)
            .map(|entries| entries.iter().filter(|e| e.enabled).collect())
            .unwrap_or_default()
    }

    /// Get all bindings grouped by category
    #[allow(dead_code)]
    pub fn get_bindings_by_category(&self) -> HashMap<&'static str, Vec<(&str, &KeybindingEntry)>> {
        let descriptions = get_action_descriptions();
        let mut categories: HashMap<&'static str, Vec<(&str, &KeybindingEntry)>> = HashMap::new();

        for (action, entries) in &self.bindings {
            let category = descriptions
                .get(action.as_str())
                .map(|d| d.category)
                .unwrap_or("Other");

            for entry in entries {
                categories
                    .entry(category)
                    .or_insert_with(Vec::new)
                    .push((action.as_str(), entry));
            }
        }

        categories
    }
}

/// Get the keybindings configuration file path
pub fn get_keybindings_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("term-manager-rs")
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

    #[test]
    fn test_conflict_detection() {
        let mut config = KeybindingConfig::defaults();
        // Add a conflicting binding
        config.add_binding("Copy", "cmd-b", None); // cmd-b is already used for ToggleSidebar

        let conflicts = config.detect_conflicts();
        assert!(!conflicts.is_empty(), "Should detect conflict");
        assert!(conflicts.iter().any(|c| c.keystroke == "cmd-b"));
    }

    #[test]
    fn test_reset_action() {
        let mut config = KeybindingConfig::defaults();
        config.bindings.remove("Copy");
        assert!(config.bindings.get("Copy").is_none());

        config.reset_action("Copy");
        assert!(config.bindings.get("Copy").is_some());
    }

    #[test]
    fn test_disable_enable_binding() {
        let mut config = KeybindingConfig::defaults();
        config.disable_binding("Copy", "cmd-c", Some("TerminalPane"));

        let enabled = config.get_enabled_bindings("Copy");
        assert!(!enabled.iter().any(|e| e.keystroke == "cmd-c"));

        config.enable_binding("Copy", "cmd-c", Some("TerminalPane"));
        let enabled = config.get_enabled_bindings("Copy");
        assert!(enabled.iter().any(|e| e.keystroke == "cmd-c"));
    }
}
