//! Curated discoverability tips shown on the empty (no terminal attached)
//! project screen.
//!
//! Each keyboard tip carries a typed action. Its shortcut is resolved from the
//! live keymap and the context described by the tip, using the same resolver as action tooltips.
//! Tips without a keybinding carry a short
//! `hint` describing how to trigger them instead (e.g. "right-click", "CLI").
//!
//! The tip set was compiled by auditing the codebase; every entry was
//! verified against the actual implementation. Desktop only.

use std::sync::atomic::{AtomicUsize, Ordering};

use gpui::{Action, App, KeyContext};

type TipAction = fn() -> Box<dyn Action>;

/// A single discoverability tip.
pub struct Tip {
    /// The tip copy shown to the user.
    pub text: &'static str,
    /// Typed action whose effective shortcut is shown as a chip.
    /// `None` when the tip is not keyboard-triggered.
    action: Option<TipAction>,
    /// This tip describes a command used with a focused terminal.
    terminal_context: bool,
    /// Static trigger hint shown as a chip when there is no `action`
    /// (e.g. "right-click", "CLI", "settings").
    pub hint: Option<&'static str>,
}

impl Tip {
    /// Resolve in the documented context, independent of another pane's focus.
    pub fn shortcut(&self, cx: &App) -> Option<String> {
        static TERMINAL_CONTEXT: std::sync::OnceLock<KeyContext> = std::sync::OnceLock::new();
        let action = (self.action?)();
        let contexts =
            if self.terminal_context {
                std::slice::from_ref(TERMINAL_CONTEXT.get_or_init(|| {
                    KeyContext::parse("TerminalPane").expect("valid terminal context")
                }))
            } else {
                &[]
            };
        okena_ui::shortcuts::shortcut_for_action_in(action.as_ref(), contexts, cx)
    }
}

/// Tip triggered by a keybinding — its chip shows the live shortcut for `action`.
const fn key<A: Action + Default>(text: &'static str) -> Tip {
    Tip {
        text,
        action: Some(|| Box::new(A::default())),
        terminal_context: false,
        hint: None,
    }
}

/// A terminal command remains discoverable even while no terminal has focus.
const fn terminal_key<A: Action + Default>(text: &'static str) -> Tip {
    Tip {
        terminal_context: true,
        ..key::<A>(text)
    }
}

/// Tip triggered some other way — its chip shows the static `hint`.
const fn hint(text: &'static str, hint: &'static str) -> Tip {
    Tip {
        text,
        action: None,
        terminal_context: false,
        hint: Some(hint),
    }
}

/// Tip with no chip (an ambient behavior, or keys are already in the text).
const fn plain(text: &'static str) -> Tip {
    Tip {
        text,
        action: None,
        terminal_context: false,
        hint: None,
    }
}

