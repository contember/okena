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
    AppSettings, CursorShape, DiffViewMode, HooksConfig, SidebarSettings,
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
            project.service_terminals.clear();
        }
    }

    // Normalize layout trees (flatten redundant nesting, unwrap single-child containers)
    for project in &mut data.projects {
        if let Some(ref mut layout) = project.layout {
            layout.normalize();
        }
    }

    // Clean up orphaned terminal metadata (terminal_names/hidden_terminals entries
    // for terminals no longer in the layout tree)
    for project in &mut data.projects {
        let layout_ids: std::collections::HashSet<String> = project.layout.as_ref()
            .map(|l| l.collect_terminal_ids().into_iter().collect())
            .unwrap_or_default();
        project.terminal_names.retain(|id, _| layout_ids.contains(id));
        project.hidden_terminals.retain(|id, _| layout_ids.contains(id));
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

/// Load workspace from disk.
/// If the file is corrupted, backs it up as `workspace.json.bak` and returns an error.
pub fn load_workspace(backend: SessionBackend) -> Result<WorkspaceData> {
    let path = get_workspace_path();

    if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        let mut data: WorkspaceData = match serde_json::from_str(&content) {
            Ok(data) => data,
            Err(e) => {
                // Back up the corrupted file so the user can recover manually
                let backup_path = path.with_extension("json.bak");
                if let Err(backup_err) = std::fs::copy(&path, &backup_path) {
                    log::error!("Failed to back up corrupted workspace to {:?}: {}", backup_path, backup_err);
                } else {
                    log::error!("Workspace file is corrupted, backed up to {:?}", backup_path);
                }
                return Err(e.into());
            }
        };

        data = migrate_workspace(data);

        let session_backend = backend.resolve();
        let clear_ids = !session_backend.supports_persistence();
        validate_workspace_data(&mut data, clear_ids);

        Ok(data)
    } else {
        // Config dir exists but workspace.json doesn't — possible data loss
        if path.parent().is_some_and(|p| p.exists()) {
            log::warn!(
                "Workspace file not found at {:?} (config dir exists). Starting with default workspace.",
                path,
            );
        }
        Ok(default_workspace())
    }
}

