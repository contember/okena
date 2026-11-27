//! Pure filtering and capture state; never writes configuration or dispatches actions.
use super::ActionBindings;
use crate::keybindings::{KeybindingConfig, KeybindingEntry, get_action_descriptions};
use gpui::Keystroke;
use okena_ui::shortcuts::{ShortcutPlatform, format_keystroke};

#[derive(Default, Clone)]
pub(super) struct CaptureQuery {
    strokes: Vec<Keystroke>,
    waiting: bool,
}

fn sequence(source: &str) -> Option<Vec<Keystroke>> {
    source
        .split_whitespace()
        .map(|s| Keystroke::parse_with_os(s, ShortcutPlatform::current().os()).ok())
        .collect()
}

pub(super) fn held_key(key: &Keystroke, cx: &gpui::App) -> String {
    cx.keyboard_mapper()
        .map_key_equivalent(key.clone(), false)
        .key()
        .to_owned()
}

fn same(a: &Keystroke, b: &Keystroke) -> bool {
    b.should_match(&gpui::KeybindingKeystroke::from_keystroke(a.clone()))
}

fn matches_prefix(keys: &[Keystroke], prefix: &[Keystroke]) -> bool {
    keys.len() >= prefix.len() && keys.iter().zip(prefix).all(|(a, b)| same(a, b))
}

impl CaptureQuery {
    pub(super) fn push(&mut self, key: &Keystroke, config: &KeybindingConfig, all_platforms: bool) {
        if matches!(
            key.key.as_str(),
            "shift" | "control" | "alt" | "platform" | "function" | ""
        ) {
            return;
        }
        if !self.waiting {
            self.strokes.clear();
        }
        self.strokes.push(key.clone());
        self.waiting = self.strokes.len() == 1
            && config.bindings.values().flatten().any(|entry| {
                (all_platforms || entry.applies_to(ShortcutPlatform::current()))
                    && sequence(&entry.keystroke)
                        .is_some_and(|keys| keys.len() > 1 && matches_prefix(&keys, &self.strokes))
            });
    }

    pub(super) fn matches(&self, keystroke: Option<&str>) -> bool {
        if self.strokes.is_empty() {
            return true;
        }
        keystroke.and_then(sequence).is_some_and(|keys| {
            matches_prefix(&keys, &self.strokes)
                && (self.waiting || keys.len() == self.strokes.len())
        })
    }

    pub(super) fn label(&self, os: gpui::OperatingSystem) -> String {
        if self.strokes.is_empty() {
            return "Press a shortcut to search. Esc clears; Text search exits recording.".into();
        }
        let label = self
            .strokes
            .iter()
            .map(|k| format_keystroke(&crate::keybindings::keystroke_to_config_string(k, os)))
            .collect::<Vec<_>>()
            .join(", ");
        if self.waiting {
            format!("{label} — Waiting for next key…")
        } else {
            label
        }
    }
}

fn real(entry: &KeybindingEntry) -> bool {
    !entry.keystroke.trim().is_empty() && entry.keystroke != "unset"
}

/// All-platform rows should show the owning platform's keys, not reinterpret Cmd as Win.
pub(super) fn display_platform(
    entry: &KeybindingEntry,
    current: ShortcutPlatform,
) -> ShortcutPlatform {
    if entry.applies_to(current) {
        current
    } else {
        entry
            .platforms
            .as_ref()
            .and_then(|p| p.first())
            .copied()
            .unwrap_or(current)
    }
}

pub(super) fn default_context(action: &str, config: &KeybindingConfig) -> Option<String> {
    if let Some(entry) = config.bindings.get(action).and_then(|v| {
        v.iter()
            .find(|e| e.enabled && real(e) && e.applies_to(ShortcutPlatform::current()))
    }) {
        return entry.context.clone();
    }
    if let Some(entry) = KeybindingConfig::defaults()
        .bindings
        .get(action)
        .and_then(|v| v.first())
    {
        return entry.context.clone();
    }
    // Commands without defaults that operate on the focused terminal.
    match action {
        "ExportTerminalBuffer" | "DetachTerminal" | "MinimizeTerminal" => {
            Some("TerminalPane".into())
        }
        _ => None,
    }
}

