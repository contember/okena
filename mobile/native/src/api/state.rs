use std::collections::HashMap;

use crate::client::manager::ConnectionManager;
use okena_core::api::{ActionRequest, ApiLayoutNode};
use okena_core::client::{collect_state_terminal_ids, WsClientMessage};
use okena_core::keys::SpecialKey;
use okena_core::types::SplitDirection;

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
#[flutter_rust_bridge::frb(sync)]
pub fn get_focused_project_id(conn_id: String) -> Option<String> {
    let mgr = ConnectionManager::get();
    mgr.get_state(&conn_id)
        .and_then(|s| s.focused_project_id.clone())
}

/// Get folders from the cached remote state.
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
            folder_color: format!("{:?}", f.folder_color).to_lowercase(),
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

/// Get fullscreen terminal info.
#[flutter_rust_bridge::frb(sync)]
pub fn get_fullscreen_terminal(conn_id: String) -> Option<FullscreenInfo> {
    let mgr = ConnectionManager::get();
    mgr.get_state(&conn_id)
        .and_then(|s| {
            s.fullscreen_terminal.as_ref().map(|f| FullscreenInfo {
                project_id: f.project_id.clone(),
                terminal_id: f.terminal_id.clone(),
            })
        })
}

/// Get layout JSON for a project.
#[flutter_rust_bridge::frb(sync)]
pub fn get_project_layout_json(conn_id: String, project_id: String) -> Option<String> {
    let mgr = ConnectionManager::get();
    let state = mgr.get_state(&conn_id)?;
    let project = state.projects.iter().find(|p| p.id == project_id)?;
    let layout = project.layout.as_ref()?;
    serde_json::to_string(layout).ok()
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

fn collect_layout_ids_vec(node: &ApiLayoutNode, ids: &mut Vec<String>) {
    match node {
        ApiLayoutNode::Terminal { terminal_id, .. } => {
            if let Some(id) = terminal_id {
                ids.push(id.clone());
            }
        }
        ApiLayoutNode::Split { children, .. }
        | ApiLayoutNode::Tabs { children, .. } => {
            for child in children {
                collect_layout_ids_vec(child, ids);
            }
        }
    }
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

// ── Terminal actions ────────────────────────────────────────────────

/// Create a new terminal in the given project.
pub async fn create_terminal(conn_id: String, project_id: String) -> anyhow::Result<()> {
    let mgr = ConnectionManager::get();
    mgr.send_action(&conn_id, ActionRequest::CreateTerminal { project_id })
        .await
}

/// Close a terminal in the given project.
pub async fn close_terminal(
    conn_id: String,
    project_id: String,
    terminal_id: String,
) -> anyhow::Result<()> {
    let mgr = ConnectionManager::get();
    mgr.send_action(
        &conn_id,
        ActionRequest::CloseTerminal {
            project_id,
            terminal_id,
        },
    )
    .await
}

/// Close multiple terminals in a project.
pub async fn close_terminals(
    conn_id: String,
    project_id: String,
    terminal_ids: Vec<String>,
) -> anyhow::Result<()> {
    let mgr = ConnectionManager::get();
    mgr.send_action(
        &conn_id,
        ActionRequest::CloseTerminals {
            project_id,
            terminal_ids,
        },
    )
    .await
}

/// Rename a terminal.
pub async fn rename_terminal(
    conn_id: String,
    project_id: String,
    terminal_id: String,
    name: String,
) -> anyhow::Result<()> {
    let mgr = ConnectionManager::get();
    mgr.send_action(
        &conn_id,
        ActionRequest::RenameTerminal {
            project_id,
            terminal_id,
            name,
        },
    )
    .await
}

/// Focus a terminal.
pub async fn focus_terminal(
    conn_id: String,
    project_id: String,
    terminal_id: String,
) -> anyhow::Result<()> {
    let mgr = ConnectionManager::get();
    mgr.send_action(
        &conn_id,
        ActionRequest::FocusTerminal {
            project_id,
            terminal_id,
        },
    )
    .await
}

/// Toggle minimized state of a terminal.
pub async fn toggle_minimized(
    conn_id: String,
    project_id: String,
    terminal_id: String,
) -> anyhow::Result<()> {
    let mgr = ConnectionManager::get();
    mgr.send_action(
        &conn_id,
        ActionRequest::ToggleMinimized {
            project_id,
            terminal_id,
        },
    )
    .await
}

/// Set/clear fullscreen terminal.
pub async fn set_fullscreen(
    conn_id: String,
    project_id: String,
    terminal_id: Option<String>,
) -> anyhow::Result<()> {
    let mgr = ConnectionManager::get();
    mgr.send_action(
        &conn_id,
        ActionRequest::SetFullscreen {
            project_id,
            terminal_id,
        },
    )
    .await
}

/// Split a terminal pane.
pub async fn split_terminal(
    conn_id: String,
    project_id: String,
    path: Vec<usize>,
    direction: String,
) -> anyhow::Result<()> {
    let dir = match direction.as_str() {
        "vertical" => SplitDirection::Vertical,
        _ => SplitDirection::Horizontal,
    };
    let mgr = ConnectionManager::get();
    mgr.send_action(
        &conn_id,
        ActionRequest::SplitTerminal {
            project_id,
            path,
            direction: dir,
        },
    )
    .await
}

/// Run a command in a terminal (presses Enter automatically).
pub async fn run_command(
    conn_id: String,
    terminal_id: String,
    command: String,
) -> anyhow::Result<()> {
    let mgr = ConnectionManager::get();
    mgr.send_action(
        &conn_id,
        ActionRequest::RunCommand {
            terminal_id,
            command,
        },
    )
    .await
}

/// Read terminal content as text.
pub async fn read_content(conn_id: String, terminal_id: String) -> anyhow::Result<String> {
    let mgr = ConnectionManager::get();
    mgr.send_action_with_response(
        &conn_id,
        ActionRequest::ReadContent { terminal_id },
    )
    .await
}

// ── Git actions ─────────────────────────────────────────────────────

/// Get detailed git status for a project.
pub async fn git_status(conn_id: String, project_id: String) -> anyhow::Result<String> {
    let mgr = ConnectionManager::get();
    mgr.send_action_with_response(
        &conn_id,
        ActionRequest::GitStatus { project_id },
    )
    .await
}

/// Get git diff summary for a project.
pub async fn git_diff_summary(conn_id: String, project_id: String) -> anyhow::Result<String> {
    let mgr = ConnectionManager::get();
    mgr.send_action_with_response(
        &conn_id,
        ActionRequest::GitDiffSummary { project_id },
    )
    .await
}

/// Get git diff for a project. Mode: "working_tree", "staged".
pub async fn git_diff(
    conn_id: String,
    project_id: String,
    mode: String,
) -> anyhow::Result<String> {
    let diff_mode = match mode.as_str() {
        "staged" => okena_core::types::DiffMode::Staged,
        _ => okena_core::types::DiffMode::WorkingTree,
    };
    let mgr = ConnectionManager::get();
    mgr.send_action_with_response(
        &conn_id,
        ActionRequest::GitDiff {
            project_id,
            mode: diff_mode,
            ignore_whitespace: false,
        },
    )
    .await
}

/// Get git branches for a project.
pub async fn git_branches(conn_id: String, project_id: String) -> anyhow::Result<String> {
    let mgr = ConnectionManager::get();
    mgr.send_action_with_response(
        &conn_id,
        ActionRequest::GitBranches { project_id },
    )
    .await
}

// ── Service actions ─────────────────────────────────────────────────

/// Start a service.
pub async fn start_service(
    conn_id: String,
    project_id: String,
    service_name: String,
) -> anyhow::Result<()> {
    let mgr = ConnectionManager::get();
    mgr.send_action(
        &conn_id,
        ActionRequest::StartService {
            project_id,
            service_name,
        },
    )
    .await
}

/// Stop a service.
pub async fn stop_service(
    conn_id: String,
    project_id: String,
    service_name: String,
) -> anyhow::Result<()> {
    let mgr = ConnectionManager::get();
    mgr.send_action(
        &conn_id,
        ActionRequest::StopService {
            project_id,
            service_name,
        },
    )
    .await
}

/// Restart a service.
pub async fn restart_service(
    conn_id: String,
    project_id: String,
    service_name: String,
) -> anyhow::Result<()> {
    let mgr = ConnectionManager::get();
    mgr.send_action(
        &conn_id,
        ActionRequest::RestartService {
            project_id,
            service_name,
        },
    )
    .await
}

/// Start all services in a project.
pub async fn start_all_services(conn_id: String, project_id: String) -> anyhow::Result<()> {
    let mgr = ConnectionManager::get();
    mgr.send_action(&conn_id, ActionRequest::StartAllServices { project_id })
        .await
}

/// Stop all services in a project.
pub async fn stop_all_services(conn_id: String, project_id: String) -> anyhow::Result<()> {
    let mgr = ConnectionManager::get();
    mgr.send_action(&conn_id, ActionRequest::StopAllServices { project_id })
        .await
}

/// Reload services config for a project.
pub async fn reload_services(conn_id: String, project_id: String) -> anyhow::Result<()> {
    let mgr = ConnectionManager::get();
    mgr.send_action(&conn_id, ActionRequest::ReloadServices { project_id })
        .await
}

// ── Project management ──────────────────────────────────────────────

/// Add a new project.
pub async fn add_project(conn_id: String, name: String, path: String) -> anyhow::Result<()> {
    let mgr = ConnectionManager::get();
    mgr.send_action(&conn_id, ActionRequest::AddProject { name, path })
        .await
}

/// Set project color.
pub async fn set_project_color(
    conn_id: String,
    project_id: String,
    color: String,
) -> anyhow::Result<()> {
    let folder_color: okena_core::theme::FolderColor =
        serde_json::from_value(serde_json::Value::String(color.clone()))
            .unwrap_or_default();
    let mgr = ConnectionManager::get();
    mgr.send_action(
        &conn_id,
        ActionRequest::SetProjectColor {
            project_id,
            color: folder_color,
        },
    )
    .await
}

/// Set folder color.
pub async fn set_folder_color(
    conn_id: String,
    folder_id: String,
    color: String,
) -> anyhow::Result<()> {
    let folder_color: okena_core::theme::FolderColor =
        serde_json::from_value(serde_json::Value::String(color.clone()))
            .unwrap_or_default();
    let mgr = ConnectionManager::get();
    mgr.send_action(
        &conn_id,
        ActionRequest::SetFolderColor {
            folder_id,
            color: folder_color,
        },
    )
    .await
}
