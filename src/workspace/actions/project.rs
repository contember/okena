//! Project management workspace actions
//!
//! Actions for creating, modifying, and deleting projects.

use crate::theme::FolderColor;
use crate::workspace::hooks;
use crate::workspace::persistence::HooksConfig;
use crate::workspace::state::{LayoutNode, ProjectData, Workspace};
use gpui::*;
use std::collections::HashMap;

/// Expand `~` or `~/...` at the start of a path to the user's home directory.
/// Does not expand `~user/...` syntax (other user's home directories).
fn expand_tilde(path: &str) -> String {
    if path == "~" || path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            let rest = &path[1..]; // "" or "/..."
            return format!("{}{}", home.display(), rest);
        }
    }
    path.to_string()
}

impl Workspace {
    /// Toggle visibility for a single worktree (no propagation to children)
    pub fn toggle_worktree_visibility(&mut self, project_id: &str, cx: &mut Context<Self>) {
        self.with_project(project_id, cx, |project| {
            project.show_in_overview = !project.show_in_overview;
            true
        });
    }

    /// Toggle project overview visibility (also toggles all worktree children)
    pub fn toggle_project_overview_visibility(&mut self, project_id: &str, cx: &mut Context<Self>) {
        let new_visible = self.project(project_id).map(|p| !p.show_in_overview);
        let Some(new_visible) = new_visible else { return };

        self.with_project(project_id, cx, |project| {
            project.show_in_overview = new_visible;
            true
        });
    }

    /// Add a new project
    /// If `with_terminal` is false, creates a bookmark project without a terminal layout.
    pub fn add_project(&mut self, name: String, path: String, with_terminal: bool, cx: &mut Context<Self>) -> String {
        let path = expand_tilde(&path);

        // Auto-detect WSL UNC paths and set default shell accordingly
        #[cfg(windows)]
        let default_shell = crate::terminal::shell_config::parse_wsl_unc_path(&path)
            .map(|(distro, _)| crate::terminal::shell_config::ShellType::Wsl {
                distro: Some(distro),
            });
        #[cfg(not(windows))]
        let default_shell: Option<crate::terminal::shell_config::ShellType> = None;

        let id = uuid::Uuid::new_v4().to_string();
        let project = ProjectData {
            id: id.clone(),
            name: name.clone(),
            path: path.clone(),
            show_in_overview: true,
            layout: if with_terminal { Some(LayoutNode::new_terminal()) } else { None },
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            remote_services: Vec::new(),
            remote_host: None,
            remote_git_status: None,
            default_shell,
            hook_terminals: HashMap::new(),
        };
        let project_hooks = project.hooks.clone();
        self.data.projects.push(project);
        self.data.project_order.push(id.clone());
        self.notify_data(cx);

        let hook_results = hooks::fire_on_project_open(&project_hooks, &id, &name, &path, cx);
        self.register_hook_results(hook_results, cx);
        id
    }

    /// Add a new terminal to a project by splitting the root layout
    pub fn add_terminal(&mut self, project_id: &str, cx: &mut Context<Self>) {
        if let Some(project) = self.project_mut(project_id) {
            if let Some(ref old_layout) = project.layout {
                let old_layout = old_layout.clone();
                project.layout = Some(LayoutNode::Split {
                    direction: crate::workspace::state::SplitDirection::Vertical,
                    sizes: vec![50.0, 50.0],
                    children: vec![old_layout, LayoutNode::new_terminal()],
                });
            } else {
                // Project has no layout - create one with a terminal
                project.layout = Some(LayoutNode::new_terminal());
            }
            self.notify_data(cx);
        }

        // Focus the newly created terminal (terminal_id: None)
        let new_path = self.project(project_id)
            .and_then(|p| p.layout.as_ref())
            .and_then(|l| l.find_uninitialized_terminal_path());
        if let Some(path) = new_path {
            self.set_focused_terminal(project_id.to_string(), path, cx);
        }
    }