pub(super) fn build_rows(
    config: &KeybindingConfig,
    query: &str,
    capture: Option<&CaptureQuery>,
    assigned_only: bool,
    all_platforms: bool,
) -> Vec<(&'static str, ActionBindings)> {
    build_rows_for_platform(
        config,
        query,
        capture,
        assigned_only,
        all_platforms,
        ShortcutPlatform::current(),
    )
}

fn build_rows_for_platform(
    config: &KeybindingConfig,
    query: &str,
    capture: Option<&CaptureQuery>,
    assigned_only: bool,
    all_platforms: bool,
    platform: ShortcutPlatform,
) -> Vec<(&'static str, ActionBindings)> {
    let descriptions = get_action_descriptions();
    let customized = config.get_customized_actions();
    let mut names: Vec<_> = descriptions
        .keys()
        .filter(|name| crate::keybindings::action_for_name(name).is_some())
        .map(|s| s.to_string())
        .chain(config.bindings.keys().cloned())
        .collect();
    names.sort();
    names.dedup();
    let mut rows = Vec::new();
    for action in names {
        let desc = descriptions.get(action.as_str());
        let category = desc.map_or("Other", |d| d.category);
        let visible: Vec<_> = config
            .bindings
            .get(&action)
            .into_iter()
            .flatten()
            .enumerate()
            .filter(|(_, e)| real(e) && (all_platforms || e.applies_to(platform)))
            .collect();
        if assigned_only && visible.is_empty() {
            continue;
        }
        let entries: Vec<_> = if let Some(capture) = capture {
            visible
                .into_iter()
                .filter(|(_, e)| capture.matches(Some(&e.keystroke)))
                .collect()
        } else {
            let metadata_matches = query.is_empty()
                || action.to_lowercase().contains(query)
                || category.to_lowercase().contains(query)
                || desc.is_some_and(|d| {
                    d.name.to_lowercase().contains(query)
                        || d.description.to_lowercase().contains(query)
                });
            visible
                .into_iter()
                .filter(|(_, e)| metadata_matches || e.keystroke.to_lowercase().contains(query))
                .collect()
        };
        let nonempty_capture = capture.is_some_and(|q| !q.strokes.is_empty());
        if entries.is_empty()
            && (nonempty_capture
                || (capture.is_none()
                    && !query.is_empty()
                    && !action.to_lowercase().contains(query)
                    && !category.to_lowercase().contains(query)
                    && !desc.is_some_and(|d| {
                        d.name.to_lowercase().contains(query)
                            || d.description.to_lowercase().contains(query)
                    })))
        {
            continue;
        }
        let details = entries
            .into_iter()
            .map(|(idx, e)| {
                (
                    e.keystroke.clone(),
                    idx,
                    customized.contains(&action),
                    e.enabled,
                )
            })
            .collect();
        rows.push((category, (action, details)));
    }
    rows.sort_by(|a, b| {
        let name = |r: &(&str, ActionBindings)| {
            descriptions
                .get(r.1.0.as_str())
                .map_or(r.1.0.as_str(), |d| d.name)
                .to_lowercase()
        };
        name(a).cmp(&name(b))
    });
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_includes_unassigned_and_preserves_filtered_indices() {
        let mut config = KeybindingConfig::defaults();
        config
            .bindings
            .insert("ExpandActiveProjects".into(), vec![]);
        assert!(
            build_rows(&config, "", None, false, false)
                .iter()
                .any(|r| r.1.0 == "ExpandActiveProjects")
        );
        assert!(
            !build_rows(&config, "", None, true, false)
                .iter()
                .any(|r| r.1.0 == "ExpandActiveProjects")
        );
        config.bindings.insert(
            "ExpandActiveProjects".into(),
            vec![
                KeybindingEntry::for_platforms("cmd-e", None, &[ShortcutPlatform::Mac]),
                KeybindingEntry::for_platforms("ctrl-e", None, &[ShortcutPlatform::Windows]),
            ],
        );
        let rows = build_rows_for_platform(
            &config,
            "expand active",
            None,
            true,
            false,
            ShortcutPlatform::Windows,
        );
        let row = rows
            .iter()
            .find(|r| r.1.0 == "ExpandActiveProjects")
            .unwrap();
        assert_eq!(row.1.1[0].1, 1);
    }
    #[test]
    fn capture_waits_without_timer_and_matches_complete_sequences() {
        let config = KeybindingConfig::defaults();
        let mut q = CaptureQuery::default();
        q.push(&Keystroke::parse("ctrl-k").unwrap(), &config, true);
        assert!(q.waiting);
        assert!(q.label(gpui::OperatingSystem::Linux).contains("Waiting"));
        q.push(&Keystroke::parse("ctrl-s").unwrap(), &config, true);
        assert!(!q.waiting);
        let rows = build_rows(&config, "ignored text", Some(&q), false, true);
        assert!(rows.iter().any(|r| r.1.0 == "ShowKeybindings"));
        assert!(!rows.iter().any(|r| r.1.0 == "ExpandActiveProjects"));
        q.push(&Keystroke::parse("ctrl-f12").unwrap(), &config, true);
        assert!(build_rows(&config, "", Some(&q), false, true).is_empty());
        q = CaptureQuery::default();
        assert!(!build_rows(&config, "", Some(&q), false, true).is_empty());
    }
    #[test]
    fn disabled_is_assigned_placeholder_is_not_and_context_survives() {
        let mut config = KeybindingConfig::defaults();
        let mut e = KeybindingEntry::new("ctrl-alt-f12", Some("TerminalPane"));
        e.enabled = false;
        config.bindings.insert("DetachTerminal".into(), vec![e]);
        config.bindings.insert(
            "ExpandActiveProjects".into(),
            vec![KeybindingEntry::new("unset", None)],
        );
        let rows = build_rows(&config, "", None, true, true);
        assert!(
            rows.iter()
                .any(|r| r.1.0 == "DetachTerminal" && !r.1.1[0].3)
        );
        assert!(!rows.iter().any(|r| r.1.0 == "ExpandActiveProjects"));
        assert_eq!(
            default_context("DetachTerminal", &config).as_deref(),
            Some("TerminalPane")
        );
    }
    #[test]
    fn prefix_includes_singles_and_characters_follow_dispatch_matching() {
        let mut config = KeybindingConfig::defaults();
        config
            .bindings
            .insert("Quit".into(), vec![KeybindingEntry::new("ctrl-k", None)]);
        let mut q = CaptureQuery::default();
        q.push(&Keystroke::parse("ctrl-k").unwrap(), &config, true);
        let rows = build_rows(&config, "", Some(&q), true, true);
        assert!(rows.iter().any(|r| r.1.0 == "Quit"));
        assert!(rows.iter().any(|r| r.1.0 == "ShowKeybindings"));
        q.push(&Keystroke::parse("ctrl-s").unwrap(), &config, true);
        assert!(
            !build_rows(&config, "", Some(&q), true, true)
                .iter()
                .any(|r| r.1.0 == "Quit")
        );
        config
            .bindings
            .insert("Quit".into(), vec![KeybindingEntry::new("@", None)]);
        q = CaptureQuery::default();
        q.push(
            &Keystroke {
                key: "q".into(),
                key_char: Some("@".into()),
                modifiers: Default::default(),
            },
            &config,
            true,
        );
        assert!(
            build_rows(&config, "", Some(&q), true, true)
                .iter()
                .any(|r| r.1.0 == "Quit")
        );
        q.push(&Keystroke::parse("ctrl-alt-q").unwrap(), &config, true);
        assert!(build_rows(&config, "", Some(&q), true, true).is_empty());
    }
    #[test]
    fn non_windows_rows_show_their_own_platform_keys() {
        let entry = KeybindingEntry::for_platforms(
            "cmd-alt-left",
            None,
            &[ShortcutPlatform::Mac, ShortcutPlatform::Linux],
        );
        assert!(matches!(
            display_platform(&entry, ShortcutPlatform::Windows),
            ShortcutPlatform::Mac
        ));
        assert!(matches!(
            display_platform(&entry, ShortcutPlatform::Linux),
            ShortcutPlatform::Linux
        ));
        let custom =
            KeybindingEntry::for_platforms("cmd-alt-left", None, &[ShortcutPlatform::Windows]);
        assert!(matches!(
            display_platform(&custom, ShortcutPlatform::Windows),
            ShortcutPlatform::Windows
        ));
    }
}
