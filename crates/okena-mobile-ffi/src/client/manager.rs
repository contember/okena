use crate::client::handler::MobileConnectionHandler;
use crate::client::terminal_holder::TerminalHolder;

use okena_core::api::{ActionRequest, ApiFullscreen, ApiLayoutNode, StateResponse};
use okena_transport::client::{
    ConnectionEvent, ConnectionStatus, RemoteClient, RemoteConnectionConfig, WsClientMessage,
    make_prefixed_id,
};
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

static MANAGER: OnceLock<ConnectionManager> = OnceLock::new();

pub struct ConnectionManager {
    runtime: Arc<tokio::runtime::Runtime>,
    connections: RwLock<HashMap<String, MobileConnection>>,
}

struct MobileConnection {
    client: RwLock<RemoteClient<MobileConnectionHandler>>,
    handler: Arc<MobileConnectionHandler>,
    status: RwLock<ConnectionStatus>,
    state_cache: RwLock<Option<StateResponse>>,
    _event_task: Option<tokio::task::JoinHandle<()>>,
}

fn merge_layout_presentation(server: &ApiLayoutNode, local: &ApiLayoutNode) -> ApiLayoutNode {
    let mut merged = merge_layout_structure(server, local);
    let mut presentation = HashMap::new();
    collect_terminal_presentation(local, &mut presentation);
    apply_terminal_presentation(&mut merged, &presentation);
    merged
}

fn merge_layout_structure(server: &ApiLayoutNode, local: &ApiLayoutNode) -> ApiLayoutNode {
    match (server, local) {
        (ApiLayoutNode::Terminal { .. }, _) => server.clone(),
        (
            ApiLayoutNode::Split {
                direction,
                sizes: server_sizes,
                children: server_children,
            },
            ApiLayoutNode::Split {
                direction: local_direction,
                sizes: local_sizes,
                children: local_children,
            },
        ) if direction == local_direction => {
            let mapping = matching_api_child_indices(server_children, local_children);
            let children =
                merge_mapped_api_children(server_children, local_children, mapping.as_deref());
            let sizes = mapping
                .filter(|indices| local_sizes.len() == indices.len())
                .map(|indices| {
                    indices
                        .into_iter()
                        .map(|index| local_sizes[index])
                        .collect()
                })
                .unwrap_or_else(|| server_sizes.clone());
            ApiLayoutNode::Split {
                direction: *direction,
                sizes,
                children,
            }
        }
        (
            ApiLayoutNode::Tabs {
                children: server_children,
                active_tab: server_active,
            },
            ApiLayoutNode::Tabs {
                children: local_children,
                active_tab: local_active,
            },
        ) => {
            let mapping = matching_api_child_indices(server_children, local_children);
            ApiLayoutNode::Tabs {
                children: merge_mapped_api_children(
                    server_children,
                    local_children,
                    mapping.as_deref(),
                ),
                active_tab: merged_api_active_tab(
                    server_children,
                    *server_active,
                    local_children,
                    *local_active,
                ),
            }
        }
        _ => server.clone(),
    }
}

fn merge_mapped_api_children(
    server: &[ApiLayoutNode],
    local: &[ApiLayoutNode],
    mapping: Option<&[usize]>,
) -> Vec<ApiLayoutNode> {
    server
        .iter()
        .enumerate()
        .map(|(server_index, server_child)| {
            mapping
                .and_then(|indices| indices.get(server_index))
                .and_then(|local_index| local.get(*local_index))
                .map(|local_child| merge_layout_structure(server_child, local_child))
                .unwrap_or_else(|| server_child.clone())
        })
        .collect()
}

fn matching_api_child_indices(
    server: &[ApiLayoutNode],
    local: &[ApiLayoutNode],
) -> Option<Vec<usize>> {
    let server_ids: Vec<HashSet<String>> = server
        .iter()
        .map(|child| child.collect_terminal_ids().into_iter().collect())
        .collect();
    let local_ids: Vec<HashSet<String>> = local
        .iter()
        .map(|child| child.collect_terminal_ids().into_iter().collect())
        .collect();

    let mut used = HashSet::new();
    let exact: Option<Vec<usize>> = server_ids
        .iter()
        .map(|ids| {
            if ids.is_empty() {
                return None;
            }
            let index = local_ids
                .iter()
                .enumerate()
                .find(|(index, candidate)| !used.contains(index) && *candidate == ids)
                .map(|(index, _)| index)?;
            used.insert(index);
            Some(index)
        })
        .collect();
    if exact.is_some() {
        return exact;
    }

    server_ids
        .iter()
        .zip(&local_ids)
        .all(|(server, local)| {
            (server.is_empty() && local.is_empty()) || server.iter().any(|id| local.contains(id))
        })
        .then(|| (0..server.len()).collect())
}

