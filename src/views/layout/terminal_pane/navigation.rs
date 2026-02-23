//! Terminal pane navigation, search, and key handling.
//!
//! Contains directional navigation between panes, sequential navigation,
//! search open/close/next/prev, and keyboard input handling.

use crate::terminal::input::key_to_bytes;
use crate::views::layout::navigation::{get_pane_map, NavigationDirection};
use gpui::*;

use super::TerminalPane;

impl TerminalPane {
    // === Navigation ===

    pub(super) fn handle_navigation(
        &mut self,
        direction: NavigationDirection,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane_map = get_pane_map();

        let source = match pane_map.find_pane(&self.project_id, &self.layout_path) {
            Some(pane) => pane.clone(),
            None => return,
        };

        if let Some(target) = pane_map.find_nearest_in_direction(&source, direction) {
            self.workspace.update(cx, |ws, cx| {
                ws.set_focused_terminal(target.project_id.clone(), target.layout_path.clone(), cx);
            });
        }
    }

    pub(super) fn handle_sequential_navigation(
        &mut self,
        next: bool,
        _window: &mut Window,
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

        if let Some(target) = target {
            self.workspace.update(cx, |ws, cx| {
                ws.set_focused_terminal(target.project_id.clone(), target.layout_path.clone(), cx);
            });
        }
    }

    // === Search ===

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

    // === Key handling ===

    pub(super) fn handle_key(&mut self, event: &KeyDownEvent, _cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal {
            // Local keyboard input reclaims resize authority from remote clients
            terminal.claim_resize_local();
            let app_cursor_mode = terminal.is_app_cursor_mode();
            if let Some(input) = key_to_bytes(event, app_cursor_mode) {
                // Predict printable ASCII chars for remote terminals
                if terminal.is_remote() && input.len() == 1 {
                    let byte = input[0];
                    if byte >= 0x20 && byte < 0x7f {
                        terminal.predict_char(byte as char);
                    }
                }
                terminal.send_bytes(&input);
            }
        }
    }
}
