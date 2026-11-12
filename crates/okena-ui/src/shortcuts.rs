//! Shared shortcut labels for tooltips, keybinding editors and command lists.
use gpui::{Action, AnyView, App, AsKeystroke, KeyBinding, KeyContext, Keystroke, Window};
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
                .map(|key| format_stroke(&key, platform))
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

fn preferred_binding(bindings: &[KeyBinding], platform: ShortcutPlatform) -> Option<&KeyBinding> {
    // Defaults contain both cmd and ctrl variants. Choose a usable native variant
    // among effective bindings, keeping registration precedence within that set.
    bindings
        .iter()
        .rev()
        .find(|binding| {
            binding.keystrokes().iter().all(|key| {
                let modifiers = key.as_keystroke().modifiers;
                platform == ShortcutPlatform::Mac || !modifiers.platform
            }) && (platform != ShortcutPlatform::Mac
                || binding
                    .keystrokes()
                    .iter()
                    .any(|key| key.as_keystroke().modifiers.platform))
        })
        .or_else(|| bindings.last())
}

/// Resolves the live keymap, including disabled/conflicting bindings, for a UI
/// action's context. No settings cache or hard-coded default shortcuts are used.
pub fn shortcut_for_action(
    action: &dyn Action,
    context: Option<&str>,
    window: &Window,
) -> Option<String> {
    let bindings = match context {
        Some(context) => {
            window.bindings_for_action_in_context(action, KeyContext::parse(context).ok()?)
        }
        None => window.bindings_for_action(action),
    };
    let platform = ShortcutPlatform::current();
    let binding = preferred_binding(&bindings, platform)?;
    Some(
        binding
            .keystrokes()
            .iter()
            .map(|key| format_stroke(key.as_keystroke(), platform))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

pub fn action_tooltip(
    label: &str,
    action: Option<&dyn Action>,
    context: Option<&str>,
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    let text = match action.and_then(|action| shortcut_for_action(action, context, window)) {
        Some(shortcut) => format!("{label} ({shortcut})"),
        None => label.to_owned(),
    };
    Tooltip::new(text).build(window, cx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, IntoElement, Render, TestAppContext, div, px, size};
    gpui::actions!(shortcut_tests, [SampleAction, OtherAction]);

    struct TestView;
    impl Render for TestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[gpui::test]
    fn resolves_live_remapped_contextual_and_shadowed_bindings(cx: &mut TestAppContext) {
        let window = cx.open_window(size(px(300.), px(200.)), |_, _| TestView);
        cx.update(|cx| {
            cx.bind_keys([
                KeyBinding::new("ctrl-d", SampleAction, Some("TerminalPane")),
                KeyBinding::new("ctrl-p", SampleAction, Some("Sidebar")),
            ])
        });
        window
            .update(cx, |_, window, _| {
                assert_eq!(
                    shortcut_for_action(&SampleAction, Some("TerminalPane"), window),
                    Some(format_keystroke("ctrl-d"))
                );
                assert_eq!(
                    shortcut_for_action(&SampleAction, Some("Sidebar"), window),
                    Some(format_keystroke("ctrl-p"))
                );
                assert_eq!(
                    shortcut_for_action(&SampleAction, Some("SearchBar"), window),
                    None
                );
            })
            .unwrap();
        cx.update(|cx| {
            cx.bind_keys([
                KeyBinding::new("ctrl-d", OtherAction, Some("TerminalPane")),
                KeyBinding::new("ctrl-k ctrl-c", SampleAction, Some("TerminalPane")),
            ])
        });
        window
            .update(cx, |_, window, _| {
                assert_eq!(
                    shortcut_for_action(&SampleAction, Some("TerminalPane"), window),
                    Some(format_keystroke("ctrl-k ctrl-c"))
                );
            })
            .unwrap();
        cx.update(|cx| cx.clear_key_bindings());
        window
            .update(cx, |_, window, _| {
                assert_eq!(
                    shortcut_for_action(&SampleAction, Some("TerminalPane"), window),
                    None
                );
            })
            .unwrap();
    }

    #[test]
    fn prefers_native_effective_binding_and_preserves_custom_fallback() {
        let bindings = [
            KeyBinding::new("cmd-d", SampleAction, None),
            KeyBinding::new("ctrl-d", SampleAction, None),
        ];
        assert_eq!(
            preferred_binding(&bindings, ShortcutPlatform::Mac)
                .unwrap()
                .keystrokes(),
            bindings[0].keystrokes()
        );
        assert_eq!(
            preferred_binding(&bindings, ShortcutPlatform::Windows)
                .unwrap()
                .keystrokes(),
            bindings[1].keystrokes()
        );
        assert_eq!(
            preferred_binding(&bindings[1..], ShortcutPlatform::Mac)
                .unwrap()
                .keystrokes(),
            bindings[1].keystrokes()
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
