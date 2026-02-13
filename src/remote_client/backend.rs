use crate::terminal::backend::TerminalBackend;
use crate::terminal::shell_config::ShellType;
use crate::terminal::terminal::TerminalTransport;
use anyhow::Result;
use okena_core::client::{make_prefixed_id, strip_prefix, WsClientMessage};
use std::path::PathBuf;
use std::sync::Arc;

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
}
