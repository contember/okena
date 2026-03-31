use okena_terminal::session_backend::SessionBackend;
use okena_core::theme::FolderColor;
use crate::state::{HookTerminalStatus, LayoutNode, ProjectData, WorkspaceData};
#[cfg(test)]
use crate::state::WorktreeMetadata;

use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// When true, the workspace was loaded from a fallback default (load failed).
/// Auto-save MUST NOT overwrite the real workspace.json in this state.
static LOADED_FROM_DEFAULT: AtomicBool = AtomicBool::new(false);

// Re-export from settings module for backward compatibility
#[allow(unused_imports)]
pub use super::settings::{
    load_settings, save_settings, get_settings_path,
    AppSettings, CursorShape, DiffViewMode, HooksConfig, ProjectHooks, TerminalHooks, WorktreeHooks, SidebarSettings,
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

/// Alias for `get_config_dir` (used by remote/auth, remote/server, session manager UI)
pub fn config_dir() -> PathBuf {
    get_config_dir()
}

/// Get the workspace file path
pub fn get_workspace_path() -> PathBuf {
    get_config_dir().join("workspace.json")
}

/// Acquire a lock file to prevent multiple instances from running simultaneously.
/// Returns a held `LockGuard` that releases the lock on drop.
/// If another instance is already running, returns an error with its PID.
pub fn acquire_instance_lock() -> Result<LockGuard> {
    let lock_path = get_config_dir().join("okena.lock");

    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Check if a lock file already exists with a live process
    if lock_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&lock_path) {
            if let Ok(pid) = content.trim().parse::<u32>() {
                if is_process_alive(pid) {
                    anyhow::bail!(
                        "Another Okena instance is already running (PID {pid}). \
                         If this is incorrect, delete {lock_path:?} and try again."
                    );
                }
                // Stale lock file from a crashed process — safe to take over
                log::info!("Removing stale lock file from PID {pid}");
            }
        }
    }

    let my_pid = std::process::id();
    std::fs::write(&lock_path, my_pid.to_string())?;

    Ok(LockGuard { path: lock_path })
}

/// Guard that removes the lock file on drop
pub struct LockGuard {
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Check whether a process with the given PID is still alive
fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // kill(pid, 0) checks existence without sending a signal
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(windows)]
    {
        // On Windows, try tasklist to check if PID exists
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }
}

