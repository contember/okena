//! Terminal pane action handlers.
//!
//! Contains handlers for split, close, minimize, fullscreen,
//! copy, paste, and file drop.

use okena_core::api::ActionRequest;
use crate::workspace::state::SplitDirection;
use gpui::*;

use super::TerminalPane;

impl TerminalPane {
    pub(super) fn handle_split(&mut self, direction: SplitDirection, cx: &mut Context<Self>) {
        let action = ActionRequest::SplitTerminal {
            project_id: self.project_id.clone(),
            path: self.layout_path.clone(),
            direction,
        };
        if let Some(ref dispatcher) = self.action_dispatcher {
            dispatcher.dispatch(action, cx);
        }
    }

    pub(super) fn handle_add_tab(&mut self, cx: &mut Context<Self>) {
        if let Some(ref dispatcher) = self.action_dispatcher {
            dispatcher.add_tab(&self.project_id, &self.layout_path, false, cx);
        }
    }

    pub(super) fn handle_close(&mut self, cx: &mut Context<Self>) {
        if let Some(terminal_id) = self.terminal_id.clone() {
            let action = ActionRequest::CloseTerminal {
                project_id: self.project_id.clone(),
                terminal_id,
            };
            if let Some(ref dispatcher) = self.action_dispatcher {
                dispatcher.dispatch(action, cx);
            }
        }
    }

    pub(super) fn handle_minimize(&mut self, cx: &mut Context<Self>) {
        if let Some(ref terminal_id) = self.terminal_id {
            let action = ActionRequest::ToggleMinimized {
                project_id: self.project_id.clone(),
                terminal_id: terminal_id.clone(),
            };
            if let Some(ref dispatcher) = self.action_dispatcher {
                dispatcher.dispatch(action, cx);
            }
        }
    }

    pub(super) fn handle_fullscreen(&mut self, cx: &mut Context<Self>) {
        if let Some(ref id) = self.terminal_id {
            let action = ActionRequest::SetFullscreen {
                project_id: self.project_id.clone(),
                terminal_id: Some(id.clone()),
            };
            if let Some(ref dispatcher) = self.action_dispatcher {
                dispatcher.dispatch(action, cx);
            }
        }
    }

    pub(super) fn handle_copy(&mut self, cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal {
            if let Some(text) = terminal.get_selected_text() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        }
    }

    pub(super) fn handle_paste(&mut self, cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal {
            if let Some(clipboard_item) = cx.read_from_clipboard() {
                if let Some(text) = clipboard_item.text() {
                    terminal.send_input(&text);
                }
            }
        }
    }

    pub(super) fn handle_file_drop(&mut self, paths: &ExternalPaths, _cx: &mut Context<Self>) {
        let Some(ref terminal) = self.terminal else {
            return;
        };

        for path in paths.paths() {
            let escaped_path = Self::shell_escape_path(path);
            terminal.send_input(&format!("{} ", escaped_path));
        }
    }

    pub(super) fn shell_escape_path(path: &std::path::Path) -> String {
        let path_str = path.to_string_lossy();
        let mut escaped = String::with_capacity(path_str.len() * 2);

        for c in path_str.chars() {
            match c {
                ' ' | '(' | ')' | '[' | ']' | '{' | '}' | '\'' | '"' | '`' | '$' | '&' | '|'
                | ';' | '<' | '>' | '*' | '?' | '!' | '#' | '~' | '\\' => {
                    escaped.push('\\');
                    escaped.push(c);
                }
                _ => escaped.push(c),
            }
        }

        escaped
    }
}
