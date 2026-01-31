//! Shared utilities for terminal overlay views.
//!
//! Contains common functionality used by both fullscreen and detached terminal views:
//! - Terminal registry lookup/creation
//! - TerminalContent initialization
//! - Key input handling
//! - Focus management

use crate::terminal::input::key_to_bytes;
use crate::terminal::pty_manager::PtyManager;
use crate::terminal::terminal::{Terminal, TerminalSize};
use crate::views::layout::terminal_pane::TerminalContent;
use crate::views::root::TerminalsRegistry;
use crate::workspace::state::Workspace;
use gpui::*;
use std::sync::Arc;

/// Default terminal size for overlay terminals.
pub const DEFAULT_TERMINAL_SIZE: TerminalSize = TerminalSize {
    cols: 120,
    rows: 40,
    cell_width: 8.0,
    cell_height: 17.0,
};

/// Get or create a terminal from the registry.
///
/// If a terminal with the given ID exists, returns it. Otherwise creates a new terminal
/// with the default size and inserts it into the registry.
/// `cwd` is used for resolving relative file paths in URL detection.
pub fn get_or_create_terminal(
    terminal_id: &str,
    pty_manager: &Arc<PtyManager>,
    terminals: &TerminalsRegistry,
    cwd: &str,
) -> Arc<Terminal> {
    let mut terminals_guard = terminals.lock();
    if let Some(existing) = terminals_guard.get(terminal_id) {
        existing.clone()
    } else {
        let terminal = Arc::new(Terminal::new(
            terminal_id.to_string(),
            DEFAULT_TERMINAL_SIZE,
            pty_manager.clone(),
            cwd.to_string(),
        ));
        terminals_guard.insert(terminal_id.to_string(), terminal.clone());
        terminal
    }
}

/// Create a new TerminalContent view with the given parameters.
///
/// This is a convenience function that creates a TerminalContent, sets its terminal,
/// and marks it as focused.
pub fn create_terminal_content<V: 'static>(
    cx: &mut Context<V>,
    focus_handle: FocusHandle,
    project_id: String,
    layout_path: Vec<usize>,
    workspace: Entity<Workspace>,
    terminal: Arc<Terminal>,
) -> Entity<TerminalContent> {
    cx.new(|cx| {
        let mut content = TerminalContent::new(
            focus_handle,
            project_id,
            layout_path,
            workspace,
            cx,
        );
        content.set_terminal(Some(terminal), cx);
        content.set_focused(true);
        content
    })
}

/// Handle keyboard input for a terminal.
///
/// Converts the key event to terminal bytes and sends them to the terminal.
/// Returns true if input was sent.
pub fn handle_terminal_key_input(terminal: &Terminal, event: &KeyDownEvent) -> bool {
    let app_cursor_mode = terminal.is_app_cursor_mode();
    if let Some(input) = key_to_bytes(event, app_cursor_mode) {
        terminal.send_bytes(&input);
        true
    } else {
        false
    }
}

/// Handle pending focus for a terminal view.
///
/// If pending_focus is true, focuses the window and clears the flag.
pub fn handle_pending_focus<V: 'static>(
    pending_focus: &mut bool,
    focus_handle: &FocusHandle,
    window: &mut Window,
    cx: &mut Context<V>,
) {
    if *pending_focus {
        *pending_focus = false;
        window.focus(focus_handle, cx);
    }
}
