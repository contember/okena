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
    terminal.process_output(b"\x1b[?1004h");
    assert!(terminal.wants_focus_events());
    (terminal, transport)
}

#[test]
fn focus_reports_use_aggregate_view_focus() {
    let (terminal, transport) = terminal_with_capture();

    terminal.update_focus_reporter(1, true);
    terminal.update_focus_reporter(2, false);
    terminal.update_focus_reporter(1, false);

    assert_eq!(
        transport.writes(),
        vec![b"\x1b[I".to_vec(), b"\x1b[O".to_vec()]
    );
}

#[test]
fn removing_one_focused_view_keeps_terminal_focused_if_another_view_is_focused() {
    let (terminal, transport) = terminal_with_capture();

    terminal.update_focus_reporter(1, true);
    terminal.update_focus_reporter(2, true);
    terminal.remove_focus_reporter(1);
    terminal.remove_focus_reporter(2);

    assert_eq!(
        transport.writes(),
        vec![b"\x1b[I".to_vec(), b"\x1b[O".to_vec()]
    );
}
