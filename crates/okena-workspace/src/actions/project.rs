//! Project management workspace actions
//!
//! Actions for creating, modifying, and deleting projects.

use okena_core::theme::FolderColor;
use crate::hooks;
use crate::persistence::HooksConfig;
use crate::state::{LayoutNode, ProjectData, Workspace, WindowId};
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
    /// Returns whether a project is hidden in this workspace's main window.
    ///
    /// Reads from `main_window.hidden_project_ids` (the per-window viewport
    /// model, the source of truth). Today this is always the main window;
    /// per-window scoping arrives with the window-scoped mutation API
    /// (slice 02). Missing entry == visible.
    pub fn is_project_hidden(&self, project_id: &str) -> bool {
        self.data
            .main_window
            .hidden_project_ids
            .contains(project_id)
    }

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

    /// Toggle project overview visibility (also toggles all worktree children).
    ///
    /// Delegates to `Workspace::toggle_hidden(window_id, ...)` after a
    /// project-existence early-return guard. The guard is load-bearing: this
    /// entrypoint is invoked from the sidebar context menu where a click
    /// landing on a stale id (project just deleted by another path) must be
    /// a silent no-op rather than insert the stale id into the persisted
    /// hidden set. The sister entrypoint `toggle_worktree_visibility` has
    /// no guard and bumps data_version unconditionally; the asymmetry is
    /// intentional.
    ///
    /// Per the multi-window viewport model, the toggle is scoped to the
    /// targeted window's `hidden_project_ids`. Unknown extra ids are a
    /// silent no-op (close-race contract inherited from `toggle_hidden`),
    /// distinct from the project-existence guard above (which gates on
    /// project, not window).
    pub fn toggle_project_overview_visibility(&mut self, window_id: WindowId, project_id: &str, cx: &mut Context<Self>) {
        if self.project(project_id).is_none() {
            return;
        }
        self.toggle_hidden(window_id, project_id, cx);
    }

    /// Add a new project
    /// If `with_terminal` is false, creates a bookmark project without a terminal layout.
    pub fn add_project(&mut self, name: String, path: String, with_terminal: bool, global_hooks: &HooksConfig, cx: &mut Context<Self>) -> String {
        let path = expand_tilde(&path);

        // Auto-detect WSL UNC paths and set default shell accordingly
        #[cfg(windows)]
        let default_shell = okena_terminal::shell_config::parse_wsl_unc_path(&path)
            .map(|(distro, _)| okena_terminal::shell_config::ShellType::Wsl {
                distro: Some(distro),
            });
        #[cfg(not(windows))]
        let default_shell: Option<okena_terminal::shell_config::ShellType> = None;

        let id = uuid::Uuid::new_v4().to_string();
        let project = ProjectData {
            id: id.clone(),
            name: name.clone(),
            path: path.clone(),
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
            default_shell,
            hook_terminals: HashMap::new(),
        };
        let project_hooks = project.hooks.clone();
        self.data.projects.push(project);
        self.data.project_order.push(id.clone());
        self.notify_data(cx);

        let folder = self.folder_for_project_or_parent(&id);
        let folder_id = folder.map(|f| f.id.as_str());
        let folder_name = folder.map(|f| f.name.as_str());
        let hook_results = hooks::fire_on_project_open(&project_hooks, &id, &name, &path, folder_id, folder_name, global_hooks, cx);
        self.register_hook_results(hook_results, cx);
        id
    }

    /// Add a new terminal to a project by splitting the root layout
    pub fn add_terminal(&mut self, project_id: &str, cx: &mut Context<Self>) {
        if let Some(project) = self.project_mut(project_id) {
            if let Some(ref old_layout) = project.layout {
                let old_layout = old_layout.clone();
                project.layout = Some(LayoutNode::Split {
                    direction: crate::state::SplitDirection::Vertical,
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
                    direction: crate::state::SplitDirection::Vertical,
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

    /// Set the folder color for a project (also propagates to worktree children without overrides)
    pub fn set_folder_color(&mut self, project_id: &str, color: FolderColor, cx: &mut Context<Self>) {
        let is_worktree = self.project(project_id)
            .and_then(|p| p.worktree_info.as_ref())
            .is_some();

        if is_worktree {
            self.set_worktree_color_override(project_id, Some(color), cx);
        } else {
            // Collect child IDs from the parent's worktree_ids to avoid a full scan
            let child_ids: Vec<String> = self.project(project_id)
                .map(|p| p.worktree_ids.clone())
                .unwrap_or_default();

            // Batch all mutations with a single notify
            let mut changed = false;
            if let Some(project) = self.project_mut(project_id) {
                project.folder_color = color;
                changed = true;
            }
            for child_id in &child_ids {
                if let Some(child) = self.project_mut(child_id) {
                    let has_override = child.worktree_info.as_ref()
                        .and_then(|wt| wt.color_override)
                        .is_some();
                    if !has_override {
                        child.folder_color = color;
                    }
                }
            }
            if changed {
                self.notify_data(cx);
            }
        }
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

    /// Delete a project
    pub fn delete_project(&mut self, project_id: &str, global_hooks: &HooksConfig, cx: &mut Context<Self>) {
        // Queue all project terminals for killing before removing state.
        // Okena (which owns PtyManager) drains this queue via observer.
        if let Some(project) = self.project(project_id) {
            let mut kill_ids: Vec<String> = Vec::new();
            if let Some(layout) = &project.layout {
                kill_ids.extend(layout.collect_terminal_ids());
            }
            kill_ids.extend(project.hook_terminals.keys().cloned());
            kill_ids.extend(project.service_terminals.values().cloned());
            self.queue_terminal_kills(kill_ids);
        }

        // Capture project info before removal for the hook
        let folder = self.folder_for_project_or_parent(project_id);
        let hook_folder_id = folder.map(|f| f.id.clone());
        let hook_folder_name = folder.map(|f| f.name.clone());
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

        // Scrub the project id from every window's per-project storage
        // (hidden set + widths map on main + every extra). Per the multi-
        // window viewport model, project delete is a workspace-level event
        // whose effect must propagate to every viewport so no orphan
        // entries survive. The trailing `notify_data(cx)` below covers the
        // data_version bump for the whole delete path.
        self.data.delete_project_scrub_all_windows(project_id);
        // Clear closing state
        self.lifecycle.finish_closing(project_id);
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
            hooks::fire_on_project_close(&project_hooks, &id, &name, &path, hook_folder_id.as_deref(), hook_folder_name.as_deref(), global_hooks, cx);
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

    /// Update project column widths on the targeted window.
    ///
    /// Wholesale-replaces the targeted window's `project_widths` map with the
    /// supplied map. The leading clear is routed through the `window_mut`
    /// lookup pair so an unknown extra id (e.g. caller raced a close) is a
    /// silent no-op for the clear; the per-entry `set_project_width` calls
    /// then also no-op via the same lookup contract. `notify_data` still
    /// bumps `data_version` so the auto-save observer's cadence is unchanged
    /// in the close-race path -- consistent with the silent-no-op contract
    /// the data-layer setters absorb.
    ///
    /// Each entry is written via `data.set_project_width(window_id, ...)` so
    /// a future migration off the wholesale shape inherits the per-entry
    /// pair-shaped contract automatically. The runtime shape of a column-resize
    /// is per-column; the wholesale shape on this entrypoint is a relic of the
    /// prior data layout where `project_widths` was a top-level field.
    ///
    /// Bumps `data_version` exactly once per call (not per entry) -- the data
    /// layer setter does not notify, so the single trailing `notify_data` keeps
    /// the auto-save observer's debounce cadence identical to the pre-migration
    /// body.
    pub fn update_project_widths(&mut self, window_id: WindowId, widths: HashMap<String, f32>, cx: &mut Context<Self>) {
        if let Some(w) = self.data.window_mut(window_id) {
            w.project_widths.clear();
        }
        for (id, w) in widths {
            self.data.set_project_width(window_id, &id, w);
        }
        self.notify_data(cx);
    }

    /// Update service panel height for a project
    pub fn update_service_panel_height(&mut self, project_id: &str, height: f32, cx: &mut Context<Self>) {
        self.data.service_panel_heights.insert(project_id.to_string(), height);
        self.notify_data(cx);
    }

    /// Update hook panel height for a project
    pub fn update_hook_panel_height(&mut self, project_id: &str, height: f32, cx: &mut Context<Self>) {
        self.data.hook_panel_heights.insert(project_id.to_string(), height);
        self.notify_data(cx);
    }

    /// Get project width or default equal distribution.
    ///
    /// Reads from the targeted window's `project_widths` map. `WindowId::Main`
    /// always lands on `main_window`. `WindowId::Extra(_)` targets the matching
    /// extra by id; an unknown extra (e.g. raced a close) routes through
    /// `data.window(window_id) == None` and falls back to the equal-distribution
    /// default, matching the "missing entry == default" contract on the lookup
    /// side. Default is `100.0 / visible_count` so a render path that asks for
    /// every visible column gets a balanced grid when no widths are set yet.
    pub fn get_project_width(&self, window_id: WindowId, project_id: &str, visible_count: usize) -> f32 {
        self.data
            .window(window_id)
            .and_then(|w| w.project_widths.get(project_id).copied())
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
        global_hooks: &HooksConfig,
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
        self.register_worktree_project(parent_project_id, branch, repo_path, worktree_path, project_path, global_hooks, cx)
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
        global_hooks: &HooksConfig,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        self.register_worktree_project_inner(parent_project_id, branch, repo_path, worktree_path, project_path, true, global_hooks, cx)
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
        global_hooks: &HooksConfig,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        self.register_worktree_project_inner(parent_project_id, branch, repo_path, worktree_path, project_path, false, global_hooks, cx)
    }

    fn register_worktree_project_inner(
        &mut self,
        parent_project_id: &str,
        branch: &str,
        _repo_path: &std::path::Path,
        _worktree_path: &str,
        project_path: &str,
        fire_hooks: bool,
        global_hooks: &HooksConfig,
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
    pub fn add_discovered_worktree(
        &mut self,
        wt_path: &str,
        branch: &str,
        parent_id: &str,
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

        // Discovered worktrees default to hidden in the main window (the
        // user did not opt in via Okena UI). Per-window scoping arrives
        // with the window-scoped mutation API; today we only have one
        // window so writing main_window matches the legacy intent.
        self.data
            .main_window
            .hidden_project_ids
            .insert(id.clone());

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

    pub fn remove_worktree_project(&mut self, project_id: &str, force: bool, global_hooks: &HooksConfig, cx: &mut Context<Self>) -> Result<(), String> {
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
        self.delete_project(project_id, global_hooks, cx);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::expand_tilde;
    use crate::state::*;
    use crate::settings::HooksConfig;
    use okena_core::theme::FolderColor;
    use std::collections::HashMap;

    fn make_project(id: &str) -> ProjectData {
        ProjectData {
            id: id.to_string(),
            name: format!("Project {}", id),
            path: "/tmp/test".to_string(),
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
            default_shell: None,
            hook_terminals: HashMap::new(),
        }
    }

    fn make_workspace_data() -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects: vec![],
            project_order: vec![],
            service_panel_heights: HashMap::new(),
            hook_panel_heights: HashMap::new(),
            folders: vec![],
            main_window: crate::state::WindowState::default(),
            extra_windows: Vec::new(),
        }
    }

    fn simulate_delete_project(data: &mut WorkspaceData, project_id: &str) {
        data.projects.retain(|p| p.id != project_id);
        data.project_order.retain(|id| id != project_id);
        for folder in &mut data.folders {
            folder.project_ids.retain(|id| id != project_id);
        }
        data.main_window.project_widths.remove(project_id);
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
            folder_color: FolderColor::default(),
        }];

        simulate_delete_project(&mut data, "p1");

        assert_eq!(data.folders[0].project_ids, vec!["p2".to_string()]);
    }

    #[test]
    fn test_get_project_width() {
        let ws = Workspace::new(make_workspace_data());
        // Default: equal distribution
        assert_eq!(ws.get_project_width(WindowId::Main, "p1", 4), 25.0);
    }

    #[test]
    fn test_get_project_width_custom() {
        let mut data = make_workspace_data();
        data.main_window.project_widths.insert("p1".to_string(), 60.0);
        let ws = Workspace::new(data);
        assert_eq!(ws.get_project_width(WindowId::Main, "p1", 2), 60.0);
    }

    #[test]
    fn get_project_width_reads_from_main_window_project_widths() {
        // Per-window viewport model: WindowId::Main routes through
        // data.window(...) and reads main_window.project_widths.
        let mut data = make_workspace_data();
        data.main_window.project_widths.insert("p1".to_string(), 75.0);
        let ws = Workspace::new(data);
        assert_eq!(ws.get_project_width(WindowId::Main, "p1", 2), 75.0);
    }

    #[test]
    fn get_project_width_extra_reads_from_targeted_window() {
        // Per-window viewport model: WindowId::Extra(uuid) routes through
        // data.window(...) and reads the matching extra's project_widths -- not
        // main's. Fixture writes p1 -> 80.0 only on the extra; main's map is
        // empty. Reading with the extra id returns 80.0; reading with Main
        // falls back to the equal-distribution default. Defends against a
        // regression that ignores window_id and unconditionally reads main.
        let mut data = make_workspace_data();
        let mut extra = WindowState::default();
        extra.project_widths.insert("p1".to_string(), 80.0);
        let extra_id = extra.id;
        data.extra_windows.push(extra);
        let ws = Workspace::new(data);

        assert_eq!(ws.get_project_width(WindowId::Extra(extra_id), "p1", 2), 80.0);
        // Main has no entry for p1 -> equal-distribution default of 50.0 (2 visible).
        assert_eq!(ws.get_project_width(WindowId::Main, "p1", 2), 50.0);
    }

    #[test]
    fn get_project_width_unknown_extra_returns_default() {
        // Close-race contract: a fresh uuid that does not match any extra is
        // a `data.window(...) == None`, which falls back to the equal-
        // distribution default rather than panicking. Mirrors the silent
        // no-op shape of the window-scoped setters when targeted at an
        // already-closed extra.
        let mut data = make_workspace_data();
        // Pre-populate main with a value to ensure the unknown-extra path
        // does NOT silently read from main as a fallback.
        data.main_window.project_widths.insert("p1".to_string(), 90.0);
        let ws = Workspace::new(data);

        let unknown = uuid::Uuid::new_v4();
        // Default for visible_count = 4 -> 25.0, NOT 90.0 (main's value).
        assert_eq!(ws.get_project_width(WindowId::Extra(unknown), "p1", 4), 25.0);
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
    use crate::state::{LayoutNode, ProjectData, WindowId, WindowState, Workspace, WorkspaceData};
    use crate::settings::HooksConfig;
    use okena_core::theme::FolderColor;
    use std::collections::HashMap;

    fn make_workspace_data() -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects: vec![],
            project_order: vec![],
            service_panel_heights: HashMap::new(),
            hook_panel_heights: HashMap::new(),
            folders: vec![],
            main_window: crate::state::WindowState::default(),
            extra_windows: Vec::new(),
        }
    }

    fn make_project(id: &str) -> ProjectData {
        ProjectData {
            id: id.to_string(),
            name: format!("Project {}", id),
            path: "/tmp/test".to_string(),
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
            default_shell: None,
            hook_terminals: HashMap::new(),
        }
    }

    #[gpui::test]
    fn test_add_project_gpui(cx: &mut gpui::TestAppContext) {
        let workspace = cx.new(|_cx| Workspace::new(make_workspace_data()));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.add_project("Test".to_string(), "/tmp/test".to_string(), true, &HooksConfig::default(), cx);
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
        let workspace = cx.new(|_cx| Workspace::new(make_workspace_data()));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.add_project("Bookmark".to_string(), "/tmp/bm".to_string(), false, &HooksConfig::default(), cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.data().projects[0].layout.is_none());
        });
    }

    #[gpui::test]
    fn test_delete_project_gpui(cx: &mut gpui::TestAppContext) {
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1"), make_project("p2")];
        data.project_order = vec!["p1".to_string(), "p2".to_string()];
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.delete_project("p1", &HooksConfig::default(), cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data().projects.len(), 1);
            assert_eq!(ws.data().projects[0].id, "p2");
            assert!(!ws.data().project_order.contains(&"p1".to_string()));
        });
    }

    #[gpui::test]
    fn is_project_hidden_reads_from_main_window_hidden_project_ids(cx: &mut gpui::TestAppContext) {
        // Per-window viewport model: hidden state is read from
        // main_window.hidden_project_ids (the source of truth). Missing
        // entry == visible.
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1"), make_project("p2")];
        data.project_order = vec!["p1".to_string(), "p2".to_string()];
        data.main_window.hidden_project_ids.insert("p1".to_string());
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.is_project_hidden("p1"));
            // Missing entry defaults to visible (not hidden).
            assert!(!ws.is_project_hidden("p2"));
            assert!(!ws.is_project_hidden("missing"));
        });
    }

    #[gpui::test]
    fn toggle_project_overview_visibility_writes_to_main_window(cx: &mut gpui::TestAppContext) {
        // Toggling project visibility flips main_window.hidden_project_ids
        // (the per-window viewport model's source of truth).
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1")];
        data.project_order = vec!["p1".to_string()];
        let workspace = cx.new(|_cx| Workspace::new(data));

        // First toggle: visible -> hidden. main_window inserts the id.
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_project_overview_visibility(WindowId::Main, "p1", cx);
        });
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.data().main_window.hidden_project_ids.contains("p1"));
        });

        // Second toggle: hidden -> visible. main_window removes the entry.
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_project_overview_visibility(WindowId::Main, "p1", cx);
        });
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(!ws.data().main_window.hidden_project_ids.contains("p1"));
        });
    }

    #[gpui::test]
    fn toggle_worktree_visibility_writes_to_main_window(cx: &mut gpui::TestAppContext) {
        // Same as toggle_project_overview_visibility but for the worktree
        // entrypoint: flip main_window.hidden_project_ids when targeted at
        // WindowId::Main.
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1")];
        data.project_order = vec!["p1".to_string()];
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_worktree_visibility(WindowId::Main, "p1", cx);
        });
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.data().main_window.hidden_project_ids.contains("p1"));
        });

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_worktree_visibility(WindowId::Main, "p1", cx);
        });
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(!ws.data().main_window.hidden_project_ids.contains("p1"));
        });
    }

    #[gpui::test]
    fn toggle_worktree_visibility_bumps_data_version_for_unknown_id(cx: &mut gpui::TestAppContext) {
        // Post-migration contract: toggle_worktree_visibility delegates through
        // Workspace::toggle_hidden(window_id, ...), which unconditionally bumps
        // data_version. The pure data setter mutates the hidden set regardless
        // of whether the id corresponds to a real project, so the mutation IS
        // a persisted state change that must trigger auto-save. The
        // pre-migration body gated notify_data on `self.project(id).is_some()`,
        // which would leave data_version at 0 here. Pinning the new behavior
        // defends against a regression that re-introduces the gate.
        let workspace = cx.new(|_cx| Workspace::new(make_workspace_data()));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_worktree_visibility(WindowId::Main, "unknown_id", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.data().main_window.hidden_project_ids.contains("unknown_id"));
            assert_eq!(ws.data_version(), 1);
        });
    }

    #[gpui::test]
    fn toggle_worktree_visibility_extra_writes_only_to_targeted_window(cx: &mut gpui::TestAppContext) {
        // Per-window viewport model: toggling on WindowId::Extra(uuid) flips
        // only that extra's hidden_project_ids -- main and any sibling extras
        // stay untouched. Defends against a regression that ignores window_id
        // and unconditionally writes to main, scatters the toggle across all
        // extras, or routes through main's slot. Pre-populate main + sibling
        // extra with sibling state to verify isolation.
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1"), make_project("p2")];
        data.project_order = vec!["p1".to_string(), "p2".to_string()];
        data.main_window.hidden_project_ids.insert("p2".to_string());
        let extra_a = WindowState::default();
        let extra_a_id = extra_a.id;
        let mut extra_b = WindowState::default();
        extra_b.hidden_project_ids.insert("p2".to_string());
        let extra_b_id = extra_b.id;
        data.extra_windows = vec![extra_a, extra_b];
        let workspace = cx.new(|_cx| Workspace::new(data));

        // First toggle: visible -> hidden in extra_a.
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_worktree_visibility(WindowId::Extra(extra_a_id), "p1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            // Targeted extra got p1 hidden.
            assert!(ws.data().extra_windows[0].hidden_project_ids.contains("p1"));
            // Main does NOT have p1 hidden.
            assert!(!ws.data().main_window.hidden_project_ids.contains("p1"));
            // Sibling extra does NOT have p1 hidden.
            assert!(!ws.data().extra_windows[1].hidden_project_ids.contains("p1"));
            // Sibling p2 state preserved on main + sibling extra.
            assert!(ws.data().main_window.hidden_project_ids.contains("p2"));
            assert!(ws.data().extra_windows[1].hidden_project_ids.contains("p2"));
            assert_eq!(extra_b_id, ws.data().extra_windows[1].id);
        });

        // Second toggle: hidden -> visible in extra_a.
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_worktree_visibility(WindowId::Extra(extra_a_id), "p1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(!ws.data().extra_windows[0].hidden_project_ids.contains("p1"));
        });
    }

    #[gpui::test]
    fn toggle_worktree_visibility_unknown_extra_is_silent_noop(cx: &mut gpui::TestAppContext) {
        // Close-race contract: a fresh uuid that does not match any extra
        // produces no panic; main_window stays untouched. Pre-populate main
        // with hidden state for p1 to ensure the unknown-extra path does NOT
        // silently fall back to main as a default. data_version still bumps
        // via notify_data, matching the silent-no-op contract on the
        // data-layer setter. Defends against a regression that replaces the
        // window_mut lookup with direct main_window access.
        let mut data = make_workspace_data();
        data.main_window.hidden_project_ids.insert("p1".to_string());
        let workspace = cx.new(|_cx| Workspace::new(data));
        let unknown = uuid::Uuid::new_v4();

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_worktree_visibility(WindowId::Extra(unknown), "p1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            // Main's p1 hidden state is unchanged (NOT toggled to visible).
            assert!(ws.data().main_window.hidden_project_ids.contains("p1"));
            assert_eq!(ws.data_version(), 1);
        });
    }

    #[gpui::test]
    fn toggle_project_overview_visibility_unknown_id_is_noop(cx: &mut gpui::TestAppContext) {
        // Post-migration contract: the project-existence early-return guard
        // (`if self.project(project_id).is_none() { return; }`) at the top of
        // toggle_project_overview_visibility is preserved through the
        // delegation onto Workspace::toggle_hidden. An unknown id must NOT
        // mutate main_window.hidden_project_ids and must NOT bump data_version
        // -- the sidebar context-menu UX expects a no-op on a stale id (the
        // entrypoint is the project-overview row, where a click landing after
        // a delete must be silent).
        //
        // This contrasts with toggle_worktree_visibility (no guard, bumps
        // unconditionally per the previous commit) and is the load-bearing
        // difference between the two delegating wrappers. Defends against a
        // regression that drops the guard "for symmetry with
        // toggle_worktree_visibility" or that lifts the guard into the
        // shared toggle_hidden setter (which would force every caller to
        // either accept the guard or bypass via direct data access).
        let workspace = cx.new(|_cx| Workspace::new(make_workspace_data()));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_project_overview_visibility(WindowId::Main, "unknown_id", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(!ws.data().main_window.hidden_project_ids.contains("unknown_id"));
            assert_eq!(ws.data_version(), 0);
        });
    }

    #[gpui::test]
    fn toggle_project_overview_visibility_extra_writes_only_to_targeted_window(cx: &mut gpui::TestAppContext) {
        // Per-window viewport model: toggling on WindowId::Extra(uuid) flips
        // only that extra's hidden_project_ids -- main and any sibling extras
        // stay untouched. Defends against a regression that ignores window_id
        // and unconditionally writes to main, scatters the toggle across all
        // extras, or routes through main's slot. Pre-populate main + sibling
        // extra with sibling state to verify isolation.
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1"), make_project("p2")];
        data.project_order = vec!["p1".to_string(), "p2".to_string()];
        data.main_window.hidden_project_ids.insert("p2".to_string());
        let extra_a = WindowState::default();
        let extra_a_id = extra_a.id;
        let mut extra_b = WindowState::default();
        extra_b.hidden_project_ids.insert("p2".to_string());
        let extra_b_id = extra_b.id;
        data.extra_windows = vec![extra_a, extra_b];
        let workspace = cx.new(|_cx| Workspace::new(data));

        // First toggle: visible -> hidden in extra_a.
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_project_overview_visibility(WindowId::Extra(extra_a_id), "p1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            // Targeted extra got p1 hidden.
            assert!(ws.data().extra_windows[0].hidden_project_ids.contains("p1"));
            // Main does NOT have p1 hidden.
            assert!(!ws.data().main_window.hidden_project_ids.contains("p1"));
            // Sibling extra does NOT have p1 hidden.
            assert!(!ws.data().extra_windows[1].hidden_project_ids.contains("p1"));
            // Sibling p2 state preserved on main + sibling extra.
            assert!(ws.data().main_window.hidden_project_ids.contains("p2"));
            assert!(ws.data().extra_windows[1].hidden_project_ids.contains("p2"));
            assert_eq!(extra_b_id, ws.data().extra_windows[1].id);
        });

        // Second toggle: hidden -> visible in extra_a. Pins the round-trip
        // semantic so a regression that hard-codes insert-only or remove-only
        // would surface here.
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_project_overview_visibility(WindowId::Extra(extra_a_id), "p1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(!ws.data().extra_windows[0].hidden_project_ids.contains("p1"));
        });
    }

    #[gpui::test]
    fn toggle_project_overview_visibility_unknown_extra_is_silent_noop(cx: &mut gpui::TestAppContext) {
        // Close-race contract: a fresh uuid that does not match any extra
        // produces no panic; main_window stays untouched. Pre-populate main
        // with hidden state for p1 to ensure the unknown-extra path does NOT
        // silently fall back to main as a default. data_version still bumps
        // via notify_data (the project-existence guard is satisfied because
        // p1 IS a real project; only the WINDOW lookup misses), matching
        // the silent-no-op contract on the data-layer setter. Defends
        // against a regression that replaces the window_mut lookup with
        // direct main_window access.
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1")];
        data.project_order = vec!["p1".to_string()];
        data.main_window.hidden_project_ids.insert("p1".to_string());
        let workspace = cx.new(|_cx| Workspace::new(data));
        let unknown = uuid::Uuid::new_v4();

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_project_overview_visibility(WindowId::Extra(unknown), "p1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            // Main's p1 hidden state is unchanged (NOT toggled to visible).
            assert!(ws.data().main_window.hidden_project_ids.contains("p1"));
            assert_eq!(ws.data_version(), 1);
        });
    }

    #[gpui::test]
    fn update_project_widths_writes_only_to_main_window(cx: &mut gpui::TestAppContext) {
        // Per-window viewport model: writes go to main_window.project_widths
        // (the source of truth). The legacy top-level WorkspaceData.project_widths
        // field has been removed entirely.
        let workspace = cx.new(|_cx| Workspace::new(make_workspace_data()));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut widths = HashMap::new();
            widths.insert("p1".to_string(), 60.0);
            widths.insert("p2".to_string(), 40.0);
            ws.update_project_widths(WindowId::Main, widths, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data().main_window.project_widths.get("p1"), Some(&60.0));
            assert_eq!(ws.data().main_window.project_widths.get("p2"), Some(&40.0));
        });
    }

    #[gpui::test]
    fn update_project_widths_wholesale_replaces_existing_entries(cx: &mut gpui::TestAppContext) {
        // Wholesale-replace contract: keys absent from the supplied map are
        // removed from main_window.project_widths. Pins the semantic so a
        // future refactor that drops the leading clear() (e.g. switching to a
        // merge body) silently breaks here. Pre-populate p1, then call with a
        // map containing only p2 -- p1 must be gone.
        let mut data = make_workspace_data();
        data.main_window.project_widths.insert("p1".to_string(), 0.50);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut widths = HashMap::new();
            widths.insert("p2".to_string(), 0.40);
            ws.update_project_widths(WindowId::Main, widths, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(!ws.data().main_window.project_widths.contains_key("p1"));
            assert_eq!(ws.data().main_window.project_widths.get("p2").copied(), Some(0.40));
        });
    }

    #[gpui::test]
    fn update_project_widths_bumps_data_version_exactly_once(cx: &mut gpui::TestAppContext) {
        // One call -> one data_version bump, even when the supplied map has
        // multiple entries. Defends against a future refactor that delegates
        // to the entity-level `set_project_width(WindowId, ...)` per entry,
        // which would bump per entry and disturb the auto-save observer's
        // debounce cadence.
        let workspace = cx.new(|_cx| Workspace::new(make_workspace_data()));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut widths = HashMap::new();
            widths.insert("p1".to_string(), 0.30);
            widths.insert("p2".to_string(), 0.40);
            widths.insert("p3".to_string(), 0.30);
            ws.update_project_widths(WindowId::Main, widths, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data_version(), 1);
        });
    }

    #[gpui::test]
    fn update_project_widths_extra_writes_only_to_targeted_window(cx: &mut gpui::TestAppContext) {
        // Per-window viewport model: writes targeted at WindowId::Extra(uuid)
        // land on that extra's project_widths only -- main and any sibling
        // extras stay untouched. Defends against a regression that ignores
        // window_id and unconditionally writes to main, scatters the write
        // across all extras, or routes through main's slot.
        let mut data = make_workspace_data();
        let mut extra_a = WindowState::default();
        let extra_a_id = extra_a.id;
        let mut extra_b = WindowState::default();
        let extra_b_id = extra_b.id;
        // Pre-populate sibling state on main + extra_b to verify isolation.
        data.main_window.project_widths.insert("p1".to_string(), 100.0);
        extra_b.project_widths.insert("p1".to_string(), 200.0);
        // extra_a starts empty.
        let _ = extra_a_id;
        data.extra_windows = vec![extra_a, extra_b];
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut widths = HashMap::new();
            widths.insert("p1".to_string(), 60.0);
            widths.insert("p2".to_string(), 40.0);
            ws.update_project_widths(WindowId::Extra(extra_a_id), widths, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            // Targeted extra got both new entries.
            let extra_a_widths = &ws.data().extra_windows[0].project_widths;
            assert_eq!(extra_a_widths.get("p1"), Some(&60.0));
            assert_eq!(extra_a_widths.get("p2"), Some(&40.0));
            // Main's p1 width is untouched.
            assert_eq!(ws.data().main_window.project_widths.get("p1"), Some(&100.0));
            // Sibling extra's p1 width is untouched.
            assert_eq!(ws.data().extra_windows[1].project_widths.get("p1"), Some(&200.0));
            // Sibling extra has no p2 from the targeted write.
            assert!(!ws.data().extra_windows[1].project_widths.contains_key("p2"));
            // Main has no p2 from the targeted write.
            assert!(!ws.data().main_window.project_widths.contains_key("p2"));
            assert_eq!(extra_b_id, ws.data().extra_windows[1].id);
        });
    }

    #[gpui::test]
    fn update_project_widths_unknown_extra_is_silent_noop(cx: &mut gpui::TestAppContext) {
        // Close-race contract: a fresh uuid that does not match any extra
        // produces no panic; main_window stays untouched. Pre-populate main
        // to ensure the unknown-extra path does NOT silently fall back to
        // main as a default. data_version still bumps via notify_data,
        // matching the silent-no-op contract on the data-layer setters.
        let mut data = make_workspace_data();
        data.main_window.project_widths.insert("p1".to_string(), 50.0);
        let workspace = cx.new(|_cx| Workspace::new(data));
        let unknown = uuid::Uuid::new_v4();

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut widths = HashMap::new();
            widths.insert("p1".to_string(), 99.0);
            ws.update_project_widths(WindowId::Extra(unknown), widths, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data().main_window.project_widths.get("p1"), Some(&50.0));
            assert_eq!(ws.data_version(), 1);
        });
    }

    #[gpui::test]
    fn delete_project_clears_main_window_project_width(cx: &mut gpui::TestAppContext) {
        // Deleting a project must scrub its width from main_window.project_widths
        // (the source of truth). Without the scrub, a re-added project with the
        // same id would inherit the deleted project's width on the next render.
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1"), make_project("p2")];
        data.project_order = vec!["p1".to_string(), "p2".to_string()];
        data.main_window.project_widths.insert("p1".to_string(), 60.0);
        data.main_window.project_widths.insert("p2".to_string(), 40.0);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.delete_project("p1", &HooksConfig::default(), cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(!ws.data().main_window.project_widths.contains_key("p1"));
            assert!(ws.data().main_window.project_widths.contains_key("p2"));
        });
    }

    #[gpui::test]
    fn delete_project_scrubs_extra_window_per_project_state(cx: &mut gpui::TestAppContext) {
        // Per the multi-window viewport model, deleting a project must scrub
        // its id from EVERY window's per-project storage -- not just main.
        // Without the fan-out, an extra window would retain orphan width and
        // hidden-set entries for a project that no longer exists; on next
        // launch those entries would either (a) bloat the on-disk shape or
        // (b) silently re-apply if a project with the same id were ever
        // re-added. This pins the slice 02 acceptance criterion "Project
        // delete invokes `delete_project_scrub_all_windows` so no orphan
        // entries remain" -- specifically the extras leg, since slice 05 has
        // not landed yet so extras only exist in manually-constructed test
        // fixtures today. Defends against a regression that drops the helper
        // call and falls back to a main-only inline scrub.
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1"), make_project("p2")];
        data.project_order = vec!["p1".to_string(), "p2".to_string()];
        data.main_window.project_widths.insert("p1".to_string(), 60.0);
        data.main_window.hidden_project_ids.insert("p1".to_string());
        let mut extra1 = WindowState::default();
        extra1.project_widths.insert("p1".to_string(), 30.0);
        extra1.project_widths.insert("p2".to_string(), 70.0);
        extra1.hidden_project_ids.insert("p1".to_string());
        let mut extra2 = WindowState::default();
        extra2.project_widths.insert("p1".to_string(), 50.0);
        extra2.hidden_project_ids.insert("p1".to_string());
        extra2.hidden_project_ids.insert("p2".to_string());
        data.extra_windows.push(extra1);
        data.extra_windows.push(extra2);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.delete_project("p1", &HooksConfig::default(), cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            // Main: p1 scrubbed from both per-project fields, p2 untouched.
            assert!(!ws.data().main_window.project_widths.contains_key("p1"));
            assert!(!ws.data().main_window.hidden_project_ids.contains("p1"));
            // Every extra: p1 scrubbed; sibling project state preserved.
            for extra in &ws.data().extra_windows {
                assert!(!extra.project_widths.contains_key("p1"));
                assert!(!extra.hidden_project_ids.contains("p1"));
            }
            assert!(ws.data().extra_windows[0].project_widths.contains_key("p2"));
            assert!(ws.data().extra_windows[1].hidden_project_ids.contains("p2"));
        });
    }

    #[gpui::test]
    fn remove_stale_worktree_scrubs_extra_window_per_project_state(cx: &mut gpui::TestAppContext) {
        // `remove_stale_worktree` is the secondary project-removal path (called
        // when a worktree's directory has been deleted on disk by an external
        // tool); it must produce the same per-window scrub fan-out as the
        // primary `delete_project` flow. Without this pinning, the worktree
        // path could regress to a main-only scrub silently while the primary
        // delete stays correct, leaving extras with orphan worktree entries.
        let mut data = make_workspace_data();
        let parent = make_project("parent");
        let wt = make_worktree_project("wt1", "parent");
        data.projects = vec![parent, wt];
        data.project_order = vec!["parent".to_string()];
        data.main_window.project_widths.insert("wt1".to_string(), 35.0);
        data.main_window.hidden_project_ids.insert("wt1".to_string());
        let mut extra = WindowState::default();
        extra.project_widths.insert("wt1".to_string(), 20.0);
        extra.hidden_project_ids.insert("wt1".to_string());
        data.extra_windows.push(extra);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, _cx| {
            ws.remove_stale_worktree("wt1");
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(!ws.data().main_window.project_widths.contains_key("wt1"));
            assert!(!ws.data().main_window.hidden_project_ids.contains("wt1"));
            assert!(!ws.data().extra_windows[0].project_widths.contains_key("wt1"));
            assert!(!ws.data().extra_windows[0].hidden_project_ids.contains("wt1"));
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
        p.worktree_info = Some(crate::state::WorktreeMetadata {
            parent_project_id: parent_id.to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: format!("/tmp/worktrees/{}", id),
            branch_name: String::new(),
        });
        p
    }

    #[gpui::test]
    fn test_delete_worktree_removes_from_parent_worktree_ids(cx: &mut gpui::TestAppContext) {
        let mut parent = make_project("parent");
        parent.worktree_ids = vec!["wt1".to_string(), "wt2".to_string()];
        let mut data = make_workspace_data();
        data.projects = vec![parent, make_worktree_project("wt1", "parent"), make_worktree_project("wt2", "parent")];
        data.project_order = vec!["parent".to_string()];
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.delete_project("wt1", &HooksConfig::default(), cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let parent = ws.project("parent").unwrap();
            assert_eq!(parent.worktree_ids, vec!["wt2".to_string()]);
            assert!(!ws.data().project_order.contains(&"wt1".to_string()));
        });
    }

    #[gpui::test]
    fn test_delete_parent_rehomes_orphaned_worktrees(cx: &mut gpui::TestAppContext) {
        let mut parent = make_project("parent");
        parent.worktree_ids = vec!["wt1".to_string(), "wt2".to_string()];
        let mut data = make_workspace_data();
        data.projects = vec![parent, make_worktree_project("wt1", "parent"), make_worktree_project("wt2", "parent")];
        data.project_order = vec!["parent".to_string()];
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.delete_project("parent", &HooksConfig::default(), cx);
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

    #[test]
    fn test_remove_stale_worktree_skips_closing_project() {
        let mut data = make_workspace_data();
        let wt = make_worktree_project("wt1", "parent");
        data.projects = vec![make_project("parent"), wt];
        data.project_order = vec!["parent".to_string()];
        let mut ws = Workspace::new(data);
        ws.lifecycle.mark_closing("wt1");

        ws.remove_stale_worktree("wt1");

        assert!(ws.project("wt1").is_some(), "closing project should not be removed");
    }

    #[test]
    fn test_remove_stale_worktree_skips_creating_project() {
        let mut data = make_workspace_data();
        let wt = make_worktree_project("wt1", "parent");
        data.projects = vec![make_project("parent"), wt];
        data.project_order = vec!["parent".to_string()];
        let mut ws = Workspace::new(data);
        ws.lifecycle.mark_creating("wt1");

        ws.remove_stale_worktree("wt1");

        assert!(ws.project("wt1").is_some(), "creating project should not be removed");
    }

    #[test]
    fn test_remove_stale_worktree_succeeds_when_not_managed() {
        let mut data = make_workspace_data();
        let wt = make_worktree_project("wt1", "parent");
        data.projects = vec![make_project("parent"), wt];
        data.project_order = vec!["parent".to_string()];
        let mut ws = Workspace::new(data);

        ws.remove_stale_worktree("wt1");

        assert!(ws.project("wt1").is_none(), "unmanaged stale worktree should be removed");
    }
}
