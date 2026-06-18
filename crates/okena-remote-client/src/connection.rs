use crate::backend::{RemoteBackend, RemoteTransport};
use okena_terminal::backend::TerminalBackend;
use okena_terminal::terminal::{Terminal, TerminalSize};
use okena_terminal::TerminalsRegistry;

use okena_core::api::StateResponse;
use okena_transport::client::{
    is_remote_terminal, ConnectionEvent, ConnectionHandler, ConnectionStatus,
    RemoteClient, RemoteConnectionConfig, WsClientMessage,
};

use std::collections::HashMap;
use std::sync::Arc;

/// Desktop-specific handler that creates `Terminal` objects and manages the registry.
pub struct DesktopConnectionHandler {
    terminals: TerminalsRegistry,
    /// Coalescing doorbell rung on every chunk of remote output so the manager's
    /// activity pump wakes and repaints server-driven sidebar indicators. See
    /// `RemoteConnectionManager::start_terminal_activity_pump`.
    activity_tx: async_channel::Sender<()>,
}

impl DesktopConnectionHandler {
    pub fn new(terminals: TerminalsRegistry, activity_tx: async_channel::Sender<()>) -> Self {
        Self {
            terminals,
            activity_tx,
        }
    }
}

impl ConnectionHandler for DesktopConnectionHandler {
    fn create_terminal(
        &self,
        connection_id: &str,
        _terminal_id: &str,
        prefixed_id: &str,
        ws_sender: async_channel::Sender<WsClientMessage>,
        cols: u16,
        rows: u16,
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
        let size = if cols > 0 && rows > 0 {
            TerminalSize { cols, rows, ..TerminalSize::default() }
        } else {
            TerminalSize::default()
        };
        let terminal = Arc::new(Terminal::new(
            prefixed_id.to_string(),
            size,
            transport,
            String::new(),
        ));
        terminals.insert(prefixed_id.to_string(), terminal);
    }

    fn on_terminal_output(&self, prefixed_id: &str, data: &[u8]) {
        let terminal = self.terminals.lock().get(prefixed_id).cloned();
        if let Some(terminal) = terminal {
            terminal.enqueue_output(data);
            // Ring the doorbell so the GPUI-side activity pump wakes and
            // repaints bell/idle indicators. Capacity 1: a full channel means a
            // wake is already pending, which will drain this terminal too.
            let _ = self.activity_tx.try_send(());
        }
    }

    fn resize_terminal(&self, prefixed_id: &str, cols: u16, rows: u16, server_owns: bool) {
        if let Some(terminal) = self.terminals.lock().get(prefixed_id) {
            // The origin's local user just reclaimed resize authority. Mark the
            // remote side as resize owner on this client so its TerminalElement
            // stops re-asserting our own window size — otherwise the origin's
            // reclaim is undone within one round-trip. The client takes control
            // back as soon as its own user types or clicks (claim_resize_local).
            if server_owns {
                terminal.claim_resize_remote();
            }
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
        use okena_transport::client::strip_prefix;
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
        activity_tx: async_channel::Sender<()>,
    ) -> Self {
        let handler = Arc::new(DesktopConnectionHandler::new(terminals, activity_tx));
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

    pub fn remote_state_mut(&mut self) -> Option<&mut StateResponse> {
        self.client.remote_state_mut()
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

#[cfg(test)]
mod tests {
    use super::*;

    // Resize authority is process-global, so authority-touching tests must run
    // serially to avoid observing each other's writes.
    static AUTH_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    fn handler_with_terminal(prefixed_id: &str) -> (DesktopConnectionHandler, Arc<Terminal>) {
        let terminals: TerminalsRegistry = Arc::new(parking_lot::Mutex::new(HashMap::new()));
        let (activity_tx, _activity_rx) = async_channel::bounded(1);
        let (ws_tx, _ws_rx) = async_channel::bounded(8);
        let transport = Arc::new(RemoteTransport { ws_tx, connection_id: "conn".into() });
        let terminal = Arc::new(Terminal::new(
            prefixed_id.to_string(),
            TerminalSize::default(),
            transport,
            String::new(),
        ));
        terminals.lock().insert(prefixed_id.to_string(), terminal.clone());
        (DesktopConnectionHandler::new(terminals, activity_tx), terminal)
    }

    #[test]
    fn server_owned_resize_makes_client_defer() {
        let _g = AUTH_LOCK.lock();
        let (handler, terminal) = handler_with_terminal("conn:t1");

        // Client is resize owner by default and would re-assert its own size.
        terminal.claim_resize_local();
        assert!(terminal.is_resize_owner_local());

        // The origin's local user reclaimed (server_owns=true): the client must
        // hand resize authority to the remote side and stop fighting back, so
        // the origin's reclaim sticks instead of being undone one round-trip later.
        handler.resize_terminal("conn:t1", 120, 40, true);
        assert!(!terminal.is_resize_owner_local());
    }

    #[test]
    fn client_owned_resize_keeps_client_authority() {
        let _g = AUTH_LOCK.lock();
        let (handler, terminal) = handler_with_terminal("conn:t2");

        terminal.claim_resize_local();
        assert!(terminal.is_resize_owner_local());

        // An echo of the client's own resize (server_owns=false) must NOT change
        // authority — the client keeps enforcing its window size.
        handler.resize_terminal("conn:t2", 100, 30, false);
        assert!(terminal.is_resize_owner_local());
    }
}
