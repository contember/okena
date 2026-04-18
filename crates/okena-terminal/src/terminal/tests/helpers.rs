use super::super::transport::TerminalTransport;
use parking_lot::Mutex;

pub(crate) struct NullTransport;
impl TerminalTransport for NullTransport {
    fn send_input(&self, _terminal_id: &str, _data: &[u8]) {}
    fn resize(&self, _terminal_id: &str, _cols: u16, _rows: u16) {}
    fn uses_mouse_backend(&self) -> bool { false }
}

/// Records every byte the sidecar writes back to the PTY so tests can
/// assert on XTVERSION / DA / color responses.
pub(crate) struct CapturingTransport {
    writes: Mutex<Vec<Vec<u8>>>,
}

impl CapturingTransport {
    pub(crate) fn new() -> Self {
        Self { writes: Mutex::new(Vec::new()) }
    }

    pub(crate) fn writes(&self) -> Vec<Vec<u8>> {
        self.writes.lock().clone()
    }
}

impl TerminalTransport for CapturingTransport {
    fn send_input(&self, _terminal_id: &str, data: &[u8]) {
        self.writes.lock().push(data.to_vec());
    }
    fn resize(&self, _terminal_id: &str, _cols: u16, _rows: u16) {}
    fn uses_mouse_backend(&self) -> bool { false }
}