/// The curated tip pool.
pub static TIPS: &[Tip] = &[
    // Windows, projects & layout
    key::<crate::keybindings::NewWindow>(
        "Open a second window onto the same workspace — each window keeps its own set of visible projects.",
    ),
    key::<crate::keybindings::TogglePaneSwitcher>(
        "Overlay numbers on every pane, then press the digit to jump straight to it.",
    ),
    terminal_key::<crate::keybindings::ToggleFullscreen>(
        "Zoom the focused terminal full-screen, then cycle between terminals with the previous/next controls.",
    ),
    key::<crate::keybindings::EqualizeLayout>(
        "Resize every column and pane to match in a single keystroke.",
    ),
    key::<crate::keybindings::ToggleProjectLayout>(
        "Cycle your projects between columns, rows and a grid of tiles — handy on a portrait or vertical monitor.",
    ),
    terminal_key::<crate::keybindings::MinimizeTerminal>(
        "Minimize a busy terminal instead of closing it — it keeps running and comes back later.",
    ),
    key::<crate::keybindings::FocusActiveProject>(
        "Zoom to just the project holding the active terminal; use Clear Focus to show them all again.",
    ),
    hint(
        "Give each project one of 12 colors so its column is easy to spot.",
        "right-click a project",
    ),
    hint(
        "Group projects into folders, drag to reorder, and \"Show Only This Folder\" to focus one group.",
        "drag / right-click",
    ),
    hint(
        "Hide a project from a window without deleting it — it stays in your other windows.",
        "right-click",
    ),
    // Splits, tabs & navigation
    terminal_key::<crate::keybindings::SplitVertical>(
        "Split a terminal into side-by-side or stacked panes.",
    ),
    key::<crate::keybindings::AddTab>(
        "Add tabs inside a pane — double-click a tab to rename, middle-click to close.",
    ),
    key::<crate::keybindings::FocusLeft>(
        "Move focus to the pane on the left; directional focus commands also move right, up and down.",
    ),
    // Find anything
    key::<crate::keybindings::ShowCommandPalette>(
        "Open the command palette to find any action — and see its shortcut.",
    ),
    key::<crate::keybindings::ShowProjectSwitcher>(
        "Jump between projects fast — Enter to focus, Space to show/hide in this window.",
    ),
    key::<crate::keybindings::ShowFileSearch>("Fuzzy-find any file in the project."),
    key::<crate::keybindings::ShowContentSearch>(
        "Search file contents across the project with literal, regex, or fuzzy matching.",
    ),
    // Terminal
    terminal_key::<crate::keybindings::JumpToPreviousPrompt>(
        "Jump to the previous or next shell prompt instantly (needs OSC 133 shell integration).",
    ),
    hint(
        "⌘/Ctrl-click a file path (file:line:col) or a URL in the terminal to open it in your editor or browser.",
        "⌘/Ctrl + click",
    ),
    terminal_key::<crate::keybindings::Search>(
        "Search the terminal scrollback with regex and a live match count.",
    ),
    hint(
        "Press Shift+Enter to insert a newline for multi-line input without submitting.",
        "⇧Enter",
    ),
    terminal_key::<crate::keybindings::ZoomIn>(
        "Zoom the font size of just one terminal; use Reset Zoom to restore the default size.",
    ),
    // Git & worktrees
    key::<crate::keybindings::ShowBranchSwitcher>(
        "Filter branches or create a new one from HEAD, right inside the branch switcher.",
    ),
    hint(
        "Spin up a git worktree as its own column — even straight from a GitHub PR (needs gh).",
        "right-click a repo",
    ),
    plain("New worktrees appear automatically and stale ones are cleaned up in the background."),
    plain(
        "In the diff viewer: S toggles side-by-side, W ignores whitespace, Tab switches staged/unstaged.",
    ),
    // Services & ports
    hint(
        "Running services show port badges — click one to open it in your browser.",
        "click a port",
    ),
    hint(
        "Define project services in okena.yaml; Docker Compose is auto-detected too.",
        "okena.yaml",
    ),
    hint(
        "Services can auto-start on project open and auto-restart on crash, with automatic port detection.",
        "okena.yaml",
    ),
    // Sessions & safety
    plain(
        "With dtach, tmux, or screen installed, your terminals keep running across app restarts.",
    ),
    plain(
        "Closed a busy terminal by accident? You get a few seconds to undo before the process is killed.",
    ),
    key::<crate::keybindings::ShowSessionManager>(
        "Save, load, and export entire workspace layouts from the session manager.",
    ),
    // Remote, CLI & web
    hint(
        "Drive any terminal from the shell or an agent: okena run, okena key, okena read, okena ls.",
        "CLI",
    ),
    hint(
        "Target a specific window from the CLI with --window <id> (or --window main).",
        "CLI",
    ),
    hint(
        "Enable the remote server and open your terminal in a browser from any device via a pairing code.",
        "remote",
    ),
    hint(
        "Run okena headless on a server: okena --headless --listen <addr> serves the API with no GUI.",
        "CLI",
    ),
    // Customize
    key::<crate::keybindings::ShowThemeSelector>(
        "Switch themes live — Auto, Dark, Light, Pastel Dark, High Contrast, or your own JSON.",
    ),
    hint(
        "Run commands automatically on project, terminal, and worktree events; prefix with \"terminal:\" to open them in a pane.",
        "settings",
    ),
    hint(
        "Set a {shell} wrapper to auto-enter a devcontainer or nix shell for every new terminal.",
        "settings",
    ),
    hint(
        "Keep separate settings, keybindings, themes, and sessions per profile (work vs. personal).",
        "command palette",
    ),
    key::<crate::keybindings::ShowKeybindings>(
        "Every shortcut is editable — open the keybindings editor to record your own.",
    ),
    hint(
        "Get notified on bell or OSC alerts from background terminals.",
        "settings",
    ),
];

/// Returns the tip at `index`, wrapping around the pool.
pub fn tip_at(index: usize) -> &'static Tip {
    &TIPS[index % TIPS.len()]
}

