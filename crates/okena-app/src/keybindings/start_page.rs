//! App-local keybindings for configured Start Page buttons.

use super::{
    KeybindingEntry, ShortcutPlatform, apply_current_keybindings, bind_managed_keys,
    keystroke_to_config_string,
};
use crate::settings::{GlobalSettings, settings_entity};
use gpui::{Action, App, AsKeystroke, Global, KeyBinding, KeyBindingContextPredicate, Keystroke};
use okena_workspace::settings::StartPageButton;
use std::rc::Rc;

const EDITING_CONTEXTS: [&str; 6] = [
    "SimpleInput",
    "Input",
    "NumberInput",
    "SearchBar",
    "TerminalRename",
    "InPageSearch",
];

#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = okena, no_json)]
pub(crate) struct RunStartPageCommand {
    pub shortcut: String,
}

/// Each button's shortcut as validated by the last keymap rebuild, by button index.
#[derive(Default)]
struct StartPageShortcutAssignments(Vec<Option<ShortcutAssignment>>);

impl Global for StartPageShortcutAssignments {}

struct ShortcutAssignment {
    raw: String,
    /// The registered config string, or why the shortcut was not registered.
    result: Result<String, String>,
}

impl StartPageShortcutAssignments {
    fn registered(&self, index: usize, raw: &str) -> Option<&ShortcutAssignment> {
        self.0
            .get(index)?
            .as_ref()
            .filter(|assignment| assignment.raw == raw)
    }
}

/// Rebuild the managed keymap when assignments change, including settings synced
/// by the daemon. The action resolves the command and shell from current settings.
pub fn observe_start_page_shortcuts(cx: &mut App) {
    let entity = settings_entity(cx);
    // Validation is cached by button position, including buttons without a shortcut.
    let assignments = |buttons: &[StartPageButton]| -> Vec<Option<String>> {
        buttons
            .iter()
            .map(|button| button.shortcut.clone())
            .collect()
    };
    let mut previous = assignments(&entity.read(cx).settings.start_page_buttons);
    if previous.iter().any(Option::is_some) {
        apply_current_keybindings(cx);
    }
    cx.observe(&entity, move |entity, cx| {
        let current = assignments(&entity.read(cx).settings.start_page_buttons);
        if current != previous {
            previous = current;
            apply_current_keybindings(cx);
        }
    })
    .detach();
}

fn parse_shortcut_stroke(raw: &str, platform: ShortcutPlatform) -> Result<Keystroke, String> {
    let raw = raw.trim();
    if raw.split_whitespace().count() != 1 || raw.eq_ignore_ascii_case("unset") {
        return Err("Enter one key combination.".into());
    }
    let raw = raw.to_ascii_lowercase();
    let stroke = Keystroke::parse_with_os(&raw, platform.os())
        .map_err(|_| "Enter a valid shortcut, such as ctrl-alt-shift-x.".to_string())?;
    if !KeybindingEntry::new(raw, None).is_supported_on(platform) {
        return Err("This shortcut is reserved by the operating system.".into());
    }
    Ok(stroke)
}

fn shortcut_stroke(raw: &str, platform: ShortcutPlatform) -> Result<Keystroke, String> {
    let stroke = parse_shortcut_stroke(raw, platform)?;
    ensure_terminal_safe_shortcut(&stroke)?;
    Ok(stroke)
}

fn ensure_terminal_safe_shortcut(stroke: &Keystroke) -> Result<(), String> {
    let is_function_key = stroke
        .key
        .strip_prefix('f')
        .and_then(|number| number.parse::<u8>().ok())
        .is_some_and(|number| (1..=24).contains(&number));
    if !stroke.modifiers.control
        && !stroke.modifiers.alt
        && !stroke.modifiers.platform
        && !is_function_key
    {
        return Err("Use Ctrl, Alt, a platform modifier, or a function key.".into());
    }
    // Preserve shell control keys and unshifted AltGr combinations.
    let printable = stroke.key.chars().count() == 1 || stroke.key == "space";
    if printable && !stroke.modifiers.shift && !stroke.modifiers.platform {
        return Err("Add Shift to Ctrl or Alt for a printable key.".into());
    }
    Ok(())
}

