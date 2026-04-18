use super::super::Terminal;
use super::super::types::TerminalSize;
use super::NullTransport;
use std::sync::Arc;

#[test]
fn test_jump_to_prompt_walks_through_history() {
    // Small viewport (5x20) so three prompts push older ones into
    // scrollback and jumping eventually lands in history.
    let size = TerminalSize {
        cols: 20,
        rows: 5,
        cell_width: 8.0,
        cell_height: 16.0,
    };
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), size, transport, "/tmp".into());

    // Three prompts with enough output between them to push the
    // oldest into scrollback.
    terminal.process_output(b"\x1b]133;A\x1b\\$ a\r\nout\r\nmore\r\n");
    terminal.process_output(b"\x1b]133;A\x1b\\$ b\r\nout\r\nmore\r\n");
    terminal.process_output(b"\x1b]133;A\x1b\\$ c\r\n");

    assert_eq!(terminal.prompt_marks().len(), 3);

    // Walk all the way back. Each press must succeed; at least one of
    // them must cross into scrollback.
    assert!(terminal.jump_to_prompt_above());
    assert!(terminal.jump_to_prompt_above());
    assert!(terminal.jump_to_prompt_above());
    assert!(
        terminal.display_offset() > 0,
        "after walking through all three prompts the display must be \
         scrolled into history, got offset {}",
        terminal.display_offset(),
    );

    // Fourth press has nothing older.
    assert!(!terminal.jump_to_prompt_above());
}

#[test]
fn test_jump_to_prompt_above_stops_at_oldest() {
    let size = TerminalSize {
        cols: 20,
        rows: 5,
        cell_width: 8.0,
        cell_height: 16.0,
    };
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), size, transport, "/tmp".into());

    terminal.process_output(b"\x1b]133;A\x1b\\$ a\r\nout\r\n");
    terminal.process_output(b"\x1b]133;A\x1b\\$ b\r\n");

    // Two prompts → two Above presses succeed, third fails.
    assert!(terminal.jump_to_prompt_above()); // newest (index 0)
    assert!(terminal.jump_to_prompt_above()); // oldest (index 1)
    let before = terminal.display_offset();
    assert!(!terminal.jump_to_prompt_above()); // nothing older
    assert_eq!(terminal.display_offset(), before);
}

#[test]
fn test_jump_to_prompt_below_reverses_walk() {
    let size = TerminalSize {
        cols: 20,
        rows: 5,
        cell_width: 8.0,
        cell_height: 16.0,
    };
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), size, transport, "/tmp".into());

    terminal.process_output(b"\x1b]133;A\x1b\\$ a\r\nout\r\nmore\r\n");
    terminal.process_output(b"\x1b]133;A\x1b\\$ b\r\nout\r\nmore\r\n");
    terminal.process_output(b"\x1b]133;A\x1b\\$ c\r\n");

    // Walk up to oldest prompt (3 presses: newest → middle → oldest).
    terminal.jump_to_prompt_above();
    terminal.jump_to_prompt_above();
    terminal.jump_to_prompt_above();
    let at_top = terminal.display_offset();

    // Step down once — must move strictly forward (smaller offset).
    assert!(terminal.jump_to_prompt_below());
    let step1 = terminal.display_offset();
    assert!(
        step1 < at_top,
        "below should reduce display offset ({step1} < {at_top})",
    );
}

#[test]
fn test_jump_below_without_walker_is_noop() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    terminal.process_output(b"\x1b]133;A\x1b\\");
    // No Above press yet — walker is disengaged, Below must no-op.
    assert!(!terminal.jump_to_prompt_below());
}

#[test]
fn test_new_output_resets_walker() {
    let size = TerminalSize {
        cols: 20,
        rows: 5,
        cell_width: 8.0,
        cell_height: 16.0,
    };
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), size, transport, "/tmp".into());

    terminal.process_output(b"\x1b]133;A\x1b\\$ a\r\nout\r\n");
    terminal.process_output(b"\x1b]133;A\x1b\\$ b\r\n");

    // Engage the walker and step back.
    terminal.jump_to_prompt_above();
    terminal.jump_to_prompt_above();

    // New shell output must reset the walker — a subsequent Below
    // press has no walker to reverse, so it no-ops.
    terminal.process_output(b"fresh output\r\n");
    assert!(!terminal.jump_to_prompt_below());
}

#[test]
fn test_jump_to_prompt_returns_false_without_marks() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    assert!(!terminal.jump_to_prompt_above());
    assert!(!terminal.jump_to_prompt_below());
}

#[test]
fn test_jump_to_prompt_ignores_non_prompt_kinds() {
    let size = TerminalSize {
        cols: 20,
        rows: 5,
        cell_width: 8.0,
        cell_height: 16.0,
    };
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), size, transport, "/tmp".into());

    // Only `C` and `D` kinds — jumping must still be a no-op because
    // PromptStart is the canonical "prompt begins here" signal.
    terminal.process_output(b"\x1b]133;C\x1b\\cmd\r\nout\r\n");
    terminal.process_output(b"\x1b]133;D;0\x1b\\");

    assert!(!terminal.jump_to_prompt_above());
}
