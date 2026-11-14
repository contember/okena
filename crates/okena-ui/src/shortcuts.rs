//! Shared shortcut labels for tooltips, keybinding editors and command lists.
use gpui::{Action, AnyView, App, AsKeystroke, KeyBinding, Keystroke, Window};
use gpui_component::tooltip::Tooltip;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ShortcutPlatform {
    Mac,
    Windows,
    Linux,
}

impl ShortcutPlatform {
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

/// Formats every stroke, preserving punctuation keys such as `ctrl--`.
/// Invalid/in-progress editor input is left intact.
pub fn format_keystroke(source: &str) -> String {
    format_keystroke_for_platform(source, ShortcutPlatform::current())
}

pub fn format_keystroke_for_platform(source: &str, platform: ShortcutPlatform) -> String {
    source
        .split_whitespace()
        .map(|source| {
            Keystroke::parse(source)
                .map(|key| {
                    if key.key.is_empty() {
                        source.to_owned()
                    } else {
                        format_stroke(&key, platform)
                    }
                })
                .unwrap_or_else(|_| source.to_owned())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_stroke(key: &Keystroke, platform: ShortcutPlatform) -> String {
    let mac = platform == ShortcutPlatform::Mac;
    let mut parts = Vec::new();
    if key.modifiers.control {
        parts.push(if mac { "⌃" } else { "Ctrl" });
    }
    if key.modifiers.alt {
        parts.push(if mac { "⌥" } else { "Alt" });
    }
    if key.modifiers.shift {
        parts.push(if mac { "⇧" } else { "Shift" });
    }
    if key.modifiers.platform {
        parts.push(match platform {
            ShortcutPlatform::Mac => "⌘",
            ShortcutPlatform::Windows => "Win",
            ShortcutPlatform::Linux => "Super",
        });
    }
    if key.modifiers.function {
        parts.push("Fn");
    }
    let name = match key.key.as_str() {
        "escape" => if mac { "⎋" } else { "Esc" }.into(),
        "enter" => if mac { "↩" } else { "Enter" }.into(),
        "backspace" => if mac { "⌫" } else { "Backspace" }.into(),
        "delete" => if mac { "⌦" } else { "Delete" }.into(),
        "pageup" => "PgUp".into(),
        "pagedown" => "PgDn".into(),
        "left" => "←".into(),
        "right" => "→".into(),
        "up" => "↑".into(),
        "down" => "↓".into(),
        "space" => "Space".into(),
        "tab" => "Tab".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        name if name.len() == 1 || name.starts_with('f') && name[1..].parse::<u8>().is_ok() => {
            name.to_uppercase()
        }
        name => {
            let mut chars = name.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        }
    };
    parts.push(&name);
    parts.join(if mac { "" } else { "+" })
}

/// Formats a binding with the same stroke formatter as the keybindings editor.
pub fn format_binding(binding: &KeyBinding) -> String {
    let platform = ShortcutPlatform::current();
    binding
        .keystrokes()
        .iter()
        .map(|key| format_stroke(key.as_keystroke(), platform))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Returns all effective bindings on the actual focused dispatch path, retaining
/// parameters, nested contexts and registration order for other UI consumers.
pub fn bindings_for_action(action: &dyn Action, window: &Window, cx: &App) -> Vec<KeyBinding> {
    let contexts = window.context_stack();
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    // Use the same keymap input resolver as GPUI dispatch, with the actual path.
    // A missing root context is an empty stack, not a synthetic empty context:
    // negated predicates (e.g. !TerminalPane) distinguish those two cases.
    keymap
        .bindings_for_action(action)
        .filter(|binding| {
            keymap
                .bindings_for_input(binding.keystrokes(), &contexts)
                .0
                .first()
                .is_some_and(|effective| effective.action().partial_eq(action))
        })
        .cloned()
        .collect()
}

/// Chooses one display binding without removing alternatives. GPUI returns effective
/// bindings in registration order; the last one wins, on every platform.
pub fn shortcut_for_action(action: &dyn Action, window: &Window, cx: &App) -> Option<String> {
    let bindings = bindings_for_action(action, window, cx);
    preferred_binding(&bindings, ShortcutPlatform::current()).map(format_binding)
}

/// Defaults register both a `cmd-…` and a Ctrl variant on every platform, so prefer
/// the native one (Cmd on macOS, no Win/Super elsewhere); among equals the last wins.
/// Other modifiers stay eligible as a fallback: custom Ctrl on macOS and Win/Super work.
fn preferred_binding(bindings: &[KeyBinding], platform: ShortcutPlatform) -> Option<&KeyBinding> {
    let is_native = |binding: &&KeyBinding| {
        binding.keystrokes().first().is_some_and(|stroke| {
            let uses_platform_key = stroke.as_keystroke().modifiers.platform;
            uses_platform_key == (platform == ShortcutPlatform::Mac)
        })
    };
    bindings
        .iter()
        .rev()
        .find(is_native)
        .or_else(|| bindings.last())
}

pub fn action_tooltip(
    label: &str,
    action: Option<&dyn Action>,
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    let text = match action.and_then(|action| shortcut_for_action(action, window, cx)) {
        Some(shortcut) => format!("{label} ({shortcut})"),
        None => label.to_owned(),
    };
    Tooltip::new(text).build(window, cx)
}

/// Resolve an explicitly named application action without reversing crate dependencies.
/// Missing/unregistered actions keep their text-only tooltip.
pub fn named_action_tooltip(
    label: &str,
    action_name: &str,
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    let action = cx.build_action(action_name, None).ok();
    action_tooltip(label, action.as_deref(), window, cx)
}

type ShortcutApplicability = Box<dyn Fn(&Window, &App) -> bool>;

/// A reusable button's shortcut and its live operation/target eligibility check.
/// The check runs when the tooltip opens, not when the button was last rendered.
pub struct ActionShortcut {
    action: Box<dyn Action>,
    is_applicable: ShortcutApplicability,
}

impl ActionShortcut {
    pub fn new(
        action: impl Action,
        is_applicable: impl Fn(&Window, &App) -> bool + 'static,
    ) -> Self {
        Self {
            action: Box::new(action),
            is_applicable: Box::new(is_applicable),
        }
    }

    pub fn tooltip(&self, label: &str, window: &mut Window, cx: &mut App) -> AnyView {
        let action = (self.is_applicable)(window, cx).then_some(self.action.as_ref());
        action_tooltip(label, action, window, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        Context, FocusHandle, InteractiveElement, IntoElement, ParentElement, Render,
        TestAppContext, div,
    };
    gpui::actions!(shortcut_tests, [SampleAction, OtherAction]);

    #[derive(Clone, Debug, PartialEq, gpui::Action)]
    #[action(namespace = shortcut_tests, no_json)]
    struct ParameterizedAction {
        target: usize,
    }

    struct TestView {
        focus: FocusHandle,
        other_focus: FocusHandle,
    }
    impl Render for TestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .key_context("Workspace")
                .child(
                    div()
                        .key_context("TerminalPane")
                        .track_focus(&self.focus)
                        .child(gpui::deferred(div())),
                )
                .child(div().key_context("Sidebar").track_focus(&self.other_focus))
        }
    }

    #[gpui::test]
    fn resolves_live_nested_contexts_remapping_removal_and_shadowing(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| {
            let focus = cx.focus_handle();
            focus.focus(window, cx);
            TestView {
                focus,
                other_focus: cx.focus_handle(),
            }
        });
        cx.update(|window, cx| {
            cx.bind_keys([
                KeyBinding::new("ctrl-d", SampleAction, Some("Workspace > TerminalPane")),
                KeyBinding::new("ctrl-p", SampleAction, Some("Sidebar")),
            ]);
            assert_eq!(
                shortcut_for_action(&SampleAction, window, cx),
                Some(format_keystroke("ctrl-d"))
            );
            cx.bind_keys([KeyBinding::new("ctrl-d", OtherAction, Some("TerminalPane"))]);
            assert_eq!(shortcut_for_action(&SampleAction, window, cx), None);
            cx.bind_keys([KeyBinding::new(
                "ctrl-k ctrl--",
                SampleAction,
                Some("Workspace > TerminalPane"),
            )]);
            assert_eq!(
                shortcut_for_action(&SampleAction, window, cx),
                Some(format_keystroke("ctrl-k ctrl--"))
            );
            cx.bind_keys([KeyBinding::new(
                "ctrl-k ctrl--",
                gpui::NoAction,
                Some("TerminalPane"),
            )]);
            assert_eq!(shortcut_for_action(&SampleAction, window, cx), None);
            // Simulate a configuration reload, including removal of old assignments.
            cx.clear_key_bindings();
            assert_eq!(shortcut_for_action(&SampleAction, window, cx), None);
            cx.bind_keys([KeyBinding::new(
                "cmd-j",
                SampleAction,
                Some("Workspace > TerminalPane"),
            )]);
            assert_eq!(
                shortcut_for_action(&SampleAction, window, cx),
                Some(format_keystroke("cmd-j"))
            );
            let other_focus = view.read(cx).other_focus.clone();
            other_focus.focus(window, cx);
            assert_eq!(shortcut_for_action(&SampleAction, window, cx), None);
        });
    }

    #[gpui::test]
    fn preserves_alternatives_parameters_and_custom_platform_modifiers(cx: &mut TestAppContext) {
        let (_view, cx) = cx.add_window_view(|window, cx| {
            let focus = cx.focus_handle();
            focus.focus(window, cx);
            TestView {
                focus,
                other_focus: cx.focus_handle(),
            }
        });
        cx.update(|window, cx| {
            cx.bind_keys([
                KeyBinding::new("ctrl-d", SampleAction, Some("TerminalPane")),
                KeyBinding::new("cmd-d", SampleAction, Some("TerminalPane")),
                KeyBinding::new(
                    "ctrl-1",
                    ParameterizedAction { target: 1 },
                    Some("TerminalPane"),
                ),
            ]);
            assert_eq!(bindings_for_action(&SampleAction, window, cx).len(), 2);
            let native = if ShortcutPlatform::current() == ShortcutPlatform::Mac {
                "cmd-d"
            } else {
                "ctrl-d"
            };
            assert_eq!(
                shortcut_for_action(&SampleAction, window, cx),
                Some(format_keystroke(native))
            );
            assert_eq!(
                shortcut_for_action(&ParameterizedAction { target: 1 }, window, cx),
                Some(format_keystroke("ctrl-1"))
            );
            assert_eq!(
                shortcut_for_action(&ParameterizedAction { target: 2 }, window, cx),
                None
            );
            let remapped = if ShortcutPlatform::current() == ShortcutPlatform::Mac {
                "cmd-j"
            } else {
                "ctrl-j"
            };
            cx.bind_keys([KeyBinding::new(
                remapped,
                SampleAction,
                Some("TerminalPane"),
            )]);
            assert_eq!(
                shortcut_for_action(&SampleAction, window, cx),
                Some(format_keystroke(remapped))
            );
        });
    }

    #[test]
    fn prefers_the_native_default_and_falls_back_to_the_last_binding() {
        let defaults = [
            KeyBinding::new("cmd-d", SampleAction, None),
            KeyBinding::new("ctrl-shift-d", SampleAction, None),
        ];
        let chosen = |bindings: &[KeyBinding], platform| {
            preferred_binding(bindings, platform).map(format_binding)
        };
        assert_eq!(
            chosen(&defaults, ShortcutPlatform::Mac),
            Some(format_binding(&defaults[0]))
        );
        for platform in [ShortcutPlatform::Linux, ShortcutPlatform::Windows] {
            assert_eq!(
                chosen(&defaults, platform),
                Some(format_binding(&defaults[1]))
            );
        }
        let custom_only = [KeyBinding::new("ctrl-j", SampleAction, None)];
        assert_eq!(
            chosen(&custom_only, ShortcutPlatform::Mac),
            Some(format_binding(&custom_only[0]))
        );
        assert_eq!(chosen(&[], ShortcutPlatform::Linux), None);
    }

    #[gpui::test]
    fn resolves_root_bindings_after_deferred_draw_without_focus(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, cx| TestView {
            focus: cx.focus_handle(),
            other_focus: cx.focus_handle(),
        });
        cx.update(|window, cx| {
            window.blur();
            assert!(window.context_stack().is_empty());
            cx.bind_keys([
                KeyBinding::new("ctrl-g", SampleAction, None),
                KeyBinding::new("ctrl-d", SampleAction, Some("TerminalPane")),
                KeyBinding::new("ctrl-n", SampleAction, Some("!TerminalPane")),
                KeyBinding::new("ctrl-g", OtherAction, Some("!TerminalPane")),
            ]);
            assert_eq!(
                shortcut_for_action(&SampleAction, window, cx),
                Some(format_keystroke("ctrl-g"))
            );
            assert_eq!(bindings_for_action(&SampleAction, window, cx).len(), 1);
        });
    }

    #[test]
    fn formats_named_keys_all_modifiers_and_preserves_invalid_input() {
        assert_eq!(
            format_keystroke_for_platform(
                "ctrl-alt-shift-cmd-fn-delete",
                ShortcutPlatform::Windows
            ),
            "Ctrl+Alt+Shift+Win+Fn+Delete"
        );
        assert_eq!(
            format_keystroke_for_platform("ctrl-alt-shift-cmd-delete", ShortcutPlatform::Mac),
            "⌃⌥⇧⌘⌦"
        );
        assert_eq!(
            format_keystroke_for_platform(
                "escape pageup pagedown f12 tab space",
                ShortcutPlatform::Linux
            ),
            "Esc PgUp PgDn F12 Tab Space"
        );
        assert_eq!(
            format_keystroke_for_platform("ctrl-", ShortcutPlatform::Windows),
            "ctrl-"
        );
        assert_eq!(
            format_keystroke_for_platform("", ShortcutPlatform::Windows),
            ""
        );
    }

    #[test]
    fn formats_platform_modifiers_chords_and_punctuation() {
        assert_eq!(
            format_keystroke_for_platform("cmd-shift-p", ShortcutPlatform::Mac),
            "⇧⌘P"
        );
        assert_eq!(
            format_keystroke_for_platform("ctrl-shift-p", ShortcutPlatform::Windows),
            "Ctrl+Shift+P"
        );
        assert_eq!(
            format_keystroke_for_platform("cmd-p", ShortcutPlatform::Windows),
            "Win+P"
        );
        assert_eq!(
            format_keystroke_for_platform("cmd-p", ShortcutPlatform::Linux),
            "Super+P"
        );
        assert_eq!(
            format_keystroke_for_platform("ctrl-k ctrl--", ShortcutPlatform::Windows),
            "Ctrl+K Ctrl+-"
        );
        assert_eq!(
            format_keystroke_for_platform("cmd-k cmd-c", ShortcutPlatform::Mac),
            "⌘K ⌘C"
        );
    }
}
