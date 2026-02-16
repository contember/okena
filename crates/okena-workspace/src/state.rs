//! Workspace GPUI entity — coordinator over persistent data and transient
//! per-session state.
//!
//! Data types (`WorkspaceData`, `ProjectData`, `LayoutNode`, etc.) live in
//! `okena-state` / `okena-layout` and are re-exported here so existing
//! `crate::state::*` imports keep working.

use okena_core::theme::FolderColor;
use crate::access_history::ProjectAccessHistory;
use crate::focus::FocusManager;
use crate::lifecycle::ProjectLifecycleTracker;
use crate::remote_sync::{RemoteProjectSnapshot, RemoteSyncState};
use crate::visibility::compute_visible_projects;
use gpui::*;
use std::collections::HashMap;

pub use okena_layout::{LayoutNode, SplitDirection};
pub use okena_state::{
    DropZone, FocusedTerminalState, FolderData, HookTerminalEntry, HookTerminalStatus,
    PendingWorktreeClose, ProjectData, WorkspaceData, WorktreeMetadata,
};

/// Global workspace wrapper for app-wide access (used by quit handler)
#[derive(Clone)]
pub struct GlobalWorkspace(pub Entity<Workspace>);

impl Global for GlobalWorkspace {}

/// GPUI Entity for workspace state.
///
/// Composes focused helper types by ownership. `Workspace` itself is a
/// coordinator — it does not own the raw transient HashSets/HashMaps directly.
pub struct Workspace {
    pub data: WorkspaceData,
    /// Unified focus manager for the workspace.
    pub focus_manager: FocusManager,
    /// Transient project lifecycle state (creating / closing / removing).
    pub lifecycle: ProjectLifecycleTracker,
    /// Remote-sync coordination state (pending focus, remote snapshots).
    pub remote_sync: RemoteSyncState,
    /// Per-project last-access timestamps, for "recently used" sorting.
    pub access_history: ProjectAccessHistory,
    /// Monotonic counter incremented only on persistent data mutations.
    /// The auto-save observer compares this to skip saves for UI-only changes.
    data_version: u64,
    /// Transient folder filter — when set, only projects from this folder are shown.
    /// Not serialized; resets to None on restart.
    pub active_folder_filter: Option<String>,
}

impl Workspace {
    pub fn new(data: WorkspaceData) -> Self {
        Self {
            data,
            focus_manager: FocusManager::new(),
            lifecycle: ProjectLifecycleTracker::new(),
            remote_sync: RemoteSyncState::new(),
            access_history: ProjectAccessHistory::new(),
            data_version: 0,
            active_folder_filter: None,
        }
    }

    /// Current data version (incremented on persistent data mutations)
    pub fn data_version(&self) -> u64 {
        self.data_version
    }

    /// Read-only access to persistent workspace data.
    pub fn data(&self) -> &WorkspaceData {
        &self.data
    }

    /// Notify that persistent data changed. Bumps version, calls cx.notify(),
    /// and refreshes all windows to bypass `.cached()` view wrappers.
    /// Use this instead of cx.notify() when mutating `self.data`.
    pub fn notify_data(&mut self, cx: &mut Context<Self>) {
        self.data_version += 1;
        cx.notify();
        cx.refresh_windows();
    }

    /// Replace workspace data wholesale (e.g. from disk reload).
    /// Does NOT bump data_version — the data came from disk, not a user edit.
    pub fn replace_data(&mut self, data: WorkspaceData, cx: &mut Context<Self>) {
        self.data = data;
        self.focus_manager.clear_all();
        self.active_folder_filter = None;
        cx.notify();
    }

    /// Record that a project was accessed (for sorting by recency)
    pub fn touch_project(&mut self, project_id: &str) {
        self.access_history.touch(project_id);
    }

    /// Get projects sorted by last access time (most recent first)
    pub fn projects_by_recency(&self) -> Vec<&ProjectData> {
        let mut projects: Vec<&ProjectData> = self.data.projects.iter().collect();
        projects.sort_by(|a, b| self.access_history.cmp_by_recency(&a.id, &b.id));
        projects
    }

    pub fn active_folder_filter(&self) -> Option<&String> {
        self.active_folder_filter.as_ref()
    }

    pub fn set_folder_filter(&mut self, folder_id: Option<String>, cx: &mut Context<Self>) {
        self.active_folder_filter = folder_id;
        cx.notify();
    }

    // === ProjectLifecycleTracker conveniences ===

    pub fn is_creating_project(&self, project_id: &str) -> bool {
        self.lifecycle.is_creating(project_id)
    }

    pub fn mark_creating_project(&mut self, project_id: &str) {
        self.lifecycle.mark_creating(project_id);
    }

    pub fn finish_creating_project(&mut self, project_id: &str) {
        self.lifecycle.finish_creating(project_id);
    }

    pub fn mark_worktree_removing(&mut self, path: &str) {
        self.lifecycle.mark_worktree_removing(path);
    }

    pub fn finish_worktree_removing(&mut self, path: &str) {
        self.lifecycle.finish_worktree_removing(path);
    }

    pub fn finish_closing_project(&mut self, project_id: &str) {
        self.lifecycle.finish_closing(project_id);
    }

    // === RemoteSyncState conveniences ===

    pub fn queue_pending_remote_focus(&mut self, project_id: &str) {
        self.remote_sync.queue_focus(project_id);
    }

    pub fn drain_pending_remote_focus(&mut self) -> Vec<String> {
        self.remote_sync.drain_pending_focus()
    }

    pub fn remote_snapshot(&self, project_id: &str) -> Option<&RemoteProjectSnapshot> {
        self.remote_sync.snapshot(project_id)
    }

    /// Update the saved service terminal IDs for a project.
    /// Called by the ServiceManager observer to persist terminal IDs across restarts.
    pub fn sync_service_terminals(&mut self, project_id: &str, terminals: HashMap<String, String>, cx: &mut Context<Self>) {
        if let Some(project) = self.data.projects.iter_mut().find(|p| p.id == project_id) {
            if project.service_terminals != terminals {
                project.service_terminals = terminals;
                self.notify_data(cx);
            }
        }
    }

