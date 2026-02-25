use crate::remote_client::backend::{RemoteBackend, RemoteTransport};
use crate::terminal::backend::TerminalBackend;
use crate::terminal::terminal::{Terminal, TerminalSize};
use crate::views::root::TerminalsRegistry;

use okena_core::api::StateResponse;
use okena_core::client::{
    is_remote_terminal, ConnectionEvent, ConnectionHandler, ConnectionStatus,
    RemoteClient, RemoteConnectionConfig, WsClientMessage,
};

use std::collections::HashMap;
use std::sync::Arc;

/// Desktop-specific handler that creates `Terminal` objects and manages the registry.
pub struct DesktopConnectionHandler {
    terminals: TerminalsRegistry,
}

impl DesktopConnectionHandler {
    pub fn new(terminals: TerminalsRegistry) -> Self {
        Self { terminals }
    }
}

impl ConnectionHandler for DesktopConnectionHandler {
    fn create_terminal(
        &self,
        connection_id: &str,
        _terminal_id: &str,
        prefixed_id: &str,
        ws_sender: async_channel::Sender<WsClientMessage>,
    ) {
        let mut terminals = self.terminals.lock();
        // Skip if terminal already exists — on reconnect the server re-sends
        // CreateTerminal for every live terminal. Reusing the existing object
        // keeps the views' Arc<Terminal> references valid and avoids leaking
        // the old Terminal (with its ~19-48 MB scrollback grid) on every reconnect.
        if terminals.contains_key(prefixed_id) {
            return;
        }
        let transport = Arc::new(RemoteTransport {
            ws_tx: ws_sender,
            connection_id: connection_id.to_string(),
        });
        let terminal = Arc::new(Terminal::new(
            prefixed_id.to_string(),
            TerminalSize::default(),
            transport,
            String::new(),
        ));
        terminals.insert(prefixed_id.to_string(), terminal);
    }

    fn on_terminal_output(&self, prefixed_id: &str, data: &[u8]) {
        if let Some(terminal) = self.terminals.lock().get(prefixed_id) {
            terminal.process_output(data);
        }
    }

    fn resize_terminal(&self, prefixed_id: &str, cols: u16, rows: u16) {
        if let Some(terminal) = self.terminals.lock().get(prefixed_id) {
            terminal.resize_grid_only(cols, rows);
        }
    }

    fn remove_terminal(&self, prefixed_id: &str) {
        self.terminals.lock().remove(prefixed_id);
    }

    fn remove_all_terminals(&self, connection_id: &str) {
        let mut terminals = self.terminals.lock();
        let to_remove: Vec<String> = terminals
            .keys()
            .filter(|k| is_remote_terminal(k, connection_id))
            .cloned()
            .collect();
        for key in to_remove {
            terminals.remove(&key);
        }
    }

    fn remove_terminals_except(
        &self,
        connection_id: &str,
        keep_ids: &std::collections::HashSet<String>,
    ) {
        use okena_core::client::strip_prefix;
        let mut terminals = self.terminals.lock();
        let to_remove: Vec<String> = terminals
            .keys()
            .filter(|k| {
                is_remote_terminal(k, connection_id)
                    && !keep_ids.contains(&strip_prefix(k, connection_id))
            })
            .cloned()
            .collect();
        for key in &to_remove {
            log::info!("Removing stale remote terminal: {}", key);
        }
        for key in to_remove {
            terminals.remove(&key);
        }
    }
}

/// Thin wrapper preserving existing API for manager.rs.
pub struct RemoteConnection {
    client: RemoteClient<DesktopConnectionHandler>,
}

impl RemoteConnection {
    pub fn new(
        config: RemoteConnectionConfig,
        runtime: Arc<tokio::runtime::Runtime>,
        terminals: TerminalsRegistry,
        event_tx: async_channel::Sender<ConnectionEvent>,
    ) -> Self {
        let handler = Arc::new(DesktopConnectionHandler::new(terminals));
        let client = RemoteClient::new(config, runtime, handler, event_tx);
        Self { client }
    }

    pub fn connect(&mut self) {
        self.client.connect();
    }

    pub fn pair(&mut self, code: &str) {
        self.client.pair(code);
    }

    pub fn disconnect(&mut self) {
        self.client.disconnect();
    }

    pub fn config(&self) -> &RemoteConnectionConfig {
        self.client.config()
    }

    pub fn config_mut(&mut self) -> &mut RemoteConnectionConfig {
        self.client.config_mut()
    }

    pub fn status(&self) -> &ConnectionStatus {
        self.client.status()
    }

    pub fn status_mut(&mut self) -> &mut ConnectionStatus {
        self.client.status_mut()
    }

    #[allow(dead_code)]
    pub fn set_status(&mut self, status: ConnectionStatus) {
        self.client.set_status(status);
    }

    pub fn remote_state(&self) -> Option<&StateResponse> {
        self.client.remote_state()
    }

    pub fn set_remote_state(&mut self, state: Option<StateResponse>) {
        self.client.set_remote_state(state);
    }

    pub fn update_stream_mappings(&mut self, mappings: HashMap<String, u32>) {
        self.client.update_stream_mappings(mappings);
    }

    pub fn update_shared_token(&self, token: &str) {
        self.client.update_shared_token(token);
    }

    /// Get a TerminalBackend for this connection.
    pub fn backend(&self) -> Arc<dyn TerminalBackend> {
        let ws_tx = self.client.ws_sender().cloned().unwrap_or_else(|| {
            let (tx, _) = async_channel::bounded::<WsClientMessage>(1);
            tx
        });
        let transport = Arc::new(RemoteTransport {
            ws_tx,
            connection_id: self.config().id.clone(),
        });
        Arc::new(RemoteBackend::new(transport, self.config().id.clone()))
    }
}
