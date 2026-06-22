use super::super::Terminal;
use super::super::TerminalNotification;
use super::super::app_version::set_app_version;
use super::super::osc_sidecar::parse_osc7_file_uri;
use super::super::types::{
    PromptMarkKind, TerminalProgress, TerminalProgressState, TerminalSize,
};
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
fn test_osc1337_current_dir_bel_terminated() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    assert_eq!(terminal.reported_cwd(), None);

    // iTerm2 shell integration: OSC 1337 ; CurrentDir=<raw path> BEL. The path
    // is a plain filesystem path — no file:// scheme, no percent-encoding.
    terminal.process_output(b"\x1b]1337;CurrentDir=/home/matej/projects/okena\x07");

    assert_eq!(
        terminal.reported_cwd().as_deref(),
        Some("/home/matej/projects/okena"),
    );
    assert_eq!(terminal.current_cwd(), "/home/matej/projects/okena");
}

#[test]
fn test_osc1337_current_dir_st_terminated() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    // ST-terminated form (ESC \) is equally valid.
    terminal.process_output(b"\x1b]1337;CurrentDir=/var/www\x1b\\");

    assert_eq!(terminal.reported_cwd().as_deref(), Some("/var/www"));
}

#[test]
fn test_osc1337_empty_current_dir_ignored() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    // An empty / whitespace-only path must not overwrite the cwd.
    terminal.process_output(b"\x1b]1337;CurrentDir=\x07");
    terminal.process_output(b"\x1b]1337;CurrentDir=   \x07");

    assert_eq!(terminal.reported_cwd(), None);
}

#[test]
fn test_osc1337_non_current_dir_subcommand_ignored() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    // 1337 carries many subcommands; only CurrentDir is ours. RemoteHost (and
    // the rest) must leave the cwd untouched.
    terminal.process_output(b"\x1b]1337;RemoteHost=matej@myhost\x07");

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

/// A title-less notification, the shape `OSC 9` produces.
fn body(text: &str) -> TerminalNotification {
    TerminalNotification { title: None, body: text.to_string() }
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
    assert_eq!(pending, vec![body("Build complete")]);
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
        vec![body("first"), body("second")],
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
        vec![body("Long running job done")],
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
        vec![body("hello")],
    );
}

#[test]
fn test_osc9_4_st1_sets_normal_progress() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), TerminalSize::default(), transport, "/tmp".into());

    assert_eq!(terminal.progress(), None);

    // OSC 9 ; 4 ; 1 ; 42 → normal progress at 42%.
    terminal.process_output(b"\x1b]9;4;1;42\x07");

    assert_eq!(
        terminal.progress(),
        Some(TerminalProgress { state: TerminalProgressState::Normal, value: 42 }),
    );
    // Progress is sticky (not drained on read).
    assert_eq!(
        terminal.progress(),
        Some(TerminalProgress { state: TerminalProgressState::Normal, value: 42 }),
    );
}

#[test]
fn test_osc9_4_st0_clears_progress() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), TerminalSize::default(), transport, "/tmp".into());

    terminal.process_output(b"\x1b]9;4;1;50\x07");
    assert!(terminal.progress().is_some());

    // st=0 removes the bar; pr is ignored.
    terminal.process_output(b"\x1b]9;4;0;\x07");
    assert_eq!(terminal.progress(), None);
}

#[test]
fn test_osc9_4_st3_indeterminate_ignores_value() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), TerminalSize::default(), transport, "/tmp".into());

    // st=3 is a spinner — pr is meaningless and normalised to 0.
    terminal.process_output(b"\x1b]9;4;3;77\x07");

    assert_eq!(
        terminal.progress(),
        Some(TerminalProgress { state: TerminalProgressState::Indeterminate, value: 0 }),
    );
}

#[test]
fn test_osc9_4_clamps_value_over_100() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), TerminalSize::default(), transport, "/tmp".into());

    // pr above 100 is clamped to 100.
    terminal.process_output(b"\x1b]9;4;1;255\x07");

    assert_eq!(
        terminal.progress(),
        Some(TerminalProgress { state: TerminalProgressState::Normal, value: 100 }),
    );
}

#[test]
fn test_osc9_4_st2_error_keeps_previous_value() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), TerminalSize::default(), transport, "/tmp".into());

    // Set a baseline, then signal an error without an explicit percent.
    terminal.process_output(b"\x1b]9;4;1;30\x07");
    terminal.process_output(b"\x1b]9;4;2;\x07");

    assert_eq!(
        terminal.progress(),
        Some(TerminalProgress { state: TerminalProgressState::Error, value: 30 }),
    );

    // An explicit pr on the error state overrides the kept value.
    terminal.process_output(b"\x1b]9;4;2;80\x07");
    assert_eq!(
        terminal.progress(),
        Some(TerminalProgress { state: TerminalProgressState::Error, value: 80 }),
    );
}

