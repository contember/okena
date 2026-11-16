use gpui::Action;
use okena_ui::shortcuts::{ShortcutPlatform, applies_to_platform};
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
    /// Platforms on which this binding is available. Missing means all platforms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platforms: Option<Vec<ShortcutPlatform>>,
}

fn default_enabled() -> bool {
    true
}

impl KeybindingEntry {
    pub fn applies_to(&self, platform: ShortcutPlatform) -> bool {
        applies_to_platform(self.platforms.as_deref(), platform)
    }

    /// Windows owns the Win modifier at OS level. Do not register any Win-key
    /// assignment, including custom JSON entries or a Win stroke inside a chord.
    /// This is an Okena-local binding policy, not a system-wide hotkey service.
    pub fn is_supported_on(&self, platform: ShortcutPlatform) -> bool {
        self.applies_to(platform)
            && !(platform == ShortcutPlatform::Windows
                && self.keystroke.split_whitespace().any(|stroke| {
                    gpui::Keystroke::parse_with_os(stroke, platform.os())
                        .is_ok_and(|key| key.modifiers.platform)
                }))
    }

    /// Explicitly scope a default variant without interpreting its modifiers.
    pub fn for_platforms(
        keystroke: impl Into<String>,
        context: Option<&str>,
        platforms: &[ShortcutPlatform],
    ) -> Self {
        Self {
            platforms: Some(platforms.to_vec()),
            ..Self::new(keystroke, context)
        }
    }

    pub fn new(keystroke: impl Into<String>, context: Option<&str>) -> Self {
        Self {
            keystroke: keystroke.into(),
            context: context.map(String::from),
            enabled: true,
            platforms: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_scopes_round_trip_without_migrating_legacy_entries() {
        let legacy = r#"{"keystroke":"cmd-d"}"#;
        let entry: KeybindingEntry = serde_json::from_str(legacy).unwrap();
        assert!(entry.enabled);
        assert_eq!(entry.platforms, None);
        assert!(!serde_json::to_string(&entry).unwrap().contains("platforms"));
        for platform in [
            ShortcutPlatform::Mac,
            ShortcutPlatform::Windows,
            ShortcutPlatform::Linux,
        ] {
            assert!(entry.applies_to(platform));
            let scoped = KeybindingEntry::for_platforms("cmd-d", None, &[platform]);
            let json = serde_json::to_string(&scoped).unwrap();
            let name = match platform {
                ShortcutPlatform::Mac => "macos",
                ShortcutPlatform::Windows => "windows",
                ShortcutPlatform::Linux => "linux",
            };
            assert!(json.contains(name));
            assert_eq!(
                serde_json::from_str::<KeybindingEntry>(&json).unwrap(),
                scoped
            );
            let empty = KeybindingEntry::for_platforms("cmd-d", None, &[]);
            assert!(!empty.applies_to(platform));
            for other in [
                ShortcutPlatform::Mac,
                ShortcutPlatform::Windows,
                ShortcutPlatform::Linux,
            ] {
                assert_eq!(scoped.applies_to(other), platform == other);
            }
        }
        assert!(
            serde_json::from_str::<KeybindingEntry>(
                r#"{"keystroke":"cmd-d","platforms":["unknown"]}"#
            )
            .is_err()
        );
    }
}
