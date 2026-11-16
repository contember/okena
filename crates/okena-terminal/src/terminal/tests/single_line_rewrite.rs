use super::super::Terminal;
use super::super::types::TerminalSize;
use super::NullTransport;
use std::sync::Arc;

/// A terminal whose initial full damage is already consumed.
fn terminal() -> Terminal {
    let terminal = Terminal::new(
        "single-line-rewrite-test".to_string(),
        TerminalSize::default(),
        Arc::new(NullTransport),
        "/tmp".to_string(),
    );
    terminal.take_single_line_rewrite();
    terminal
}

#[test]
fn a_spinner_rewriting_its_line_in_place_is_a_single_line_rewrite() {
    let terminal = terminal();
    terminal.process_output(b"| working");
    assert!(terminal.take_single_line_rewrite());
    for frame in [b"\r/ working", b"\r- working", b"\r\\ working"] {
        terminal.process_output(frame);
        assert!(terminal.take_single_line_rewrite());
    }
}

#[test]
fn output_on_two_lines_or_a_scroll_is_not() {
    let terminal = terminal();
    terminal.process_output(b"one\r\ntwo");
    assert!(!terminal.take_single_line_rewrite(), "two lines changed");

    let rows = TerminalSize::default().rows as usize;
    terminal.process_output("line\r\n".repeat(rows + 1).as_bytes());
    assert!(!terminal.take_single_line_rewrite(), "the screen scrolled");
}

#[test]
fn each_call_reports_only_the_output_since_the_previous_one() {
    let terminal = terminal();
    terminal.process_output(b"one\r\ntwo");
    assert!(!terminal.take_single_line_rewrite());
    terminal.process_output(b"\rTWO");
    assert!(terminal.take_single_line_rewrite());
}

#[test]
fn queued_remote_output_counts_once_it_is_parsed() {
    let terminal = terminal();
    terminal.enqueue_output(b"one\r\ntwo");
    terminal.process_pending_output();
    assert!(!terminal.take_single_line_rewrite());
}