fn merged_api_active_tab(
    server_children: &[ApiLayoutNode],
    server_active: usize,
    local_children: &[ApiLayoutNode],
    local_active: usize,
) -> usize {
    let fallback = server_active.min(server_children.len().saturating_sub(1));
    let Some(local_child) = local_children.get(local_active) else {
        return fallback;
    };
    let selected_ids: HashSet<String> = local_child.collect_terminal_ids().into_iter().collect();
    if selected_ids.is_empty() {
        return local_active.min(server_children.len().saturating_sub(1));
    }

    server_children
        .iter()
        .position(|child| {
            child
                .collect_terminal_ids()
                .iter()
                .any(|id| selected_ids.contains(id))
        })
        .unwrap_or(fallback)
}

fn collect_terminal_presentation(
    layout: &ApiLayoutNode,
    presentation: &mut HashMap<String, (bool, bool)>,
) {
    match layout {
        ApiLayoutNode::Terminal {
            terminal_id: Some(id),
            minimized,
            detached,
            ..
        } => {
            presentation.insert(id.clone(), (*minimized, *detached));
        }
        ApiLayoutNode::Split { children, .. } | ApiLayoutNode::Tabs { children, .. } => {
            for child in children {
                collect_terminal_presentation(child, presentation);
            }
        }
        ApiLayoutNode::Terminal {
            terminal_id: None, ..
        } => {}
    }
}

fn apply_terminal_presentation(
    layout: &mut ApiLayoutNode,
    presentation: &HashMap<String, (bool, bool)>,
) {
    match layout {
        ApiLayoutNode::Terminal {
            terminal_id: Some(id),
            minimized,
            detached,
            ..
        } => {
            if let Some(&(local_minimized, local_detached)) = presentation.get(id) {
                *minimized = local_minimized;
                *detached = local_detached;
            }
        }
        ApiLayoutNode::Split { children, .. } | ApiLayoutNode::Tabs { children, .. } => {
            for child in children {
                apply_terminal_presentation(child, presentation);
            }
        }
        ApiLayoutNode::Terminal {
            terminal_id: None, ..
        } => {}
    }
}

fn merge_state_presentation(next: &mut StateResponse, previous: &StateResponse) {
    for project in &mut next.projects {
        let Some(previous_project) = previous.projects.iter().find(|old| old.id == project.id)
        else {
            continue;
        };
        if let (Some(server), Some(local)) = (&project.layout, &previous_project.layout) {
            project.layout = Some(merge_layout_presentation(server, local));
        }
    }

    next.fullscreen_terminal = previous
        .fullscreen_terminal
        .as_ref()
        .and_then(|fullscreen| {
            let terminal_exists = next.projects.iter().any(|project| {
                project.id == fullscreen.project_id
                    && project.layout.as_ref().is_some_and(|layout| {
                        layout_contains_terminal(layout, &fullscreen.terminal_id)
                    })
            });
            terminal_exists.then(|| fullscreen.clone())
        });
}

fn layout_contains_terminal(layout: &ApiLayoutNode, terminal_id: &str) -> bool {
    match layout {
        ApiLayoutNode::Terminal {
            terminal_id: Some(id),
            ..
        } => id == terminal_id,
        ApiLayoutNode::Split { children, .. } | ApiLayoutNode::Tabs { children, .. } => children
            .iter()
            .any(|child| layout_contains_terminal(child, terminal_id)),
        ApiLayoutNode::Terminal {
            terminal_id: None, ..
        } => false,
    }
}

