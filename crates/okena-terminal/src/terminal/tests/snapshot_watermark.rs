use super::super::types::TerminalSize;
use super::super::Terminal;
use super::NullTransport;
use std::sync::Arc;

#[test]
fn snapshot_reports_the_last_incorporated_output_sequence() {
    let terminal = Terminal::new(
        "snapshot-sequence".to_string(),
        TerminalSize::default(),
        Arc::new(NullTransport),
        "/tmp".to_string(),
    );

    terminal.process_output_with_sequence(b"first", 41);
    let (_, first_watermark) = terminal.render_snapshot_with_sequence();
    assert_eq!(first_watermark, 41);

    terminal.process_output_with_sequence(b"second", 42);
    let (_, second_watermark) = terminal.render_snapshot_with_sequence();
    assert_eq!(second_watermark, 42);
}
