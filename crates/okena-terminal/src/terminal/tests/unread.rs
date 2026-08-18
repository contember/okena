//! Manual "mark as unread" — the bell raised by hand rather than by BEL.

use super::super::Terminal;
use super::super::types::TerminalSize;
use super::NullTransport;
use std::sync::Arc;

fn terminal() -> Terminal {
    Terminal::new(
        "t".to_string(),
        TerminalSize::default(),
        Arc::new(NullTransport),
        "/tmp".to_string(),
    )
}

#[test]
fn marking_unread_lights_the_bell_and_holds_it() {
    let terminal = terminal();

    assert!(!terminal.has_bell());
    terminal.mark_unread();

    assert!(terminal.has_bell(), "the mark lights the bell indicator");
    assert!(
        terminal.is_manually_unread(),
        "the hold is what survives the render path's clear-on-focus"
    );
}

#[test]
fn a_bell_from_the_shell_is_not_held() {
    let terminal = terminal();

    terminal.process_output(b"\x07");

    assert!(terminal.has_bell());
    assert!(
        !terminal.is_manually_unread(),
        "a BEL still clears as soon as the pane is focused"
    );
}

#[test]
fn releasing_the_hold_keeps_the_bell_lit() {
    let terminal = terminal();
    terminal.mark_unread();

    terminal.release_manual_unread();

    assert!(terminal.has_bell(), "focus leaving does not read the pane");
    assert!(
        !terminal.is_manually_unread(),
        "the next visit clears the bell like any other"
    );
}

#[test]
fn toggle_flips_both_ways_and_clears_a_shell_bell() {
    let terminal = terminal();

    assert!(terminal.toggle_unread(), "off -> unread");
    assert!(terminal.has_bell());

    assert!(!terminal.toggle_unread(), "unread -> read");
    assert!(!terminal.has_bell());
    assert!(!terminal.is_manually_unread());

    // A bell the shell rang reads as unread too, so the toggle dismisses it.
    terminal.process_output(b"\x07");
    assert!(!terminal.toggle_unread(), "shell bell -> read");
    assert!(!terminal.has_bell());
}
