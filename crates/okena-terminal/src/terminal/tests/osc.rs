use super::super::Terminal;
use super::super::app_version::set_app_version;
use super::super::osc_sidecar::parse_osc7_file_uri;
use super::super::types::{PromptMarkKind, TerminalSize};
use super::{CapturingTransport, NullTransport};
use std::sync::Arc;

#[test]
fn test_osc_title_set() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "test-id".to_string(),
        TerminalSize::default(),
        transport,
        "/tmp".to_string(),
    );

    // Feed OSC 0 (set title) sequence: ESC ] 0 ; MOJE_JMENO BEL
    let osc_data = b"\x1b]0;MOJE_JMENO\x07";
    terminal.process_output(osc_data);

    assert_eq!(terminal.title(), Some("MOJE_JMENO".to_string()));
}

#[test]
fn test_osc_title_with_surrounding_data() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "test-id".to_string(),
        TerminalSize::default(),
        transport,
        "/tmp".to_string(),
    );

    // Simulate what dtach sends: clear screen + OSC title + some output
    let data = b"\x1b[H\x1b[J\x1b]0;MOJE_JMENO\x07DONE\r\n";
    terminal.process_output(data);

    assert_eq!(terminal.title(), Some("MOJE_JMENO".to_string()));
}

#[test]
fn test_osc_title_split_across_chunks() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "test-id".to_string(),
        TerminalSize::default(),
        transport,
        "/tmp".to_string(),
    );

    // Split the OSC sequence across two process_output calls
    terminal.process_output(b"\x1b]0;MOJE");
    assert_eq!(terminal.title(), None); // Not complete yet

    terminal.process_output(b"_JMENO\x07");
    assert_eq!(terminal.title(), Some("MOJE_JMENO".to_string()));
}

#[test]
fn test_osc7_reports_cwd() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "test-id".to_string(),
        TerminalSize::default(),
        transport,
        "/tmp".to_string(),
    );

    assert_eq!(terminal.reported_cwd(), None);
    assert_eq!(terminal.current_cwd(), "/tmp");

    terminal.process_output(b"\x1b]7;file://myhost/home/matej/projects/okena\x1b\\");

    assert_eq!(
        terminal.reported_cwd().as_deref(),
        Some("/home/matej/projects/okena"),
    );
    assert_eq!(terminal.current_cwd(), "/home/matej/projects/okena");
}

#[test]
fn test_osc7_percent_decoded() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    terminal.process_output(b"\x1b]7;file:///home/user/My%20Projects/foo%20bar\x07");

    assert_eq!(
        terminal.reported_cwd().as_deref(),
        Some("/home/user/My Projects/foo bar"),
    );
}

#[test]
fn test_osc7_empty_host() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    terminal.process_output(b"\x1b]7;file:///home/user\x07");

    assert_eq!(terminal.reported_cwd().as_deref(), Some("/home/user"));
}

#[test]
fn test_osc7_split_across_chunks() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    terminal.process_output(b"\x1b]7;file:///home");
    assert_eq!(terminal.reported_cwd(), None);

    terminal.process_output(b"/user/proj\x07");
    assert_eq!(terminal.reported_cwd().as_deref(), Some("/home/user/proj"));
}

#[test]
fn test_osc7_updates_on_cd() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    terminal.process_output(b"\x1b]7;file:///a\x07");
    assert_eq!(terminal.reported_cwd().as_deref(), Some("/a"));

    terminal.process_output(b"\x1b]7;file:///b/c\x07");
    assert_eq!(terminal.reported_cwd().as_deref(), Some("/b/c"));
}

#[test]
fn test_osc7_invalid_scheme_ignored() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    terminal.process_output(b"\x1b]7;http://example/x\x07");
    assert_eq!(terminal.reported_cwd(), None);
}

#[test]
fn test_parse_osc7_file_uri() {
    assert_eq!(
        parse_osc7_file_uri("file:///home/user").as_deref(),
        Some("/home/user"),
    );
    assert_eq!(
        parse_osc7_file_uri("file://host/home/user").as_deref(),
        Some("/home/user"),
    );
    assert_eq!(
        parse_osc7_file_uri("file:///path/with%20space").as_deref(),
        Some("/path/with space"),
    );
    assert_eq!(parse_osc7_file_uri("http://example/x"), None);
    assert_eq!(parse_osc7_file_uri("file://host-without-path"), None);
}

