use super::super::Terminal;
use super::super::resize_authority::{
    claim_remote_resize_if_allowed, reset_resize_authority, resize_authority_snapshot,
};
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
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        String::new(),
    );
    assert!(terminal.is_resize_owner_local());
}

#[test]
fn resize_owner_transitions() {
    let _g = RESIZE_AUTH_TEST_LOCK.lock();
    reset_resize_authority();
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        String::new(),
    );

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
    let term_a = Terminal::new(
        "a".into(),
        TerminalSize::default(),
        transport.clone(),
        String::new(),
    );
    let term_b = Terminal::new(
        "b".into(),
        TerminalSize::default(),
        transport,
        String::new(),
    );

    term_a.claim_resize_remote();
    assert!(!term_b.is_resize_owner_local());

    term_b.claim_resize_local();
    assert!(term_a.is_resize_owner_local());
}

#[test]
fn remote_resize_is_limited_to_current_owner() {
    let _g = RESIZE_AUTH_TEST_LOCK.lock();
    reset_resize_authority();

    assert!(claim_remote_resize_if_allowed("t", "conn-a"));
    assert_eq!(
        resize_authority_snapshot("t").remote_owner_id.as_deref(),
        Some("conn-a")
    );

    assert!(claim_remote_resize_if_allowed("t", "conn-a"));
    assert!(!claim_remote_resize_if_allowed("t", "conn-b"));
}

#[test]
fn remote_resize_owner_is_process_global() {
    let _g = RESIZE_AUTH_TEST_LOCK.lock();
    reset_resize_authority();

    assert!(claim_remote_resize_if_allowed("t1", "conn-a"));
    assert!(claim_remote_resize_if_allowed("t2", "conn-a"));
    assert!(!claim_remote_resize_if_allowed("t2", "conn-b"));
}

#[test]
fn local_input_blocks_remote_resize_until_remote_input() {
    let _g = RESIZE_AUTH_TEST_LOCK.lock();
    reset_resize_authority();
    let transport = Arc::new(NullTransport);
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport,
        String::new(),
    );

    terminal.claim_resize_local();
    assert!(!claim_remote_resize_if_allowed("t", "conn-a"));

    terminal.claim_resize_remote_owner("conn-a");
    assert!(claim_remote_resize_if_allowed("other", "conn-a"));
}

#[test]
fn resize_grid_only_does_not_call_transport() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct SpyTransport {
        resize_called: AtomicBool,
    }
    impl TerminalTransport for SpyTransport {
        fn send_input(&self, _: &str, _: &[u8]) {}
        fn resize(&self, _: &str, _: u16, _: u16) {
            self.resize_called.store(true, Ordering::Relaxed);
        }
        fn uses_mouse_backend(&self) -> bool {
            false
        }
    }

    let transport = Arc::new(SpyTransport {
        resize_called: AtomicBool::new(false),
    });
    let terminal = Terminal::new(
        "t".into(),
        TerminalSize::default(),
        transport.clone(),
        String::new(),
    );

    terminal.resize_grid_only(120, 40);
    assert!(!transport.resize_called.load(Ordering::Relaxed));
    assert_eq!(terminal.resize_state.lock().size.cols, 120);
    assert_eq!(terminal.resize_state.lock().size.rows, 40);
}