    pub fn register_hook_terminal(
        &mut self,
        project_id: &str,
        terminal_id: &str,
        entry: HookTerminalEntry,
        cx: &mut Context<Self>,
    ) {
        if let Some(project) = self.data.projects.iter_mut().find(|p| p.id == project_id) {
            let label = entry.label.clone();
            project.hook_terminals.insert(terminal_id.to_string(), entry);

            // Hook terminals are displayed in the dedicated HookPanel (not in the layout tree).
            // Set the terminal name so the panel can display it.
            project.terminal_names.insert(terminal_id.to_string(), label);

            self.notify_data(cx);
        }
    }

    /// Register hook terminal results from a hook execution.
    /// Convenience wrapper that converts `HookTerminalResult`s into `HookTerminalEntry`s.
    pub fn register_hook_results(
        &mut self,
        results: Vec<crate::hooks::HookTerminalResult>,
        cx: &mut Context<Self>,
    ) {
        for result in results {
            self.register_hook_terminal(&result.project_id, &result.terminal_id, HookTerminalEntry {
                label: result.label,
                status: HookTerminalStatus::Running,
                hook_type: result.hook_type.to_string(),
                command: result.command,
                cwd: result.cwd,
            }, cx);
        }
    }

    pub fn update_hook_terminal_status(
        &mut self,
        terminal_id: &str,
        status: HookTerminalStatus,
        cx: &mut Context<Self>,
    ) {
        for project in &mut self.data.projects {
            if let Some(entry) = project.hook_terminals.get_mut(terminal_id) {
                if entry.status != status {
                    entry.status = status;
                    cx.notify();
                }
                return;
            }
        }
    }

    pub fn remove_hook_terminal(
        &mut self,
        terminal_id: &str,
        cx: &mut Context<Self>,
    ) {
        for project in &mut self.data.projects {
            if project.hook_terminals.remove(terminal_id).is_some() {
                if let Some(ref layout) = project.layout {
                    if let Some(path) = layout.find_terminal_path(terminal_id) {
                        if path.is_empty() {
                            project.layout = None;
                        } else if let Some(ref mut layout) = project.layout {
                            layout.remove_at_path(&path);
                        }
                    }
                }
                project.terminal_names.remove(terminal_id);
                self.notify_data(cx);
                return;
            }
        }
    }

    pub fn is_hook_terminal(&self, terminal_id: &str) -> Option<String> {
        for project in &self.data.projects {
            if project.hook_terminals.contains_key(terminal_id) {
                return Some(project.id.clone());
            }
        }
        None
    }

    /// Find the project that owns a terminal by scanning project layouts.
    /// Returns a reference to the `ProjectData` if found.
    pub fn find_project_for_terminal(&self, terminal_id: &str) -> Option<&ProjectData> {
        self.data.projects.iter().find(|p| {
            p.layout.as_ref().map_or(false, |l| l.find_terminal_path(terminal_id).is_some())
        })
    }