#[test]
fn test_osc9_4_garbage_st_ignored() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), TerminalSize::default(), transport, "/tmp".into());

    terminal.process_output(b"\x1b]9;4;1;25\x07");
    // A non-numeric / out-of-range st must leave the current bar untouched and
    // must never produce a notification.
    terminal.process_output(b"\x1b]9;4;abc;5\x07");
    terminal.process_output(b"\x1b]9;4;9;5\x07");

    assert_eq!(
        terminal.progress(),
        Some(TerminalProgress { state: TerminalProgressState::Normal, value: 25 }),
    );
    assert!(terminal.take_pending_notifications().is_empty());
}

#[test]
fn test_osc9_4_does_not_produce_notification() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), TerminalSize::default(), transport, "/tmp".into());

    // The progress subtype must NOT be treated as notification text...
    terminal.process_output(b"\x1b]9;4;1;42\x07");
    assert!(terminal.take_pending_notifications().is_empty());
    assert!(terminal.progress().is_some());

    // ...while a plain OSC 9 message still produces a notification and leaves
    // progress untouched (no regression).
    terminal.process_output(b"\x1b]9;Build complete\x07");
    assert_eq!(terminal.take_pending_notifications(), vec![body("Build complete")]);
    assert_eq!(
        terminal.progress(),
        Some(TerminalProgress { state: TerminalProgressState::Normal, value: 42 }),
    );
}

#[test]
fn test_osc777_notify_title_and_body() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), TerminalSize::default(), transport, "/tmp".into());

    // urxvt-style: OSC 777 ; notify ; <title> ; <body>
    terminal.process_output(b"\x1b]777;notify;Claude;Waiting for your input\x07");

    assert_eq!(
        terminal.take_pending_notifications(),
        vec![TerminalNotification {
            title: Some("Claude".to_string()),
            body: "Waiting for your input".to_string(),
        }],
    );
}

#[test]
fn test_osc777_body_keeps_semicolons() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), TerminalSize::default(), transport, "/tmp".into());

    // A body containing semicolons must be rejoined, not truncated.
    terminal.process_output(b"\x1b]777;notify;Build;done: a; b; c\x07");

    assert_eq!(
        terminal.take_pending_notifications(),
        vec![TerminalNotification {
            title: Some("Build".to_string()),
            body: "done: a; b; c".to_string(),
        }],
    );
}

#[test]
fn test_osc777_non_notify_subcommand_ignored() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), TerminalSize::default(), transport, "/tmp".into());

    // 777 carries unrelated subcommands (e.g. precmd) — those must not queue.
    terminal.process_output(b"\x1b]777;precmd;something\x07");
    // ...and a notify with an empty body is dropped, like empty OSC 9.
    terminal.process_output(b"\x1b]777;notify;TitleOnly\x07");

    assert!(terminal.take_pending_notifications().is_empty());
}

#[test]
fn test_osc99_simple_single_chunk() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), TerminalSize::default(), transport, "/tmp".into());

    // Minimal kitty form: empty metadata, payload is the (title) text. With no
    // body it maps to a title-less notification, like OSC 9.
    terminal.process_output(b"\x1b]99;;Hello world\x07");

    assert_eq!(
        terminal.take_pending_notifications(),
        vec![TerminalNotification { title: None, body: "Hello world".to_string() }],
    );
}

#[test]
fn test_osc99_title_and_body_chunks() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), TerminalSize::default(), transport, "/tmp".into());

    // First chunk: title, d=0 (not complete) → nothing displayed yet.
    terminal.process_output(b"\x1b]99;i=x:d=0;My Title\x07");
    assert!(terminal.take_pending_notifications().is_empty(), "d=0 must not emit");

    // Second chunk: body, d defaults to 1 (complete) → emit assembled pair.
    terminal.process_output(b"\x1b]99;i=x:p=body;Body text\x07");
    assert_eq!(
        terminal.take_pending_notifications(),
        vec![TerminalNotification {
            title: Some("My Title".to_string()),
            body: "Body text".to_string(),
        }],
    );
}

#[test]
fn test_osc99_base64_payload() {
    use base64::Engine as _;
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), TerminalSize::default(), transport, "/tmp".into());

    let encoded = base64::engine::general_purpose::STANDARD.encode("Encoded msg");
    terminal.process_output(format!("\x1b]99;e=1;{encoded}\x07").as_bytes());

    assert_eq!(
        terminal.take_pending_notifications(),
        vec![TerminalNotification { title: None, body: "Encoded msg".to_string() }],
    );
}

#[test]
fn test_osc99_close_drops_pending() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), TerminalSize::default(), transport, "/tmp".into());

    // Start a chunked notification, then close it before completion.
    terminal.process_output(b"\x1b]99;i=x:d=0;Partial\x07");
    terminal.process_output(b"\x1b]99;i=x:c=1;\x07");
    assert!(terminal.take_pending_notifications().is_empty());

    // A subsequent final chunk for the same id has nothing to complete.
    terminal.process_output(b"\x1b]99;i=x:d=1;\x07");
    assert!(terminal.take_pending_notifications().is_empty());
}

#[test]
fn test_osc99_query_payload_ignored() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), TerminalSize::default(), transport, "/tmp".into());

    // A capability query (p=?) carries no displayable text.
    terminal.process_output(b"\x1b]99;p=?;\x07");
    assert!(terminal.take_pending_notifications().is_empty());
}

