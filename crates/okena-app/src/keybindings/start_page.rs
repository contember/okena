//! App-local keybindings for configured Start Page buttons.

use super::{
    KeybindingEntry, ShortcutPlatform, apply_current_keybindings, bind_managed_keys,
    keystroke_to_config_string,
};
use crate::settings::{GlobalSettings, settings_entity};
use gpui::{Action, App, AsKeystroke, KeyBinding, KeyBindingContextPredicate, Keystroke, Window};
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

/// Rebuild the managed keymap when assignments change, including settings synced
/// by the daemon. The action resolves the command and shell from current settings.
pub fn observe_start_page_shortcuts(cx: &mut App) {
    let entity = settings_entity(cx);
    let assignments = |buttons: &[StartPageButton]| -> Vec<String> {
        buttons
            .iter()
            .filter_map(|button| button.shortcut.clone())
            .collect()
    };
    let mut previous = assignments(&entity.read(cx).settings.start_page_buttons);
    if !previous.is_empty() {
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
    let bindings = buttons
        .iter()
        .enumerate()
        .filter_map(|(index, button)| {
            let raw = button.shortcut.as_deref()?;
            let shortcut = match validate_start_page_shortcut(raw, buttons, Some(index), cx) {
                Ok(shortcut) => shortcut,
                Err(error) => {
                    log::warn!("Ignoring Start Page shortcut for {}: {error}", button.name);
                    return None;
                }
            };
            KeyBinding::load(
                &shortcut,
                Box::new(RunStartPageCommand {
                    shortcut: shortcut.clone(),
                }),
                Some(context.clone()),
                false,
                None,
                cx,
            )
            .ok()
        })
        .collect::<Vec<_>>();
    bind_managed_keys(cx, bindings);
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

pub(crate) fn start_page_button_for_shortcut<'a>(
    shortcut: &str,
    buttons: &'a [StartPageButton],
) -> Option<&'a StartPageButton> {
    let platform = ShortcutPlatform::current();
    let shortcut = shortcut_stroke(shortcut, platform).ok()?;
    let mut matching = buttons.iter().filter(|button| {
        button
            .shortcut
            .as_deref()
            .and_then(|raw| shortcut_stroke(raw, platform).ok())
            .is_some_and(|stroke| stroke == shortcut)
    });
    let button = matching.next()?;
    matching.next().is_none().then_some(button)
}

pub(crate) fn start_page_shortcut_is_editing(window: &Window) -> bool {
    window
        .context_stack()
        .iter()
        .any(|context| EDITING_CONTEXTS.iter().any(|name| context.contains(name)))
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
            assert!(start_page_button_for_shortcut("ctrl-alt-shift-x", &buttons).is_none());
            assert_eq!(
                start_page_button_for_shortcut("ctrl-alt-shift-x", &buttons[..1])
                    .map(|button| button.name.as_str()),
                Some("Codex")
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
        });
    }
}
