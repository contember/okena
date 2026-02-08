use crate::client::manager::ConnectionManager;
use okena_core::client::{collect_state_terminal_ids, WsClientMessage};
use okena_core::keys::SpecialKey;

/// Flat FFI-friendly project info.
#[derive(Debug, Clone)]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub is_visible: bool,
    pub terminal_ids: Vec<String>,
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

/// Check if a terminal has unprocessed output (dirty flag).
#[flutter_rust_bridge::frb(sync)]
pub fn is_dirty(conn_id: String, terminal_id: String) -> bool {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| holder.is_dirty())
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

/// Get all terminal IDs from the cached remote state (flat list).
#[flutter_rust_bridge::frb(sync)]
pub fn get_all_terminal_ids(conn_id: String) -> Vec<String> {
    let mgr = ConnectionManager::get();
    match mgr.get_state(&conn_id) {
        Some(state) => collect_state_terminal_ids(&state),
        None => Vec::new(),
    }
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