#[test]
fn test_bell_edge_is_one_shot() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), TerminalSize::default(), transport, "/tmp".into());

    assert!(!terminal.take_pending_bell(), "no bell yet");

    // A lone BEL (0x07) rings the bell.
    terminal.process_output(b"\x07");
    assert!(terminal.take_pending_bell(), "edge fires once");
    assert!(!terminal.take_pending_bell(), "edge is consumed (one-shot)");

    // The sticky UI flag is independent of the one-shot notification edge.
    assert!(terminal.has_bell(), "has_bell stays set until focus clears it");
}

#[test]
fn test_osc52_read_request_queues_responder() {
    let transport = Arc::new(CapturingTransport::new());
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport.clone(),
        "/tmp".into(),
    );

    assert!(!terminal.has_pending_clipboard_reads(), "nothing queued yet");

    // OSC 52 ; c ; ? — the app asks to READ the clipboard.
    terminal.process_output(b"\x1b]52;c;?\x07");

    assert!(
        terminal.has_pending_clipboard_reads(),
        "a read request must be queued",
    );
    // Queuing the request must not write anything to the PTY on its own —
    // the reply is only sent once answered with clipboard contents.
    assert!(
        transport.writes().is_empty(),
        "no PTY reply until answered: {:?}",
        transport.writes(),
    );
}

#[test]
fn test_osc52_answer_clipboard_reads_replies_and_drains() {
    let transport = Arc::new(CapturingTransport::new());
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport.clone(),
        "/tmp".into(),
    );

    terminal.process_output(b"\x1b]52;c;?\x07");
    assert!(terminal.has_pending_clipboard_reads());

    terminal.answer_clipboard_reads("hi");

    // The queue is drained once answered.
    assert!(
        !terminal.has_pending_clipboard_reads(),
        "queue must be empty after answering",
    );
    // A non-empty OSC 52 response was written back to the PTY.
    let writes = transport.writes();
    assert_eq!(writes.len(), 1, "expected exactly one PTY reply");
    assert!(!writes[0].is_empty(), "reply must not be empty");
    let body = std::str::from_utf8(&writes[0]).unwrap();
    assert!(body.contains("52;"), "reply should be an OSC 52 sequence: {body:?}");
}

#[test]
fn test_osc52_drop_clipboard_reads_clears_without_reply() {
    let transport = Arc::new(CapturingTransport::new());
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport.clone(),
        "/tmp".into(),
    );

    terminal.process_output(b"\x1b]52;c;?\x07");
    assert!(terminal.has_pending_clipboard_reads());

    // Silent deny: drop the request without writing anything to the PTY.
    terminal.drop_clipboard_reads();

    assert!(!terminal.has_pending_clipboard_reads(), "queue must be cleared");
    assert!(
        transport.writes().is_empty(),
        "dropping must not reply: {:?}",
        transport.writes(),
    );
}

#[test]
fn test_osc52_write_does_not_enqueue_read() {
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        "/tmp".into(),
    );

    // A plain OSC 52 *write* (base64 "hi" == "aGk=") stores clipboard text and
    // must NOT enqueue a read request.
    terminal.process_output(b"\x1b]52;c;aGk=\x07");

    assert!(
        !terminal.has_pending_clipboard_reads(),
        "a write must not enqueue a read",
    );
    assert_eq!(
        terminal.take_pending_clipboard_writes(),
        vec!["hi".to_string()],
        "the write text must reach the clipboard-write queue",
    );
}

#[test]
fn test_xtwinops_14t_reports_text_area_size_in_pixels() {
    // Explicit size so the pixel math is unambiguous: 80 cols x 24 rows at
    // 8x16 px cells → text area is 640 px wide and 384 px tall.
    let size = TerminalSize {
        cols: 80,
        rows: 24,
        cell_width: 8.0,
        cell_height: 16.0,
    };
    let transport = Arc::new(CapturingTransport::new());
    let terminal = Terminal::new("t".into(), size, transport.clone(), "/tmp".into());

    // CSI 14 t — report text-area size in pixels.
    terminal.process_output(b"\x1b[14t");

    let writes = transport.writes();
    assert_eq!(writes.len(), 1, "expected exactly one PTY reply");
    // Reply is `CSI 4 ; <height> ; <width> t` = rows*cell_height ; cols*cell_width.
    assert_eq!(writes[0], b"\x1b[4;384;640t");
}

#[test]
fn test_xtwinops_18t_still_reports_size_in_cells() {
    // No regression: CSI 18 t (size in cells) keeps replying via PtyWrite.
    let size = TerminalSize {
        cols: 80,
        rows: 24,
        cell_width: 8.0,
        cell_height: 16.0,
    };
    let transport = Arc::new(CapturingTransport::new());
    let terminal = Terminal::new("t".into(), size, transport.clone(), "/tmp".into());

    terminal.process_output(b"\x1b[18t");

    let writes = transport.writes();
    assert_eq!(writes.len(), 1, "expected exactly one PTY reply");
    // Reply is `CSI 8 ; <rows> ; <cols> t`.
    assert_eq!(writes[0], b"\x1b[8;24;80t");
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