pub(crate) fn validate_start_page_shortcut(
    raw: &str,
    buttons: &[StartPageButton],
    excluded_index: Option<usize>,
    cx: &App,
) -> Result<String, String> {
    let platform = ShortcutPlatform::current();
    let stroke = parse_shortcut_stroke(raw, platform)?;
    let mut single_conflict = false;
    for binding in cx.key_bindings().borrow().bindings() {
        if binding.action().as_any().is::<RunStartPageCommand>()
            || !binding
                .keystrokes()
                .first()
                .is_some_and(|key| key.as_keystroke() == &stroke)
        {
            continue;
        }
        if binding.keystrokes().len() > 1 {
            return Err(
                "This key starts an existing multi-step shortcut. Choose another combination."
                    .into(),
            );
        }
        single_conflict = true;
    }
    ensure_terminal_safe_shortcut(&stroke)?;
    for (index, button) in buttons.iter().enumerate() {
        if Some(index) != excluded_index
            && button
                .shortcut
                .as_deref()
                .and_then(|value| shortcut_stroke(value, platform).ok())
                .is_some_and(|other| other == stroke)
        {
            return Err(format!(
                "This shortcut is already assigned to {}.",
                button.name
            ));
        }
    }
    if single_conflict {
        return Err("This shortcut conflicts with an existing Okena binding.".into());
    }
    Ok(keystroke_to_config_string(&stroke, platform.os()))
}

pub(super) fn register_start_page_shortcuts(cx: &mut App) {
    let Some(settings) = cx.try_global::<GlobalSettings>() else {
        return;
    };
    let buttons = &settings.0.read(cx).settings.start_page_buttons;
    let context = EDITING_CONTEXTS
        .map(|context| format!("!{context}"))
        .join(" && ");
    let Ok(context) = KeyBindingContextPredicate::parse(&context) else {
        return;
    };
    let context = Rc::new(context);
    let mut assignments = Vec::with_capacity(buttons.len());
    let mut bindings = Vec::new();
    for (index, button) in buttons.iter().enumerate() {
        let Some(raw) = button.shortcut.clone() else {
            assignments.push(None);
            continue;
        };
        let result =
            validate_start_page_shortcut(&raw, buttons, Some(index), cx).and_then(|shortcut| {
                let binding = KeyBinding::load(
                    &shortcut,
                    Box::new(RunStartPageCommand {
                        shortcut: shortcut.clone(),
                    }),
                    Some(context.clone()),
                    false,
                    None,
                    cx,
                )
                .map_err(|error| error.to_string())?;
                bindings.push(binding);
                Ok(shortcut)
            });
        if let Err(error) = &result {
            log::warn!("Ignoring Start Page shortcut for {}: {error}", button.name);
        }
        assignments.push(Some(ShortcutAssignment { raw, result }));
    }
    bind_managed_keys(cx, bindings);
    cx.set_global(StartPageShortcutAssignments(assignments));
}

pub(crate) fn start_page_shortcut_label(shortcut: &str, cx: &App) -> Option<String> {
    active_start_page_shortcut(shortcut, cx).map(|(_, label)| label)
}

pub(crate) fn active_start_page_shortcut(shortcut: &str, cx: &App) -> Option<(String, String)> {
    let platform = ShortcutPlatform::current();
    let stroke = shortcut_stroke(shortcut, platform).ok()?;
    let action = RunStartPageCommand {
        shortcut: keystroke_to_config_string(&stroke, platform.os()),
    };
    // Discovery surfaces describe the registered shortcut in a terminal context
    // even when the current focus is in a settings dialog or another pane.
    let context = gpui::KeyContext::parse("TerminalPane").ok()?;
    let label = okena_ui::shortcuts::shortcut_for_action_in(&action, &[context], cx)?;
    Some((action.shortcut, label))
}

/// Why a button's saved shortcut is not active, if it is not.
pub(crate) fn start_page_shortcut_error(
    index: usize,
    buttons: &[StartPageButton],
    cx: &App,
) -> Option<String> {
    let raw = buttons.get(index)?.shortcut.as_deref()?;
    let registered = cx
        .try_global::<StartPageShortcutAssignments>()
        .and_then(|assignments| assignments.registered(index, raw));
    match registered {
        Some(assignment) => assignment.result.clone().err(),
        // The list differs from the last rebuild, for example while a save is in flight.
        None => validate_start_page_shortcut(raw, buttons, Some(index), cx).err(),
    }
}