    /// Get all hook terminal IDs for a project (for cleanup before deletion).
    pub fn hook_terminal_ids_for_project(&self, project_id: &str) -> Vec<String> {
        self.project(project_id)
            .map(|p| p.hook_terminals.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Swap a hook terminal's ID (for rerun). Updates hook_terminals, layout tree, and terminal_names.
    /// Resets status back to Running.
    pub fn swap_hook_terminal_id(
        &mut self,
        project_id: &str,
        old_id: &str,
        new_id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.data.projects.iter_mut().find(|p| p.id == project_id) else {
            return;
        };

        if let Some(mut entry) = project.hook_terminals.remove(old_id) {
            entry.status = HookTerminalStatus::Running;
            project.hook_terminals.insert(new_id.to_string(), entry);
        }

        if let Some(ref mut layout) = project.layout {
            layout.replace_terminal_id(old_id, new_id);
        }

        if let Some(name) = project.terminal_names.remove(old_id) {
            project.terminal_names.insert(new_id.to_string(), name);
        }

        self.notify_data(cx);
    }

    /// Register a pending worktree close that will execute when the hook terminal exits.
    pub fn register_pending_worktree_close(&mut self, pending: PendingWorktreeClose) {
        self.lifecycle.register_pending_close(pending);
    }

    /// Take a pending worktree close for the given terminal ID (removes it).
    pub fn take_pending_worktree_close(&mut self, terminal_id: &str) -> Option<PendingWorktreeClose> {
        self.lifecycle.take_pending_close(terminal_id)
    }

    /// Cancel a pending worktree close: remove it and unmark the project as closing.
    pub fn cancel_pending_worktree_close(&mut self, terminal_id: &str) {
        self.lifecycle.cancel_pending_close(terminal_id);
    }

    /// Check if a project is currently being closed (hook running or removal in progress).
    pub fn is_project_closing(&self, project_id: &str) -> bool {
        self.lifecycle.is_closing(project_id)
    }

    pub fn projects(&self) -> &[ProjectData] {
        &self.data.projects
    }

    /// Get the currently focused/zoomed project ID.
    /// Delegates to FocusManager (single source of truth).
    pub fn focused_project_id(&self) -> Option<&String> {
        self.focus_manager.focused_project_id()
    }

    /// Get visible projects in order, expanding folders into their contained projects.
    /// When a folder filter is active, only projects from that folder are shown
    /// (top-level projects are hidden). Focused project override still takes priority.
    pub fn visible_projects(&self) -> Vec<&ProjectData> {
        compute_visible_projects(
            &self.data,
            self.focused_project_id(),
            self.focus_manager.is_focus_individual(),
            self.active_folder_filter.as_ref(),
        )
    }

    /// Get IDs of worktree children for a given parent project.
    pub fn worktree_child_ids(&self, parent_id: &str) -> Vec<String> {
        self.data.projects.iter()
            .filter(|p| p.worktree_info.as_ref().map_or(false, |w| w.parent_project_id == parent_id))
            .map(|p| p.id.clone())
            .collect()
    }

    /// Get a project by ID
    pub fn project(&self, id: &str) -> Option<&ProjectData> {
        self.data.projects.iter().find(|p| p.id == id)
    }

    /// Get the parent project's path for a worktree project (i.e. the main repo path).
    pub fn worktree_parent_path(&self, project_id: &str) -> Option<String> {
        self.project(project_id)
            .and_then(|p| p.worktree_info.as_ref())
            .and_then(|wt| self.project(&wt.parent_project_id))
            .map(|parent| parent.path.clone())
    }

    /// Get the effective folder color for a project, resolving through worktree parent if needed.
    /// Worktrees with a `color_override` use that; otherwise they inherit the parent's color.
    pub fn effective_folder_color(&self, project: &ProjectData) -> FolderColor {
        if let Some(ref wt) = project.worktree_info {
            if let Some(override_color) = wt.color_override {
                override_color
            } else {
                self.project(&wt.parent_project_id)
                    .map(|p| p.folder_color)
                    .unwrap_or(project.folder_color)
            }
        } else {
            project.folder_color
        }
    }

    /// Get a mutable project by ID
    pub(crate) fn project_mut(&mut self, id: &str) -> Option<&mut ProjectData> {
        self.data.projects.iter_mut().find(|p| p.id == id)
    }

    /// Get a folder by ID
    pub fn folder(&self, id: &str) -> Option<&FolderData> {
        self.data.folders.iter().find(|f| f.id == id)
    }

    /// Get a mutable folder by ID
    pub(crate) fn folder_mut(&mut self, id: &str) -> Option<&mut FolderData> {
        self.data.folders.iter_mut().find(|f| f.id == id)
    }

    /// Check if an ID in project_order refers to a folder
    #[allow(dead_code)]
    pub fn is_folder(&self, id: &str) -> bool {
        self.data.folders.iter().any(|f| f.id == id)
    }

    /// Find which folder (if any) contains a given project
    pub fn folder_for_project(&self, project_id: &str) -> Option<&FolderData> {
        self.data.folders.iter().find(|f| f.project_ids.contains(&project_id.to_string()))
    }

    /// Find folder for a project, falling back to the parent project's folder for worktrees.
    pub fn folder_for_project_or_parent(&self, project_id: &str) -> Option<&FolderData> {
        self.folder_for_project(project_id)
            .or_else(|| {
                self.project(project_id)
                    .and_then(|p| p.worktree_info.as_ref())
                    .and_then(|wt| self.folder_for_project(&wt.parent_project_id))
            })
    }

    /// Collect all detached terminals across all projects by traversing layout trees.
    /// Returns (terminal_id, project_id, layout_path) tuples.
    pub fn collect_all_detached_terminals(&self) -> Vec<(String, String, Vec<usize>)> {
        let mut result = Vec::new();
        for project in &self.data.projects {
            if let Some(ref layout) = project.layout {
                for (terminal_id, layout_path) in layout.collect_detached_terminals() {
                    result.push((terminal_id, project.id.clone(), layout_path));
                }
            }
        }
        result
    }

    /// Check if a project is remote
    #[allow(dead_code)]
    pub fn is_remote_project(&self, id: &str) -> bool {
        self.data.projects.iter().any(|p| p.id == id && p.is_remote)
    }

    /// Remove all remote projects (and their folder) for a given connection_id.
    #[allow(dead_code)]
    pub fn remove_remote_projects(&mut self, connection_id: &str, cx: &mut Context<Self>) {
        let prefix = format!("remote:{}:", connection_id);

        self.data.projects.retain(|p| !p.id.starts_with(&prefix));
        self.data.folders.retain(|f| !f.id.starts_with(&prefix));
        self.data.project_order.retain(|id| !id.starts_with(&prefix));
        self.data.project_widths.retain(|id, _| !id.starts_with(&prefix));

        self.remote_sync.retain_not_starting_with(&prefix);

        if let Some(focused) = self.focus_manager.focused_project_id() {
            if focused.starts_with(&prefix) {
                self.focus_manager.set_focused_project_id(None);
            }
        }

        cx.notify();
    }

    /// Notify UI without bumping data_version (for remote state changes that shouldn't trigger auto-save).
    pub fn notify_ui_only(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }

    /// Helper to mutate a layout node at a path, with automatic notify.
    /// Returns true if the mutation was applied.
    pub fn with_layout_node<F>(&mut self, project_id: &str, path: &[usize], cx: &mut Context<Self>, f: F) -> bool
    where
        F: FnOnce(&mut LayoutNode) -> bool,
    {
        if let Some(project) = self.project_mut(project_id) {
            if let Some(ref mut layout) = project.layout {
                if let Some(node) = layout.get_at_path_mut(path) {
                    if f(node) {
                        self.notify_data(cx);
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Helper to mutate a project, with automatic notify.
    /// Returns true if the mutation was applied.
    pub fn with_project<F>(&mut self, project_id: &str, cx: &mut Context<Self>, f: F) -> bool
    where
        F: FnOnce(&mut ProjectData) -> bool,
    {
        if let Some(project) = self.project_mut(project_id) {
            if f(project) {
                self.notify_data(cx);
                return true;
            }
        }
        false
    }
}


#[cfg(test)]
mod workspace_tests {
    use crate::state::{
        FolderData, LayoutNode, ProjectData, SplitDirection, Workspace, WorkspaceData,
        WorktreeMetadata,
    };
    use okena_terminal::shell_config::ShellType;
    use okena_core::theme::FolderColor;
    use crate::settings::HooksConfig;
    use std::collections::HashMap;

    fn make_project(id: &str, visible: bool) -> ProjectData {
        ProjectData {
            id: id.to_string(),
            name: format!("Project {}", id),
            path: "/tmp/test".to_string(),
            show_in_overview: visible,
            layout: Some(LayoutNode::Terminal {
                terminal_id: Some(format!("term_{}", id)),
                minimized: false,
                detached: false,
                shell_type: ShellType::Default,
                zoom_level: 1.0,
            }),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            default_shell: None,
            hook_terminals: HashMap::new(),
        }
    }

    fn make_workspace_data(projects: Vec<ProjectData>, order: Vec<&str>) -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects,
            project_order: order.into_iter().map(String::from).collect(),
            project_widths: HashMap::new(),
            service_panel_heights: HashMap::new(),
            hook_panel_heights: HashMap::new(),
            folders: Vec::new(),
        }
    }

    #[test]
    fn test_visible_projects_filters_hidden() {
        let data = make_workspace_data(
            vec![make_project("p1", true), make_project("p2", false), make_project("p3", true)],
            vec!["p1", "p2", "p3"],
        );
        let ws = Workspace::new(data);

        let visible = ws.visible_projects();
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].id, "p1");
        assert_eq!(visible[1].id, "p3");
    }

    #[test]
    fn test_visible_projects_with_focused_project() {
        let data = make_workspace_data(
            vec![make_project("p1", true), make_project("p2", true), make_project("p3", false)],
            vec!["p1", "p2", "p3"],
        );
        let mut ws = Workspace::new(data);

        ws.focus_manager.set_focused_project_id(Some("p3".to_string()));

        let visible = ws.visible_projects();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "p3");
    }

    #[test]
    fn test_visible_projects_with_folder() {
        let mut data = make_workspace_data(
            vec![make_project("p1", true), make_project("p2", true)],
            vec!["f1"],
        );
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["p1".to_string(), "p2".to_string()],
            collapsed: false,
            folder_color: FolderColor::default(),
        }];

        let ws = Workspace::new(data);

        let visible = ws.visible_projects();
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].id, "p1");
        assert_eq!(visible[1].id, "p2");
    }

    #[test]
    fn test_projects_by_recency() {
        let data = make_workspace_data(
            vec![make_project("p1", true), make_project("p2", true), make_project("p3", true)],
            vec!["p1", "p2", "p3"],
        );
        let mut ws = Workspace::new(data);

        ws.touch_project("p3");
        ws.touch_project("p1");

        let recency = ws.projects_by_recency();
        assert_eq!(recency[0].id, "p1");
        assert_eq!(recency[1].id, "p3");
        assert_eq!(recency[2].id, "p2");
    }

    #[test]
    fn test_collect_all_detached_terminals() {
        let mut project = make_project("p1", true);
        project.layout = Some(LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![50.0, 50.0],
            children: vec![
                LayoutNode::Terminal {
                    terminal_id: Some("t1".to_string()),
                    minimized: false,
                    detached: true,
                    shell_type: ShellType::Default,
                    zoom_level: 1.0,
                },
                LayoutNode::Terminal {
                    terminal_id: Some("t2".to_string()),
                    minimized: false,
                    detached: false,
                    shell_type: ShellType::Default,
                    zoom_level: 1.0,
                },
            ],
        });
        let data = make_workspace_data(vec![project], vec!["p1"]);
        let ws = Workspace::new(data);

        let detached = ws.collect_all_detached_terminals();
        assert_eq!(detached.len(), 1);
        assert_eq!(detached[0].0, "t1");
        assert_eq!(detached[0].1, "p1");
        assert_eq!(detached[0].2, vec![0]);
    }

    #[test]
    fn test_folder_for_project() {
        let mut data = make_workspace_data(
            vec![make_project("p1", true), make_project("p2", true)],
            vec!["f1", "p2"],
        );
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["p1".to_string()],
            collapsed: false,
            folder_color: FolderColor::default(),
        }];
        let ws = Workspace::new(data);

        assert_eq!(ws.folder_for_project("p1").unwrap().id, "f1");
        assert!(ws.folder_for_project("p2").is_none());
    }

    #[test]
    fn test_visible_projects_with_folder_filter() {
        let mut data = make_workspace_data(
            vec![
                make_project("p1", true), make_project("p2", true),
                make_project("p3", true), make_project("p4", true),
                make_project("p5", true),
            ],
            vec!["f1", "f2", "p5"],
        );
        data.folders = vec![
            FolderData {
                id: "f1".to_string(),
                name: "Folder 1".to_string(),
                project_ids: vec!["p1".to_string(), "p2".to_string()],
                collapsed: false,
                folder_color: FolderColor::default(),
            },
            FolderData {
                id: "f2".to_string(),
                name: "Folder 2".to_string(),
                project_ids: vec!["p3".to_string(), "p4".to_string()],
                collapsed: false,
                folder_color: FolderColor::default(),
            },
        ];

        let mut ws = Workspace::new(data);

        assert_eq!(ws.visible_projects().len(), 5);

        ws.active_folder_filter = Some("f1".to_string());
        let visible = ws.visible_projects();
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].id, "p1");
        assert_eq!(visible[1].id, "p2");

        ws.active_folder_filter = Some("f2".to_string());
        let visible = ws.visible_projects();
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].id, "p3");
        assert_eq!(visible[1].id, "p4");
    }

    #[test]
    fn test_folder_filter_hides_top_level_projects() {
        let mut data = make_workspace_data(
            vec![
                make_project("p1", true), make_project("p2", true),
                make_project("p3", true),
            ],
            vec!["f1", "p3"],
        );
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["p1".to_string(), "p2".to_string()],
            collapsed: false,
            folder_color: FolderColor::default(),
        }];

        let mut ws = Workspace::new(data);
        ws.active_folder_filter = Some("f1".to_string());

        let visible = ws.visible_projects();
        assert_eq!(visible.len(), 2);
        assert!(visible.iter().all(|p| p.id != "p3"));
    }

    #[test]
    fn test_visible_projects_worktree_focus() {
        let mut p1 = make_project("p1", true);
        p1.worktree_ids = vec!["w1".to_string(), "w2".to_string()];
        let mut w1 = make_project("w1", true);
        w1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "p1".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: "branch-w1".to_string(),
        });
        let mut w2 = make_project("w2", true);
        w2.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "p1".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt2".to_string(),
            branch_name: "branch-w2".to_string(),
        });

        let data = make_workspace_data(
            vec![p1, w1, w2, make_project("p2", true)],
            vec!["p1", "p2"],
        );
        let mut ws = Workspace::new(data);

        ws.focus_manager.set_focused_project_id(Some("p1".to_string()));
        let visible = ws.visible_projects();
        assert_eq!(visible.len(), 3);
        assert_eq!(visible[0].id, "p1");
        assert_eq!(visible[1].id, "w1");
        assert_eq!(visible[2].id, "w2");

        ws.focus_manager.set_focused_project_id(Some("w1".to_string()));
        let visible = ws.visible_projects();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "w1");

        ws.focus_manager.set_focused_project_id(None);
        let visible = ws.visible_projects();
        assert_eq!(visible.len(), 4);
    }

    #[test]
    fn test_folder_filter_includes_worktree_children() {
        let mut p1 = make_project("p1", true);
        p1.worktree_ids = vec!["w1".to_string(), "w2".to_string()];
        let mut w1 = make_project("w1", true);
        w1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "p1".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: "branch-w1".to_string(),
        });
        let mut w2 = make_project("w2", true);
        w2.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "p1".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt2".to_string(),
            branch_name: "branch-w2".to_string(),
        });

        let mut data = make_workspace_data(
            vec![p1, w1, w2, make_project("p2", true)],
            vec!["f1", "p2"],
        );
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["p1".to_string()],
            collapsed: false,
            folder_color: FolderColor::default(),
        }];

        let mut ws = Workspace::new(data);

        assert_eq!(ws.visible_projects().len(), 4);

        ws.active_folder_filter = Some("f1".to_string());
        let visible = ws.visible_projects();
        assert_eq!(visible.len(), 3);
        assert_eq!(visible[0].id, "p1");
        assert_eq!(visible[1].id, "w1");
        assert_eq!(visible[2].id, "w2");
    }

    #[test]
    fn test_folder_filter_worktree_children_not_duplicated() {
        let mut w1 = make_project("w1", true);
        w1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "p1".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: "branch-w1".to_string(),
        });

        let mut p1 = make_project("p1", true);
        p1.worktree_ids = vec!["w1".to_string()];

        let mut data = make_workspace_data(
            vec![p1, w1, make_project("p2", true)],
            vec!["f1", "w1", "p2"],
        );
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["p1".to_string()],
            collapsed: false,
            folder_color: FolderColor::default(),
        }];

        let mut ws = Workspace::new(data);
        ws.active_folder_filter = Some("f1".to_string());

        let visible = ws.visible_projects();
        assert_eq!(visible.len(), 2);
        assert_eq!(visible.iter().filter(|p| p.id == "w1").count(), 1);
    }

    #[test]
    fn test_worktree_children_ordered_within_folder_section() {
        let mut w1 = make_project("w1", true);
        w1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "p1".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: "branch-w1".to_string(),
        });

        let mut p1 = make_project("p1", true);
        p1.worktree_ids = vec!["w1".to_string()];

        let mut data = make_workspace_data(
            vec![p1, make_project("p2", true), w1, make_project("p3", true)],
            vec!["f1", "w1", "f2", "p3"],
        );
        data.folders = vec![
            FolderData {
                id: "f1".to_string(),
                name: "Folder 1".to_string(),
                project_ids: vec!["p1".to_string()],
                collapsed: false,
                folder_color: FolderColor::default(),
            },
            FolderData {
                id: "f2".to_string(),
                name: "Folder 2".to_string(),
                project_ids: vec!["p2".to_string()],
                collapsed: false,
                folder_color: FolderColor::default(),
            },
        ];

        let ws = Workspace::new(data);
        let visible = ws.visible_projects();

        assert_eq!(visible.len(), 4);
        assert_eq!(visible[0].id, "p1");
        assert_eq!(visible[1].id, "w1");
        assert_eq!(visible[2].id, "p2");
        assert_eq!(visible[3].id, "p3");
    }

    #[test]
    fn test_worktree_before_parent_folder_in_project_order() {
        let mut w1 = make_project("w1", true);
        w1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "p2".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: "branch-w1".to_string(),
        });

        let mut p2 = make_project("p2", false);
        p2.worktree_ids = vec!["w1".to_string()];

        let mut data = make_workspace_data(
            vec![make_project("p1", true), p2, w1],
            vec!["w1", "f1", "f2"],
        );
        data.folders = vec![
            FolderData {
                id: "f1".to_string(),
                name: "Folder 1".to_string(),
                project_ids: vec!["p1".to_string()],
                collapsed: false,
                folder_color: FolderColor::default(),
            },
            FolderData {
                id: "f2".to_string(),
                name: "Folder 2".to_string(),
                project_ids: vec!["p2".to_string()],
                collapsed: false,
                folder_color: FolderColor::default(),
            },
        ];

        let ws = Workspace::new(data);
        let visible = ws.visible_projects();

        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].id, "p1");
        assert_eq!(visible[1].id, "w1");
        assert_eq!(visible.iter().filter(|p| p.id == "w1").count(), 1);
    }

    #[test]
    fn test_worktree_children_ordered_when_parent_hidden() {
        let mut w1 = make_project("w1", true);
        w1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "p1".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: "branch-w1".to_string(),
        });

        let mut p1 = make_project("p1", false);
        p1.worktree_ids = vec!["w1".to_string()];

        let mut data = make_workspace_data(
            vec![p1, make_project("p2", true), w1],
            vec!["f1", "w1", "f2"],
        );
        data.folders = vec![
            FolderData {
                id: "f1".to_string(),
                name: "Folder 1".to_string(),
                project_ids: vec!["p1".to_string()],
                collapsed: false,
                folder_color: FolderColor::default(),
            },
            FolderData {
                id: "f2".to_string(),
                name: "Folder 2".to_string(),
                project_ids: vec!["p2".to_string()],
                collapsed: false,
                folder_color: FolderColor::default(),
            },
        ];

        let ws = Workspace::new(data);
        let visible = ws.visible_projects();

        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].id, "w1");
        assert_eq!(visible[1].id, "p2");
    }

    #[test]
    fn test_worktree_child_in_folder_not_duplicated() {
        let mut w1 = make_project("w1", true);
        w1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "p1".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: "branch-w1".to_string(),
        });

        let mut data = make_workspace_data(
            vec![make_project("p1", true), w1, make_project("p2", true)],
            vec!["f1", "f2"],
        );
        data.folders = vec![
            FolderData {
                id: "f1".to_string(),
                name: "Folder 1".to_string(),
                project_ids: vec!["p1".to_string(), "w1".to_string()],
                collapsed: false,
                folder_color: FolderColor::default(),
            },
            FolderData {
                id: "f2".to_string(),
                name: "Folder 2".to_string(),
                project_ids: vec!["p2".to_string()],
                collapsed: false,
                folder_color: FolderColor::default(),
            },
        ];

        let ws = Workspace::new(data);
        let visible = ws.visible_projects();

        assert_eq!(visible.len(), 3);
        assert_eq!(visible[0].id, "p1");
        assert_eq!(visible[1].id, "w1");
        assert_eq!(visible[2].id, "p2");
        assert_eq!(visible.iter().filter(|p| p.id == "w1").count(), 1);
    }

    #[test]
    fn test_orphan_worktree_shown_when_parent_not_in_result() {
        let mut w1 = make_project("w1", true);
        w1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "p1".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: "branch-w1".to_string(),
        });

        let data = make_workspace_data(
            vec![make_project("p1", false), w1],
            vec!["p1", "w1"],
        );
        let ws = Workspace::new(data);

        let visible = ws.visible_projects();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "w1");
    }

    #[test]
    fn test_folder_filter_with_focus_override() {
        let mut data = make_workspace_data(
            vec![
                make_project("p1", true), make_project("p2", true),
                make_project("p3", true),
            ],
            vec!["f1", "p3"],
        );
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["p1".to_string(), "p2".to_string()],
            collapsed: false,
            folder_color: FolderColor::default(),
        }];

        let mut ws = Workspace::new(data);
        ws.active_folder_filter = Some("f1".to_string());

        ws.focus_manager.set_focused_project_id(Some("p3".to_string()));

        let visible = ws.visible_projects();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "p3");
    }

    #[test]
    fn test_visible_projects_includes_worktree_children() {
        let mut parent = make_project("parent", true);
        parent.worktree_ids = vec!["wt1".to_string(), "wt2".to_string()];
        let mut wt1 = make_project("wt1", true);
        wt1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "parent".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: String::new(),
        });
        let mut wt2 = make_project("wt2", true);
        wt2.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "parent".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt2".to_string(),
            branch_name: String::new(),
        });
        let data = make_workspace_data(vec![parent, wt1, wt2], vec!["parent"]);
        let ws = Workspace::new(data);

        let visible = ws.visible_projects();
        assert_eq!(visible.len(), 3);
        assert_eq!(visible[0].id, "parent");
        assert_eq!(visible[1].id, "wt1");
        assert_eq!(visible[2].id, "wt2");
    }

    #[test]
    fn test_visible_projects_worktree_children_in_folder() {
        let mut parent = make_project("parent", true);
        parent.worktree_ids = vec!["wt1".to_string()];
        let mut wt1 = make_project("wt1", true);
        wt1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "parent".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: String::new(),
        });
        let other = make_project("other", true);
        let mut data = make_workspace_data(vec![parent, wt1, other], vec!["f1", "other"]);
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["parent".to_string()],
            collapsed: false,
            folder_color: FolderColor::default(),
        }];
        let ws = Workspace::new(data);

        let visible = ws.visible_projects();
        assert_eq!(visible.len(), 3);
        assert_eq!(visible[0].id, "parent");
        assert_eq!(visible[1].id, "wt1");
        assert_eq!(visible[2].id, "other");
    }

    #[test]
    fn test_focus_parent_shows_parent_and_worktrees() {
        let mut parent = make_project("parent", true);
        parent.worktree_ids = vec!["wt1".to_string(), "wt2".to_string()];
        let mut wt1 = make_project("wt1", true);
        wt1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "parent".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: String::new(),
        });
        let mut wt2 = make_project("wt2", true);
        wt2.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "parent".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt2".to_string(),
            branch_name: String::new(),
        });
        let data = make_workspace_data(vec![parent, wt1, wt2], vec!["parent"]);
        let mut ws = Workspace::new(data);
        ws.focus_manager.set_focused_project_id(Some("parent".to_string()));

        let visible = ws.visible_projects();
        assert_eq!(visible.len(), 3);
        assert_eq!(visible[0].id, "parent");
        assert_eq!(visible[1].id, "wt1");
        assert_eq!(visible[2].id, "wt2");
    }

    #[test]
    fn test_focus_worktree_shows_only_worktree() {
        let mut parent = make_project("parent", true);
        parent.worktree_ids = vec!["wt1".to_string(), "wt2".to_string()];
        let mut wt1 = make_project("wt1", true);
        wt1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "parent".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: String::new(),
        });
        let mut wt2 = make_project("wt2", true);
        wt2.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "parent".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt2".to_string(),
            branch_name: String::new(),
        });
        let data = make_workspace_data(vec![parent, wt1, wt2], vec!["parent"]);
        let mut ws = Workspace::new(data);
        ws.focus_manager.set_focused_project_id(Some("wt1".to_string()));

        let visible = ws.visible_projects();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "wt1");
    }

    #[test]
    fn test_focus_parent_individual_shows_only_parent() {
        let mut parent = make_project("parent", true);
        parent.worktree_ids = vec!["wt1".to_string(), "wt2".to_string()];
        let mut wt1 = make_project("wt1", true);
        wt1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "parent".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: String::new(),
        });
        let mut wt2 = make_project("wt2", true);
        wt2.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "parent".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt2".to_string(),
            branch_name: String::new(),
        });
        let data = make_workspace_data(vec![parent, wt1, wt2], vec!["parent"]);
        let mut ws = Workspace::new(data);

        ws.focus_manager.set_focused_project_id_individual(Some("parent".to_string()));
        let visible = ws.visible_projects();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "parent");

        ws.focus_manager.set_focused_project_id(Some("parent".to_string()));
        let visible = ws.visible_projects();
        assert_eq!(visible.len(), 3);
    }
}

