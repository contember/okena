//! Worktree lifecycle workspace actions
//!
//! Actions for creating, registering, discovering, and removing git
//! worktree projects, plus worktree-specific properties and ordering.

use okena_core::theme::FolderColor;
use crate::focus::FocusManager;
use crate::hooks;
use crate::persistence::HooksConfig;
use crate::state::{LayoutNode, ProjectData, Workspace, WindowId};
use gpui::*;
use std::collections::HashMap;

impl Workspace {
    /// Toggle visibility for a single worktree (no propagation to children).
    ///
    /// Delegates to `Workspace::toggle_hidden(window_id, ...)`, which flips
    /// membership in the targeted window's `hidden_project_ids` and bumps
    /// `data_version` so the auto-save observer triggers. Per the multi-window
    /// viewport model, hidden state IS persisted -- the bump is unconditional,
    /// even for ids that do not currently match a project. Unknown extra ids
    /// are a silent no-op (close-race contract inherited from `toggle_hidden`).
    pub fn toggle_worktree_visibility(&mut self, window_id: WindowId, project_id: &str, cx: &mut Context<Self>) {
        self.toggle_hidden(window_id, project_id, cx);
    }

    /// Set or clear the color override for a worktree project
    pub fn set_worktree_color_override(&mut self, project_id: &str, color: Option<FolderColor>, cx: &mut Context<Self>) {
        self.with_project(project_id, cx, |project| {
            if let Some(ref mut wt) = project.worktree_info {
                wt.color_override = color;
                true
            } else {
                false
            }
        });
    }

