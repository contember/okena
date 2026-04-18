use super::super::Terminal;
use super::super::resize_authority::reset_resize_authority;
use super::super::transport::TerminalTransport;
use super::super::types::TerminalSize;
use super::NullTransport;
use std::sync::Arc;

// The resize authority is process-global; these tests share a mutex so
// they don't observe each other's writes.
static RESIZE_AUTH_TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

#[test]
fn resize_owner_defaults_to_local() {
    let _g = RESIZE_AUTH_TEST_LOCK.lock();
    reset_resize_authority();
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), TerminalSize::default(), transport, String::new());
    assert!(terminal.is_resize_owner_local());
}

#[test]
fn resize_owner_transitions() {
    let _g = RESIZE_AUTH_TEST_LOCK.lock();
    reset_resize_authority();
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new("t".into(), TerminalSize::default(), transport, String::new());

    terminal.claim_resize_remote();
    assert!(!terminal.is_resize_owner_local());

    terminal.claim_resize_local();
    assert!(terminal.is_resize_owner_local());
}

#[test]
fn resize_owner_is_process_global() {
    let _g = RESIZE_AUTH_TEST_LOCK.lock();
    reset_resize_authority();
    let transport = Arc::new(NullTransport);
    let term_a = Terminal::new("a".into(), TerminalSize::default(), transport.clone(), String::new());
    let term_b = Terminal::new("b".into(), TerminalSize::default(), transport, String::new());

    // Claiming remote on A flips authority for B as well.
    term_a.claim_resize_remote();
    assert!(!term_b.is_resize_owner_local());

    // Claiming local on B flips authority back for A.
    term_b.claim_resize_local();
    assert!(term_a.is_resize_owner_local());
}

#[test]
fn resize_grid_only_does_not_call_transport() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct SpyTransport { resize_called: AtomicBool }
    impl TerminalTransport for SpyTransport {
        fn send_input(&self, _: &str, _: &[u8]) {}
        fn resize(&self, _: &str, _: u16, _: u16) {
            self.resize_called.store(true, Ordering::Relaxed);
        }
        fn uses_mouse_backend(&self) -> bool { false }
    }

    let transport = Arc::new(SpyTransport { resize_called: AtomicBool::new(false) });
    let terminal = Terminal::new("t".into(), TerminalSize::default(), transport.clone(), String::new());

    terminal.resize_grid_only(120, 40);
    assert!(!transport.resize_called.load(Ordering::Relaxed));
    assert_eq!(terminal.resize_state.lock().size.cols, 120);
    assert_eq!(terminal.resize_state.lock().size.rows, 40);
}
