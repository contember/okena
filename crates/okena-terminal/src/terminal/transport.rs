/// Transport trait for terminal I/O operations.
/// Implemented by PtyManager (local) and RemoteTransport (remote).
pub trait TerminalTransport: Send + Sync {
    fn send_input(&self, terminal_id: &str, data: &[u8]);
    /// Write a terminal→program *reply* (Device Attributes, cursor/size report,
    /// OSC color answer, …). These race the querying program's exit back to the
    /// shell, so the local PTY writes them synchronously ahead of the batched
    /// input queue. The default routes through `send_input`.
    fn send_response(&self, terminal_id: &str, data: &[u8]) {
        self.send_input(terminal_id, data)
    }
    fn resize(&self, terminal_id: &str, cols: u16, rows: u16);
    fn uses_mouse_backend(&self) -> bool;
    /// Debounce interval for transport resize calls (ms).
    /// Local PTY uses 16ms (just enough to batch rapid resizes).
    /// Remote uses longer interval to avoid flooding the network.
    fn resize_debounce_ms(&self) -> u64 { 16 }
}
