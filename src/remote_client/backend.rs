use crate::terminal::backend::TerminalBackend;
use crate::terminal::shell_config::ShellType;
use crate::terminal::terminal::TerminalTransport;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;

/// Messages sent from the GPUI/UI thread to the WebSocket writer task.
#[derive(Debug)]
pub(crate) enum WsClientMessage {
    /// Send text input to a remote terminal
    SendText { terminal_id: String, text: String },
    /// Resize a remote terminal
    Resize {
        terminal_id: String,
        cols: u16,
        rows: u16,
    },
    /// Close a remote terminal
    CloseTerminal { terminal_id: String },
    /// Subscribe to terminal output streams
    Subscribe { terminal_ids: Vec<String> },
    /// Unsubscribe from terminal output streams
    Unsubscribe { terminal_ids: Vec<String> },
}

/// Transport implementation for remote terminals.
///
/// Sends input and resize commands over the WebSocket connection.
/// Used inside `Terminal` objects for I/O - the Terminal doesn't know
/// it's remote vs local.
pub struct RemoteTransport {
    pub(crate) ws_tx: async_channel::Sender<WsClientMessage>,
    pub(crate) connection_id: String,
}

impl TerminalTransport for RemoteTransport {
    fn send_input(&self, terminal_id: &str, data: &[u8]) {
        let remote_id = strip_prefix(terminal_id, &self.connection_id);
        let _ = self.ws_tx.try_send(WsClientMessage::SendText {
            terminal_id: remote_id,
            text: String::from_utf8_lossy(data).to_string(),
        });
    }

    fn resize(&self, terminal_id: &str, cols: u16, rows: u16) {
        let remote_id = strip_prefix(terminal_id, &self.connection_id);
        let _ = self.ws_tx.try_send(WsClientMessage::Resize {
            terminal_id: remote_id,
            cols,
            rows,
        });
    }

    fn uses_mouse_backend(&self) -> bool {
        // Remote doesn't use tmux backend locally
        false
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
        // MVP: not supported. Remote terminals are pre-existing on the server.
        anyhow::bail!("Creating terminals on remote server is not supported")
    }

    fn reconnect_terminal(
        &self,
        id: &str,
        _cwd: &str,
        _shell: Option<&ShellType>,
    ) -> Result<String> {
        // Remote terminal already exists on server.
        // Return prefixed ID for local registry.
        Ok(make_prefixed_id(&self.connection_id, id))
    }

    fn kill(&self, terminal_id: &str) {
        let remote_id = strip_prefix(terminal_id, &self.connection_id);
        let _ = self
            .transport
            .ws_tx
            .try_send(WsClientMessage::CloseTerminal {
                terminal_id: remote_id,
            });
    }

    fn capture_buffer(&self, _terminal_id: &str) -> Option<PathBuf> {
        None
    }

    fn supports_buffer_capture(&self) -> bool {
        false
    }

    fn is_remote(&self) -> bool {
        true
    }
}

// ── ID namespacing helpers ──────────────────────────────────────────────────

/// Prefix format: "remote:{connection_id}:{terminal_id}"
pub(crate) fn make_prefixed_id(connection_id: &str, terminal_id: &str) -> String {
    format!("remote:{}:{}", connection_id, terminal_id)
}

/// Strip the "remote:{connection_id}:" prefix from a terminal ID.
/// If the ID doesn't have the expected prefix, returns it unchanged.
pub(crate) fn strip_prefix(terminal_id: &str, connection_id: &str) -> String {
    let prefix = format!("remote:{}:", connection_id);
    if let Some(stripped) = terminal_id.strip_prefix(&prefix) {
        stripped.to_string()
    } else {
        terminal_id.to_string()
    }
}

/// Check if a terminal ID belongs to a specific remote connection.
pub(crate) fn is_remote_terminal(terminal_id: &str, connection_id: &str) -> bool {
    let prefix = format!("remote:{}:", connection_id);
    terminal_id.starts_with(&prefix)
}