/// The button whose registered shortcut is the single keystroke `shortcut`.
pub(crate) fn start_page_button_for_shortcut<'a>(
    shortcut: &str,
    buttons: &'a [StartPageButton],
    cx: &App,
) -> Option<&'a StartPageButton> {
    let platform = ShortcutPlatform::current();
    let shortcut =
        keystroke_to_config_string(&shortcut_stroke(shortcut, platform).ok()?, platform.os());
    let assignments = cx.try_global::<StartPageShortcutAssignments>()?;
    buttons.iter().enumerate().find_map(|(index, button)| {
        let assignment = assignments.registered(index, button.shortcut.as_deref()?)?;
        matches!(&assignment.result, Ok(registered) if *registered == shortcut).then_some(button)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::AppContext;

    #[test]
    fn windows_start_page_shortcut_accepts_modifier_orders_and_rejects_chords() {
        let platform = ShortcutPlatform::Windows;
        assert_eq!(
            shortcut_stroke("ctrl-alt-shift-x", platform).unwrap(),
            shortcut_stroke("shift-ctrl-alt-x", platform).unwrap()
        );
        assert!(shortcut_stroke("ctrl-k ctrl-x", platform).is_err());
        assert!(shortcut_stroke("win-x", platform).is_err());
        assert!(shortcut_stroke("shift-x", platform).is_err());
        assert!(shortcut_stroke("ctrl-c", platform).is_err());
        assert!(shortcut_stroke("alt-b", platform).is_err());
        assert!(shortcut_stroke("ctrl-alt-x", platform).is_err());
        assert!(shortcut_stroke("f12", platform).is_ok());
    }

    #[gpui::test]
    fn start_page_shortcuts_do_not_override_existing_or_duplicate_assignments(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            cx.bind_keys([
                KeyBinding::new("ctrl-k ctrl-s", super::super::ShowKeybindings, None),
                KeyBinding::new("ctrl-shift-k ctrl-x", super::super::AddTab, None),
                KeyBinding::new("ctrl-shift-y", super::super::AddTab, None),
                KeyBinding::new("ctrl-c", super::super::AddTab, None),
            ]);
            let button = StartPageButton {
                name: "Codex".into(),
                shortcut: Some("shift-ctrl-alt-x".into()),
                ..Default::default()
            };
            let buttons = [button.clone(), button];
            for prefix in ["ctrl-k", "ctrl-shift-k"] {
                let error = validate_start_page_shortcut(prefix, &[], None, cx).unwrap_err();
                assert!(error.contains("multi-step"));
            }
            for unsafe_key in ["ctrl-c", "ctrl-alt-x"] {
                let error = validate_start_page_shortcut(unsafe_key, &[], None, cx).unwrap_err();
                assert!(error.contains("Add Shift"));
            }
            let error = validate_start_page_shortcut("ctrl-shift-y", &[], None, cx).unwrap_err();
            assert!(error.contains("existing Okena binding"));
            assert!(
                validate_start_page_shortcut("ctrl-alt-shift-x", &buttons[..1], Some(0), cx)
                    .is_ok()
            );
            assert!(
                validate_start_page_shortcut("ctrl-alt-shift-x", &buttons, Some(0), cx).is_err()
            );
        });
    }

    #[gpui::test]
    fn rebuilding_keymap_applies_changed_and_removed_start_page_assignments(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let settings = cx.new(|_| {
                let mut settings = okena_workspace::settings::AppSettings::default();
                settings.start_page_buttons.push(StartPageButton {
                    name: "Codex".into(),
                    shortcut: Some("ctrl-alt-shift-x".into()),
                    ..Default::default()
                });
                crate::settings::SettingsState::new(settings)
            });
            cx.set_global(GlobalSettings(settings.clone()));
            let config = super::super::KeybindingConfig::defaults();
            let assignments = |cx: &App| {
                cx.key_bindings()
                    .borrow()
                    .bindings()
                    .filter_map(|binding| {
                        binding
                            .action()
                            .as_any()
                            .downcast_ref::<RunStartPageCommand>()
                            .map(|action| action.shortcut.clone())
                    })
                    .collect::<Vec<_>>()
            };
            super::super::apply_keybindings(cx, &config);
            assert_eq!(
                assignments(cx),
                [validate_start_page_shortcut("ctrl-alt-shift-x", &[], None, cx).unwrap()]
            );
            let buttons = settings.read(cx).settings.start_page_buttons.clone();
            assert_eq!(
                start_page_button_for_shortcut("shift-ctrl-alt-x", &buttons, cx)
                    .map(|button| button.name.as_str()),
                Some("Codex")
            );
            assert!(start_page_button_for_shortcut("ctrl-alt-shift-y", &buttons, cx).is_none());
            assert_eq!(start_page_shortcut_error(0, &buttons, cx), None);

            {
                let keymap = cx.key_bindings();
                let keymap = keymap.borrow();
                let binding = keymap
                    .bindings()
                    .find(|binding| binding.action().as_any().is::<RunStartPageCommand>())
                    .unwrap();
                assert_eq!(
                    start_page_shortcut_label("shift-ctrl-alt-x", cx),
                    Some(okena_ui::shortcuts::format_keystroke("ctrl-alt-shift-x"))
                );
                assert!(start_page_shortcut_label("ctrl-k", cx).is_none());
                let contexts = [gpui::KeyContext::parse("TerminalPane").unwrap()];
                assert_eq!(
                    keymap
                        .bindings_for_input(binding.keystrokes(), &contexts)
                        .0
                        .len(),
                    1
                );
                let contexts = [
                    gpui::KeyContext::parse("TerminalPane").unwrap(),
                    gpui::KeyContext::parse("SimpleInput").unwrap(),
                ];
                assert!(
                    keymap
                        .bindings_for_input(binding.keystrokes(), &contexts)
                        .0
                        .is_empty()
                );
            }

            settings.update(cx, |settings, _| {
                settings.settings.start_page_buttons[0].shortcut = Some("ctrl-alt-shift-y".into());
            });
            super::super::apply_keybindings(cx, &config);
            assert_eq!(
                assignments(cx),
                [validate_start_page_shortcut("ctrl-alt-shift-y", &[], None, cx).unwrap()]
            );

            settings.update(cx, |settings, _| {
                settings.settings.start_page_buttons[0].shortcut = None;
            });
            super::super::apply_keybindings(cx, &config);
            assert!(assignments(cx).is_empty());
            assert!(start_page_shortcut_label("ctrl-alt-shift-y", cx).is_none());

            // A hand-edited setting may be syntactically valid but blocked by
            // an existing action. Its button must not advertise a working key.
            cx.bind_keys([KeyBinding::new(
                "ctrl-alt-shift-y",
                super::super::AddTab,
                None,
            )]);
            settings.update(cx, |settings, _| {
                settings.settings.start_page_buttons[0].shortcut = Some("ctrl-alt-shift-y".into());
            });
            super::super::apply_keybindings(cx, &config);
            assert!(start_page_shortcut_label("ctrl-alt-shift-y", cx).is_none());
            let buttons = settings.read(cx).settings.start_page_buttons.clone();
            assert!(start_page_button_for_shortcut("ctrl-alt-shift-y", &buttons, cx).is_none());
            assert!(
                start_page_shortcut_error(0, &buttons, cx)
                    .is_some_and(|error| error.contains("existing Okena binding"))
            );
        });
    }

    #[gpui::test]
    fn deleting_an_unbound_button_keeps_the_following_shortcut(cx: &mut gpui::TestAppContext) {
        let shortcut = "ctrl-alt-shift-x";
        let settings = cx.update(|cx| {
            let settings = cx.new(|_| {
                let mut settings = okena_workspace::settings::AppSettings::default();
                settings.start_page_buttons = vec![
                    StartPageButton {
                        name: "Shell".into(),
                        ..Default::default()
                    },
                    StartPageButton {
                        name: "Codex".into(),
                        command: "codex --no-daemon".into(),
                        shortcut: Some(shortcut.into()),
                        ..Default::default()
                    },
                ];
                crate::settings::SettingsState::new(settings)
            });
            cx.set_global(GlobalSettings(settings.clone()));
            super::super::set_test_keybinding_config(super::super::KeybindingConfig::defaults());
            observe_start_page_shortcuts(cx);
            settings
        });

        let command_for_shortcut = |cx: &App| {
            let buttons = &settings.read(cx).settings.start_page_buttons;
            start_page_button_for_shortcut(shortcut, buttons, cx)
                .map(|button| button.command.clone())
        };
        cx.update(|cx| {
            assert_eq!(
                command_for_shortcut(cx).as_deref(),
                Some("codex --no-daemon")
            )
        });
        cx.update(|cx| {
            settings.update(cx, |state, cx| {
                let mut buttons = state.settings.start_page_buttons.clone();
                buttons.remove(0);
                state.set_start_page_buttons(buttons, cx);
            });
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(
                command_for_shortcut(cx).as_deref(),
                Some("codex --no-daemon")
            );
        });
        cx.update(|cx| {
            settings.update(cx, |state, cx| state.set_start_page_buttons(Vec::new(), cx));
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(command_for_shortcut(cx).is_none());
            assert!(start_page_shortcut_label(shortcut, cx).is_none());
        });
    }

    #[gpui::test]
    fn duplicate_start_page_assignments_register_neither_button(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let button = |name: &str| StartPageButton {
                name: name.into(),
                shortcut: Some("ctrl-alt-shift-x".into()),
                ..Default::default()
            };
            let settings = cx.new(|_| {
                let mut settings = okena_workspace::settings::AppSettings::default();
                settings.start_page_buttons = vec![button("Codex"), button("Claude")];
                crate::settings::SettingsState::new(settings)
            });
            cx.set_global(GlobalSettings(settings.clone()));
            super::super::apply_keybindings(cx, &super::super::KeybindingConfig::defaults());

            let buttons = settings.read(cx).settings.start_page_buttons.clone();
            assert!(start_page_button_for_shortcut("ctrl-alt-shift-x", &buttons, cx).is_none());
            assert!(start_page_shortcut_label("ctrl-alt-shift-x", cx).is_none());
            assert!(
                start_page_shortcut_error(0, &buttons, cx)
                    .is_some_and(|error| error.contains("Claude"))
            );
            assert!(
                start_page_shortcut_error(1, &buttons, cx)
                    .is_some_and(|error| error.contains("Codex"))
            );
        });
    }
}
