//! Project management workspace actions
//!
//! Actions for creating, modifying, and deleting projects.

use crate::theme::FolderColor;
use crate::workspace::hooks;
use crate::workspace::persistence::HooksConfig;
use crate::workspace::state::{LayoutNode, ProjectData, Workspace};
use gpui::*;
use std::collections::HashMap;

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
    }

    /// Rename a project
    pub fn rename_project(&mut self, project_id: &str, new_name: String, cx: &mut Context<Self>) {
        self.with_project(project_id, cx, |project| {
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
