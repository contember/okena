use std::sync::Arc;

use parking_lot::Mutex;

use super::NullTransport;
use crate::terminal::{Terminal, TerminalModeState, TerminalSize, TerminalTransport};

fn terminal(id: &str) -> Terminal {
    Terminal::new(
        id.into(),
        TerminalSize::default(),
        Arc::new(NullTransport),
        "/tmp".into(),
    )
}

#[test]
fn snapshot_restores_mouse_and_alternate_screen_modes() {
    let source = terminal("source");
    source.process_output(b"\x1b[?1049h\x1b[?1002h\x1b[?1006hcontent");

    let mirror = terminal("mirror");
    mirror.process_output(&source.render_snapshot());

    assert!(mirror.is_alt_screen());
    assert!(mirror.is_mouse_mode());
    assert!(mirror.supports_mouse_drag());
}

#[test]
fn snapshot_clears_modes_that_are_no_longer_active() {
    let source = terminal("source");
    let mirror = terminal("mirror");
    mirror.process_output(b"\x1b[?1049h\x1b[?1002h\x1b[?1006h");

    mirror.process_output(&source.render_snapshot());

    assert!(!mirror.is_alt_screen());
    assert!(!mirror.is_mouse_mode());
    assert!(!mirror.supports_mouse_drag());
}

#[derive(Default)]
struct ModeTransport {
    loaded: Mutex<Option<TerminalModeState>>,
    persisted: Mutex<Option<TerminalModeState>>,
}

impl TerminalTransport for ModeTransport {
    fn send_input(&self, _terminal_id: &str, _data: &[u8]) {}
    fn resize(&self, _terminal_id: &str, _cols: u16, _rows: u16) {}
    fn uses_mouse_backend(&self) -> bool {
        false
    }
    fn load_terminal_modes(&self, _terminal_id: &str) -> Option<TerminalModeState> {
        *self.loaded.lock()
    }
    fn persist_terminal_modes(&self, _terminal_id: &str, modes: TerminalModeState) {
        *self.persisted.lock() = Some(modes);
    }
}

#[test]
fn persisted_modes_initialize_a_reattached_terminal() {
    let transport = Arc::new(ModeTransport::default());
    let original = Terminal::new(
        "original".into(),
        TerminalSize::default(),
        transport.clone(),
        "/tmp".into(),
    );
    original.process_output(b"\x1b[?1049h\x1b[?1002h\x1b[?1006h");

    let persisted = transport.persisted.lock().expect("modes were persisted");
    assert_eq!(
        TerminalModeState::decode(&persisted.encode()),
        Some(persisted)
    );
    *transport.loaded.lock() = Some(persisted);
    let reattached = Terminal::new(
        "reattached".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    assert!(reattached.is_alt_screen());
    assert!(reattached.is_mouse_mode());
    assert!(reattached.supports_mouse_drag());
}
