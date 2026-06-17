use gpui::Action;
use serde::{Deserialize, Serialize};

/// Represents a single keybinding configuration
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeybindingEntry {
    /// The keystroke string (e.g., "cmd-b", "ctrl-shift-d")
    pub keystroke: String,
    /// Optional context for the keybinding (e.g., "TerminalPane")
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
#[derive(Clone)]
pub struct ActionDescription {
    pub name: &'static str,
    pub description: &'static str,
    pub category: &'static str,
    /// Factory to create a boxed Action for dispatch
    pub factory: fn() -> Box<dyn Action>,
}
