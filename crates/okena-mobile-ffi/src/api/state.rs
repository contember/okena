//! Cached-state extraction into plain, FFI-friendly structs.
//!
//! Only the read-side accessors that `lib.rs` delegates to live here. Every
//! mutating action (terminal / git / service / project / layout) is exported
//! directly from `lib.rs` via uniffi against `ConnectionManager`, so it is not
//! duplicated here. The uniffi mirrors of these structs (with
//! `#[derive(uniffi::Record)]`) live in `crate::types`.

use std::collections::HashMap;

use crate::client::manager::ConnectionManager;
use okena_core::api::ApiLayoutNode;

/// Flat FFI-friendly project info.
#[derive(Debug, Clone)]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub show_in_overview: bool,
    pub terminal_ids: Vec<String>,
    pub terminal_names: HashMap<String, String>,
    pub git_branch: Option<String>,
    pub git_lines_added: u32,
    pub git_lines_removed: u32,
    pub services: Vec<ServiceInfo>,
    pub folder_color: String,
}

/// FFI-friendly service info.
#[derive(Debug, Clone)]
pub struct ServiceInfo {
    pub name: String,
    pub status: String,
    pub terminal_id: Option<String>,
    pub ports: Vec<u16>,
    pub exit_code: Option<u32>,
    pub kind: String,
    pub is_extra: bool,
}

/// FFI-friendly folder info.
#[derive(Debug, Clone)]
pub struct FolderInfo {
    pub id: String,
    pub name: String,
    pub project_ids: Vec<String>,
    pub folder_color: String,
}

/// FFI-friendly fullscreen info.
#[derive(Debug, Clone)]
pub struct FullscreenInfo {
    pub project_id: String,
    pub terminal_id: String,
}

/// Get all projects from the cached remote state.
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
                collect_layout_ids_vec(layout, &mut ids);
                ids
            } else {
                Vec::new()
            };
            let (git_branch, git_lines_added, git_lines_removed) =
                if let Some(ref gs) = p.git_status {
                    (gs.branch.clone(), gs.lines_added as u32, gs.lines_removed as u32)
                } else {
                    (None, 0, 0)
                };
            let services = p
                .services
                .iter()
                .map(|s| ServiceInfo {
                    name: s.name.clone(),
                    status: s.status.clone(),
                    terminal_id: s.terminal_id.clone(),
                    ports: s.ports.clone(),
                    exit_code: s.exit_code,
                    kind: s.kind.clone(),
                    is_extra: s.is_extra,
                })
                .collect();
            ProjectInfo {
                id: p.id.clone(),
                name: p.name.clone(),
                path: p.path.clone(),
                show_in_overview: p.show_in_overview,
                terminal_ids,
                terminal_names: p.terminal_names.clone(),
                git_branch,
                git_lines_added,
                git_lines_removed,
                services,
                folder_color: format!("{:?}", p.folder_color).to_lowercase(),
            }
        })
        .collect()
}

/// Get the focused project ID from the cached remote state.
pub fn get_focused_project_id(conn_id: String) -> Option<String> {
    let mgr = ConnectionManager::get();
    mgr.get_state(&conn_id)
        .and_then(|s| s.focused_project_id.clone())
}

/// Get folders from the cached remote state.
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
            folder_color: format!("{:?}", f.folder_color).to_lowercase(),
        })
        .collect()
}

/// Get the project order from the cached remote state.
pub fn get_project_order(conn_id: String) -> Vec<String> {
    let mgr = ConnectionManager::get();
    mgr.get_state(&conn_id)
        .map(|s| s.project_order.clone())
        .unwrap_or_default()
}

/// Get fullscreen terminal info.
pub fn get_fullscreen_terminal(conn_id: String) -> Option<FullscreenInfo> {
    let mgr = ConnectionManager::get();
    mgr.get_state(&conn_id).and_then(|s| {
        s.fullscreen_terminal.as_ref().map(|f| FullscreenInfo {
            project_id: f.project_id.clone(),
            terminal_id: f.terminal_id.clone(),
        })
    })
}

fn collect_layout_ids_vec(node: &ApiLayoutNode, ids: &mut Vec<String>) {
    match node {
        ApiLayoutNode::Terminal { terminal_id, .. } => {
            if let Some(id) = terminal_id {
                ids.push(id.clone());
            }
        }
        ApiLayoutNode::Split { children, .. } | ApiLayoutNode::Tabs { children, .. } => {
            for child in children {
                collect_layout_ids_vec(child, ids);
            }
        }
    }
}
