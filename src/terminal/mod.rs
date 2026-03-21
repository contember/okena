// Re-export everything from the okena-terminal crate.
// This allows existing `use crate::terminal::*` imports to keep working.
pub use okena_terminal::backend;
pub use okena_terminal::pty_manager;
pub use okena_terminal::session_backend;
pub use okena_terminal::shell_config;
pub use okena_terminal::terminal;

/// GPUI-specific input adapter.
/// Converts `gpui::KeyDownEvent` to the framework-agnostic `KeyEvent`
/// used by `okena_terminal::input::key_to_bytes`.
pub mod input {
    pub use okena_terminal::input::{KeyEvent, KeyModifiers, key_to_bytes};

    use gpui::KeyDownEvent;

    /// Convert a GPUI key event to terminal input bytes.
    pub fn gpui_key_to_bytes(event: &KeyDownEvent, app_cursor_mode: bool) -> Option<Vec<u8>> {
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
}