    /// Add a new terminal running a specific command to a project
    pub fn add_terminal_with_command(
        &mut self,
        project_id: &str,
        command: &str,
        env_vars: &HashMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(project) = self.project_mut(project_id) {
            let new_node = LayoutNode::new_terminal_with_command(command, env_vars);
            if let Some(ref old_layout) = project.layout {
                let old_layout = old_layout.clone();
                project.layout = Some(LayoutNode::Split {
                    direction: crate::workspace::state::SplitDirection::Vertical,
                    sizes: vec![50.0, 50.0],
                    children: vec![old_layout, new_node],
                });
            } else {
                project.layout = Some(new_node);
            }
            self.notify_data(cx);
        }
    }

    /// Rename a project
    pub fn rename_project(&mut self, project_id: &str, new_name: String, cx: &mut Context<Self>) {
        self.with_project(project_id, cx, |project| {
            project.name = new_name;
            true
        });
    }

    /// Rename a project's directory path and update the project name to match
    pub fn rename_project_directory(&mut self, project_id: &str, new_path: String, new_name: String, cx: &mut Context<Self>) {
        self.with_project(project_id, cx, |project| {
            project.path = new_path;
            project.name = new_name;
            true
        });
    }

    /// Set the folder color for a project (also propagates to worktree children)
    pub fn set_folder_color(&mut self, project_id: &str, color: FolderColor, cx: &mut Context<Self>) {
        self.with_project(project_id, cx, |project| {
            project.folder_color = color;
            true
        });
        // Propagate color to worktree children
        for child_id in self.worktree_child_ids(project_id) {
            self.with_project(&child_id, cx, |project| {
                project.folder_color = color;
                true
            });
        }
    }

    /// Delete a project
    pub fn delete_project(&mut self, project_id: &str, cx: &mut Context<Self>) {
        // Capture project info before removal for the hook
        let hook_info = self.project(project_id).map(|p| {
            (p.hooks.clone(), p.id.clone(), p.name.clone(), p.path.clone())
        });

        // Collect orphaned worktree children (if deleting a parent)
        let orphaned_worktrees: Vec<String> = self.project(project_id)
            .map(|p| p.worktree_ids.clone())
            .unwrap_or_default();

        // Remove from parent's worktree_ids (if deleting a worktree child)
        for parent in &mut self.data.projects {
            parent.worktree_ids.retain(|id| id != project_id);
        }

        // Remove from projects list
        self.data.projects.retain(|p| p.id != project_id);
        // Remove from project order
        self.data.project_order.retain(|id| id != project_id);
        // Remove from any folder's project_ids
        for folder in &mut self.data.folders {
            folder.project_ids.retain(|id| id != project_id);
        }

        // Re-home orphaned worktrees to project_order
        for wt_id in orphaned_worktrees {
            if self.data.projects.iter().any(|p| p.id == wt_id) && !self.data.project_order.contains(&wt_id) {
                self.data.project_order.push(wt_id);
            }
        }

        // Remove from widths
        self.data.project_widths.remove(project_id);
        // Clear closing state
        self.closing_projects.remove(project_id);
        // Clear focus if this was the focused project
        if self.focus_manager.focused_project_id().map(|s| s.as_str()) == Some(project_id) {
            self.focus_manager.set_focused_project_id(None);
        }
        // Exit fullscreen if this project's terminal was in fullscreen
        if self.focus_manager.fullscreen_project_id() == Some(project_id) {
            self.focus_manager.exit_fullscreen();
        }
        self.notify_data(cx);

        if let Some((project_hooks, id, name, path)) = hook_info {
            hooks::fire_on_project_close(&project_hooks, &id, &name, &path, cx);
        }
    }

