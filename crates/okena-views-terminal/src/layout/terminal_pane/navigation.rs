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

        self.workspace.update(cx, |ws, cx| {
            ws.set_active_tab(&project_id, &new_layout_path[..new_layout_path.len() - 1], new_tab, cx);
            ws.set_focused_terminal(project_id, new_layout_path, cx);
        });
        true
    }

    fn current_pane(&self) -> Option<PaneBounds> {
        get_pane_map().find_pane(&self.project_id, &self.layout_path).cloned()
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

        let source = match self.current_pane() {
            Some(pane) => pane,
            None => return,
        };

        if let Some(target) = get_pane_map().find_nearest_in_direction(&source, direction) {
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

        let source = match self.current_pane() {
            Some(pane) => pane,
            None => return,
        };

        let pane_map = get_pane_map();
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

    pub(super) fn handle_key(&mut self, event: &KeyDownEvent, _cx: &mut Context<Self>) {
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