/// Picks a starting index for an empty-state column. The base is randomized
/// once per app launch (seeded from the clock) so the first tip differs each
/// run, then advances sequentially so columns shown together don't repeat.
pub fn next_start_index() -> usize {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    static BASE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let base = *BASE.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as usize)
            .unwrap_or(0)
    });
    base.wrapping_add(COUNTER.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn tip_shortcuts_follow_live_remapping_shadowing_and_removal(cx: &mut gpui::TestAppContext) {
        use gpui::{Context, IntoElement, KeyBinding, Render, Window, div};
        struct Empty;
        impl Render for Empty {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
            }
        }
        let (_, cx) = cx.add_window_view(|_, _| Empty);
        cx.update(|_window, cx| {
            let tip = key::<crate::keybindings::ShowSettings>("Settings");
            assert_eq!(tip.shortcut(cx), None);
            cx.bind_keys([KeyBinding::new(
                "ctrl-k ctrl-s",
                crate::keybindings::ShowSettings,
                None,
            )]);
            assert_eq!(
                tip.shortcut(cx),
                Some(okena_ui::shortcuts::format_keystroke("ctrl-k ctrl-s"))
            );
            cx.bind_keys([KeyBinding::new("ctrl-k ctrl-s", gpui::NoAction, None)]);
            assert_eq!(tip.shortcut(cx), None);
            cx.clear_key_bindings();
            cx.bind_keys([KeyBinding::new(
                "ctrl-j",
                crate::keybindings::ShowSettings,
                None,
            )]);
            assert_eq!(
                tip.shortcut(cx),
                Some(okena_ui::shortcuts::format_keystroke("ctrl-j"))
            );
            cx.clear_key_bindings();
            assert_eq!(tip.shortcut(cx), None);
            assert_eq!(hint("Context menu", "right-click").shortcut(cx), None);
        });
    }

    #[gpui::test]
    fn terminal_tips_do_not_depend_on_current_focus_and_reload_cached_views(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::{
            AppContext as _, Context, IntoElement, KeyBinding, ParentElement, Render, Window, div,
        };
        use std::{cell::RefCell, rc::Rc};
        let displayed = Rc::new(RefCell::new(None));
        struct TipView(Rc<RefCell<Option<String>>>);
        impl Render for TipView {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                *self.0.borrow_mut() =
                    terminal_key::<crate::keybindings::SplitVertical>("Split").shortcut(cx);
                div()
            }
        }
        struct Root(gpui::Entity<TipView>);
        impl Render for Root {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().child(
                    gpui::AnyView::from(self.0.clone()).cached(gpui::StyleRefinement::default()),
                )
            }
        }
        let output = displayed.clone();
        let (_, cx) = cx.add_window_view(move |_, cx| Root(cx.new(|_| TipView(output))));
        cx.update(|window, cx| {
            assert!(window.context_stack().is_empty());
            cx.bind_keys([KeyBinding::new(
                "ctrl-d",
                crate::keybindings::SplitVertical,
                Some("TerminalPane"),
            )]);
        });
        cx.run_until_parked();
        cx.update(|window, cx| _ = window.draw(cx));
        assert_eq!(
            *displayed.borrow(),
            Some(okena_ui::shortcuts::format_keystroke("ctrl-d"))
        );
        cx.update(|_, cx| {
            cx.clear_key_bindings();
            cx.bind_keys([KeyBinding::new(
                "ctrl-j",
                crate::keybindings::SplitVertical,
                Some("TerminalPane"),
            )]);
        });
        cx.run_until_parked();
        cx.update(|window, cx| _ = window.draw(cx));
        assert_eq!(
            *displayed.borrow(),
            Some(okena_ui::shortcuts::format_keystroke("ctrl-j"))
        );
        cx.update(|_, cx| cx.clear_key_bindings());
        cx.run_until_parked();
        cx.update(|window, cx| _ = window.draw(cx));
        assert_eq!(*displayed.borrow(), None);
    }

    #[gpui::test]
    fn every_keyboard_tip_has_a_binding_in_its_documented_default_context(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::{DummyKeyboardMapper, KeyBinding, KeyBindingContextPredicate};
        let defaults = crate::keybindings::KeybindingConfig::defaults();
        cx.update(|cx| {
            for tip in TIPS {
                let Some(build_action) = tip.action else {
                    continue;
                };
                let action = build_action();
                let name = action.name().rsplit("::").next().unwrap();
                let entries = defaults
                    .bindings
                    .get(name)
                    .expect("tip action has defaults");
                cx.clear_key_bindings();
                for entry in entries.iter().filter(|entry| entry.enabled) {
                    let context = entry.context.as_deref().map(|context| {
                        std::rc::Rc::new(KeyBindingContextPredicate::parse(context).unwrap())
                    });
                    cx.bind_keys([KeyBinding::load(
                        &entry.keystroke,
                        action.boxed_clone(),
                        context,
                        false,
                        None,
                        &DummyKeyboardMapper,
                    )
                    .unwrap()]);
                }
                assert!(
                    tip.shortcut(cx).is_some(),
                    "tip context must match defaults: {name}"
                );
            }
        });
    }

    #[test]
    fn tip_at_wraps_around() {
        let len = TIPS.len();
        assert!(len > 0);
        // Same tip is returned for index and index + len.
        assert!(std::ptr::eq(tip_at(0), tip_at(len)));
        assert!(std::ptr::eq(tip_at(3), tip_at(len + 3)));
    }

    #[test]
    fn every_tip_has_exactly_one_chip_source_or_none() {
        // A tip never carries both an action and a hint (the chip would be
        // ambiguous); it has at most one chip source.
        for tip in TIPS {
            assert!(
                !(tip.action.is_some() && tip.hint.is_some()),
                "tip has both action and hint: {}",
                tip.text
            );
        }
    }

    #[test]
    fn next_start_index_advances() {
        let a = next_start_index();
        let b = next_start_index();
        assert_ne!(a, b);
    }
}
