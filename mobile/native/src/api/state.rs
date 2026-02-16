use crate::client::manager::ConnectionManager;
use okena_core::api::ActionRequest;
use okena_core::client::{collect_state_terminal_ids, WsClientMessage};
use okena_core::keys::SpecialKey;
use okena_core::theme::FolderColor;

fn folder_color_to_string(color: FolderColor) -> String {
    // FolderColor serializes as lowercase string via serde (e.g. "default", "red", "blue")
    serde_json::to_value(color)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| "default".to_string())
}

/// Flat FFI-friendly project info.
#[derive(Debug, Clone)]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub is_visible: bool,
    pub terminal_ids: Vec<String>,
    pub folder_color: String,
}

/// FFI-friendly folder info.
#[derive(Debug, Clone)]
pub struct FolderInfo {
    pub id: String,
    pub name: String,
    pub project_ids: Vec<String>,
    pub folder_color: String,
}

/// Get all projects from the cached remote state.
#[flutter_rust_bridge::frb(sync)]
pub fn get_projects(conn_id: String) -> Vec<ProjectInfo> {
    let mgr = ConnectionManager::get();
    let state = match mgr.get_state(&conn_id) {
        Some(s) => s,
        None => return Vec::new(),
    };

    state
        .projects
        .iter()
        .map(|p| {
            let terminal_ids = if let Some(ref layout) = p.layout {
                let mut ids = Vec::new();
                collect_layout_ids(layout, &mut ids);
                ids
            } else {
                Vec::new()
            };
            ProjectInfo {
                id: p.id.clone(),
                name: p.name.clone(),
                path: p.path.clone(),
                is_visible: p.is_visible,
                terminal_ids,
                folder_color: folder_color_to_string(p.folder_color),
            }
        })
        .collect()
}

/// Get the focused project ID from the cached remote state.
#[flutter_rust_bridge::frb(sync)]
pub fn get_focused_project_id(conn_id: String) -> Option<String> {
    let mgr = ConnectionManager::get();
    mgr.get_state(&conn_id)
        .and_then(|s| s.focused_project_id.clone())
}

/// Get all folders from the cached remote state.
#[flutter_rust_bridge::frb(sync)]
pub fn get_folders(conn_id: String) -> Vec<FolderInfo> {
    let mgr = ConnectionManager::get();
    let state = match mgr.get_state(&conn_id) {
        Some(s) => s,
        None => return Vec::new(),
    };

    state
        .folders
        .iter()
        .map(|f| FolderInfo {
            id: f.id.clone(),
            name: f.name.clone(),
            project_ids: f.project_ids.clone(),
            folder_color: folder_color_to_string(f.folder_color),
        })
        .collect()
}

/// Get the project order from the cached remote state.
#[flutter_rust_bridge::frb(sync)]
pub fn get_project_order(conn_id: String) -> Vec<String> {
    let mgr = ConnectionManager::get();
    mgr.get_state(&conn_id)
        .map(|s| s.project_order.clone())
        .unwrap_or_default()
}

/// Check if a terminal has unprocessed output (dirty flag).
#[flutter_rust_bridge::frb(sync)]
pub fn is_dirty(conn_id: String, terminal_id: String) -> bool {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| holder.take_dirty())
        .unwrap_or(false)
}

/// Send a special key (e.g. "Enter", "Tab", "Escape") to a terminal.
///
/// The key name is deserialized from JSON (e.g. `"Enter"`, `"CtrlC"`, `"ArrowUp"`).
pub async fn send_special_key(
    conn_id: String,
    terminal_id: String,
    key: String,
) -> anyhow::Result<()> {
    let special_key: SpecialKey = serde_json::from_value(serde_json::Value::String(key.clone()))
        .map_err(|_| anyhow::anyhow!("Unknown special key: {}", key))?;
    let text = String::from_utf8_lossy(special_key.to_bytes()).to_string();
    let mgr = ConnectionManager::get();
    mgr.send_ws_message(
        &conn_id,
        WsClientMessage::SendText {
            terminal_id,
            text,
        },
    );
    Ok(())
}

