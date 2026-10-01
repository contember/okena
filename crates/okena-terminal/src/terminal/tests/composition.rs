use super::super::Terminal;
use super::super::types::TerminalSize;
use super::CapturingTransport;
use std::sync::Arc;

fn terminal_with_capture() -> (Terminal, Arc<CapturingTransport>) {
    let transport = Arc::new(CapturingTransport::new());
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport.clone(),
        "/tmp".into(),
    );
    (terminal, transport)
}

#[test]
fn marking_text_holds_it_back_from_the_pty() {
    let (terminal, transport) = terminal_with_capture();

    terminal.set_marked_text("ˇ");

    assert!(transport.writes().is_empty());
    assert_eq!(terminal.marked_text().as_deref(), Some("ˇ"));
    assert_eq!(terminal.marked_text_range_utf16(), Some(0..1));
}

#[test]
fn marking_empty_text_ends_the_composition() {
    let (terminal, transport) = terminal_with_capture();

    terminal.set_marked_text("ˇ");
    terminal.set_marked_text("");

    assert!(transport.writes().is_empty());
    assert_eq!(terminal.marked_text(), None);
}

#[test]
fn clearing_marked_text_sends_nothing() {
    let (terminal, transport) = terminal_with_capture();

    terminal.set_marked_text("ˇ");
    terminal.clear_marked_text();

    assert!(transport.writes().is_empty());
    assert_eq!(terminal.marked_text(), None);
}

#[test]
fn marked_text_range_counts_utf16_units() {
    let (terminal, _transport) = terminal_with_capture();

    terminal.set_marked_text("𝄞ˇ");

    assert_eq!(terminal.marked_text_range_utf16(), Some(0..3));
}
