use super::modes::TerminalModeState;

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
    fn resize_debounce_ms(&self) -> u64 {
        16
    }
    /// Whether this side's emulator answers terminal queries (DSR, DA, OSC
    /// color, CSI 14/18 t). True only for the process that owns the PTY;
    /// remote mirrors must not answer — duplicated replies corrupt the app's
    /// input and the server would count them as user input (resize ownership).
    fn answers_terminal_queries(&self) -> bool {
        true
    }
    /// Whether this side reaches a system clipboard, and so should queue OSC 52
    /// requests for someone to service.
    ///
    /// True only in a process with a UI. The daemon parses the same byte stream
    /// into its own emulator but has no clipboard and no drain, so anything
    /// queued there is never read and never freed — every `printf '\e]52;c;…'`
    /// from nvim, tmux or a `pbcopy` shim would accumulate for the life of the
    /// terminal. Kept separate from [`Self::answers_terminal_queries`], which
    /// asks the opposite question (does this side own the PTY).
    fn handles_clipboard(&self) -> bool {
        true
    }
    /// Load terminal modes retained by a transparent persistent session backend.
    fn load_terminal_modes(&self, _terminal_id: &str) -> Option<TerminalModeState> {
        None
    }
    /// Retain terminal modes for a future attach to the same transparent session.
    fn persist_terminal_modes(&self, _terminal_id: &str, _modes: TerminalModeState) {}
}
