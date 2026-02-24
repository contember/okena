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
    /// Toggle project visibility
    pub fn toggle_project_visibility(&mut self, project_id: &str, cx: &mut Context<Self>) {
        self.with_project(project_id, cx, |project| {
            project.is_visible = !project.is_visible;
            true
        });
    }

    /// Add a new project
    /// If `with_terminal` is false, creates a bookmark project without a terminal layout.
    pub fn add_project(&mut self, name: String, path: String, with_terminal: bool, cx: &mut Context<Self>) {
        let path = expand_tilde(&path);
        let id = uuid::Uuid::new_v4().to_string();
        let project = ProjectData {
            id: id.clone(),
            name: name.clone(),
            path: path.clone(),
            is_visible: true,
            layout: if with_terminal { Some(LayoutNode::new_terminal()) } else { None },
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
        };
        let project_hooks = project.hooks.clone();
        self.data.projects.push(project);
        self.data.project_order.push(id.clone());
        self.notify_data(cx);

        hooks::fire_on_project_open(&project_hooks, &id, &name, &path, cx);
    }

    /// Start a terminal for a project that doesn't have one (bookmark -> active project)
    pub fn start_terminal(&mut self, project_id: &str, cx: &mut Context<Self>) {
        if let Some(project) = self.project_mut(project_id) {
            if project.layout.is_none() {
                project.layout = Some(LayoutNode::new_terminal());
                self.notify_data(cx);
            }
        }
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

    /// Set the folder color for a project
    pub fn set_folder_color(&mut self, project_id: &str, color: FolderColor, cx: &mut Context<Self>) {
        self.with_project(project_id, cx, |project| {
            project.folder_color = color;
            true
        });
    }

    /// Delete a project
    pub fn delete_project(&mut self, project_id: &str, cx: &mut Context<Self>) {
        // Capture project info before removal for the hook
        let hook_info = self.project(project_id).map(|p| {
            (p.hooks.clone(), p.id.clone(), p.name.clone(), p.path.clone())
        });

        // Remove from projects list
        self.data.projects.retain(|p| p.id != project_id);
        // Remove from project order
        self.data.project_order.retain(|id| id != project_id);
        // Remove from any folder's project_ids
        for folder in &mut self.data.folders {
            folder.project_ids.retain(|id| id != project_id);
        }
        // Remove from widths
        self.data.project_widths.remove(project_id);
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
    pub fn move_project(&mut self, project_id: &str, new_index: usize, cx: &mut Context<Self>) {
        // Remove from any folder first
        for folder in &mut self.data.folders {
            folder.project_ids.retain(|id| id != project_id);
        }

        // Find current index in project_order
        if let Some(current_index) = self.data.project_order.iter().position(|id| id == project_id) {
            // Remove from current position
            let id = self.data.project_order.remove(current_index);
            // Adjust target index if needed
            let target = if new_index > current_index {
                new_index.saturating_sub(1)
            } else {
                new_index
            };
            // Insert at new position
            let target = target.min(self.data.project_order.len());
            self.data.project_order.insert(target, id);
        } else {
            // Project wasn't in project_order (was only in a folder) - insert at target
            let target = new_index.min(self.data.project_order.len());
            self.data.project_order.insert(target, project_id.to_string());
        }
        self.notify_data(cx);
    }

    /// Update project column widths
    pub fn update_project_widths(&mut self, widths: HashMap<String, f32>, cx: &mut Context<Self>) {
        self.data.project_widths = widths;
        self.notify_data(cx);
    }

    /// Get project width or default equal distribution
    pub fn get_project_width(&self, project_id: &str, visible_count: usize) -> f32 {
        self.data.project_widths
            .get(project_id)
            .copied()
            .unwrap_or_else(|| 100.0 / visible_count as f32)
    }

    /// Create a worktree project from an existing project
    /// Returns the new project ID on success
    pub fn create_worktree_project(
        &mut self,
        parent_project_id: &str,
        branch: &str,
        target_path: &str,
        create_branch: bool,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        // Get parent project info
        let parent = self.project(parent_project_id)
            .ok_or_else(|| "Parent project not found".to_string())?;

        let parent_path = parent.path.clone();
        let parent_layout = parent.layout.clone();

        // Determine the actual repo path (if parent is a worktree, use its main repo)
        let repo_path = if let Some(ref wt_info) = parent.worktree_info {
            std::path::PathBuf::from(&wt_info.main_repo_path)
        } else {
            std::path::PathBuf::from(&parent_path)
        };

        // Create the git worktree
        let target = std::path::PathBuf::from(target_path);
        crate::git::create_worktree(&repo_path, branch, &target, create_branch)?;

        // Create new project with cloned layout (or new terminal if parent has no layout)
        let id = uuid::Uuid::new_v4().to_string();
        let project_name = format!("{} ({})",
            std::path::Path::new(&parent_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Project"),
            branch
        );

        let new_layout = parent_layout
            .as_ref()
            .map(|l| l.clone_structure());

        let project = ProjectData {
            id: id.clone(),
            name: project_name,
            path: target_path.to_string(),
            is_visible: true,
            layout: new_layout,
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: Some(crate::workspace::state::WorktreeMetadata {
                parent_project_id: parent_project_id.to_string(),
                main_repo_path: repo_path.to_string_lossy().to_string(),
            }),
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
        };

        // Insert after parent project in order
        let parent_index = self.data.project_order
            .iter()
            .position(|pid| pid == parent_project_id)
            .unwrap_or(self.data.project_order.len());

        let new_project_hooks = project.hooks.clone();
        let new_project_name = project.name.clone();
        self.data.projects.push(project);
        self.data.project_order.insert(parent_index + 1, id.clone());

        self.notify_data(cx);

        hooks::fire_on_worktree_create(
            &new_project_hooks,
            &id,
            &new_project_name,
            target_path,
            branch,
            cx,
        );

        Ok(id)
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
        let worktree_path = std::path::PathBuf::from(&project_path);

        // Remove the git worktree
        crate::git::remove_worktree(&worktree_path, force)?;

        // Delete the project from workspace (this also fires on_project_close)
        self.delete_project(project_id, cx);

        // Fire worktree-specific hook
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
            is_visible: true,
            layout: Some(LayoutNode::new_terminal()),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
        }
    }

    fn make_workspace_data() -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects: vec![],
            project_order: vec![],
            project_widths: HashMap::new(),
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
            folders: vec![],
        }
    }

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
            hooks: HooksConfig::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
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