    /// Reorder a worktree within its parent's worktree_ids list
    pub fn reorder_worktree(&mut self, parent_id: &str, worktree_id: &str, new_index: usize, cx: &mut Context<Self>) {
        if let Some(parent) = self.data.projects.iter_mut().find(|p| p.id == parent_id)
            && let Some(current_index) = parent.worktree_ids.iter().position(|id| id == worktree_id) {
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

    /// Create a worktree project from an existing project.
    /// `repo_path` is the git repository root to create the worktree from.
    /// Returns the new project ID on success.
    ///
    /// This is a synchronous/blocking operation (calls `git worktree add`).
    /// For non-blocking creation, use `register_worktree_project` after
    /// creating the git worktree on a background thread.
    ///
    /// `window_id` identifies the spawning window for the multi-window
    /// new-project visibility rule (PRD user story 14): the new worktree
    /// project is visible in the spawning window only and hidden in every
    /// other window via `data.add_project_hide_in_other_windows` after
    /// the project is pushed. Threaded through to
    /// `register_worktree_project` -> `register_worktree_project_inner`.
    // Worktree identity is described by several cohesive path/branch params;
    // a param struct would add indirection without grouping anything reusable.
    #[allow(clippy::too_many_arguments)]
    pub fn create_worktree_project(
        &mut self,
        parent_project_id: &str,
        branch: &str,
        repo_path: &std::path::Path,
        worktree_path: &str,
        project_path: &str,
        create_branch: bool,
        global_hooks: &HooksConfig,
        window_id: WindowId,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        // Create the git worktree at the repo-level target path
        let target = std::path::PathBuf::from(worktree_path);
        okena_git::create_worktree(repo_path, branch, &target, create_branch)
            .map_err(|e| match &e {
                okena_git::GitError::WorktreeExists { path } => {
                    format!("Directory '{}' is already an active worktree", path.display())
                }
                other => other.to_string(),
            })?;

        // Register in workspace state
        self.register_worktree_project(parent_project_id, branch, repo_path, worktree_path, project_path, global_hooks, window_id, cx)
    }

    /// Register a worktree project in workspace state.
    /// When `fire_hooks` is true the worktree must already exist on disk
    /// (hooks may cd into the project path). Pass `false` to defer hooks
    /// and call `fire_worktree_hooks` after the directory is ready.
    /// Returns the new project ID on success.
    ///
    /// `window_id` identifies the spawning window for the multi-window
    /// new-project visibility rule (PRD user story 14). See
    /// `create_worktree_project` for details.
    #[allow(clippy::too_many_arguments)] // cohesive worktree path/branch params
    pub fn register_worktree_project(
        &mut self,
        parent_project_id: &str,
        branch: &str,
        repo_path: &std::path::Path,
        worktree_path: &str,
        project_path: &str,
        global_hooks: &HooksConfig,
        window_id: WindowId,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        self.register_worktree_project_inner(parent_project_id, branch, repo_path, worktree_path, project_path, true, global_hooks, window_id, cx)
    }

    /// Same as `register_worktree_project` but defers on_worktree_create hooks.
    /// Call `fire_worktree_hooks` once the worktree directory exists on disk.
    ///
    /// `window_id` identifies the spawning window for the multi-window
    /// new-project visibility rule (PRD user story 14). See
    /// `create_worktree_project` for details.
    #[allow(clippy::too_many_arguments)] // cohesive worktree path/branch params
    pub fn register_worktree_project_deferred_hooks(
        &mut self,
        parent_project_id: &str,
        branch: &str,
        repo_path: &std::path::Path,
        worktree_path: &str,
        project_path: &str,
        global_hooks: &HooksConfig,
        window_id: WindowId,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        self.register_worktree_project_inner(parent_project_id, branch, repo_path, worktree_path, project_path, false, global_hooks, window_id, cx)
    }

    #[allow(clippy::too_many_arguments)] // cohesive worktree path/branch params
    fn register_worktree_project_inner(
        &mut self,
        parent_project_id: &str,
        branch: &str,
        _repo_path: &std::path::Path,
        _worktree_path: &str,
        project_path: &str,
        fire_hooks: bool,
        global_hooks: &HooksConfig,
        window_id: WindowId,
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
            // When hooks are deferred the worktree directory doesn't exist yet.
            // Use None so no terminals are spawned until creation finishes.
            layout: if fire_hooks { new_layout } else { None },
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: Some(crate::state::WorktreeMetadata {
                parent_project_id: parent_project_id.to_string(),
                color_override: None,
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

        // Multi-window new-project visibility rule (PRD user story 14):
        // worktree children inherit the rule for the window the worktree
        // was created from -- visible in the spawning window only, hidden
        // in every other window. Single-window users (zero extras) see no
        // behavior change since the rule degenerates to a no-op.
        self.data.add_project_hide_in_other_windows(&id, window_id);

        self.notify_data(cx);

        if fire_hooks {
            let folder = self.folder_for_project_or_parent(&id);
            let folder_id = folder.map(|f| f.id.as_str());
            let folder_name = folder.map(|f| f.name.as_str());
            let hook_results = hooks::fire_on_worktree_create(
                &new_project_hooks,
                &id,
                &new_project_name,
                project_path,
                branch,
                folder_id,
                folder_name,
                global_hooks,
                cx,
            );
            self.register_hook_results(hook_results, cx);
        }

        Ok(id)
    }

    /// Finalize a deferred worktree: set the layout from the parent and fire hooks.
    /// Called once the worktree directory exists on disk.
    pub fn fire_worktree_hooks(&mut self, project_id: &str, global_hooks: &HooksConfig, cx: &mut Context<Self>) {
        let Some(project) = self.project(project_id) else { return };
        let hooks_config = project.hooks.clone();
        let name = project.name.clone();
        let path = project.path.clone();
        // Read branch from git at runtime, falling back to project name
        let branch = okena_git::repository::get_current_branch(std::path::Path::new(&path))
            .unwrap_or_else(|| name.clone());

        // If layout is still None (deferred creation), clone it from the parent
        if project.layout.is_none() {
            let parent_layout = project.worktree_info.as_ref()
                .and_then(|wt| self.project(&wt.parent_project_id))
                .and_then(|p| p.layout.as_ref())
                .map(|l| l.clone_structure());
            let layout = parent_layout.or_else(|| Some(crate::state::LayoutNode::new_terminal()));
            if let Some(p) = self.data.projects.iter_mut().find(|p| p.id == project_id) {
                p.layout = layout;
            }
        }

        let folder = self.folder_for_project_or_parent(project_id);
        let folder_id = folder.map(|f| f.id.as_str());
        let folder_name = folder.map(|f| f.name.as_str());
        let hook_results = hooks::fire_on_worktree_create(
            &hooks_config,
            project_id,
            &name,
            &path,
            &branch,
            folder_id,
            folder_name,
            global_hooks,
            cx,
        );
        self.register_hook_results(hook_results, cx);
    }

    /// Add a worktree project discovered by the periodic sync watcher.
    /// Does NOT fire hooks (the worktree was created outside Okena).
    /// Returns the new project ID, or None if already tracked.
    ///
    /// `window_id` identifies the spawning window for the multi-window
    /// new-project visibility rule (PRD user story 14): the discovered
    /// worktree becomes visible in the spawning window only, hidden in
    /// every other window. The user explicitly clicks to add the
    /// discovery from a sidebar in a window, so the click site IS the
    /// opt-in -- mirroring the user-initiated add path. Single-window
    /// users (zero extras) see the prior "default hidden" behavior since
    /// `WindowId::Main` with no extras degenerates to a no-op.
    pub fn add_discovered_worktree(
        &mut self,
        wt_path: &str,
        branch: &str,
        parent_id: &str,
        window_id: WindowId,
    ) -> Option<String> {
        // For monorepo projects, resolve the subdirectory offset so the
        // project path points to the right place inside the worktree.
        let parent_path = self.project(parent_id)
            .map(|p| p.path.clone())
            .unwrap_or_default();
        let (_git_root, subdir) = okena_git::resolve_git_root_and_subdir(
            std::path::Path::new(&parent_path),
        );
        let project_path = okena_git::repository::project_path_in_worktree(wt_path, &subdir);

        if self.data.projects.iter().any(|p| p.path == project_path || p.path == wt_path) {
            return None;
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
            path: project_path,
            layout: Some(LayoutNode::new_terminal()),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: Some(crate::state::WorktreeMetadata {
                parent_project_id: parent_id.to_string(),
                color_override: None,
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
            hook_terminals: HashMap::new(),
        };

        // Multi-window new-project visibility rule (PRD user story 14):
        // visible in the spawning window only, hidden in every other
        // window. Replaces the prior unconditional "hide in main only"
        // semantic which left discovered worktrees visible in extras --
        // a stale-default that broke per-window curation. Single-window
        // users see no behavior change for `WindowId::Main` since the
        // helper degenerates to a no-op when no extras exist.
        self.data.add_project_hide_in_other_windows(&id, window_id);

        // Insert after parent in project_order
        self.data.projects.push(project);
        if let Some(parent_index) = self.data.project_order.iter().position(|pid| pid == parent_id) {
            self.data.project_order.insert(parent_index + 1, id.clone());
        } else {
            self.data.project_order.push(id.clone());
        }
        // Note: caller is responsible for calling notify_data
        Some(id)
    }

    /// Add a worktree project ID to its parent's worktree_ids list (deduped).
    /// Also removes the worktree from project_order since it lives under its parent now.
    pub fn add_to_worktree_ids(&mut self, parent_id: &str, worktree_id: &str) {
        if let Some(parent) = self.data.projects.iter_mut().find(|p| p.id == parent_id)
            && !parent.worktree_ids.iter().any(|id| id == worktree_id) {
                parent.worktree_ids.push(worktree_id.to_string());
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
        // Skip projects that are being actively managed (hook running, being created, etc.)
        if self.lifecycle.is_closing(project_id) || self.lifecycle.is_creating(project_id) {
            return;
        }

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
        // Scrub the worktree id from every window's per-project storage
        // (hidden set + widths map on main + every extra). Same fan-out as
        // the primary `delete_project` path.
        self.data.delete_project_scrub_all_windows(project_id);
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
    pub fn remove_worktree_project(&mut self, focus_manager: &mut FocusManager, project_id: &str, force: bool, global_hooks: &HooksConfig, cx: &mut Context<Self>) -> Result<(), String> {
        let project = self.project(project_id)
            .ok_or_else(|| "Project not found".to_string())?;

        // Ensure it's a worktree project
        if project.worktree_info.is_none() {
            return Err("Not a worktree project".to_string());
        }

        // Capture info before removal for the hook
        let folder = self.folder_for_project_or_parent(project_id);
        let hook_folder_id = folder.map(|f| f.id.clone());
        let hook_folder_name = folder.map(|f| f.name.clone());
        let project_hooks = project.hooks.clone();
        let project_name = project.name.clone();
        let project_path = project.path.clone();
        // For monorepos the project path is a subdirectory inside the worktree checkout.
        // Resolve the actual worktree root via git so `git worktree remove` gets the right path.
        let project_pathbuf = std::path::PathBuf::from(&project_path);
        let worktree_path = okena_git::get_repo_root(&project_pathbuf)
            .unwrap_or(project_pathbuf);

        // Resolve branch BEFORE removal (git worktree remove deletes the checkout)
        let branch = okena_git::get_current_branch(&worktree_path).unwrap_or_default();

        // Fire on_worktree_close hook BEFORE removal so the hook has a valid CWD
        hooks::fire_on_worktree_close(&project_hooks, project_id, &project_name, &project_path, &branch, hook_folder_id.as_deref(), hook_folder_name.as_deref(), global_hooks, cx);

        // Remove the git worktree
        okena_git::remove_worktree(&worktree_path, force)
            .map_err(|e| e.to_string())?;

        // Delete the project from workspace (this also fires on_project_close)
        self.delete_project(focus_manager, project_id, global_hooks, cx);

        Ok(())
    }
}