    /// Move a project to a new position in the top-level order.
    /// Also removes the project from any folder it may be in.
    /// Worktree children are moved along with their parent.
    pub fn move_project(&mut self, project_id: &str, new_index: usize, cx: &mut Context<Self>) {
        // Remove from any folder first
        for folder in &mut self.data.folders {
            folder.project_ids.retain(|id| id != project_id);
        }

        // Collect worktree children IDs that should move with this project
        let wt_child_ids = self.worktree_child_ids(project_id);

        // Remove parent and its worktree children from project_order
        let removed: Vec<String> = {
            let ids_to_remove: std::collections::HashSet<&str> = std::iter::once(project_id)
                .chain(wt_child_ids.iter().map(|s| s.as_str()))
                .collect();
            let mut removed = Vec::new();
            self.data.project_order.retain(|id| {
                if ids_to_remove.contains(id.as_str()) {
                    removed.push(id.clone());
                    false
                } else {
                    true
                }
            });
            removed
        };

        // Insert at new position (parent first, then children in original relative order)
        let target = new_index.min(self.data.project_order.len());
        let mut to_insert: Vec<String> = Vec::with_capacity(removed.len() + 1);
        // Parent first (always insert, even if it wasn't in project_order before)
        to_insert.push(project_id.to_string());
        // Then worktree children in their original order
        for id in &removed {
            if id != project_id {
                to_insert.push(id.clone());
            }
        }
        for (offset, id) in to_insert.into_iter().enumerate() {
            let insert_at = (target + offset).min(self.data.project_order.len());
            self.data.project_order.insert(insert_at, id);
        }

        self.notify_data(cx);
    }

    /// Reorder a worktree within its parent's worktree_ids list
    pub fn reorder_worktree(&mut self, parent_id: &str, worktree_id: &str, new_index: usize, cx: &mut Context<Self>) {
        if let Some(parent) = self.data.projects.iter_mut().find(|p| p.id == parent_id) {
            if let Some(current_index) = parent.worktree_ids.iter().position(|id| id == worktree_id) {
                let id = parent.worktree_ids.remove(current_index);
                let target = if new_index > current_index {
                    new_index.saturating_sub(1)
                } else {
                    new_index
                };
                let target = target.min(parent.worktree_ids.len());
                parent.worktree_ids.insert(target, id);
                self.notify_data(cx);
            }
        }
    }

    /// Update project column widths
    pub fn update_project_widths(&mut self, widths: HashMap<String, f32>, cx: &mut Context<Self>) {
        self.data.project_widths = widths;
        self.notify_data(cx);
    }

    /// Update service panel height for a project
    pub fn update_service_panel_height(&mut self, project_id: &str, height: f32, cx: &mut Context<Self>) {
        self.data.service_panel_heights.insert(project_id.to_string(), height);
        self.notify_data(cx);
    }

    /// Get project width or default equal distribution
    pub fn get_project_width(&self, project_id: &str, visible_count: usize) -> f32 {
        self.data.project_widths
            .get(project_id)
            .copied()
            .unwrap_or_else(|| 100.0 / visible_count as f32)
    }

    /// Create a worktree project from an existing project.
    /// `repo_path` is the git repository root to create the worktree from.
    /// Returns the new project ID on success.
    ///
    /// This is a synchronous/blocking operation (calls `git worktree add`).
    /// For non-blocking creation, use `register_worktree_project` after
    /// creating the git worktree on a background thread.
    pub fn create_worktree_project(
        &mut self,
        parent_project_id: &str,
        branch: &str,
        repo_path: &std::path::Path,
        worktree_path: &str,
        project_path: &str,
        create_branch: bool,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        // Create the git worktree at the repo-level target path
        let target = std::path::PathBuf::from(worktree_path);
        crate::git::create_worktree(repo_path, branch, &target, create_branch)?;

        // Register in workspace state
        self.register_worktree_project(parent_project_id, branch, repo_path, worktree_path, project_path, cx)
    }

    /// Register a worktree project in workspace state.
    /// When `fire_hooks` is true the worktree must already exist on disk
    /// (hooks may cd into the project path). Pass `false` to defer hooks
    /// and call `fire_worktree_hooks` after the directory is ready.
    /// Returns the new project ID on success.
    pub fn register_worktree_project(
        &mut self,
        parent_project_id: &str,
        branch: &str,
        repo_path: &std::path::Path,
        worktree_path: &str,
        project_path: &str,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        self.register_worktree_project_inner(parent_project_id, branch, repo_path, worktree_path, project_path, true, cx)
    }

    /// Same as `register_worktree_project` but defers on_worktree_create hooks.
    /// Call `fire_worktree_hooks` once the worktree directory exists on disk.
    pub fn register_worktree_project_deferred_hooks(
        &mut self,
        parent_project_id: &str,
        branch: &str,
        repo_path: &std::path::Path,
        worktree_path: &str,
        project_path: &str,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        self.register_worktree_project_inner(parent_project_id, branch, repo_path, worktree_path, project_path, false, cx)
    }