/// Validate and fix workspace data consistency.
/// Called after deserialization in all load paths.
pub(crate) fn validate_workspace_data(
    data: &mut WorkspaceData,
    clear_terminal_ids: bool,
    #[cfg_attr(not(windows), allow(unused))]
    backend_preference: SessionBackend,
) {
    // Auto-detect WSL default shell for projects with WSL UNC paths that don't have it set.
    // This must run BEFORE clearing terminal IDs so we can check WSL backend availability.
    #[cfg(windows)]
    for project in &mut data.projects {
        if project.default_shell.is_none() {
            if let Some((distro, _)) = okena_terminal::shell_config::parse_wsl_unc_path(&project.path) {
                project.default_shell = Some(okena_terminal::shell_config::ShellType::Wsl {
                    distro: Some(distro),
                });
            }
        }
    }

    // Optionally clear terminal IDs (on app restart without session persistence).
    // On Windows, WSL projects may have their own session backend (dtach/tmux/screen)
    // even though the host has none — preserve their terminal IDs for reconnection.
    // Hook terminal IDs are always preserved so they retain their hook identity.
    if clear_terminal_ids {
        for project in &mut data.projects {
            #[cfg(windows)]
            {
                use okena_terminal::shell_config::ShellType;
                if let Some(ShellType::Wsl { distro }) = &project.default_shell {
                    let wsl_backend = okena_terminal::session_backend::resolve_for_wsl(
                        distro.as_deref(),
                        backend_preference,
                    );
                    if wsl_backend.supports_persistence() {
                        // WSL project with session backend — keep terminal IDs for reconnection
                        continue;
                    }
                }
            }
            // Preserve hook terminal IDs so they're recognized after restart
            let hook_ids: std::collections::HashSet<&str> = project.hook_terminals
                .keys().map(|s| s.as_str()).collect();
            if let Some(ref mut layout) = project.layout {
                layout.clear_terminal_ids_except(&hook_ids);
            }
            project.service_terminals.clear();

            // Reset Running hooks to Succeeded (the process is dead after restart)
            for entry in project.hook_terminals.values_mut() {
                if entry.status == HookTerminalStatus::Running {
                    entry.status = HookTerminalStatus::Succeeded;
                }
            }
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

    // Populate worktree_ids from worktree_info back-references (migration for old data)
    {
        // Collect worktree relationships: parent_id -> vec of (worktree_id, position_in_project_order)
        let mut parent_to_children: HashMap<String, Vec<(String, Option<usize>)>> = HashMap::new();
        for project in &data.projects {
            if let Some(ref wt_info) = project.worktree_info {
                let pos = data.project_order.iter().position(|id| id == &project.id);
                parent_to_children
                    .entry(wt_info.parent_project_id.clone())
                    .or_default()
                    .push((project.id.clone(), pos));
            }
        }

        for project in &mut data.projects {
            if project.worktree_ids.is_empty() {
                if let Some(mut children) = parent_to_children.remove(&project.id) {
                    // Sort by position in project_order for deterministic migration
                    children.sort_by_key(|(_, pos)| pos.unwrap_or(usize::MAX));
                    project.worktree_ids = children.into_iter().map(|(id, _)| id).collect();
                }
            }
        }

        // Remove non-orphan worktrees from project_order (they live in parent's worktree_ids now)
        let worktree_ids_in_parents: std::collections::HashSet<String> = data.projects.iter()
            .flat_map(|p| p.worktree_ids.iter().cloned())
            .collect();
        data.project_order.retain(|id| !worktree_ids_in_parents.contains(id));

        // Also remove from folder project_ids
        for folder in &mut data.folders {
            folder.project_ids.retain(|id| !worktree_ids_in_parents.contains(id));
        }
    }

    // Ensure project_order contains all project IDs (that aren't in a folder or worktree_ids)
    let folder_project_ids: std::collections::HashSet<String> = data.folders.iter()
        .flat_map(|f| f.project_ids.iter().cloned())
        .collect();
    let worktree_child_ids: std::collections::HashSet<String> = data.projects.iter()
        .flat_map(|p| p.worktree_ids.iter().cloned())
        .collect();
    for project in &data.projects {
        if !data.project_order.contains(&project.id)
            && !folder_project_ids.contains(&project.id)
            && !worktree_child_ids.contains(&project.id)
        {
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
/// On error, the caller should fall back to `default_workspace()` — auto-save is
/// automatically blocked to prevent overwriting valid data on disk.
pub fn load_workspace(backend: SessionBackend) -> Result<WorkspaceData> {
    let path = get_workspace_path();

    // If workspace.json is missing, try to auto-recover from backup
    if !path.exists() {
        let bak_path = path.with_extension("json.bak");
        if bak_path.exists() {
            log::warn!(
                "workspace.json missing but backup found at {:?} — restoring from backup.",
                bak_path,
            );
            if let Err(e) = std::fs::copy(&bak_path, &path) {
                log::error!("Failed to restore workspace backup: {}", e);
            }
            // Fall through — path.exists() check below will pick it up if copy succeeded
        }
    }

    if path.exists() {
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                // I/O error reading the file — block auto-save to protect the file on disk
                LOADED_FROM_DEFAULT.store(true, Ordering::Relaxed);
                return Err(e.into());
            }
        };
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
                // Block auto-save so the default workspace doesn't overwrite the real file
                LOADED_FROM_DEFAULT.store(true, Ordering::Relaxed);
                return Err(e.into());
            }
        };

        data = migrate_workspace(data);

        let session_backend = backend.resolve();
        let clear_ids = !session_backend.supports_persistence();
        validate_workspace_data(&mut data, clear_ids, backend);
        sync_worktrees(&mut data);

        // Successful load — allow saving
        LOADED_FROM_DEFAULT.store(false, Ordering::Relaxed);
        Ok(data)
    } else {
        let bak_path = path.with_extension("json.bak");
        if bak_path.exists() {
            // Backup exists but workspace.json doesn't and recovery above failed —
            // block auto-save to prevent overwriting recoverable data.
            log::warn!(
                "Workspace file not found at {:?} but backup exists. \
                 Starting with default workspace. Auto-save DISABLED to protect data.",
                path,
            );
            LOADED_FROM_DEFAULT.store(true, Ordering::Relaxed);
        } else {
            // Fresh install — no workspace.json and no backup. Allow saving.
            log::info!("No workspace file found — starting with default workspace.");
        }
        Ok(default_workspace())
    }
}

/// Save workspace to disk using atomic write (write to temp file + rename).
/// Remote projects are excluded. Refuses to save after a load failure.
///
/// Safety layers (all must pass for a save to proceed):
/// 1. LOADED_FROM_DEFAULT — blocks save entirely if load failed or file was missing
/// 2. Empty-workspace guard — refuses to save 0 local projects
/// 3. Rolling backup — always creates .bak before overwriting
/// 4. Atomic write — tmp + fsync + rename prevents partial writes
pub fn save_workspace(data: &WorkspaceData) -> Result<()> {
    // Layer 1: block save if we loaded from fallback default
    if LOADED_FROM_DEFAULT.load(Ordering::Relaxed) {
        log::warn!("Skipping workspace save — loaded from fallback default, protecting file on disk.");
        return Ok(());
    }

    let path = get_workspace_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let local_data = data.without_remote_projects();

    // Layer 2: refuse to save an empty workspace (likely a bug, not user intent)
    if local_data.projects.is_empty() {
        log::error!(
            "Refusing to save workspace with 0 local projects — this is likely a bug. \
             Blocking all future saves to protect data on disk."
        );
        LOADED_FROM_DEFAULT.store(true, Ordering::Relaxed);
        return Ok(());
    }

    let json = serde_json::to_string_pretty(&local_data)?;

    // Layer 3: rolling backup — always keep the previous version as .bak
    if path.exists() {
        let backup_path = path.with_extension("json.bak");
        if let Err(e) = std::fs::copy(&path, &backup_path) {
            log::warn!("Failed to create workspace backup: {}", e);
        }
    }

    // Layer 4: atomic write — tmp + fsync + rename ensures the file is never partial
    let tmp_path = path.with_extension("json.tmp");
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp_path)?;
        f.write_all(json.as_bytes())?;
        f.sync_all()?;
    }
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