#[cfg(test)]
mod gpui_tests {
    use gpui::AppContext as _;
    use crate::state::{HookTerminalEntry, HookTerminalStatus, LayoutNode, ProjectData, Workspace, WorkspaceData};
    use crate::settings::HooksConfig;
    use okena_terminal::shell_config::ShellType;
    use okena_core::theme::FolderColor;
    use std::collections::HashMap;

    fn make_project(id: &str) -> ProjectData {
        ProjectData {
            id: id.to_string(),
            name: format!("Project {}", id),
            path: "/tmp/test".to_string(),
            show_in_overview: true,
            layout: Some(LayoutNode::Terminal {
                terminal_id: Some(format!("term_{}", id)),
                minimized: false,
                detached: false,
                shell_type: ShellType::Default,
                zoom_level: 1.0,
            }),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            default_shell: None,
            hook_terminals: HashMap::new(),
        }
    }

    fn make_workspace_data(projects: Vec<ProjectData>, order: Vec<&str>) -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects,
            project_order: order.into_iter().map(String::from).collect(),
            project_widths: HashMap::new(),
            service_panel_heights: HashMap::new(),
            hook_panel_heights: HashMap::new(),
            folders: vec![],
        }
    }

    #[gpui::test]
    fn test_with_layout_node_applies_mutation(cx: &mut gpui::TestAppContext) {
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let result = workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.with_layout_node("p1", &[], cx, |node| {
                if let LayoutNode::Terminal { minimized, .. } = node {
                    *minimized = true;
                    true
                } else {
                    false
                }
            })
        });
        assert!(result);

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            match layout {
                LayoutNode::Terminal { minimized, .. } => assert!(*minimized),
                _ => panic!("Expected terminal"),
            }
            assert_eq!(ws.data_version(), 1);
        });
    }

    #[gpui::test]
    fn test_with_layout_node_invalid_path_returns_false(cx: &mut gpui::TestAppContext) {
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let result = workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.with_layout_node("p1", &[99], cx, |_node| true)
        });
        assert!(!result);

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data_version(), 0);
        });
    }

    #[gpui::test]
    fn test_with_layout_node_invalid_project_returns_false(cx: &mut gpui::TestAppContext) {
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let result = workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.with_layout_node("nonexistent", &[], cx, |_node| true)
        });
        assert!(!result);

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data_version(), 0);
        });
    }


    #[gpui::test]
    fn test_replace_data_resets_focus(cx: &mut gpui::TestAppContext) {
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, _cx| {
            ws.focus_manager.set_focused_project_id(Some("p1".to_string()));
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.focused_project_id().is_some());
        });

        let new_data = make_workspace_data(vec![make_project("p2")], vec!["p2"]);
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.replace_data(new_data, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.focused_project_id().is_none());
            assert_eq!(ws.data().projects.len(), 1);
            assert_eq!(ws.data().projects[0].id, "p2");
        });
    }

    #[gpui::test]
    fn test_visible_projects_gpui(cx: &mut gpui::TestAppContext) {
        let mut p1 = make_project("p1");
        let p2 = make_project("p2");
        let mut p3 = make_project("p3");
        p1.show_in_overview = false;
        p3.show_in_overview = false;
        let data = make_workspace_data(vec![p1, p2, p3], vec!["p1", "p2", "p3"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let visible = ws.visible_projects();
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0].id, "p2");
        });

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_project_overview_visibility("p1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let visible = ws.visible_projects();
            assert_eq!(visible.len(), 2);
            assert_eq!(visible[0].id, "p1");
            assert_eq!(visible[1].id, "p2");
        });
    }

    fn make_remote_project(id: &str, conn_id: &str) -> ProjectData {
        let mut p = make_project(id);
        p.is_remote = true;
        p.connection_id = Some(conn_id.to_string());
        p
    }

    #[gpui::test]
    fn test_remove_remote_projects(cx: &mut gpui::TestAppContext) {
        use crate::state::FolderData;

        let local = make_project("local1");
        let remote1 = make_remote_project("remote:conn1:p1", "conn1");
        let remote2 = make_remote_project("remote:conn1:p2", "conn1");
        let remote3 = make_remote_project("remote:conn2:p1", "conn2");

        let mut data = make_workspace_data(
            vec![local, remote1, remote2, remote3],
            vec!["local1", "remote:conn1:folder1", "remote:conn2:folder2"],
        );
        data.folders.push(FolderData {
            id: "remote:conn1:folder1".to_string(),
            name: "Server 1".to_string(),
            project_ids: vec!["remote:conn1:p1".to_string(), "remote:conn1:p2".to_string()],
            collapsed: false,
            folder_color: FolderColor::default(),
        });
        data.folders.push(FolderData {
            id: "remote:conn2:folder2".to_string(),
            name: "Server 2".to_string(),
            project_ids: vec!["remote:conn2:p1".to_string()],
            collapsed: false,
            folder_color: FolderColor::default(),
        });

        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.remove_remote_projects("conn1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data.projects.len(), 2);
            assert!(ws.project("local1").is_some());
            assert!(ws.project("remote:conn2:p1").is_some());
            assert!(ws.project("remote:conn1:p1").is_none());

            assert_eq!(ws.data.folders.len(), 1);
            assert_eq!(ws.data.folders[0].id, "remote:conn2:folder2");

            assert!(!ws.data.project_order.contains(&"remote:conn1:folder1".to_string()));
            assert!(ws.data.project_order.contains(&"remote:conn2:folder2".to_string()));
        });
    }

    #[gpui::test]
    fn test_visible_projects_includes_remote_in_folders(cx: &mut gpui::TestAppContext) {
        use crate::state::FolderData;

        let local = make_project("local1");
        let mut remote1 = make_remote_project("remote:conn1:p1", "conn1");
        remote1.show_in_overview = true;
        let mut remote2 = make_remote_project("remote:conn1:p2", "conn1");
        remote2.show_in_overview = false;

        let mut data = make_workspace_data(
            vec![local, remote1, remote2],
            vec!["local1", "remote:conn1:folder1"],
        );
        data.folders.push(FolderData {
            id: "remote:conn1:folder1".to_string(),
            name: "Server 1".to_string(),
            project_ids: vec!["remote:conn1:p1".to_string(), "remote:conn1:p2".to_string()],
            collapsed: false,
            folder_color: FolderColor::default(),
        });

        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let visible = ws.visible_projects();
            assert_eq!(visible.len(), 2);
            assert_eq!(visible[0].id, "local1");
            assert_eq!(visible[1].id, "remote:conn1:p1");
        });
    }

    fn make_hook_entry(hook_type: &str) -> HookTerminalEntry {
        HookTerminalEntry {
            label: format!("{} (test)", hook_type),
            status: HookTerminalStatus::Running,
            hook_type: hook_type.to_string(),
            command: "echo test".to_string(),
            cwd: ".".to_string(),
        }
    }

    #[gpui::test]
    fn test_register_hook_terminal_no_layout(cx: &mut gpui::TestAppContext) {
        let mut p = make_project("p1");
        p.layout = None;
        let data = make_workspace_data(vec![p], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.register_hook_terminal("p1", "hook-1", make_hook_entry("on_project_open"), cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let p = ws.project("p1").unwrap();
            assert!(p.layout.is_none());
            assert!(p.hook_terminals.contains_key("hook-1"));
            assert!(p.terminal_names.contains_key("hook-1"));
        });
    }

    #[gpui::test]
    fn test_register_hook_terminal_does_not_modify_layout(cx: &mut gpui::TestAppContext) {
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.register_hook_terminal("p1", "hook-1", make_hook_entry("on_project_open"), cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let p = ws.project("p1").unwrap();
            let layout = p.layout.as_ref().unwrap();
            assert!(matches!(layout, LayoutNode::Terminal { terminal_id: Some(id), .. } if id == "term_p1"));
            assert!(p.hook_terminals.contains_key("hook-1"));
        });
    }

    #[gpui::test]
    fn test_register_multiple_hooks_stored_in_hashmap(cx: &mut gpui::TestAppContext) {
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.register_hook_terminal("p1", "hook-1", make_hook_entry("on_project_open"), cx);
            ws.register_hook_terminal("p1", "hook-2", make_hook_entry("pre_merge"), cx);
            ws.register_hook_terminal("p1", "hook-3", make_hook_entry("post_merge"), cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let p = ws.project("p1").unwrap();
            assert_eq!(p.hook_terminals.len(), 3);
            assert!(p.hook_terminals.contains_key("hook-1"));
            assert!(p.hook_terminals.contains_key("hook-2"));
            assert!(p.hook_terminals.contains_key("hook-3"));
            assert!(matches!(p.layout.as_ref().unwrap(), LayoutNode::Terminal { .. }));
        });
    }

    #[gpui::test]
    fn test_remove_hook_terminal_cleans_hashmap(cx: &mut gpui::TestAppContext) {
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.register_hook_terminal("p1", "hook-1", make_hook_entry("on_project_open"), cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.project("p1").unwrap().hook_terminals.contains_key("hook-1"));
        });

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.remove_hook_terminal("hook-1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let p = ws.project("p1").unwrap();
            assert!(p.hook_terminals.is_empty());
            assert!(!p.terminal_names.contains_key("hook-1"));
        });
    }

    #[gpui::test]
    fn test_hook_terminal_sets_name(cx: &mut gpui::TestAppContext) {
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.register_hook_terminal("p1", "hook-1", HookTerminalEntry {
                label: "on_project_open (feature/foo)".to_string(),
                status: HookTerminalStatus::Running,
                hook_type: "on_project_open".to_string(),
                command: "echo test".to_string(),
                cwd: ".".to_string(),
            }, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let name = ws.project("p1").unwrap().terminal_names.get("hook-1").unwrap();
            assert_eq!(name, "on_project_open (feature/foo)");
        });
    }

    #[gpui::test]
    fn test_swap_hook_terminal_id(cx: &mut gpui::TestAppContext) {
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.register_hook_terminal("p1", "hook-1", make_hook_entry("on_project_open"), cx);
            ws.update_hook_terminal_status("hook-1", HookTerminalStatus::Succeeded, cx);
        });

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.swap_hook_terminal_id("p1", "hook-1", "hook-1-new", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let project = ws.project("p1").unwrap();
            assert!(!project.hook_terminals.contains_key("hook-1"));
            let entry = project.hook_terminals.get("hook-1-new").unwrap();
            assert_eq!(entry.status, HookTerminalStatus::Running);
            assert_eq!(entry.hook_type, "on_project_open");
            assert!(!project.terminal_names.contains_key("hook-1"));
            assert!(project.terminal_names.contains_key("hook-1-new"));
        });
    }

    #[gpui::test]
    fn test_hook_terminal_ids_for_project(cx: &mut gpui::TestAppContext) {
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.register_hook_terminal("p1", "hook-1", make_hook_entry("on_project_open"), cx);
            ws.register_hook_terminal("p1", "hook-2", make_hook_entry("pre_merge"), cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let ids = ws.hook_terminal_ids_for_project("p1");
            assert_eq!(ids.len(), 2);
            assert!(ids.contains(&"hook-1".to_string()));
            assert!(ids.contains(&"hook-2".to_string()));

            assert!(ws.hook_terminal_ids_for_project("nonexistent").is_empty());
        });
    }
}