/// Save workspace to disk using atomic write (write to temp file + rename)
/// to prevent data loss if the app crashes mid-write.
/// Remote projects are excluded from persistence (they're materialized from remote state).
pub fn save_workspace(data: &WorkspaceData) -> Result<()> {
    let path = get_workspace_path();

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Filter out remote projects before saving
    let remote_project_ids: std::collections::HashSet<&str> = data.projects.iter()
        .filter(|p| p.is_remote)
        .map(|p| p.id.as_str())
        .collect();

    let filtered = if remote_project_ids.is_empty() {
        // Fast path: no remote projects, save as-is
        serde_json::to_string_pretty(data)?
    } else {
        let filtered_data = WorkspaceData {
            version: data.version,
            projects: data.projects.iter()
                .filter(|p| !p.is_remote)
                .cloned()
                .collect(),
            project_order: data.project_order.iter()
                .filter(|id| !id.starts_with("remote-folder:") && !remote_project_ids.contains(id.as_str()))
                .cloned()
                .collect(),
            project_widths: data.project_widths.iter()
                .filter(|(id, _)| !remote_project_ids.contains(id.as_str()))
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
            service_panel_heights: data.service_panel_heights.iter()
                .filter(|(id, _)| !remote_project_ids.contains(id.as_str()))
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
            folders: data.folders.iter()
                .filter(|f| !f.id.starts_with("remote-folder:"))
                .cloned()
                .collect(),
        };
        serde_json::to_string_pretty(&filtered_data)?
    };

    // Safety backup: if the existing file has more projects than what we're about
    // to save, create a .bak copy first. This protects against accidental data loss
    // if a bad load caused us to start with a near-empty workspace.
    let new_local_project_count = data.projects.iter().filter(|p| !p.is_remote).count();
    if path.exists() && new_local_project_count <= 1 {
        if let Ok(existing) = std::fs::read_to_string(&path) {
            if let Ok(existing_data) = serde_json::from_str::<WorkspaceData>(&existing) {
                if existing_data.projects.len() > new_local_project_count {
                    let backup_path = path.with_extension("json.bak");
                    log::warn!(
                        "Saving workspace with {} project(s) over existing file with {} project(s). \
                         Creating safety backup at {:?}.",
                        new_local_project_count,
                        existing_data.projects.len(),
                        backup_path,
                    );
                    if let Err(e) = std::fs::copy(&path, &backup_path) {
                        log::error!("Failed to create safety backup: {}", e);
                    }
                }
            }
        }
    }

    // Atomic write: write to a temp file first, fsync, then rename over the target.
    // This ensures the file is either fully old or fully new, never partial.
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, &filtered)?;
    // fsync to ensure data is durable on disk before rename
    std::fs::File::open(&tmp_path)?.sync_all()?;
    std::fs::rename(&tmp_path, &path)?;

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
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            remote_services: Vec::new(),
            remote_host: None,
            remote_git_status: None,
        }],
        project_order: vec![project_id],
        project_widths: HashMap::new(),
        service_panel_heights: HashMap::new(),
        folders: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::state::{FolderData, SplitDirection};

    fn make_project(id: &str) -> ProjectData {
        ProjectData {
            id: id.to_string(),
            name: format!("Project {}", id),
            path: "/tmp/test".to_string(),
            is_visible: true,
            layout: Some(LayoutNode::new_terminal()),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            folder_color: FolderColor::default(),
            hooks: super::super::settings::HooksConfig::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            remote_services: Vec::new(),
            remote_host: None,
            remote_git_status: None,
        }
    }

    fn make_workspace(projects: Vec<ProjectData>, order: Vec<&str>, folders: Vec<FolderData>) -> WorkspaceData {
        WorkspaceData {
            version: WORKSPACE_VERSION,
            projects,
            project_order: order.into_iter().map(String::from).collect(),
            project_widths: HashMap::new(),
            service_panel_heights: HashMap::new(),
            folders,
        }
    }

    // === validate_workspace_data ===

    #[test]
    fn validate_orphaned_project_added_to_order() {
        let mut data = make_workspace(
            vec![make_project("p1"), make_project("p2")],
            vec!["p1"], // p2 is orphaned
            vec![],
        );
        validate_workspace_data(&mut data, false);
        assert!(data.project_order.contains(&"p2".to_string()));
    }

    #[test]
    fn validate_stale_folder_refs_removed() {
        let mut data = make_workspace(
            vec![make_project("p1")],
            vec!["f1", "p1"],
            vec![FolderData {
                id: "f1".to_string(),
                name: "Folder".to_string(),
                project_ids: vec!["p1".to_string(), "deleted_project".to_string()],
                collapsed: false,
                folder_color: FolderColor::default(),
            }],
        );
        validate_workspace_data(&mut data, false);
        assert_eq!(data.folders[0].project_ids, vec!["p1".to_string()]);
    }

    #[test]
    fn validate_invalid_folder_id_removed_from_order() {
        let mut data = make_workspace(
            vec![make_project("p1")],
            vec!["nonexistent_folder", "p1"],
            vec![],
        );
        validate_workspace_data(&mut data, false);
        assert!(!data.project_order.contains(&"nonexistent_folder".to_string()));
        assert!(data.project_order.contains(&"p1".to_string()));
    }

    #[test]
    fn validate_clear_terminal_ids() {
        let mut project = make_project("p1");
        project.layout = Some(LayoutNode::Terminal {
            terminal_id: Some("tid1".to_string()),
            minimized: true,
            detached: true,
            shell_type: crate::terminal::shell_config::ShellType::Default,
            zoom_level: 1.0,
        });
        project.service_terminals.insert("web".to_string(), "svc-term-1".to_string());
        let mut data = make_workspace(vec![project], vec!["p1"], vec![]);
        validate_workspace_data(&mut data, true);

        let layout = data.projects[0].layout.as_ref().unwrap();
        match layout {
            LayoutNode::Terminal { terminal_id, minimized, detached, .. } => {
                assert!(terminal_id.is_none());
                assert!(!minimized);
                assert!(!detached);
            }
            _ => panic!("Expected terminal"),
        }
        assert!(data.projects[0].service_terminals.is_empty());
    }

    #[test]
    fn validate_layout_normalization() {
        let mut project = make_project("p1");
        // Single-child split should normalize to just the child
        project.layout = Some(LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![100.0],
            children: vec![LayoutNode::new_terminal()],
        });
        let mut data = make_workspace(vec![project], vec!["p1"], vec![]);
        validate_workspace_data(&mut data, false);

        assert!(matches!(data.projects[0].layout, Some(LayoutNode::Terminal { .. })));
    }

    #[test]
    fn validate_combined_issues() {
        let mut data = make_workspace(
            vec![make_project("p1"), make_project("p2"), make_project("p3")],
            vec!["bad_folder", "p1"], // p2, p3 orphaned; bad_folder invalid
            vec![FolderData {
                id: "f1".to_string(),
                name: "Folder".to_string(),
                project_ids: vec!["p3".to_string(), "deleted".to_string()],
                collapsed: false,
                folder_color: FolderColor::default(),
            }],
        );
        // Note: f1 is in folders but not in project_order
        data.project_order.push("f1".to_string());

        validate_workspace_data(&mut data, false);

        // bad_folder should be removed (not a valid project or folder)
        assert!(!data.project_order.contains(&"bad_folder".to_string()));
        // p2 should be added (orphaned, not in any folder)
        assert!(data.project_order.contains(&"p2".to_string()));
        // f1 should remain (valid folder)
        assert!(data.project_order.contains(&"f1".to_string()));
        // Stale ref 'deleted' removed from folder
        assert_eq!(data.folders[0].project_ids, vec!["p3".to_string()]);
    }

    // === migrate_workspace ===

    #[test]
    fn migrate_v0_to_v1() {
        let data = WorkspaceData {
            version: 0,
            projects: vec![],
            project_order: vec![],
            project_widths: HashMap::new(),
            service_panel_heights: HashMap::new(),
            folders: vec![],
        };
        let migrated = migrate_workspace(data);
        assert_eq!(migrated.version, 1);
    }

    #[test]
    fn migrate_current_version_noop() {
        let data = WorkspaceData {
            version: WORKSPACE_VERSION,
            projects: vec![],
            project_order: vec![],
            project_widths: HashMap::new(),
            service_panel_heights: HashMap::new(),
            folders: vec![],
        };
        let migrated = migrate_workspace(data);
        assert_eq!(migrated.version, WORKSPACE_VERSION);
    }

    // === Serialization ===

    #[test]
    fn default_workspace_round_trips() {
        let data = default_workspace();
        let json = serde_json::to_string(&data).unwrap();
        let deserialized: WorkspaceData = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.projects.len(), 1);
        assert_eq!(deserialized.project_order.len(), 1);
        assert_eq!(deserialized.version, WORKSPACE_VERSION);
    }

    #[test]
    fn workspace_with_folders_round_trips() {
        let mut data = make_workspace(
            vec![make_project("p1"), make_project("p2")],
            vec!["f1", "p1"],
            vec![FolderData {
                id: "f1".to_string(),
                name: "My Folder".to_string(),
                project_ids: vec!["p2".to_string()],
                collapsed: true,
                folder_color: FolderColor::default(),
            }],
        );
        data.project_widths.insert("p1".to_string(), 60.0);

        let json = serde_json::to_string(&data).unwrap();
        let deserialized: WorkspaceData = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.folders.len(), 1);
        assert_eq!(deserialized.folders[0].name, "My Folder");
        assert!(deserialized.folders[0].collapsed);
        assert_eq!(deserialized.project_widths.get("p1"), Some(&60.0));
    }

    #[test]
    fn validate_cleans_orphaned_terminal_metadata() {
        let mut project = make_project("p1");
        project.layout = Some(LayoutNode::Terminal {
            terminal_id: Some("t1".to_string()),
            minimized: false,
            detached: false,
            shell_type: crate::terminal::shell_config::ShellType::Default,
            zoom_level: 1.0,
        });
        // t1 is in layout, t2 and t3 are orphaned
        project.terminal_names.insert("t1".to_string(), "Term 1".to_string());
        project.terminal_names.insert("t2".to_string(), "Term 2".to_string());
        project.terminal_names.insert("t3".to_string(), "Term 3".to_string());
        project.hidden_terminals.insert("t2".to_string(), true);

        let mut data = make_workspace(vec![project], vec!["p1"], vec![]);
        validate_workspace_data(&mut data, false);

        assert!(data.projects[0].terminal_names.contains_key("t1"));
        assert!(!data.projects[0].terminal_names.contains_key("t2"));
        assert!(!data.projects[0].terminal_names.contains_key("t3"));
        assert!(!data.projects[0].hidden_terminals.contains_key("t2"));
    }

    #[test]
    fn validate_cleans_all_metadata_when_no_layout() {
        let mut project = make_project("p1");
        project.layout = None;
        project.terminal_names.insert("t1".to_string(), "Term 1".to_string());
        project.terminal_names.insert("t2".to_string(), "Term 2".to_string());

        let mut data = make_workspace(vec![project], vec!["p1"], vec![]);
        validate_workspace_data(&mut data, false);

        assert!(data.projects[0].terminal_names.is_empty());
    }

    #[test]
    fn save_filters_remote_projects() {
        // Create mixed local + remote workspace data
        let local = make_project("local1");
        let mut remote1 = make_project("remote:conn1:p1");
        remote1.is_remote = true;
        remote1.connection_id = Some("conn1".to_string());
        let mut remote2 = make_project("remote:conn1:p2");
        remote2.is_remote = true;
        remote2.connection_id = Some("conn1".to_string());

        let mut data = make_workspace(
            vec![local, remote1, remote2],
            vec!["local1", "remote-folder:conn1"],
            vec![FolderData {
                id: "remote-folder:conn1".to_string(),
                name: "Server 1".to_string(),
                project_ids: vec!["remote:conn1:p1".to_string(), "remote:conn1:p2".to_string()],
                collapsed: false,
                folder_color: FolderColor::default(),
            }],
        );
        data.project_widths.insert("local1".to_string(), 50.0);
        data.project_widths.insert("remote:conn1:p1".to_string(), 40.0);

        // Save and reload
        let result = save_workspace(&data);
        assert!(result.is_ok());

        let loaded = load_workspace(crate::terminal::session_backend::SessionBackend::None).unwrap();

        // Remote projects should be filtered out
        assert_eq!(loaded.projects.len(), 1);
        assert_eq!(loaded.projects[0].id, "local1");

        // Remote folder should be filtered out
        assert!(loaded.folders.is_empty());

        // Remote folder should be removed from project_order
        assert_eq!(loaded.project_order, vec!["local1".to_string()]);

        // Remote project widths should be filtered out
        assert_eq!(loaded.project_widths.len(), 1);
        assert!(loaded.project_widths.contains_key("local1"));
    }
}