#[test]
fn test_osc_title_reset() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "test-id".to_string(),
        TerminalSize::default(),
        transport,
        "/tmp".to_string(),
    );

    // Set title
    terminal.process_output(b"\x1b]0;MOJE_JMENO\x07");
    assert_eq!(terminal.title(), Some("MOJE_JMENO".to_string()));

    // Reset title (OSC 0 with empty string => set_title(None) in alacritty)
    terminal.process_output(b"\x1b]0;\x07");
    // After reset, title should be cleared or set to empty
    let title = terminal.title();
    assert!(title.is_none() || title.as_deref() == Some(""), "title should be empty or None, got: {:?}", title);
}

#[test]
fn test_osc9_notification_collected() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    terminal.process_output(b"\x1b]9;Build complete\x07");

    let pending = terminal.take_pending_notifications();
    assert_eq!(pending, vec!["Build complete".to_string()]);
    // Second drain is empty (consumed).
    assert!(terminal.take_pending_notifications().is_empty());
}

#[test]
fn test_osc9_multiple_notifications_queued() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    terminal.process_output(b"\x1b]9;first\x07\x1b]9;second\x07");

    assert_eq!(
        terminal.take_pending_notifications(),
        vec!["first".to_string(), "second".to_string()],
    );
}

#[test]
fn test_osc9_empty_message_ignored() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    // Empty body should not queue a blank toast.
    terminal.process_output(b"\x1b]9;\x07");
    terminal.process_output(b"\x1b]9;   \x07");

    assert!(terminal.take_pending_notifications().is_empty());
}

#[test]
fn test_osc9_split_across_chunks() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    terminal.process_output(b"\x1b]9;Long ");
    assert!(terminal.take_pending_notifications().is_empty());
    terminal.process_output(b"running job done\x07");

    assert_eq!(
        terminal.take_pending_notifications(),
        vec!["Long running job done".to_string()],
    );
}

#[test]
fn test_osc9_st_terminator() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    // ST-terminated form (ESC \) is equally valid.
    terminal.process_output(b"\x1b]9;hello\x1b\\");

    assert_eq!(
        terminal.take_pending_notifications(),
        vec!["hello".to_string()],
    );
}

#[test]
fn test_xtversion_responds_with_okena_name() {
    set_app_version("0.20.0-test");

    let transport = Arc::new(CapturingTransport::new());
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport.clone(),
        "/tmp".into(),
    );

    // XTVERSION query: `CSI > q` with empty Ps.
    terminal.process_output(b"\x1b[>q");

    let writes = transport.writes();
    assert_eq!(writes.len(), 1, "expected exactly one PTY response");
    let body = std::str::from_utf8(&writes[0]).unwrap();
    // Response must be `DCS > | okena(<version>) ST` and start with ESC P.
    assert!(body.starts_with("\x1bP>|okena("), "got: {body:?}");
    assert!(body.ends_with("\x1b\\"), "got: {body:?}");
    // The version slot is filled from whatever was injected first; since
    // set_app_version uses OnceLock, we can't rely on the exact string
    // across tests. Assert that *some* non-empty version is reported.
    assert!(body.contains("okena("), "got: {body:?}");
    assert!(!body.contains("okena()"), "version must not be empty: {body:?}");
}

#[test]
fn test_xtversion_ignores_nonzero_ps() {
    // `CSI > 1 q` is NOT XTVERSION — xterm uses it for unrelated
    // reporting modes. We must stay silent, otherwise we corrupt
    // whatever the real handler expects.
    set_app_version("0.20.0-test");

    let transport = Arc::new(CapturingTransport::new());
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport.clone(),
        "/tmp".into(),
    );

    terminal.process_output(b"\x1b[>1q");

    assert!(
        transport.writes().is_empty(),
        "non-zero Ps must not trigger a response: {:?}",
        transport.writes(),
    );
}

#[test]
fn test_xtversion_ignores_unrelated_csi() {
    set_app_version("0.20.0-test");

    let transport = Arc::new(CapturingTransport::new());
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport.clone(),
        "/tmp".into(),
    );

    // Cursor positioning and SGR must not trip the sidecar.
    terminal.process_output(b"\x1b[1;1H\x1b[31mhello\x1b[0m");

    assert!(transport.writes().is_empty());
}

// Tests for OSC 133 parsing (but not jump-to-prompt, which lives in prompt_jump.rs)
#[test]
fn test_osc133_prompt_start_captures_cursor_position() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    // Two lines of output, then a prompt marker. After the newline
    // and carriage return the cursor sits at column 0 of line 2, and
    // that's where the prompt begins.
    terminal.process_output(b"hi\r\nok\r\n\x1b]133;A\x1b\\$ ");

    let marks = terminal.prompt_marks();
    assert_eq!(marks.len(), 1);
    let mark = marks[0];
    assert_eq!(mark.kind, PromptMarkKind::PromptStart);
    assert_eq!(mark.line, 2);
    assert_eq!(mark.column, 0);
}

