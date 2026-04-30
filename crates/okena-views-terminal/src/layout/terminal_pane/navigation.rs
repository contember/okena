//! Terminal pane navigation, search, and key handling.

use crate::ActionDispatch;
use okena_terminal::input::{KeyEvent, KeyModifiers, key_to_bytes};
use crate::layout::navigation::{get_pane_map, PaneBounds, NavigationDirection};
use gpui::*;

use super::TerminalPane;

impl<D: ActionDispatch + Send + Sync> TerminalPane<D> {
    pub(super) fn handle_navigation(
        &mut self,
        direction: NavigationDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane_map = get_pane_map();

        let source = match pane_map.find_pane(&self.project_id, &self.layout_path) {
            Some(pane) => pane.clone(),
            None => return,
        };

        if let Some(target) = pane_map.find_nearest_in_direction(&source, direction) {
            self.focus_target(target, window, cx);
        }
    }

    pub(super) fn handle_sequential_navigation(
        &mut self,
        next: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane_map = get_pane_map();

        let source = match pane_map.find_pane(&self.project_id, &self.layout_path) {
            Some(pane) => pane.clone(),
            None => return,
        };

        let target = if next {
            pane_map.find_next_pane(&source)
        } else {
            pane_map.find_prev_pane(&source)
        };

        if let Some(ref target) = target {
            self.focus_target(target, window, cx);
        }
    }

    fn focus_target(&self, target: &PaneBounds, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ref fh) = target.focus_handle {
            window.focus(fh, cx);
        }
        self.workspace.update(cx, |ws, cx| {
            ws.set_focused_terminal(target.project_id.clone(), target.layout_path.clone(), cx);
        });
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
                && !terminal.has_running_child()
            {
                if terminal.delete_selection() {
                    return;
                }
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
            {
                if let Some(text) = terminal.get_selected_text() {
                    if !text.is_empty() {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        terminal.clear_selection();
                        cx.notify();
                        return;
                    }
                }
            }

            let app_cursor_mode = terminal.is_app_cursor_mode();
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
            if let Some(input) = key_to_bytes(&key_event, app_cursor_mode) {
                terminal.send_bytes(&input);
            }
        }
    }
}