    fn register_worktree_project_inner(
        &mut self,
        parent_project_id: &str,
        branch: &str,
        repo_path: &std::path::Path,
        worktree_path: &str,
        project_path: &str,
        fire_hooks: bool,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        // Get parent project info
        let parent = self.project(parent_project_id)
            .ok_or_else(|| "Parent project not found".to_string())?;

        let parent_layout = parent.layout.clone();
        let parent_hooks = parent.hooks.clone();
        let parent_color = parent.folder_color;

        // Create new project with cloned layout (or new terminal if parent has no layout)
        let id = uuid::Uuid::new_v4().to_string();
        let project_name = branch.to_string();

        let new_layout = parent_layout
            .as_ref()
            .map(|l| l.clone_structure());

        let project = ProjectData {
            id: id.clone(),
            name: project_name,
            path: project_path.to_string(),
            show_in_overview: true,
            // When hooks are deferred the worktree directory doesn't exist yet.
            // Use None so no terminals are spawned until creation finishes.
            layout: if fire_hooks { new_layout } else { None },
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: Some(crate::workspace::state::WorktreeMetadata {
                parent_project_id: parent_project_id.to_string(),
                main_repo_path: String::new(),
                worktree_path: String::new(),
                branch_name: String::new(),
            }),
            worktree_ids: Vec::new(),
            folder_color: parent_color,
            hooks: parent_hooks,
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            remote_services: Vec::new(),
            remote_host: None,
            remote_git_status: None,
            default_shell: None,
            hook_terminals: HashMap::new(),
        };

        let new_project_hooks = project.hooks.clone();
        let new_project_name = project.name.clone();
        self.data.projects.push(project);

        // Add to parent's worktree_ids (not project_order)
        if let Some(parent) = self.data.projects.iter_mut().find(|p| p.id == parent_project_id) {
            parent.worktree_ids.push(id.clone());
        }

        self.notify_data(cx);

        if fire_hooks {
            let hook_results = hooks::fire_on_worktree_create(
                &new_project_hooks,
                &id,
                &new_project_name,
                project_path,
                branch,
                cx,
            );
            self.register_hook_results(hook_results, cx);
        }

        Ok(id)
    }

    /// Finalize a deferred worktree: set the layout from the parent and fire hooks.
    /// Called once the worktree directory exists on disk.
    pub fn fire_worktree_hooks(&mut self, project_id: &str, cx: &mut Context<Self>) {
        let Some(project) = self.project(project_id) else { return };
        let hooks_config = project.hooks.clone();
        let name = project.name.clone();
        let path = project.path.clone();
        // Read branch from git at runtime, falling back to project name
        let branch = crate::git::repository::get_current_branch(std::path::Path::new(&path))
            .unwrap_or_else(|| name.clone());

        // If layout is still None (deferred creation), clone it from the parent
        if project.layout.is_none() {
            let parent_layout = project.worktree_info.as_ref()
                .and_then(|wt| self.project(&wt.parent_project_id))
                .and_then(|p| p.layout.as_ref())
                .map(|l| l.clone_structure());
            let layout = parent_layout.or_else(|| Some(crate::workspace::state::LayoutNode::new_terminal()));
            if let Some(p) = self.data.projects.iter_mut().find(|p| p.id == project_id) {
                p.layout = layout;
            }
        }

        let hook_results = hooks::fire_on_worktree_create(
            &hooks_config,
            project_id,
            &name,
            &path,
            &branch,
            cx,
        );
        self.register_hook_results(hook_results, cx);
    }

