//! Terminal pane navigation, search, and key handling.

use crate::ActionDispatch;
use okena_terminal::input::{KeyEvent, KeyModifiers, key_to_bytes};
use crate::layout::navigation::{get_pane_map, PaneBounds, NavigationDirection};
use okena_workspace::state::LayoutNode;
use gpui::*;

use super::TerminalPane;

impl<D: ActionDispatch + Send + Sync> TerminalPane<D> {
    /// Try to switch to an adjacent tab within a Tabs node.
    /// Returns true if a tab switch happened, false if at edge or not in a tab group.
    fn try_switch_tab(&mut self, next: bool, cx: &mut Context<Self>) -> bool {
        if self.layout_path.is_empty() {
            return false;
        }

        let parent_path = &self.layout_path[..self.layout_path.len() - 1];
        let current_tab_index = self.layout_path[self.layout_path.len() - 1];

        let tab_count = {
            let ws = self.workspace.read(cx);
            ws.project(&self.project_id).and_then(|p| {
                p.layout.as_ref().and_then(|layout| {
                    layout.get_at_path(parent_path).and_then(|node| match node {
                        LayoutNode::Tabs { children, .. } => Some(children.len()),
                        _ => None,
                    })
                })
            })
        };

        let num_tabs = match tab_count.filter(|&n| n > 1) {
            Some(n) => n,
            None => return false,
        };

        let at_edge = if next {
            current_tab_index == num_tabs - 1
        } else {
            current_tab_index == 0
        };

        if at_edge {
            return false;
        }

        let new_tab = if next { current_tab_index + 1 } else { current_tab_index - 1 };
        let project_id = self.project_id.clone();
        let mut new_layout_path = parent_path.to_vec();
        new_layout_path.push(new_tab);

        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| {
                ws.set_active_tab(&project_id, &new_layout_path[..new_layout_path.len() - 1], new_tab, cx);
                ws.set_focused_terminal(fm, project_id, new_layout_path, cx);
            });
            cx.notify();
        });
        true
    }

    pub(super) fn handle_navigation(
        &mut self,
        direction: NavigationDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Left/Right: try switching tabs first, fall through to spatial nav at edges
        if matches!(direction, NavigationDirection::Left | NavigationDirection::Right) {
            let next = matches!(direction, NavigationDirection::Right);
            if self.try_switch_tab(next, cx) {
                return;
            }
        }

        let pane_map = get_pane_map(self.window_id);

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
        if self.try_switch_tab(next, cx) {
            return;
        }

        let pane_map = get_pane_map(self.window_id);

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
        let target_project = target.project_id.clone();
        let target_path = target.layout_path.clone();
        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| {
                ws.set_focused_terminal(fm, target_project, target_path, cx);
            });
            cx.notify();
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
                && !terminal.has_running_child()
                && terminal.delete_selection() {
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
                    && !text.is_empty() {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        terminal.clear_selection();
                        cx.notify();
                        return;
                    }

            let app_cursor_mode = terminal.is_app_cursor_mode();
            let kitty = terminal.kitty_keyboard_flags();
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
            if let Some(input) = key_to_bytes(&key_event, app_cursor_mode, kitty) {
                terminal.send_bytes(&input);
            }
        }
    }
}
