//! Terminal pane navigation, search, and key handling.

use crate::ActionDispatch;
use crate::layout::navigation::{NavigationDirection, PaneNavigator};
use gpui::*;
use okena_terminal::input::{
    KeyEncodeOptions, KeyEvent, KeyModifiers, consumes_composed_text, key_to_bytes,
};

use super::TerminalPane;

impl<D: ActionDispatch + Send + Sync> TerminalPane<D> {
    fn pane_navigator(&self) -> PaneNavigator<'_> {
        PaneNavigator {
            workspace: &self.workspace,
            focus_manager: &self.focus_manager,
            window_id: self.window_id,
            project_id: &self.project_id,
            layout_path: &self.layout_path,
        }
    }

    pub(super) fn handle_navigation(
        &mut self,
        direction: NavigationDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_navigator().in_direction(direction, window, cx);
    }

    pub(super) fn handle_sequential_navigation(
        &mut self,
        next: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_navigator().sequential(next, window, cx);
    }

    pub(super) fn start_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_bar.update(cx, |search_bar, cx| {
            search_bar.open(window, cx);
        });
        cx.notify();
    }

    pub(super) fn close_search(&mut self, cx: &mut Context<Self>) {
        self.search_bar.update(cx, |search_bar, cx| {
            search_bar.close(cx);
        });
        cx.notify();
    }

    pub(super) fn next_match(&mut self, cx: &mut Context<Self>) {
        self.search_bar.update(cx, |search_bar, cx| {
            search_bar.next_match(cx);
        });
    }

    pub(super) fn prev_match(&mut self, cx: &mut Context<Self>) {
        self.search_bar.update(cx, |search_bar, cx| {
            search_bar.prev_match(cx);
        });
    }

    pub(super) fn handle_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        // Windows: intercept Ctrl+V and route through the clipboard-aware
        // Paste handler. On macOS / Linux the running TUI (Claude Code, etc.)
        // can read the OS clipboard itself via pbpaste / xclip / wl-paste, so
        // we let the raw keystroke pass through. On Windows neither path
        // works: PowerShell ignores `\x16`, and Claude inside WSL has no
        // Linux clipboard, so it shows "No image found in clipboard."
        // Ctrl+Shift+V still goes through the regular Paste action binding.
        #[cfg(target_os = "windows")]
        if event.keystroke.key == "v"
            && event.keystroke.modifiers.control
            && !event.keystroke.modifiers.shift
            && !event.keystroke.modifiers.alt
            && !event.keystroke.modifiers.platform
        {
            self.handle_paste(cx);
            return;
        }

        if let Some(ref terminal) = self.terminal {
            terminal.claim_resize_local();

            // Backspace with selection: delete selected text (only in plain shell)
            if event.keystroke.key == "backspace"
                && !event.keystroke.modifiers.control
                && !event.keystroke.modifiers.alt
                && !event.keystroke.modifiers.platform
                && terminal.has_selection()
                && !terminal.is_mouse_mode()
                && !terminal.is_alt_screen()
                && terminal.can_rewrite_shell_input()
                && terminal.delete_selection()
            {
                return;
            }

            // Opt-in: Ctrl+C copies selection (and clears it) instead of sending SIGINT.
            // Without a (non-empty) selection, falls through to the normal Ctrl+C → SIGINT path.
            // Ctrl+Shift+C is handled by the Copy action and never reaches here.
            if event.keystroke.key == "c"
                && event.keystroke.modifiers.control
                && !event.keystroke.modifiers.shift
                && !event.keystroke.modifiers.alt
                && !event.keystroke.modifiers.platform
                && crate::terminal_view_settings(cx).ctrl_c_copies_selection
                && let Some(text) = terminal.get_selected_text()
                && !text.is_empty()
            {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                terminal.clear_selection();
                cx.notify();
                return;
            }

            let options = KeyEncodeOptions {
                app_cursor_mode: terminal.is_app_cursor_mode(),
                kitty: terminal.kitty_keyboard_flags(),
                option_as_meta: crate::terminal_view_settings(cx).option_as_meta,
            };
            let key_event = KeyEvent {
                key: event.keystroke.key.clone(),
                key_char: event.keystroke.key_char.clone(),
                modifiers: KeyModifiers {
                    control: event.keystroke.modifiers.control,
                    shift: event.keystroke.modifiers.shift,
                    alt: event.keystroke.modifiers.alt,
                    platform: event.keystroke.modifiers.platform,
                },
            };
            if let Some(input) = key_to_bytes(&key_event, options) {
                terminal.send_bytes(&input);
                // GPUI keeps forwarding to the input context, which would commit
                // the character macOS composed on top of the meta sequence.
                if consumes_composed_text(&key_event, options) {
                    cx.stop_propagation();
                }
            }
        }
    }
}