    /// Add a worktree project discovered by the periodic sync watcher.
    /// Does NOT fire hooks (the worktree was created outside Okena).
    pub fn add_discovered_worktree(
        &mut self,
        wt_path: &str,
        branch: &str,
        parent_id: &str,
        main_repo_path: &str,
    ) {
        // Double-check it's not already tracked
        if self.data.projects.iter().any(|p| p.path == wt_path) {
            return;
        }

        let dir_name = std::path::Path::new(wt_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("worktree");
        let project_name = format!("{} ({})", dir_name, branch);
        let id = uuid::Uuid::new_v4().to_string();

        let project = ProjectData {
            id: id.clone(),
            name: project_name,
            path: wt_path.to_string(),
            show_in_overview: false,
            layout: Some(LayoutNode::new_terminal()),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: Some(crate::workspace::state::WorktreeMetadata {
                parent_project_id: parent_id.to_string(),
                main_repo_path: String::new(),
                worktree_path: String::new(),
                branch_name: String::new(),
            }),
            worktree_ids: Vec::new(),
            default_shell: None,
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            remote_services: Vec::new(),
            remote_host: None,
            remote_git_status: None,
            hook_terminals: HashMap::new(),
        };

        // Insert after parent in project_order
        self.data.projects.push(project);
        if let Some(parent_index) = self.data.project_order.iter().position(|pid| pid == parent_id) {
            self.data.project_order.insert(parent_index + 1, id);
        } else {
            self.data.project_order.push(id);
        }
        // Note: caller is responsible for calling notify_data
    }

    /// Add a worktree project ID to its parent's worktree_ids list (deduped).
    /// Also removes the worktree from project_order since it lives under its parent now.
    pub fn add_to_worktree_ids(&mut self, parent_id: &str, worktree_id: &str) {
        if let Some(parent) = self.data.projects.iter_mut().find(|p| p.id == parent_id) {
            if !parent.worktree_ids.iter().any(|id| id == worktree_id) {
                parent.worktree_ids.push(worktree_id.to_string());
            }
        }
        // Worktrees in worktree_ids don't belong in project_order
        self.data.project_order.retain(|id| id != worktree_id);
        // Also remove from any folder's project_ids
        for folder in &mut self.data.folders {
            folder.project_ids.retain(|id| id != worktree_id);
        }
    }

    /// Remove a stale worktree project whose directory no longer exists.
    /// Does NOT fire hooks or call git worktree remove (the directory is already gone).
    pub fn remove_stale_worktree(&mut self, project_id: &str) {
        // Only remove if it's actually a worktree project
        let is_worktree = self.data.projects.iter()
            .any(|p| p.id == project_id && p.worktree_info.is_some());
        if !is_worktree {
            return;
        }

        self.data.projects.retain(|p| p.id != project_id);
        self.data.project_order.retain(|id| id != project_id);
        for folder in &mut self.data.folders {
            folder.project_ids.retain(|id| id != project_id);
        }
        self.data.project_widths.remove(project_id);
        // Note: caller is responsible for calling notify_data
    }

    /// Gather the data needed for quick worktree creation without blocking.
    /// Returns (parent_path, main_repo_path) or None if parent not found.
    pub fn prepare_quick_create(
        &self,
        parent_project_id: &str,
    ) -> Option<(String, Option<String>)> {
        let parent = self.project(parent_project_id)?;
        let main_repo = self.worktree_parent_path(parent_project_id);
        Some((
            parent.path.clone(),
            main_repo,
        ))
    }

    /// Remove a worktree project and its git worktree

    pub fn remove_worktree_project(&mut self, project_id: &str, force: bool, cx: &mut Context<Self>) -> Result<(), String> {
        let project = self.project(project_id)
            .ok_or_else(|| "Project not found".to_string())?;

        // Ensure it's a worktree project
        if project.worktree_info.is_none() {
            return Err("Not a worktree project".to_string());
        }

        // Capture info before removal for the hook
        let project_hooks = project.hooks.clone();
        let project_name = project.name.clone();
        let project_path = project.path.clone();
        // Use the stored worktree path (repo-level checkout), falling back to project path
        // for backwards compatibility with worktrees created before monorepo support
        let worktree_path = std::path::PathBuf::from(&project_path);

        // Remove the git worktree
        crate::git::remove_worktree(&worktree_path, force)?;

        // Delete the project from workspace (this also fires on_project_close)
        self.delete_project(project_id, cx);

        // Fire worktree-specific hook (runs headlessly since project is deleted)
        hooks::fire_on_worktree_close(&project_hooks, project_id, &project_name, &project_path, cx);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::expand_tilde;
    use crate::workspace::state::*;
    use crate::workspace::settings::HooksConfig;
    use crate::theme::FolderColor;
    use std::collections::HashMap;

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
            hooks: HooksConfig::default(),
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

    fn make_workspace_data() -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects: vec![],
            project_order: vec![],
            project_widths: HashMap::new(),
            service_panel_heights: HashMap::new(),
            folders: vec![],
        }
    }