fn toggle_terminal_minimized(layout: &mut ApiLayoutNode, terminal_id: &str) -> bool {
    match layout {
        ApiLayoutNode::Terminal {
            terminal_id: Some(id),
            minimized,
            ..
        } if id == terminal_id => {
            *minimized = !*minimized;
            true
        }
        ApiLayoutNode::Split { children, .. } | ApiLayoutNode::Tabs { children, .. } => children
            .iter_mut()
            .any(|child| toggle_terminal_minimized(child, terminal_id)),
        ApiLayoutNode::Terminal { .. } => false,
    }
}

fn layout_at_path_mut<'a>(
    mut layout: &'a mut ApiLayoutNode,
    path: &[usize],
) -> Option<&'a mut ApiLayoutNode> {
    for &index in path {
        layout = match layout {
            ApiLayoutNode::Split { children, .. } | ApiLayoutNode::Tabs { children, .. } => {
                children.get_mut(index)?
            }
            ApiLayoutNode::Terminal { .. } => return None,
        };
    }
    Some(layout)
}

impl ConnectionManager {
    /// Initialize the global singleton. Call once at app startup.
    pub fn init() {
        MANAGER.get_or_init(|| {
            #[allow(
                clippy::expect_used,
                reason = "tokio runtime must start for the mobile app to function; abort on failure"
            )]
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime");

            ConnectionManager {
                runtime: Arc::new(runtime),
                connections: RwLock::new(HashMap::new()),
            }
        });
    }

    /// Get the global singleton. Panics if `init()` hasn't been called.
    #[allow(
        clippy::expect_used,
        reason = "invariant: init() is called at app startup before any FFI access"
    )]
    pub fn get() -> &'static ConnectionManager {
        MANAGER.get().expect("ConnectionManager not initialized")
    }

    /// Tear down a connection and remove it from the map.
    ///
    /// Removing the `MobileConnection` drops its `RemoteClient`, whose `Drop`
    /// aborts the background WS task and closes the event channel. We also call
    /// `disconnect()` first (idempotent) to remove this connection's terminals,
    /// and explicitly abort the `_event_task` JoinHandle. Closing the event
    /// channel (via the dropped `RemoteClient`'s `event_tx`) already unblocks
    /// `process_events`, but aborting is a belt-and-suspenders cleanup.
    ///
    /// Removing a non-existent id is a no-op.
    pub fn remove_connection(&self, conn_id: &str) {
        // Take ownership out of the map under the write lock, then release the
        // lock before running teardown/Drop so we never hold the connections
        // write lock across the per-connection client lock or the task abort.
        let connection = self.connections.write().remove(conn_id);

        let Some(connection) = connection else {
            return;
        };

        // Abort WS task + drop terminals for this connection (idempotent).
        connection.client.write().disconnect();

        // Abort the event-processor task. `process_events` also exits on its own
        // once the entry is gone from the map and the event channel is closed by
        // the dropped RemoteClient, so this is just immediate cleanup.
        if let Some(task) = connection._event_task.as_ref() {
            task.abort();
        }

        // `connection` is dropped here, dropping the RemoteClient (whose Drop
        // aborts the WS task again, harmlessly) and the JoinHandle.
    }

    /// Create a new connection and return its ID.
    ///
    /// If a connection already exists for the same `host:port`, it is torn down
    /// and removed first so that reconnecting to the same server replaces the
    /// stale entry rather than accumulating a new one (which would leak the old
    /// RemoteClient, its WS task, and its event-processor task).
    pub fn add_connection(
        &self,
        host: &str,
        port: u16,
        saved_token: Option<String>,
        tls: bool,
        pinned_cert_sha256: Option<String>,
    ) -> String {
        // Replace any existing connection targeting the same server. We collect
        // matching ids first (read lock), then remove them (which takes its own
        // write lock) — never holding a lock across the teardown.
        let stale_ids: Vec<String> = {
            let connections = self.connections.read();
            connections
                .iter()
                .filter(|(_, conn)| {
                    let cfg = conn.client.read();
                    let cfg = cfg.config();
                    cfg.host == host && cfg.port == port
                })
                .map(|(id, _)| id.clone())
                .collect()
        };
        for id in stale_ids {
            self.remove_connection(&id);
        }

        let config = RemoteConnectionConfig {
            id: uuid::Uuid::new_v4().to_string(),
            name: format!("{}:{}", host, port),
            host: host.to_string(),
            port,
            saved_token,
            token_obtained_at: None,
            tls,
            pinned_cert_sha256,
            local_endpoint: None,
        };
        let conn_id = config.id.clone();

        let terminals: Arc<RwLock<HashMap<String, TerminalHolder>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let handler = Arc::new(MobileConnectionHandler::new(terminals));

        let (event_tx, event_rx) = async_channel::bounded::<ConnectionEvent>(256);

        let client = RemoteClient::new(config, self.runtime.clone(), handler.clone(), event_tx);

        // Spawn event processor task
        let conn_id_clone = conn_id.clone();
        let status = RwLock::new(ConnectionStatus::Disconnected);
        let state_cache = RwLock::new(None);

        let connection = MobileConnection {
            client: RwLock::new(client),
            handler,
            status,
            state_cache,
            _event_task: None,
        };

        self.connections.write().insert(conn_id.clone(), connection);

        // Spawn event processor
        let event_task = self
            .runtime
            .spawn(Self::process_events(conn_id_clone.clone(), event_rx));

        // Store the task handle
        if let Some(conn) = self.connections.write().get_mut(&conn_id) {
            conn._event_task = Some(event_task);
        }

        conn_id
    }

    /// Start connecting to the remote server.
    pub fn connect(&self, conn_id: &str) {
        let connections = self.connections.read();
        if let Some(conn) = connections.get(conn_id) {
            conn.client.write().connect();
        }
    }

    /// Pair with the remote server using a pairing code.
    pub fn pair(&self, conn_id: &str, code: &str) {
        let connections = self.connections.read();
        if let Some(conn) = connections.get(conn_id) {
            conn.client.write().pair(code);
        }
    }

    /// Disconnect from the remote server.
    pub fn disconnect(&self, conn_id: &str) {
        let connections = self.connections.read();
        if let Some(conn) = connections.get(conn_id) {
            conn.client.write().disconnect();
            *conn.status.write() = ConnectionStatus::Disconnected;
            *conn.state_cache.write() = None;
        }
    }

    /// Get the current connection status.
    pub fn get_status(&self, conn_id: &str) -> ConnectionStatus {
        let connections = self.connections.read();
        if let Some(conn) = connections.get(conn_id) {
            conn.status.read().clone()
        } else {
            ConnectionStatus::Disconnected
        }
    }

    /// Get the current auth token for a connection.
    pub fn get_token(&self, conn_id: &str) -> Option<String> {
        let connections = self.connections.read();
        connections
            .get(conn_id)
            .and_then(|conn| conn.client.read().config().saved_token.clone())
    }

    /// Get the cached remote state.
    pub fn get_state(&self, conn_id: &str) -> Option<StateResponse> {
        let connections = self.connections.read();
        connections
            .get(conn_id)
            .and_then(|conn| conn.state_cache.read().clone())
    }

    pub fn toggle_minimized_local(
        &self,
        conn_id: &str,
        project_id: &str,
        terminal_id: &str,
    ) -> Result<(), String> {
        let connections = self.connections.read();
        let connection = connections
            .get(conn_id)
            .ok_or_else(|| format!("Connection not found: {conn_id}"))?;
        let mut state = connection.state_cache.write();
        let project = state
            .as_mut()
            .and_then(|state| {
                state
                    .projects
                    .iter_mut()
                    .find(|project| project.id == project_id)
            })
            .ok_or_else(|| format!("Project not found: {project_id}"))?;
        let changed = project
            .layout
            .as_mut()
            .is_some_and(|layout| toggle_terminal_minimized(layout, terminal_id));
        if changed {
            Ok(())
        } else {
            Err(format!("Terminal not found: {terminal_id}"))
        }
    }

    pub fn set_fullscreen_local(
        &self,
        conn_id: &str,
        project_id: &str,
        terminal_id: Option<String>,
    ) -> Result<(), String> {
        let connections = self.connections.read();
        let connection = connections
            .get(conn_id)
            .ok_or_else(|| format!("Connection not found: {conn_id}"))?;
        let mut state = connection.state_cache.write();
        let state = state
            .as_mut()
            .ok_or_else(|| format!("State unavailable for connection: {conn_id}"))?;

        state.fullscreen_terminal = match terminal_id {
            Some(terminal_id) => {
                let exists = state.projects.iter().any(|project| {
                    project.id == project_id
                        && project
                            .layout
                            .as_ref()
                            .is_some_and(|layout| layout_contains_terminal(layout, &terminal_id))
                });
                if !exists {
                    return Err(format!("Terminal not found: {terminal_id}"));
                }
                Some(ApiFullscreen {
                    project_id: project_id.to_string(),
                    terminal_id,
                })
            }
            None => None,
        };
        Ok(())
    }

    pub fn set_active_tab_local(
        &self,
        conn_id: &str,
        project_id: &str,
        path: &[usize],
        index: usize,
    ) -> Result<(), String> {
        let connections = self.connections.read();
        let connection = connections
            .get(conn_id)
            .ok_or_else(|| format!("Connection not found: {conn_id}"))?;
        let mut state = connection.state_cache.write();
        let layout = state
            .as_mut()
            .and_then(|state| {
                state
                    .projects
                    .iter_mut()
                    .find(|project| project.id == project_id)
            })
            .and_then(|project| project.layout.as_mut())
            .and_then(|layout| layout_at_path_mut(layout, path))
            .ok_or_else(|| format!("Tab group not found at path: {path:?}"))?;
        let ApiLayoutNode::Tabs {
            children,
            active_tab,
        } = layout
        else {
            return Err(format!("Layout at path is not a tab group: {path:?}"));
        };
        if index >= children.len() {
            return Err(format!("Tab index out of range: {index}"));
        }
        *active_tab = index;
        Ok(())
    }

    /// Access a terminal holder for reading cells / cursor.
    /// The callback receives the TerminalHolder if found.
    pub fn with_terminal<F, R>(&self, conn_id: &str, terminal_id: &str, f: F) -> Option<R>
    where
        F: FnOnce(&TerminalHolder) -> R,
    {
        let connections = self.connections.read();
        let conn = connections.get(conn_id)?;
        let prefixed_id = make_prefixed_id(conn_id, terminal_id);
        let terminals = conn.handler.terminals().read();
        let holder = terminals.get(&prefixed_id)?;
        Some(f(holder))
    }

    /// Get seconds since last WS activity for a connection.
    pub fn seconds_since_activity(&self, conn_id: &str) -> f64 {
        let connections = self.connections.read();
        connections
            .get(conn_id)
            .map(|conn| conn.handler.seconds_since_activity())
            .unwrap_or(f64::MAX)
    }

    /// Send a WebSocket message for a connection.
    pub fn send_ws_message(&self, conn_id: &str, msg: WsClientMessage) {
        let connections = self.connections.read();
        if let Some(conn) = connections.get(conn_id) {
            let client = conn.client.read();
            if let Some(sender) = client.ws_sender() {
                let _ = sender.try_send(msg);
            }
        }
    }

    /// Resize a terminal holder and send the resize message to the server.
    pub fn resize_terminal(&self, conn_id: &str, terminal_id: &str, cols: u16, rows: u16) {
        let connections = self.connections.read();
        if let Some(conn) = connections.get(conn_id) {
            let prefixed_id = make_prefixed_id(conn_id, terminal_id);
            let terminals = conn.handler.terminals().read();
            if let Some(holder) = terminals.get(&prefixed_id) {
                holder.resize(cols, rows);
            }
        }
        // Also send WS resize message
        self.send_ws_message(
            conn_id,
            WsClientMessage::Resize {
                terminal_id: terminal_id.to_string(),
                cols,
                rows,
            },
        );
    }

    /// Send an action to the remote server via POST /v1/actions.
    pub async fn send_action(&self, conn_id: &str, action: ActionRequest) -> anyhow::Result<()> {
        self.send_action_with_response(conn_id, action).await?;
        Ok(())
    }

    /// Send an action to the remote server and return the response body.
    pub async fn send_action_with_response(
        &self,
        conn_id: &str,
        action: ActionRequest,
    ) -> anyhow::Result<String> {
        let (config, token) = {
            let connections = self.connections.read();
            let conn = connections
                .get(conn_id)
                .ok_or_else(|| anyhow::anyhow!("Connection not found: {}", conn_id))?;
            let config = conn.client.read().config().clone();
            let token = config
                .effective_auth_token()
                .ok_or_else(|| anyhow::anyhow!("No auth token for connection: {}", conn_id))?;
            (config, token)
        };

        let response = okena_transport::remote_action::post_action_async(&config, &token, action)
            .await
            .map_err(anyhow::Error::msg)?;
        match response {
            Some(value) => Ok(serde_json::to_string(&value)?),
            None => Ok(String::new()),
        }
    }

    /// Background task that drains the event channel and updates connection state.
    async fn process_events(conn_id: String, event_rx: async_channel::Receiver<ConnectionEvent>) {
        while let Ok(event) = event_rx.recv().await {
            let mgr = match MANAGER.get() {
                Some(m) => m,
                None => break,
            };
            let connections = mgr.connections.read();
            let conn = match connections.get(&conn_id) {
                Some(c) => c,
                None => break,
            };

            match event {
                ConnectionEvent::StatusChanged { status, .. } => {
                    *conn.status.write() = status;
                }
                ConnectionEvent::TokenObtained {
                    token,
                    cert_fingerprint,
                    ..
                } => {
                    conn.client.write().config_mut().saved_token = Some(token.clone());
                    conn.client.write().config_mut().token_obtained_at = Some(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs() as i64,
                    );
                    // Pin the cert on first successful TLS pairing (TOFU).
                    if cert_fingerprint.is_some() {
                        conn.client.write().config_mut().pinned_cert_sha256 =
                            cert_fingerprint.clone();
                    }
                }
                ConnectionEvent::TlsUpgraded {
                    cert_fingerprint, ..
                } => {
                    let mut client = conn.client.write();
                    client.config_mut().tls = true;
                    client.config_mut().pinned_cert_sha256 = cert_fingerprint.clone();
                }
                ConnectionEvent::TokenRefreshed { token, .. } => {
                    conn.client.read().update_shared_token(&token);
                    conn.client.write().config_mut().saved_token = Some(token.clone());
                    conn.client.write().config_mut().token_obtained_at = Some(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs() as i64,
                    );
                }
                ConnectionEvent::StateReceived { mut state, .. } => {
                    let mut cache = conn.state_cache.write();
                    if let Some(previous) = cache.as_ref() {
                        merge_state_presentation(&mut state, previous);
                    } else {
                        state.fullscreen_terminal = None;
                    }
                    *cache = Some(state);
                }
                ConnectionEvent::SettingsChanged { .. } => {}
                ConnectionEvent::SubscriptionMappings { mappings, .. } => {
                    conn.client.write().update_stream_mappings(mappings);
                }
                ConnectionEvent::GitStatusChanged { statuses, .. } => {
                    if let Some(state) = conn.state_cache.write().as_mut() {
                        for project in &mut state.projects {
                            project.git_status = statuses.get(&project.id).cloned();
                        }
                    }
                }
                ConnectionEvent::SystemStatsChanged { .. }
                | ConnectionEvent::TerminalFocusRequested { .. } => {}
                ConnectionEvent::ServerWarning { message, .. } => {
                    log::warn!("Server warning for {}: {}", conn_id, message);
                }
                ConnectionEvent::Toast { toast, .. } => {
                    // The mobile bridge has no toast surface yet; surfacing these
                    // in the RN UI would need a dedicated FFI callback. Log for now
                    // so daemon-originated toasts are at least observable.
                    log::info!("Toast for {} [{}]: {}", conn_id, toast.level, toast.message);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_core::api::ApiProject;
    use okena_core::shell::ShellType;
    use okena_core::types::SplitDirection;

    fn terminal(id: &str, minimized: bool, shell_type: ShellType) -> ApiLayoutNode {
        ApiLayoutNode::Terminal {
            terminal_id: Some(id.to_string()),
            minimized,
            detached: false,
            shell_type,
            cols: None,
            rows: None,
        }
    }

    fn project(id: &str, layout: ApiLayoutNode) -> ApiProject {
        ApiProject {
            id: id.to_string(),
            name: id.to_string(),
            path: "/tmp".to_string(),
            show_in_overview: true,
            layout: Some(layout),
            terminal_names: HashMap::new(),
            git_status: None,
            folder_color: Default::default(),
            services: Vec::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            pinned: false,
            last_activity_at: None,
            default_shell: None,
            hook_terminals: Vec::new(),
            hooks: Default::default(),
            is_creating: false,
            is_closing: false,
        }
    }

    fn state(projects: Vec<ApiProject>) -> StateResponse {
        StateResponse {
            state_version: 1,
            projects,
            focused_project_id: None,
            fullscreen_terminal: None,
            project_order: Vec::new(),
            folders: Vec::new(),
            windows: Vec::new(),
            hooks: Vec::new(),
        }
    }

    #[test]
    fn mobile_layout_merge_keeps_local_presentation_and_server_shell() {
        let server = ApiLayoutNode::Tabs {
            children: vec![
                terminal(
                    "one",
                    false,
                    ShellType::Custom {
                        path: "/bin/fish".to_string(),
                        args: Vec::new(),
                    },
                ),
                terminal("two", false, ShellType::Default),
            ],
            active_tab: 0,
        };
        let local = ApiLayoutNode::Tabs {
            children: vec![
                terminal("one", true, ShellType::Default),
                terminal("two", false, ShellType::Default),
            ],
            active_tab: 1,
        };

        let merged = merge_layout_presentation(&server, &local);
        let ApiLayoutNode::Tabs {
            children,
            active_tab,
        } = merged
        else {
            panic!("expected tabs");
        };
        assert_eq!(active_tab, 1);
        let ApiLayoutNode::Terminal {
            minimized,
            shell_type,
            ..
        } = &children[0]
        else {
            panic!("expected terminal");
        };
        assert!(*minimized);
        assert_eq!(
            shell_type,
            &ShellType::Custom {
                path: "/bin/fish".to_string(),
                args: Vec::new(),
            }
        );
    }

    #[test]
    fn mobile_layout_merge_follows_reordered_terminal_identity() {
        let server = ApiLayoutNode::Tabs {
            children: vec![
                terminal("two", false, ShellType::Default),
                terminal("one", false, ShellType::Default),
            ],
            active_tab: 1,
        };
        let local = ApiLayoutNode::Tabs {
            children: vec![
                terminal("one", false, ShellType::Default),
                terminal("two", true, ShellType::Default),
            ],
            active_tab: 0,
        };

        let merged = merge_layout_presentation(&server, &local);
        let ApiLayoutNode::Tabs {
            children,
            active_tab,
        } = merged
        else {
            panic!("expected tabs");
        };
        assert_eq!(active_tab, 1);
        assert!(matches!(
            &children[0],
            ApiLayoutNode::Terminal {
                terminal_id: Some(id),
                minimized: true,
                ..
            } if id == "two"
        ));
    }

    #[test]
    fn mobile_layout_merge_does_not_reuse_sizes_across_direction_change() {
        let server = ApiLayoutNode::Split {
            direction: SplitDirection::Vertical,
            sizes: vec![40.0, 60.0],
            children: vec![
                terminal("one", false, ShellType::Default),
                terminal("two", false, ShellType::Default),
            ],
        };
        let local = ApiLayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![25.0, 75.0],
            children: vec![
                terminal("one", false, ShellType::Default),
                terminal("two", false, ShellType::Default),
            ],
        };

        let merged = merge_layout_presentation(&server, &local);
        let ApiLayoutNode::Split { sizes, .. } = merged else {
            panic!("expected split");
        };
        assert_eq!(sizes, vec![40.0, 60.0]);
    }

    #[test]
    fn mobile_fullscreen_survives_resync_only_while_terminal_exists() {
        let mut previous = state(vec![project(
            "project",
            terminal("terminal", false, ShellType::Default),
        )]);
        previous.fullscreen_terminal = Some(ApiFullscreen {
            project_id: "project".to_string(),
            terminal_id: "terminal".to_string(),
        });

        let mut next = state(vec![project(
            "project",
            terminal("terminal", false, ShellType::Default),
        )]);
        merge_state_presentation(&mut next, &previous);
        assert!(next.fullscreen_terminal.is_some());

        let mut without_terminal = state(vec![project(
            "project",
            terminal("replacement", false, ShellType::Default),
        )]);
        merge_state_presentation(&mut without_terminal, &previous);
        assert!(without_terminal.fullscreen_terminal.is_none());
    }
}
