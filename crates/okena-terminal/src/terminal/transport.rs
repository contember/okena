/// Transport trait for terminal I/O operations.
/// Implemented by PtyManager (local) and RemoteTransport (remote).
pub trait TerminalTransport: Send + Sync {
    fn send_input(&self, terminal_id: &str, data: &[u8]);
    fn resize(&self, terminal_id: &str, cols: u16, rows: u16);
    fn uses_mouse_backend(&self) -> bool;
    /// Debounce interval for transport resize calls (ms).
    /// Local PTY uses 16ms (just enough to batch rapid resizes).
    /// Remote uses longer interval to avoid flooding the network.
    fn resize_debounce_ms(&self) -> u64 { 16 }
}