    fn simulate_delete_project(data: &mut WorkspaceData, project_id: &str) {
        data.projects.retain(|p| p.id != project_id);
        data.project_order.retain(|id| id != project_id);
        for folder in &mut data.folders {
            folder.project_ids.retain(|id| id != project_id);
        }
        data.project_widths.remove(project_id);
    }

    #[test]
    fn test_delete_project_removes_from_folders() {
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1"), make_project("p2")];
        data.project_order = vec!["f1".to_string()];
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["p1".to_string(), "p2".to_string()],
            collapsed: false,
            folder_color: FolderColor::default(),
        }];

        simulate_delete_project(&mut data, "p1");

        assert_eq!(data.folders[0].project_ids, vec!["p2".to_string()]);
    }

    #[test]
    fn test_get_project_width() {
        let ws = Workspace::new(make_workspace_data());
        // Default: equal distribution
        assert_eq!(ws.get_project_width("p1", 4), 25.0);
    }

    #[test]
    fn test_get_project_width_custom() {
        let mut data = make_workspace_data();
        data.project_widths.insert("p1".to_string(), 60.0);
        let ws = Workspace::new(data);
        assert_eq!(ws.get_project_width("p1", 2), 60.0);
    }

    #[test]
    fn test_expand_tilde_with_subpath() {
        let home = dirs::home_dir().unwrap();
        let result = expand_tilde("~/Developer/project");
        assert_eq!(result, format!("{}/Developer/project", home.display()));
    }

    #[test]
    fn test_expand_tilde_home_only() {
        let home = dirs::home_dir().unwrap();
        let result = expand_tilde("~");
        assert_eq!(result, format!("{}", home.display()));
    }

    #[test]
    fn test_expand_tilde_absolute_path_unchanged() {
        let result = expand_tilde("/usr/local/bin");
        assert_eq!(result, "/usr/local/bin");
    }

    #[test]
    fn test_expand_tilde_relative_path_unchanged() {
        let result = expand_tilde("some/relative/path");
        assert_eq!(result, "some/relative/path");
    }

    #[test]
    fn test_expand_tilde_other_user_unchanged() {
        let result = expand_tilde("~otheruser/path");
        assert_eq!(result, "~otheruser/path");
    }
}

#[cfg(test)]
mod gpui_tests {
    use gpui::AppContext as _;
    use crate::workspace::state::{LayoutNode, ProjectData, Workspace, WorkspaceData};
    use crate::workspace::settings::HooksConfig;
    use crate::settings::{GlobalSettings, SettingsState};
    use crate::theme::FolderColor;
    use std::collections::HashMap;