/// Get a project's layout tree as JSON.
///
/// Returns the `ApiLayoutNode` serialized as JSON, or `None` if the project
/// has no layout. Using JSON avoids complex recursive enum FRB codegen.
#[flutter_rust_bridge::frb(sync)]
pub fn get_project_layout_json(conn_id: String, project_id: String) -> Option<String> {
    let mgr = ConnectionManager::get();
    let state = mgr.get_state(&conn_id)?;
    let project = state.projects.iter().find(|p| p.id == project_id)?;
    let layout = project.layout.as_ref()?;
    serde_json::to_string(layout).ok()
}

/// Get all terminal IDs from the cached remote state (flat list).
#[flutter_rust_bridge::frb(sync)]
pub fn get_all_terminal_ids(conn_id: String) -> Vec<String> {
    let mgr = ConnectionManager::get();
    match mgr.get_state(&conn_id) {
        Some(state) => collect_state_terminal_ids(&state),
        None => Vec::new(),
    }
}

/// Server terminal size returned via FFI.
#[derive(Debug, Clone)]
pub struct ServerTerminalSize {
    pub cols: u16,
    pub rows: u16,
}

/// Get the server-side terminal size from the cached state.
/// Returns (0, 0) if the terminal is not found or size is not available.
#[flutter_rust_bridge::frb(sync)]
pub fn get_server_terminal_size(conn_id: String, terminal_id: String) -> ServerTerminalSize {
    let mgr = ConnectionManager::get();
    let state = match mgr.get_state(&conn_id) {
        Some(s) => s,
        None => return ServerTerminalSize { cols: 0, rows: 0 },
    };

    for project in &state.projects {
        if let Some(ref layout) = project.layout {
            if let Some(size) = find_terminal_size(layout, &terminal_id) {
                return size;
            }
        }
    }

    ServerTerminalSize { cols: 0, rows: 0 }
}

fn find_terminal_size(
    node: &okena_core::api::ApiLayoutNode,
    target_id: &str,
) -> Option<ServerTerminalSize> {
    match node {
        okena_core::api::ApiLayoutNode::Terminal {
            terminal_id,
            cols,
            rows,
            ..
        } => {
            if terminal_id.as_deref() == Some(target_id) {
                match (cols, rows) {
                    (Some(c), Some(r)) => Some(ServerTerminalSize { cols: *c, rows: *r }),
                    _ => None,
                }
            } else {
                None
            }
        }
        okena_core::api::ApiLayoutNode::Split { children, .. }
        | okena_core::api::ApiLayoutNode::Tabs { children, .. } => {
            for child in children {
                if let Some(size) = find_terminal_size(child, target_id) {
                    return Some(size);
                }
            }
            None
        }
    }
}

/// Create a new terminal in a project.
pub async fn create_terminal(conn_id: String, project_id: String) -> anyhow::Result<()> {
    let mgr = ConnectionManager::get();
    mgr.execute_action(&conn_id, ActionRequest::CreateTerminal { project_id })
        .await?;
    Ok(())
}

/// Close a terminal in a project.
pub async fn close_terminal(
    conn_id: String,
    project_id: String,
    terminal_id: String,
) -> anyhow::Result<()> {
    let mgr = ConnectionManager::get();
    mgr.execute_action(
        &conn_id,
        ActionRequest::CloseTerminal {
            project_id,
            terminal_id,
        },
    )
    .await?;
    Ok(())
}

/// Focus a terminal in a project.
pub async fn focus_terminal(
    conn_id: String,
    project_id: String,
    terminal_id: String,
) -> anyhow::Result<()> {
    let mgr = ConnectionManager::get();
    mgr.execute_action(
        &conn_id,
        ActionRequest::FocusTerminal {
            project_id,
            terminal_id,
        },
    )
    .await?;
    Ok(())
}

fn collect_layout_ids(node: &okena_core::api::ApiLayoutNode, ids: &mut Vec<String>) {
    match node {
        okena_core::api::ApiLayoutNode::Terminal { terminal_id, .. } => {
            if let Some(id) = terminal_id {
                ids.push(id.clone());
            }
        }
        okena_core::api::ApiLayoutNode::Split { children, .. }
        | okena_core::api::ApiLayoutNode::Tabs { children, .. } => {
            for child in children {
                collect_layout_ids(child, ids);
            }
        }
    }
}
