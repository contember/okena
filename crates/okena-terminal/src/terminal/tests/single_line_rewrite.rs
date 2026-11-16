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
    terminal.take_single_line_rewrite();
    for frame in [b"\r/ working", b"\r- working", b"\r\\ working"] {
        terminal.process_output(frame);
        assert!(terminal.take_single_line_rewrite());
    }
}

#[test]
fn a_tui_spinner_drawn_away_from_the_parked_cursor_is_a_single_line_rewrite() {
    let terminal = terminal();
    // Draw the spinner on row 5 and park the cursor on row 20, as Ink does.
    // The first frame also moves the cursor from the origin.
    for _ in 0..2 {
        terminal.process_output(b"\x1b[5;1H\\ Working\x1b[20;1H");
        terminal.take_single_line_rewrite();
    }
    for glyph in ["|", "/", "-"] {
        terminal.process_output(format!("\x1b[5;1H{glyph} Working\x1b[20;1H").as_bytes());
        assert!(terminal.take_single_line_rewrite());
    }
}

#[test]
fn text_streamed_into_a_tui_row_with_a_parked_cursor_is_not_a_rewrite() {
    let terminal = terminal();
    terminal.process_output(b"\x1b[5;1H> answer:\x1b[20;1H");
    terminal.take_single_line_rewrite();
    let mut row = String::from("> answer:");
    for token in [" streamed", " tokens", " keep", " coming"] {
        row.push_str(token);
        terminal.process_output(format!("\x1b[5;1H{row}\x1b[20;1H").as_bytes());
        assert!(
            !terminal.take_single_line_rewrite(),
            "a growing row keeps the full frame rate"
        );
    }
}

#[test]
fn a_rewrite_counts_only_from_its_second_identical_frame() {
    let terminal = terminal();
    terminal.process_output(b"| working");
    assert!(!terminal.take_single_line_rewrite(), "the cursor moved");
    terminal.process_output(b"\r/ working");
    assert!(terminal.take_single_line_rewrite());
    terminal.process_output(b"\r- working!");
    assert!(
        !terminal.take_single_line_rewrite(),
        "a longer line changes another span"
    );
}

#[test]
fn text_appended_to_a_line_is_not_a_rewrite() {
    let terminal = terminal();
    terminal.process_output(b"$ hello");
    terminal.take_single_line_rewrite();
    terminal.process_output(b" world, streamed tokens");
    assert!(
        !terminal.take_single_line_rewrite(),
        "streamed text keeps the full frame rate"
    );
}

#[test]
fn output_on_two_lines_a_scroll_or_a_cursor_move_is_not() {
    let terminal = terminal();
    terminal.process_output(b"one\r\ntwo");
    assert!(!terminal.take_single_line_rewrite(), "two lines changed");

    let rows = TerminalSize::default().rows as usize;
    terminal.process_output("line\r\n".repeat(rows + 1).as_bytes());
    assert!(!terminal.take_single_line_rewrite(), "the screen scrolled");

    terminal.process_output(b"\x1b[3;1H");
    assert!(!terminal.take_single_line_rewrite(), "the cursor moved");
}

#[test]
fn the_alternate_screen_and_a_resize_are_not() {
    let terminal = terminal();
    terminal.process_output(b"\x1b[?1049h");
    assert!(!terminal.take_single_line_rewrite());
    terminal.take_single_line_rewrite();
    terminal.resize_grid_only(100, 30);
    assert!(!terminal.take_single_line_rewrite());
}

#[test]
fn each_call_reports_only_the_output_since_the_previous_one() {
    let terminal = terminal();
    terminal.process_output(b"one\r\ntwo");
    assert!(!terminal.take_single_line_rewrite());
    terminal.process_output(b"\rTWO");
    assert!(
        !terminal.take_single_line_rewrite(),
        "the previous call was no rewrite"
    );
    terminal.process_output(b"\rtwo");
    assert!(terminal.take_single_line_rewrite());
}

#[test]
fn queued_remote_output_counts_once_it_is_parsed() {
    let terminal = terminal();
    terminal.enqueue_output(b"one\r\ntwo");
    terminal.process_pending_output();
    assert!(!terminal.take_single_line_rewrite());
}