    fn make_workspace_data() -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects: vec![],
            project_order: vec![],
            project_widths: HashMap::new(),
            service_panel_heights: HashMap::new(),
            folders: vec![],
        }
    }

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
            hooks: HooksConfig::default(),
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

    /// Initialize GlobalSettings for tests that call hooks (add_project, delete_project)
    fn init_test_settings(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let entity = cx.new(|_cx| SettingsState::new(Default::default()));
            cx.set_global(GlobalSettings(entity));
        });
    }

    #[gpui::test]
    fn test_add_project_gpui(cx: &mut gpui::TestAppContext) {
        init_test_settings(cx);
        let workspace = cx.new(|_cx| Workspace::new(make_workspace_data()));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.add_project("Test".to_string(), "/tmp/test".to_string(), true, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data().projects.len(), 1);
            assert_eq!(ws.data().projects[0].name, "Test");
            assert!(ws.data().projects[0].layout.is_some());
            assert_eq!(ws.data().project_order.len(), 1);
            assert_eq!(ws.data().project_order[0], ws.data().projects[0].id);
            assert!(ws.data_version() > 0);
        });
    }

    #[gpui::test]
    fn test_add_bookmark_project_gpui(cx: &mut gpui::TestAppContext) {
        init_test_settings(cx);
        let workspace = cx.new(|_cx| Workspace::new(make_workspace_data()));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.add_project("Bookmark".to_string(), "/tmp/bm".to_string(), false, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.data().projects[0].layout.is_none());
        });
    }

    #[gpui::test]
    fn test_delete_project_gpui(cx: &mut gpui::TestAppContext) {
        init_test_settings(cx);
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1"), make_project("p2")];
        data.project_order = vec!["p1".to_string(), "p2".to_string()];
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.delete_project("p1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data().projects.len(), 1);
            assert_eq!(ws.data().projects[0].id, "p2");
            assert!(!ws.data().project_order.contains(&"p1".to_string()));
        });
    }

    #[gpui::test]
    fn test_move_project_gpui(cx: &mut gpui::TestAppContext) {
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1"), make_project("p2"), make_project("p3")];
        data.project_order = vec!["p1".to_string(), "p2".to_string(), "p3".to_string()];
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.move_project("p3", 0, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data().project_order, vec!["p3", "p1", "p2"]);
        });
    }

    fn make_worktree_project(id: &str, parent_id: &str) -> ProjectData {
        let mut p = make_project(id);
        p.worktree_info = Some(crate::workspace::state::WorktreeMetadata {
            parent_project_id: parent_id.to_string(),
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: format!("/tmp/worktrees/{}", id),
            branch_name: String::new(),
        });
        p
    }

    #[gpui::test]
    fn test_delete_worktree_removes_from_parent_worktree_ids(cx: &mut gpui::TestAppContext) {
        init_test_settings(cx);
        let mut parent = make_project("parent");
        parent.worktree_ids = vec!["wt1".to_string(), "wt2".to_string()];
        let mut data = make_workspace_data();
        data.projects = vec![parent, make_worktree_project("wt1", "parent"), make_worktree_project("wt2", "parent")];
        data.project_order = vec!["parent".to_string()];
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.delete_project("wt1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let parent = ws.project("parent").unwrap();
            assert_eq!(parent.worktree_ids, vec!["wt2".to_string()]);
            assert!(!ws.data().project_order.contains(&"wt1".to_string()));
        });
    }

    #[gpui::test]
    fn test_delete_parent_rehomes_orphaned_worktrees(cx: &mut gpui::TestAppContext) {
        init_test_settings(cx);
        let mut parent = make_project("parent");
        parent.worktree_ids = vec!["wt1".to_string(), "wt2".to_string()];
        let mut data = make_workspace_data();
        data.projects = vec![parent, make_worktree_project("wt1", "parent"), make_worktree_project("wt2", "parent")];
        data.project_order = vec!["parent".to_string()];
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.delete_project("parent", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            // Orphaned worktrees should be added to project_order
            assert!(ws.data().project_order.contains(&"wt1".to_string()));
            assert!(ws.data().project_order.contains(&"wt2".to_string()));
            assert!(!ws.data().project_order.contains(&"parent".to_string()));
        });
    }

    #[gpui::test]
    fn test_reorder_worktree(cx: &mut gpui::TestAppContext) {
        let mut parent = make_project("parent");
        parent.worktree_ids = vec!["wt1".to_string(), "wt2".to_string(), "wt3".to_string()];
        let mut data = make_workspace_data();
        data.projects = vec![parent, make_worktree_project("wt1", "parent"), make_worktree_project("wt2", "parent"), make_worktree_project("wt3", "parent")];
        data.project_order = vec!["parent".to_string()];
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.reorder_worktree("parent", "wt3", 0, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let parent = ws.project("parent").unwrap();
            assert_eq!(parent.worktree_ids, vec!["wt3", "wt1", "wt2"]);
        });
    }

    #[gpui::test]
    fn test_add_terminal_gpui(cx: &mut gpui::TestAppContext) {
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1")];
        data.project_order = vec!["p1".to_string()];
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.add_terminal("p1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            match layout {
                LayoutNode::Split { children, .. } => {
                    assert_eq!(children.len(), 2);
                }
                _ => panic!("Expected split after add_terminal"),
            }
        });
    }
}