#[test]
fn test_osc133_all_four_kinds_captured_in_order() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    // Full prompt lifecycle on one line: A (prompt) B (cmd input) C
    // (executing) D (done with exit code).
    terminal.process_output(
        b"\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\ls\r\n\x1b]133;C\x1b\\output\r\n\x1b]133;D;0\x1b\\",
    );

    let marks = terminal.prompt_marks();
    assert_eq!(marks.len(), 4);
    assert_eq!(marks[0].kind, PromptMarkKind::PromptStart);
    assert_eq!(marks[1].kind, PromptMarkKind::CommandStart);
    assert_eq!(marks[2].kind, PromptMarkKind::CommandExecuted);
    assert_eq!(
        marks[3].kind,
        PromptMarkKind::CommandFinished { exit_code: Some(0) },
    );
}

#[test]
fn test_osc133_d_parses_nonzero_exit_code() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    terminal.process_output(b"\x1b]133;D;127\x1b\\");

    let marks = terminal.prompt_marks();
    assert_eq!(
        marks[0].kind,
        PromptMarkKind::CommandFinished { exit_code: Some(127) },
    );
}

#[test]
fn test_osc133_d_without_exit_code_is_none() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    terminal.process_output(b"\x1b]133;D\x1b\\");

    let marks = terminal.prompt_marks();
    assert_eq!(
        marks[0].kind,
        PromptMarkKind::CommandFinished { exit_code: None },
    );
}

#[test]
fn test_osc133_ignores_unknown_kind() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    // `E` is not a valid OSC 133 kind — must be dropped silently.
    terminal.process_output(b"\x1b]133;E\x1b\\");

    assert!(terminal.prompt_marks().is_empty());
}

#[test]
fn test_osc133_split_across_chunks() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    terminal.process_output(b"\x1b]133");
    assert!(terminal.prompt_marks().is_empty());
    terminal.process_output(b";A\x1b\\");

    let marks = terminal.prompt_marks();
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].kind, PromptMarkKind::PromptStart);
}

#[test]
fn test_osc133_marks_shift_when_content_scrolls() {
    // Small viewport so we can provoke scrollback growth without
    // flooding the test. 5 rows, 20 columns.
    let size = TerminalSize {
        cols: 20,
        rows: 5,
        cell_width: 8.0,
        cell_height: 16.0,
    };
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), size, transport, "/tmp".into());

    // Capture a prompt at the top of the viewport.
    terminal.process_output(b"\x1b]133;A\x1b\\$ ");
    assert_eq!(terminal.prompt_marks()[0].line, 0);

    // Push three lines of output — content scrolls, prompt should
    // still be tracked but at a lower line value (scrollback).
    terminal.process_output(b"\r\na\r\nb\r\nc\r\nd\r\ne");

    let marks = terminal.prompt_marks();
    assert_eq!(marks.len(), 1, "mark must survive scroll within cap");
    // After five linefeeds with a five-row viewport the original
    // prompt row is pushed one row into scrollback.
    assert!(
        marks[0].line < 0,
        "expected prompt to slide into scrollback, got {}",
        marks[0].line,
    );
}

#[test]
fn test_osc133_ring_buffer_evicts_oldest() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    // Drive 70 PromptStart marks through — ring capacity is 64, so
    // the 6 oldest must be evicted and the newest kept.
    for _ in 0..70 {
        terminal.process_output(b"\x1b]133;A\x1b\\");
    }

    let marks = terminal.prompt_marks();
    assert_eq!(marks.len(), 64);
    assert!(marks.iter().all(|m| m.kind == PromptMarkKind::PromptStart));
}

#[test]
fn test_parse_osc133_kind() {
    use super::super::prompt_marks::parse_osc133_kind;
    assert_eq!(parse_osc133_kind(b'A', &[]), Some(PromptMarkKind::PromptStart));
    assert_eq!(parse_osc133_kind(b'B', &[]), Some(PromptMarkKind::CommandStart));
    assert_eq!(parse_osc133_kind(b'C', &[]), Some(PromptMarkKind::CommandExecuted));
    assert_eq!(
        parse_osc133_kind(b'D', &[b"42"]),
        Some(PromptMarkKind::CommandFinished { exit_code: Some(42) }),
    );
    // Non-numeric extra params mean "unknown exit".
    assert_eq!(
        parse_osc133_kind(b'D', &[b"aid=abc"]),
        Some(PromptMarkKind::CommandFinished { exit_code: None }),
    );
    assert_eq!(parse_osc133_kind(b'Z', &[]), None);
}
