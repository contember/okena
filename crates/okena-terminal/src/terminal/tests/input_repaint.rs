use super::super::Terminal;
use super::super::types::TerminalSize;
use super::NullTransport;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

fn terminal() -> Terminal {
    Terminal::new(
        "input-repaint-test".to_string(),
        TerminalSize::default(),
        Arc::new(NullTransport),
        "/tmp".to_string(),
    )
}

#[test]
fn user_input_promotes_the_next_processed_output_once() {
    let terminal = terminal();

    terminal.send_input("a");
    assert!(
        !terminal.take_input_repaint_request(),
        "input alone does not promote an old terminal frame"
    );

    terminal.process_output(b"a");
    assert!(terminal.take_input_repaint_request());
    assert!(
        !terminal.take_input_repaint_request(),
        "the request is one-shot until more input arrives"
    );
}

#[test]
fn pre_input_backlog_cannot_consume_the_request() {
    let terminal = terminal();

    terminal.enqueue_output(b"before input");
    terminal.send_input("a");
    terminal.process_pending_output();
    assert!(!terminal.take_input_repaint_request());

    terminal.enqueue_output(b"after input");
    terminal.process_pending_output();
    assert!(terminal.take_input_repaint_request());
}

#[test]
fn concurrent_enqueue_and_input_never_lose_the_priority_request() {
    for _ in 0..16 {
        let terminal = Arc::new(terminal());
        let start = Arc::new(Barrier::new(3));

        let enqueue = {
            let terminal = terminal.clone();
            let start = start.clone();
            thread::spawn(move || {
                start.wait();
                terminal.enqueue_output(b"racing output");
            })
        };
        let input = {
            let terminal = terminal.clone();
            let start = start.clone();
            thread::spawn(move || {
                start.wait();
                terminal.send_input("a");
            })
        };

        start.wait();
        assert!(enqueue.join().is_ok(), "enqueue thread panicked");
        assert!(input.join().is_ok(), "input thread panicked");
        terminal.process_pending_output();

        if !terminal.take_input_repaint_request() {
            // The racing output was ordered before the input. The first output
            // ordered after it must still consume the request.
            terminal.enqueue_output(b"after input");
            terminal.process_pending_output();
            assert!(terminal.take_input_repaint_request());
        }
    }
}

#[test]
fn unanswered_and_empty_input_do_not_leave_stale_priority() {
    let terminal = terminal();

    terminal.send_input("");
    terminal.process_output(b"unrelated");
    assert!(!terminal.take_input_repaint_request());

    terminal.send_bytes(b"x");
    assert!(!terminal.take_input_repaint_request_at(Instant::now() + Duration::from_secs(10)));
    terminal.process_output(b"much later");
    assert!(!terminal.take_input_repaint_request());
}

#[test]
fn terminal_output_alone_does_not_request_input_priority() {
    let terminal = terminal();

    terminal.process_output(b"background output");

    assert!(!terminal.take_input_repaint_request());
}
