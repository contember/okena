use crate::remote::types::StateResponse;
use crate::remote_client::config::RemoteConnectionConfig;
use crate::remote_client::connection::{ConnectionEvent, ConnectionStatus, RemoteConnection};
use crate::terminal::backend::TerminalBackend;
use crate::views::root::TerminalsRegistry;
use crate::workspace::settings::{load_settings, save_settings};

use gpui::*;
use std::collections::HashMap;
use std::sync::Arc;

/// GPUI Entity managing all remote connections.
///
/// Observed by the Sidebar for rendering remote projects,
/// and by RootView for focus coordination.
pub struct RemoteConnectionManager {
    connections: HashMap<String, RemoteConnection>,
    terminals: TerminalsRegistry,
    runtime: Arc<tokio::runtime::Runtime>,

    /// Channel for events coming from tokio tasks
    event_tx: async_channel::Sender<ConnectionEvent>,

    /// Currently focused remote project, if any: (connection_id, project_id)
    focused_remote: Option<(String, String)>,
}

impl RemoteConnectionManager {
    pub fn new(terminals: TerminalsRegistry, cx: &mut Context<Self>) -> Self {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .thread_name("remote-client")
                .build()
                .expect("Failed to create tokio runtime for remote client"),
        );

        let (event_tx, event_rx) = async_channel::bounded::<ConnectionEvent>(256);

        // Spawn event processing loop
        cx.spawn({
            let event_rx = event_rx.clone();
            async move |this: WeakEntity<Self>, cx| {
                while let Ok(event) = event_rx.recv().await {
                    let should_continue = this
                        .update(cx, |this, cx| {
                            this.handle_event(event, cx);
                        })
                        .is_ok();
                    if !should_continue {
                        break;
                    }
                }
            }
        })
        .detach();

        Self {
            connections: HashMap::new(),
            terminals,
            runtime,
            event_tx,
            focused_remote: None,
        }
    }

    /// Add a new connection and start connecting.
    pub fn add_connection(&mut self, config: RemoteConnectionConfig, cx: &mut Context<Self>) {
        let id = config.id.clone();
        let mut conn = RemoteConnection::new(
            config,
            self.runtime.clone(),
            self.terminals.clone(),
            self.event_tx.clone(),
        );
        conn.connect();
        self.connections.insert(id, conn);
        cx.notify();
    }

    /// Reconnect an existing connection (disconnect then connect again).
    pub fn reconnect(&mut self, connection_id: &str, cx: &mut Context<Self>) {
        if let Some(conn) = self.connections.get_mut(connection_id) {
            conn.disconnect();
            conn.connect();
            cx.notify();
        }
    }

    /// Remove a connection (disconnects first).
    pub fn remove_connection(&mut self, connection_id: &str, cx: &mut Context<Self>) {
        if let Some(mut conn) = self.connections.remove(connection_id) {
            conn.disconnect();
        }
        // Clear focused remote if it belonged to this connection
        if let Some((ref cid, _)) = self.focused_remote {
            if cid == connection_id {
                self.focused_remote = None;
            }
        }
        // Remove from saved settings
        let mut settings = load_settings();
        settings
            .remote_connections
            .retain(|c| c.id != connection_id);
        let _ = save_settings(&settings);
        cx.notify();
    }

    /// Pair with a remote server using a code.
    pub fn pair(&mut self, connection_id: &str, code: &str, cx: &mut Context<Self>) {
        if let Some(conn) = self.connections.get_mut(connection_id) {
            conn.pair(code);
            cx.notify();
        }
    }

    /// Get all connections for sidebar rendering.
    pub fn connections(
        &self,
    ) -> Vec<(
        &RemoteConnectionConfig,
        &ConnectionStatus,
        Option<&StateResponse>,
    )> {
        self.connections
            .values()
            .map(|conn| {
                (
                    &conn.config,
                    &conn.status,
                    conn.remote_state.as_ref(),
                )
            })
            .collect()
    }

    /// Get the backend for a specific connection.
    pub fn backend_for(&self, connection_id: &str) -> Option<Arc<dyn TerminalBackend>> {
        self.connections
            .get(connection_id)
            .map(|conn| conn.backend())
    }

    /// Get the remote state for a specific connection.
    pub fn remote_state(&self, connection_id: &str) -> Option<&StateResponse> {
        self.connections
            .get(connection_id)
            .and_then(|conn| conn.remote_state.as_ref())
    }

    /// Auto-connect to all saved connections with valid tokens.
    pub fn auto_connect_all(&mut self, cx: &mut Context<Self>) {
        let settings = load_settings();
        for config in settings.remote_connections {
            if config.saved_token.is_some() && !self.connections.contains_key(&config.id) {
                let id = config.id.clone();
                let mut conn = RemoteConnection::new(
                    config,
                    self.runtime.clone(),
                    self.terminals.clone(),
                    self.event_tx.clone(),
                );
                conn.connect();
                self.connections.insert(id, conn);
            }
        }
        cx.notify();
    }

    /// Get currently focused remote project.
    pub fn focused_remote(&self) -> Option<(&str, &str)> {
        self.focused_remote
            .as_ref()
            .map(|(c, p)| (c.as_str(), p.as_str()))
    }

    /// Set the focused remote project.
    pub fn set_focused_remote(
        &mut self,
        focus: Option<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        self.focused_remote = focus;
        cx.notify();
    }

    /// Handle an event from a connection's tokio task.
    fn handle_event(&mut self, event: ConnectionEvent, cx: &mut Context<Self>) {
        match event {
            ConnectionEvent::StatusChanged {
                connection_id,
                status,
            } => {
                if let Some(conn) = self.connections.get_mut(&connection_id) {
                    conn.status = status;
                }
                cx.notify();
            }
            ConnectionEvent::TokenObtained {
                connection_id,
                token,
            } => {
                if let Some(conn) = self.connections.get_mut(&connection_id) {
                    conn.config.saved_token = Some(token.clone());
                }
                // Persist token to settings
                let mut settings = load_settings();
                if let Some(saved) = settings
                    .remote_connections
                    .iter_mut()
                    .find(|c| c.id == connection_id)
                {
                    saved.saved_token = Some(token);
                }
                let _ = save_settings(&settings);
                cx.notify();
            }
            ConnectionEvent::StateReceived {
                connection_id,
                state,
            } => {
                if let Some(conn) = self.connections.get_mut(&connection_id) {
                    conn.remote_state = Some(state);
                }
                cx.notify();
            }
            ConnectionEvent::SubscriptionMappings {
                connection_id,
                mappings,
            } => {
                if let Some(conn) = self.connections.get_mut(&connection_id) {
                    conn.update_stream_mappings(mappings);
                }
            }
        }
    }
}
