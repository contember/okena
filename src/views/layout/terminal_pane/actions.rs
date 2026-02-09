//! Terminal pane action handlers.
//!
//! Contains handlers for split, close, minimize, fullscreen, detach,
//! copy, paste, clear, select all, rename, export buffer, and file drop.

use crate::remote::types::ActionRequest;
use crate::workspace::actions::execute::execute_action;
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
        let backend = self.backend.clone();
        let terminals = self.terminals.clone();
        self.workspace.update(cx, |ws, cx| {
            execute_action(action, ws, &*backend, &terminals, cx);
        });
    }

    pub(super) fn handle_add_tab(&mut self, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| {
            ws.add_tab(&self.project_id, &self.layout_path, cx);
        });
    }

    pub(super) fn handle_close(&mut self, cx: &mut Context<Self>) {
        if let Some(terminal_id) = self.terminal_id.clone() {
            let action = ActionRequest::CloseTerminal {
                project_id: self.project_id.clone(),
                terminal_id,
            };
            let backend = self.backend.clone();
            let terminals = self.terminals.clone();
            self.workspace.update(cx, |ws, cx| {
                execute_action(action, ws, &*backend, &terminals, cx);
            });
        }
    }

    pub(super) fn handle_minimize(&mut self, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| {
            ws.toggle_terminal_minimized(&self.project_id, &self.layout_path, cx);
        });
    }

    pub(super) fn handle_fullscreen(&mut self, cx: &mut Context<Self>) {
        if let Some(ref id) = self.terminal_id {
            self.workspace.update(cx, |ws, cx| {
                ws.set_fullscreen_terminal(self.project_id.clone(), id.clone(), cx);
            });
        }
    }

    pub(super) fn handle_detach(&mut self, cx: &mut Context<Self>) {
        if self.terminal_id.is_some() {
            self.workspace.update(cx, |ws, cx| {
                ws.detach_terminal(&self.project_id, &self.layout_path, cx);
            });
        }
    }

    pub(super) fn handle_export_buffer(&mut self, cx: &mut Context<Self>) {
        if let Some(ref terminal_id) = self.terminal_id {
            if let Some(path) = self.backend.capture_buffer(terminal_id) {
                cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
            }
        }
    }

    pub(super) fn handle_rename(&mut self, new_name: String, cx: &mut Context<Self>) {
        if let Some(ref terminal_id) = self.terminal_id {
            let project_id = self.project_id.clone();
            let terminal_id = terminal_id.clone();
            self.workspace.update(cx, |ws, cx| {
                ws.rename_terminal(&project_id, &terminal_id, new_name, cx);
            });
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

    pub(super) fn handle_clear(&mut self, _cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal {
            terminal.clear();
        }
    }

    pub(super) fn handle_select_all(&mut self, cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal {
            terminal.select_all();
            cx.notify();
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
