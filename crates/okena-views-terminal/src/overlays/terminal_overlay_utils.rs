//! Shared utilities for terminal overlay views.
//!
//! Contains common functionality used by both fullscreen and detached terminal views:
//! - Terminal registry lookup/creation
//! - TerminalContent initialization
//! - Key input handling
//! - Focus management

use okena_terminal::input::{KeyEvent, KeyModifiers, key_to_bytes};
use okena_terminal::terminal::{Terminal, TerminalSize, TerminalTransport};
use okena_terminal::TerminalsRegistry;
use crate::layout::terminal_pane::TerminalContent;
use okena_workspace::state::Workspace;
use gpui::*;
use std::sync::Arc;

/// Convert a GPUI key event to terminal input bytes.
fn gpui_key_to_bytes(event: &KeyDownEvent, app_cursor_mode: bool) -> Option<Vec<u8>> {
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
    key_to_bytes(&key_event, app_cursor_mode)
}

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
    transport: &Arc<dyn TerminalTransport>,
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
            transport.clone(),
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
            None,
            project_id,
            layout_path,
            workspace,
            cx,
        );
        content.set_terminal(Some(terminal), cx);
        content
    })
}

/// Handle keyboard input for a terminal.
///
/// Converts the key event to terminal bytes and sends them to the terminal.
/// Returns true if input was sent.
pub fn handle_terminal_key_input(terminal: &Terminal, event: &KeyDownEvent) -> bool {
    let app_cursor_mode = terminal.is_app_cursor_mode();
    if let Some(input) = gpui_key_to_bytes(event, app_cursor_mode) {
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
