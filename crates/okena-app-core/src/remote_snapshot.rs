//! Shared, gpui-free assembly of the remote `GetState` snapshot.
//!
//! Both remote command loops answer `RemoteCommand::GetState` by projecting the
//! same [`WorkspaceData`] onto the same wire DTOs:
//!
//! * GUI: `okena-app`'s `app/remote_commands.rs` `remote_command_loop`
//!   (reads an `Entity<Workspace>` / `Entity<ServiceManager>`).
//! * Headless: `okena-daemon-core`'s `command_loop.rs` `daemon_command_loop`
//!   (reads `Arc<Mutex<Workspace>>` / `Arc<Mutex<ServiceManager>>`).
//!
//! The two differ only in how they *gather* the inputs (entity reads vs. lock
//! guards) and in how they enumerate windows (GUI enumerates real OS windows;
//! the daemon serves a single synthetic `"main"` window). The pure projection —
//! ordered projects (following `project_order` + folder expansion + orphans),
//! folders, and the final [`StateResponse`] — is identical, so it lives here.
//!
//! Each caller pre-builds `services_by_project` (project id → wire
//! [`ApiServiceInfo`] list) from its own `ServiceManager` (via
//! `ServiceInstance::to_api`), keeping the `okena-services` dependency out of
//! `okena-app-core`, then hands plain data to these builders.

use std::collections::{HashMap, HashSet};

use okena_core::api::{
    ApiFolder, ApiFullscreen, ApiGitStatus, ApiProject, ApiServiceInfo, ApiWindow, ApiWorktreeMetadata,
    StateResponse,
};
use okena_workspace::state::{FolderData, ProjectData, WorkspaceData};

/// Pure visibility projection for the remote `ApiProject.show_in_overview` wire
/// flag: a project is "shown in overview" iff it is absent from the per-window
/// hidden set (today: `main_window.hidden_project_ids`).
pub fn api_project_visibility(project_id: &str, hidden_project_ids: &HashSet<String>) -> bool {
    !hidden_project_ids.contains(project_id)
}

/// Project a single [`ProjectData`] onto its wire [`ApiProject`].
///
/// Inputs are already gathered by the caller:
/// * `git_statuses` — project id → latest [`ApiGitStatus`].
/// * `services_by_project` — project id → wire service list (built from the
///   caller's `ServiceManager`; absent ⇒ no services).
/// * `hidden_project_ids` — per-window hidden set driving `show_in_overview`.
/// * `size_map` — terminal id → `(cols, rows)` for `layout.to_api_with_sizes`.
pub fn build_api_project(
    p: &ProjectData,
    git_statuses: &HashMap<String, ApiGitStatus>,
    services_by_project: &HashMap<String, Vec<ApiServiceInfo>>,
    hidden_project_ids: &HashSet<String>,
    size_map: &HashMap<String, (u16, u16)>,
) -> ApiProject {
    ApiProject {
        id: p.id.clone(),
        name: p.name.clone(),
        path: p.path.clone(),
        show_in_overview: api_project_visibility(&p.id, hidden_project_ids),
        layout: p.layout.as_ref().map(|l| l.to_api_with_sizes(size_map)),
        terminal_names: p.terminal_names.clone(),
        git_status: git_statuses.get(&p.id).cloned(),
        folder_color: p.folder_color,
        services: services_by_project.get(&p.id).cloned().unwrap_or_default(),
        worktree_info: p.worktree_info.as_ref().map(|wt| ApiWorktreeMetadata {
            parent_project_id: wt.parent_project_id.clone(),
            color_override: wt.color_override,
        }),
        worktree_ids: p.worktree_ids.clone(),
        pinned: p.pinned,
        last_activity_at: p.last_activity_at,
        default_shell: p.default_shell.clone(),
        hook_terminals: p
            .hook_terminals
            .iter()
            .map(|(tid, e)| e.to_api(tid.clone()))
            .collect(),
        hooks: p.hooks.to_api(),
    }
}

/// Build the ordered wire project list following `project_order` + folder
/// expansion, then appending orphan projects not referenced by any order entry.
///
/// Mirrors the historical inline loop shared by both command loops: for each id
/// in `project_order`, if it names a folder expand its `project_ids` in order,
/// otherwise treat it as a project id; a `seen` set dedupes so a project listed
/// both directly and via a folder appears once.
pub fn build_api_projects(
    data: &WorkspaceData,
    git_statuses: &HashMap<String, ApiGitStatus>,
    services_by_project: &HashMap<String, Vec<ApiServiceInfo>>,
    hidden_project_ids: &HashSet<String>,
    size_map: &HashMap<String, (u16, u16)>,
) -> Vec<ApiProject> {
    let project_map: HashMap<&str, &ProjectData> =
        data.projects.iter().map(|p| (p.id.as_str(), p)).collect();

    let mut projects: Vec<ApiProject> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let push = |projects: &mut Vec<ApiProject>, p: &ProjectData| {
        projects.push(build_api_project(
            p,
            git_statuses,
            services_by_project,
            hidden_project_ids,
            size_map,
        ));
    };

    for id in &data.project_order {
        if let Some(folder) = data.folders.iter().find(|f| &f.id == id) {
            for pid in &folder.project_ids {
                if seen.insert(pid.clone())
                    && let Some(p) = project_map.get(pid.as_str())
                {
                    push(&mut projects, p);
                }
            }
        } else if seen.insert(id.clone())
            && let Some(p) = project_map.get(id.as_str())
        {
            push(&mut projects, p);
        }
    }

    // Append orphan projects not in any order.
    for p in &data.projects {
        if seen.insert(p.id.clone()) {
            push(&mut projects, p);
        }
    }

    projects
}

/// Project the workspace folder list onto the wire [`ApiFolder`] list.
pub fn build_folders(folders: &[FolderData]) -> Vec<ApiFolder> {
    folders
        .iter()
        .map(|f| ApiFolder {
            id: f.id.clone(),
            name: f.name.clone(),
            project_ids: f.project_ids.clone(),
            folder_color: f.folder_color,
        })
        .collect()
}

/// Assemble the final [`StateResponse`] from already-gathered inputs.
///
/// `windows` is supplied by the caller (GUI enumerates real OS windows; the
/// daemon passes a single synthetic `"main"` window). The back-compat flat
/// fields (`focused_project_id`, `fullscreen_terminal`) are derived here from
/// the active window so old clients keep a sensible focused project /
/// fullscreen.
pub fn build_state_response(
    state_version: u64,
    data: &WorkspaceData,
    git_statuses: &HashMap<String, ApiGitStatus>,
    services_by_project: &HashMap<String, Vec<ApiServiceInfo>>,
    hidden_project_ids: &HashSet<String>,
    size_map: &HashMap<String, (u16, u16)>,
    windows: Vec<ApiWindow>,
) -> StateResponse {
    let projects = build_api_projects(
        data,
        git_statuses,
        services_by_project,
        hidden_project_ids,
        size_map,
    );
    let folders = build_folders(&data.folders);

    let focused_project_id: Option<String> = windows
        .iter()
        .find(|w| w.active)
        .and_then(|w| w.focused_project_id.clone());
    let fullscreen: Option<ApiFullscreen> = windows
        .iter()
        .find(|w| w.active)
        .and_then(|w| w.fullscreen.clone());

    StateResponse {
        state_version,
        projects,
        focused_project_id,
        fullscreen_terminal: fullscreen,
        project_order: data.project_order.clone(),
        folders,
        windows,
    }
}