/// Remove stale worktree projects whose directories no longer exist on disk.
///
/// Worktrees are only added as projects explicitly by the user (via the worktree
/// list popover or the create worktree dialog). This function only cleans up
/// worktree projects that have become stale.
pub(crate) fn sync_worktrees(data: &mut WorkspaceData) {
    let stale_ids: Vec<String> = data.projects.iter()
        .filter(|p| p.worktree_info.is_some())
        .filter(|p| !Path::new(&p.path).exists())
        .map(|p| p.id.clone())
        .collect();

    for id in &stale_ids {
        data.projects.retain(|p| p.id != *id);
        data.project_order.retain(|pid| pid != id);
        for folder in &mut data.folders {
            folder.project_ids.retain(|pid| pid != id);
        }
    }
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
            show_in_overview: true,
            layout: Some(LayoutNode::new_terminal()),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: FolderColor::default(),
            hooks: super::settings::HooksConfig::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            remote_services: Vec::new(),
            remote_host: None,
            remote_git_status: None,
            default_shell: None,
            hook_terminals: HashMap::new(),
        }],
        project_order: vec![project_id],
        project_widths: HashMap::new(),
        service_panel_heights: HashMap::new(),
        hook_panel_heights: HashMap::new(),
        folders: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{FolderData, SplitDirection};

    fn make_project(id: &str) -> ProjectData {
        ProjectData {
            id: id.to_string(),
            name: format!("Project {}", id),
            path: "/tmp/test".to_string(),
            show_in_overview: true,
            layout: Some(LayoutNode::new_terminal()),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: FolderColor::default(),
            hooks: super::super::settings::HooksConfig::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            remote_services: Vec::new(),
            remote_host: None,
            remote_git_status: None,
            default_shell: None,
            hook_terminals: HashMap::new(),
        }
    }

    fn make_workspace(projects: Vec<ProjectData>, order: Vec<&str>, folders: Vec<FolderData>) -> WorkspaceData {
        WorkspaceData {
            version: WORKSPACE_VERSION,
            projects,
            project_order: order.into_iter().map(String::from).collect(),
            project_widths: HashMap::new(),
            service_panel_heights: HashMap::new(),
        hook_panel_heights: HashMap::new(),
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
        validate_workspace_data(&mut data, false, SessionBackend::None);
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
        validate_workspace_data(&mut data, false, SessionBackend::None);
        assert_eq!(data.folders[0].project_ids, vec!["p1".to_string()]);
    }

    #[test]
    fn validate_invalid_folder_id_removed_from_order() {
        let mut data = make_workspace(
            vec![make_project("p1")],
            vec!["nonexistent_folder", "p1"],
            vec![],
        );
        validate_workspace_data(&mut data, false, SessionBackend::None);
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
            shell_type: okena_terminal::shell_config::ShellType::Default,
            zoom_level: 1.0,
        });
        project.service_terminals.insert("web".to_string(), "svc-term-1".to_string());
        let mut data = make_workspace(vec![project], vec!["p1"], vec![]);
        validate_workspace_data(&mut data, true, SessionBackend::None);

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
    fn validate_preserves_hook_terminal_ids() {
        use crate::state::{HookTerminalEntry, HookTerminalStatus, SplitDirection};

        let mut project = make_project("p1");
        project.layout = Some(LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![0.7, 0.3],
            children: vec![
                LayoutNode::Terminal {
                    terminal_id: Some("regular-term".to_string()),
                    minimized: false,
                    detached: false,
                    shell_type: okena_terminal::shell_config::ShellType::Default,
                    zoom_level: 1.0,
                },
                LayoutNode::Terminal {
                    terminal_id: Some("hook-term".to_string()),
                    minimized: false,
                    detached: false,
                    shell_type: okena_terminal::shell_config::ShellType::Default,
                    zoom_level: 1.0,
                },
            ],
        });
        project.hook_terminals.insert("hook-term".to_string(), HookTerminalEntry {
            label: "on_project_open".to_string(),
            status: HookTerminalStatus::Running,
            hook_type: "on_project_open".to_string(),
            command: "echo hello".to_string(),
            cwd: "/tmp".to_string(),
        });

        let mut data = make_workspace(vec![project], vec!["p1"], vec![]);
        validate_workspace_data(&mut data, true, SessionBackend::None);

        let layout = data.projects[0].layout.as_ref().unwrap();
        match layout {
            LayoutNode::Split { children, .. } => {
                // Regular terminal should have its ID cleared
                if let LayoutNode::Terminal { terminal_id, .. } = &children[0] {
                    assert!(terminal_id.is_none(), "regular terminal ID should be cleared");
                }
                // Hook terminal should keep its ID
                if let LayoutNode::Terminal { terminal_id, .. } = &children[1] {
                    assert_eq!(terminal_id.as_deref(), Some("hook-term"), "hook terminal ID should be preserved");
                }
            }
            _ => panic!("Expected split"),
        }

        // Hook terminal entry should still exist with status reset to Succeeded
        let entry = &data.projects[0].hook_terminals["hook-term"];
        assert_eq!(entry.status, HookTerminalStatus::Succeeded);
        assert_eq!(entry.label, "on_project_open");
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
        validate_workspace_data(&mut data, false, SessionBackend::None);

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

        validate_workspace_data(&mut data, false, SessionBackend::None);

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
        hook_panel_heights: HashMap::new(),
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
        hook_panel_heights: HashMap::new(),
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
            shell_type: okena_terminal::shell_config::ShellType::Default,
            zoom_level: 1.0,
        });
        // t1 is in layout, t2 and t3 are orphaned
        project.terminal_names.insert("t1".to_string(), "Term 1".to_string());
        project.terminal_names.insert("t2".to_string(), "Term 2".to_string());
        project.terminal_names.insert("t3".to_string(), "Term 3".to_string());
        project.hidden_terminals.insert("t2".to_string(), true);

        let mut data = make_workspace(vec![project], vec!["p1"], vec![]);
        validate_workspace_data(&mut data, false, SessionBackend::None);

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
        validate_workspace_data(&mut data, false, SessionBackend::None);

        assert!(data.projects[0].terminal_names.is_empty());
    }

    #[test]
    fn without_remote_projects_filters_correctly() {
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

        let filtered = data.without_remote_projects();

        // Remote projects should be filtered out
        assert_eq!(filtered.projects.len(), 1);
        assert_eq!(filtered.projects[0].id, "local1");

        // Remote folder should be filtered out
        assert!(filtered.folders.is_empty());

        // Remote folder should be removed from project_order
        assert_eq!(filtered.project_order, vec!["local1".to_string()]);

        // Remote project widths should be filtered out
        assert_eq!(filtered.project_widths.len(), 1);
        assert!(filtered.project_widths.contains_key("local1"));
    }

    fn make_worktree_project(id: &str, parent_id: &str) -> ProjectData {
        let mut p = make_project(id);
        p.worktree_info = Some(crate::state::WorktreeMetadata {
            parent_project_id: parent_id.to_string(),
                color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: format!("/tmp/worktrees/{}", id),
            branch_name: String::new(),
        });
        p
    }

    // === sync_worktrees ===

    #[test]
    fn sync_worktrees_cleans_up_stale_worktree_projects() {
        let mut wt_project = make_project("wt1");
        wt_project.path = "/nonexistent/path/that/does/not/exist".to_string();
        wt_project.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "p1".to_string(),
                color_override: None,
            main_repo_path: "/tmp/test".to_string(),
            worktree_path: String::new(),
            branch_name: "some-branch".to_string(),
        });

        let mut data = make_workspace(
            vec![make_project("p1"), wt_project],
            vec!["p1", "wt1"],
            vec![],
        );

        sync_worktrees(&mut data);

        // Stale worktree should be removed
        assert_eq!(data.projects.len(), 1);
        assert_eq!(data.projects[0].id, "p1");
        assert!(!data.project_order.contains(&"wt1".to_string()));
    }

    #[test]
    fn sync_worktrees_cleans_up_stale_worktree_from_folders() {
        let mut wt_project = make_project("wt1");
        wt_project.path = "/nonexistent/path".to_string();
        wt_project.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "p1".to_string(),
                color_override: None,
            main_repo_path: "/tmp/test".to_string(),
            worktree_path: String::new(),
            branch_name: "some-branch".to_string(),
        });

        let mut data = make_workspace(
            vec![make_project("p1"), wt_project],
            vec!["f1"],
            vec![FolderData {
                id: "f1".to_string(),
                name: "Folder".to_string(),
                project_ids: vec!["p1".to_string(), "wt1".to_string()],
                collapsed: false,
                folder_color: FolderColor::default(),
            }],
        );

        sync_worktrees(&mut data);

        assert_eq!(data.folders[0].project_ids, vec!["p1".to_string()]);
    }

    #[test]
    fn sync_worktrees_preserves_existing_worktree_with_valid_path() {
        let mut wt_project = make_project("wt1");
        // Use a path that exists (temp dir)
        let tmp = std::env::temp_dir();
        wt_project.path = tmp.to_string_lossy().to_string();
        wt_project.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "p1".to_string(),
                color_override: None,
            main_repo_path: "/tmp/test".to_string(),
            worktree_path: String::new(),
            branch_name: "some-branch".to_string(),
        });

        let mut data = make_workspace(
            vec![make_project("p1"), wt_project],
            vec!["p1", "wt1"],
            vec![],
        );

        sync_worktrees(&mut data);

        // Should still have both projects
        assert_eq!(data.projects.len(), 2);
        assert!(data.project_order.contains(&"wt1".to_string()));
    }

    // === validate_workspace_data worktree migration ===

    #[test]
    fn validate_populates_worktree_ids_from_worktree_info() {
        // Simulate old data: worktrees in project_order, parent has empty worktree_ids
        let mut data = make_workspace(
            vec![make_project("parent"), make_worktree_project("wt1", "parent"), make_worktree_project("wt2", "parent")],
            vec!["parent", "wt1", "wt2"],
            vec![],
        );
        validate_workspace_data(&mut data, false, SessionBackend::None);

        // Parent should now have worktree_ids populated
        let parent = data.projects.iter().find(|p| p.id == "parent").unwrap();
        assert_eq!(parent.worktree_ids, vec!["wt1".to_string(), "wt2".to_string()]);
    }

    #[test]
    fn validate_removes_worktrees_from_project_order() {
        let mut data = make_workspace(
            vec![make_project("parent"), make_worktree_project("wt1", "parent")],
            vec!["parent", "wt1"],
            vec![],
        );
        validate_workspace_data(&mut data, false, SessionBackend::None);

        // wt1 should be removed from project_order (lives in parent.worktree_ids now)
        assert!(!data.project_order.contains(&"wt1".to_string()));
        assert!(data.project_order.contains(&"parent".to_string()));
    }

    #[test]
    fn validate_removes_worktrees_from_folder_project_ids() {
        let mut data = make_workspace(
            vec![make_project("parent"), make_worktree_project("wt1", "parent")],
            vec!["f1"],
            vec![FolderData {
                id: "f1".to_string(),
                name: "Folder".to_string(),
                project_ids: vec!["parent".to_string(), "wt1".to_string()],
                collapsed: false,
                folder_color: FolderColor::default(),
            }],
        );
        validate_workspace_data(&mut data, false, SessionBackend::None);

        // wt1 should be removed from folder's project_ids
        assert_eq!(data.folders[0].project_ids, vec!["parent".to_string()]);
    }

    #[test]
    fn validate_preserves_existing_worktree_ids() {
        // Parent already has worktree_ids set — migration should not overwrite
        let mut parent = make_project("parent");
        parent.worktree_ids = vec!["wt2".to_string(), "wt1".to_string()]; // custom order
        let mut data = make_workspace(
            vec![parent, make_worktree_project("wt1", "parent"), make_worktree_project("wt2", "parent")],
            vec!["parent"],
            vec![],
        );
        validate_workspace_data(&mut data, false, SessionBackend::None);

        let parent = data.projects.iter().find(|p| p.id == "parent").unwrap();
        // Should preserve existing order, not overwrite
        assert_eq!(parent.worktree_ids, vec!["wt2".to_string(), "wt1".to_string()]);
    }
}
