//! Integration tests for the kitty keyboard protocol: the full chain from an
//! app pushing a mode (`CSI > flags u`) through alacritty's `TermMode` to
//! `Terminal::kitty_keyboard_flags()` and on into the key encoder. The
//! `input::tests` unit tests exercise the encoder with hand-built flags; these
//! tests guard the wiring those bypass — in particular that
//! `TermConfig::kitty_keyboard` stays enabled (without it alacritty silently
//! ignores every keyboard-mode sequence and the flag never flips).

use super::super::Terminal;
use super::super::types::TerminalSize;
use super::{CapturingTransport, NullTransport};
use crate::input::{KeyEvent, KeyModifiers, key_to_bytes};
use std::sync::Arc;

fn term() -> Terminal {
    Terminal::new(
        "test-id".to_string(),
        TerminalSize::default(),
        Arc::new(NullTransport),
        "/tmp".to_string(),
    )
}

#[test]
fn push_and_pop_toggle_disambiguate_flag() {
    let t = term();
    assert!(
        !t.kitty_keyboard_flags().disambiguate_escape_codes,
        "disambiguate must start off"
    );

    // `CSI > 1 u` — push the disambiguate-escape-codes flag.
    t.process_output(b"\x1b[>1u");
    assert!(
        t.kitty_keyboard_flags().disambiguate_escape_codes,
        "push must set the flag (regression guard for TermConfig.kitty_keyboard)"
    );

    // `CSI < u` — pop one level off the stack, back to no modes.
    t.process_output(b"\x1b[<u");
    assert!(
        !t.kitty_keyboard_flags().disambiguate_escape_codes,
        "pop must clear the flag"
    );
}

#[test]
fn escape_encodes_as_csi_u_only_after_push() {
    let t = term();
    let esc = KeyEvent {
        key: "escape".to_string(),
        key_char: None,
        modifiers: KeyModifiers::default(),
    };

    // Before the app enables the protocol: legacy bare ESC.
    assert_eq!(
        key_to_bytes(&esc, false, t.kitty_keyboard_flags()),
        Some(b"\x1b".to_vec())
    );

    // After `CSI > 1 u`: the encoder reads the live flag and emits `CSI 27 u`.
    t.process_output(b"\x1b[>1u");
    assert_eq!(
        key_to_bytes(&esc, false, t.kitty_keyboard_flags()),
        Some(b"\x1b[27u".to_vec())
    );
}

#[test]
fn send_escape_and_backtab_actions_are_kitty_aware() {
    // Esc / Tab / Shift+Tab arrive via dedicated GPUI actions that bypass the
    // on_key_down path; `Terminal::send_escape` / `send_backtab` route them back
    // through the encoder. Guard that they honor the live kitty flag.
    let transport = Arc::new(CapturingTransport::new());
    let t = Terminal::new(
        "test-id".to_string(),
        TerminalSize::default(),
        transport.clone(),
        "/tmp".to_string(),
    );

    // Legacy before the protocol is enabled.
    t.send_escape();
    t.send_backtab();
    assert_eq!(
        transport.writes(),
        vec![b"\x1b".to_vec(), b"\x1b[Z".to_vec()]
    );

    // After the app pushes disambiguate, the same actions emit CSI u.
    t.process_output(b"\x1b[>1u");
    t.send_escape();
    t.send_backtab();
    let writes = transport.writes();
    assert_eq!(&writes[2..], &[b"\x1b[27u".to_vec(), b"\x1b[9;2u".to_vec()]);

    // Plain Tab is never disambiguated at level 1.
    t.send_tab();
    assert_eq!(transport.writes().last().unwrap(), b"\t");
}
