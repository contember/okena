use anyhow::Result;
use okena_terminal::backend::TerminalBackend;
use okena_terminal::shell_config::ShellType;
use okena_terminal::terminal::TerminalTransport;
use okena_transport::client::{
    REMOTE_TERMINAL_ANSWERS_QUERIES, REMOTE_TERMINAL_RESIZE_DEBOUNCE_MS,
    REMOTE_TERMINAL_USES_MOUSE_BACKEND, WsClientMessage, close_remote_terminal, make_prefixed_id,
    resize_remote_terminal, send_remote_terminal_input,
};
use std::path::PathBuf;
use std::sync::Arc;

/// Transport implementation for remote terminals.
///
/// Sends input and resize commands over the WebSocket connection.
/// Used inside `Terminal` objects for I/O - the Terminal doesn't know
/// it's remote vs local.
pub struct RemoteTransport {
    ws_tx: parking_lot::RwLock<async_channel::Sender<WsClientMessage>>,
    pub(crate) connection_id: String,
}

impl RemoteTransport {
    pub(crate) fn new(
        ws_tx: async_channel::Sender<WsClientMessage>,
        connection_id: String,
    ) -> Self {
        Self {
            ws_tx: parking_lot::RwLock::new(ws_tx),
            connection_id,
        }
    }

    pub(crate) fn replace_sender(&self, ws_tx: async_channel::Sender<WsClientMessage>) {
        *self.ws_tx.write() = ws_tx;
    }

    fn sender(&self) -> async_channel::Sender<WsClientMessage> {
        self.ws_tx.read().clone()
    }
}

impl TerminalTransport for RemoteTransport {
    fn send_input(&self, terminal_id: &str, data: &[u8]) {
        send_remote_terminal_input(&self.sender(), &self.connection_id, terminal_id, data);
    }

    /// No-op: the daemon owns the PTY and is the sole responder to terminal
    /// queries (Device Attributes, DSR, DECRQM, …). A remote client is a
    /// render-only mirror of the daemon's grid — if it also answered, the reply
    /// would be a duplicate that round-trips over the WebSocket and lands at the
    /// PTY long after the querying program exited (the stray `6c` at the shell
    /// prompt after closing nvim). The default `send_response` routes to
    /// `send_input`, so we must explicitly suppress it here.
    fn send_response(&self, _terminal_id: &str, _data: &[u8]) {}

    fn resize(&self, terminal_id: &str, cols: u16, rows: u16) {
        resize_remote_terminal(&self.sender(), &self.connection_id, terminal_id, cols, rows);
    }

    fn uses_mouse_backend(&self) -> bool {
        REMOTE_TERMINAL_USES_MOUSE_BACKEND
    }

    fn resize_debounce_ms(&self) -> u64 {
        REMOTE_TERMINAL_RESIZE_DEBOUNCE_MS
    }

    fn answers_terminal_queries(&self) -> bool {
        // Mirror of a server-owned PTY: the server's emulator answers DSR/DA/
        // OSC-color queries. Answering here too would duplicate every reply
        // and let emulator chatter steal the server's resize ownership.
        REMOTE_TERMINAL_ANSWERS_QUERIES
    }
}

/// Backend implementation for remote terminals.
///
/// Implements `TerminalBackend` so that `TerminalPane` and `LayoutContainer`
/// can use it interchangeably with `LocalBackend`.
pub struct RemoteBackend {
    transport: Arc<RemoteTransport>,
    connection_id: String,
}

impl RemoteBackend {
    pub fn new(transport: Arc<RemoteTransport>, connection_id: String) -> Self {
        Self {
            transport,
            connection_id,
        }
    }
}

impl TerminalBackend for RemoteBackend {
    fn transport(&self) -> Arc<dyn TerminalTransport> {
        self.transport.clone()
    }

    fn create_terminal(&self, _cwd: &str, _shell: Option<&ShellType>) -> Result<String> {
        anyhow::bail!("Creating terminals on remote server is not supported")
    }

    fn reconnect_terminal(
        &self,
        id: &str,
        _cwd: &str,
        _shell: Option<&ShellType>,
    ) -> Result<String> {
        Ok(make_prefixed_id(&self.connection_id, id))
    }

    fn kill(&self, terminal_id: &str) {
        close_remote_terminal(&self.transport.sender(), &self.connection_id, terminal_id);
    }

    fn capture_buffer(&self, _terminal_id: &str) -> Option<PathBuf> {
        None
    }

    fn supports_buffer_capture(&self) -> bool {
        // The daemon performs the actual `capture-pane`; the GUI routes the
        // export through the action dispatcher (server-side op over HTTP), so
        // the button stays visible for remote terminals.
        true
    }

    fn is_remote(&self) -> bool {
        true
    }

    fn get_shell_pid(&self, _terminal_id: &str) -> Option<u32> {
        None // Remote terminals don't expose shell PID
    }

    fn get_service_pids(&self, _terminal_id: &str) -> Vec<u32> {
        Vec::new() // Remote terminals don't expose PIDs
    }
}
