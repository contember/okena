use crate::terminal::session_backend::SessionBackend;
use crate::theme::FolderColor;
use crate::workspace::state::{LayoutNode, ProjectData, WorkspaceData};

use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;

// Re-export from settings module for backward compatibility
#[allow(unused_imports)]
pub use super::settings::{
    load_settings, save_settings, get_settings_path,
    AppSettings, DiffViewMode, HooksConfig, SidebarSettings,
    DEFAULT_SIDEBAR_WIDTH, MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH,
    SETTINGS_VERSION,
};

// Re-export from sessions module for backward compatibility
#[allow(unused_imports)]
pub use super::sessions::{
    list_sessions, save_session, load_session, delete_session, rename_session, session_exists,
    export_workspace, import_workspace,
    SessionInfo, ExportedWorkspace,
};

/// Current workspace schema version - increment when making breaking changes
pub const WORKSPACE_VERSION: u32 = 1;

/// Get the config directory path
pub fn get_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("okena")
}

/// Get the workspace file path
pub fn get_workspace_path() -> PathBuf {
    get_config_dir().join("workspace.json")
}

/// Get the config directory path (public for UI display)
pub fn config_dir() -> PathBuf {
    get_config_dir()
}

/// Validate and fix workspace data consistency.
/// Called after deserialization in all load paths.
pub(crate) fn validate_workspace_data(data: &mut WorkspaceData, clear_terminal_ids: bool) {
    // Optionally clear terminal IDs (on app restart without session persistence)
    if clear_terminal_ids {
        for project in &mut data.projects {
            if let Some(ref mut layout) = project.layout {
                layout.clear_terminal_ids();
            }
        }
    }

    // Normalize layout trees (flatten redundant nesting, unwrap single-child containers)
    for project in &mut data.projects {
        if let Some(ref mut layout) = project.layout {
            layout.normalize();
        }
    }

    // Ensure project_order contains all project IDs (that aren't in a folder)
    let folder_project_ids: std::collections::HashSet<String> = data.folders.iter()
        .flat_map(|f| f.project_ids.iter().cloned())
        .collect();
    for project in &data.projects {
        if !data.project_order.contains(&project.id) && !folder_project_ids.contains(&project.id) {
            data.project_order.push(project.id.clone());
        }
    }

    // Folder consistency checks
    {
        let valid_project_ids: std::collections::HashSet<&str> = data.projects.iter().map(|p| p.id.as_str()).collect();

        // Remove stale project refs from folders
        for folder in &mut data.folders {
            folder.project_ids.retain(|pid| valid_project_ids.contains(pid.as_str()));
        }

        // Ensure folder IDs in project_order match actual folders
        let valid_folder_ids: std::collections::HashSet<&str> = data.folders.iter().map(|f| f.id.as_str()).collect();
        data.project_order.retain(|id| {
            valid_project_ids.contains(id.as_str()) || valid_folder_ids.contains(id.as_str())
        });
    }
}

/// Load workspace from disk
pub fn load_workspace(backend: SessionBackend) -> Result<WorkspaceData> {
    let path = get_workspace_path();

    if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        let mut data: WorkspaceData = serde_json::from_str(&content)?;

        data = migrate_workspace(data);

        let session_backend = backend.resolve();
        let clear_ids = !session_backend.supports_persistence();
        validate_workspace_data(&mut data, clear_ids);

        Ok(data)
    } else {
        Ok(default_workspace())
    }
}

/// Save workspace to disk
pub fn save_workspace(data: &WorkspaceData) -> Result<()> {
    let path = get_workspace_path();

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let content = serde_json::to_string_pretty(data)?;
    std::fs::write(&path, content)?;

    Ok(())
}

/// Migrate workspace data from older versions to the current version
pub(crate) fn migrate_workspace(mut data: WorkspaceData) -> WorkspaceData {
    let original_version = data.version;

    // Migration from version 0 (pre-versioning) to version 1
    if data.version == 0 {
        log::info!("Migrating workspace from pre-versioning (v0) to v1");
        data.version = 1;
    }

    // Future migrations would go here:
    // if data.version == 1 {
    //     log::info!("Migrating workspace from v1 to v2");
    //     // Perform v1 -> v2 migration
    //     data.version = 2;
    // }

    if original_version != data.version {
        log::info!("Workspace migrated from v{} to v{}", original_version, data.version);
    }

    data
}

/// Create a default workspace with one project
pub fn default_workspace() -> WorkspaceData {
    let project_id = uuid::Uuid::new_v4().to_string();
    let home_dir = dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "/".to_string());

    WorkspaceData {
        version: WORKSPACE_VERSION,
        projects: vec![ProjectData {
            id: project_id.clone(),
            name: "Default".to_string(),
            path: home_dir,
            is_visible: true,
            layout: Some(LayoutNode::new_terminal()),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            folder_color: FolderColor::default(),
            hooks: super::settings::HooksConfig::default(),
        }],
        project_order: vec![project_id],
        project_widths: HashMap::new(),
        folders: Vec::new(),
    }
}
