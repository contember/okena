//! Worktree lifecycle workspace actions
//!
//! Actions for creating, registering, discovering, and removing git
//! worktree projects, plus worktree-specific properties and ordering.

use okena_core::theme::FolderColor;
use crate::context::WorkspaceCx;
use crate::focus::FocusManager;
use crate::hooks;
use crate::persistence::HooksConfig;
use crate::state::{LayoutNode, PendingWorktreeClose, ProjectData, Workspace, WindowId};
use std::collections::HashMap;

/// Captured inputs for a two-phase worktree removal. [`Workspace::begin_worktree_removal`]
/// snapshots everything the finalize step needs (branch, paths, hooks) BEFORE the
/// git worktree checkout is deleted, so the daemon can run the slow, blocking
/// `git worktree remove` off the command-loop thread and then
/// [`Workspace::finish_worktree_removal`] applies the state change from this
/// snapshot — the checkout is gone by then, so branch/paths can't be re-read.
pub struct WorktreeRemovalPlan {
    pub project_id: String,
    /// The git worktree root to remove (may differ from project.path for monorepos).
    pub worktree_path: std::path::PathBuf,
    /// The main repo path — used for `git worktree prune` in the fast removal.
    pub main_repo_path: String,
    branch: String,
    project_hooks: HooksConfig,
    project_name: String,
    folder_id: Option<String>,
    folder_name: Option<String>,
}

/// Result of the worktree-close merge pipeline ([`close_worktree_merge_git`]).
/// The pipeline is pure git + headless hooks (no workspace access), so it can run
/// off the daemon reactor; the caller applies the workspace-side effects.
pub enum CloseWorktreeGitOutcome {
    /// Merge (or a no-op merge) succeeded. `did_stash` gates `force_remove`.
    Ok { did_stash: bool },
    /// Rebase hit a conflict. The `on_rebase_conflict` hook produced these
    /// terminal commands + hook results for the caller to apply under the lock,
    /// then abort the close with `error`.
    RebaseConflict {
        error: String,
        terminal_actions: Vec<(String, HashMap<String, String>)>,
        hook_results: Vec<crate::hooks::HookTerminalResult>,
    },
    /// A git step (or the `pre_merge` hook) failed; stash-pop recovery already ran.
    Err(String),
}

/// Restore a stash after a failed merge step (best-effort; a failed pop only warns).
fn stash_pop_recover(did_stash: bool, project_path: &str, branch: &str, step: &str) {
    if did_stash
        && let Err(pop_err) = okena_git::stash_pop(std::path::Path::new(project_path))
    {
        log::warn!(
            "Failed to restore stashed changes for worktree '{}' at {} after {} failure: {}. Your changes remain in the git stash — run `git stash pop` in that worktree to recover them.",
            branch, project_path, step, pop_err
        );
    }
}

/// The worktree-close merge pipeline: stash → fetch → pre_merge hook → rebase →
/// merge → post_merge hook → push → delete-branch, with stash-pop recovery on any
/// failing step. PURE: only git subprocesses + headless hooks (monitor, no PTY
/// runner), no `&mut Workspace` — so the daemon runs it on a blocking thread with
/// no lock held. `on_rebase_conflict` is fired headless (no runner) and its
/// terminal/hook results are RETURNED for the caller to apply, not spawned here.
/// Call only when the merge is actually enabled.
#[allow(clippy::too_many_arguments)] // cohesive close-pipeline inputs
pub fn close_worktree_merge_git(
    stash_enabled: bool,
    fetch_enabled: bool,
    push_enabled: bool,
    delete_branch_enabled: bool,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    branch: &str,
    default_branch: &str,
    main_repo_path: &str,
    project_hooks: &HooksConfig,
    global_hooks: &HooksConfig,
    folder_id: Option<&str>,
    folder_name: Option<&str>,
    monitor: Option<&okena_hooks::HookMonitor>,
    runner: Option<&okena_hooks::HookRunner>,
) -> CloseWorktreeGitOutcome {
    use std::path::Path;
    let mut did_stash = false;

    if stash_enabled {
        if let Err(e) = okena_git::stash_changes(Path::new(project_path)) {
            return CloseWorktreeGitOutcome::Err(format!("Stash failed: {}", e));
        }
        did_stash = true;
    }

    if fetch_enabled
        && let Err(e) = okena_git::fetch_all(Path::new(project_path))
    {
        stash_pop_recover(did_stash, project_path, branch, "fetch");
        return CloseWorktreeGitOutcome::Err(format!("Fetch failed: {}", e));
    }

    // pre_merge hook (sync, headless — no PTY runner).
    if let Err(e) = hooks::fire_pre_merge(
        project_hooks, global_hooks, project_id, project_name, project_path,
        branch, default_branch, main_repo_path, folder_id, folder_name, monitor, None,
    ) {
        stash_pop_recover(did_stash, project_path, branch, "pre_merge hook");
        return CloseWorktreeGitOutcome::Err(format!("pre_merge hook failed: {}", e));
    }

    // Rebase; on conflict, fire on_rebase_conflict headless and return its data.
    if let Err(e) = okena_git::rebase_onto(Path::new(project_path), default_branch) {
        let error_msg = e.to_string();
        let (terminal_actions, hook_results) = hooks::fire_on_rebase_conflict(
            project_hooks, global_hooks, project_id, project_name, project_path,
            branch, default_branch, main_repo_path, &error_msg, folder_id, folder_name, monitor, runner,
        );
        stash_pop_recover(did_stash, project_path, branch, "rebase");
        return CloseWorktreeGitOutcome::RebaseConflict {
            error: format!("Rebase failed: {}", e),
            terminal_actions,
            hook_results,
        };
    }

    // Merge (ff-only) in the main repo.
    if let Err(e) = okena_git::merge_branch(Path::new(main_repo_path), branch, true) {
        stash_pop_recover(did_stash, project_path, branch, "merge");
        return CloseWorktreeGitOutcome::Err(format!("Merge failed: {}", e));
    }

    // post_merge hook (headless, fire-and-forget).
    let _ = hooks::fire_post_merge(
        project_hooks, global_hooks, project_id, project_name, project_path,
        branch, default_branch, main_repo_path, folder_id, folder_name, monitor, runner,
    );

    if push_enabled
        && let Err(e) = okena_git::push_branch(Path::new(main_repo_path), default_branch)
    {
        log::warn!("Push failed (continuing): {}", e);
    }

    if delete_branch_enabled {
        if let Err(e) = okena_git::delete_local_branch(Path::new(main_repo_path), branch) {
            log::warn!("Delete local branch failed (continuing): {}", e);
        }
        if let Err(e) = okena_git::delete_remote_branch(Path::new(main_repo_path), branch) {
            log::warn!("Delete remote branch failed (continuing): {}", e);
        }
    }

    CloseWorktreeGitOutcome::Ok { did_stash }
}

impl Workspace {
    /// Toggle visibility for a single worktree (no propagation to children).
    ///
    /// Delegates to `Workspace::toggle_hidden(window_id, ...)`, which flips
    /// membership in the targeted window's `hidden_project_ids` and bumps
    /// `data_version` so the auto-save observer triggers. Per the multi-window
    /// viewport model, hidden state IS persisted -- the bump is unconditional,
    /// even for ids that do not currently match a project. Unknown extra ids
    /// are a silent no-op (close-race contract inherited from `toggle_hidden`).
    pub fn toggle_worktree_visibility(&mut self, window_id: WindowId, project_id: &str, cx: &mut impl WorkspaceCx) {
        self.toggle_hidden(window_id, project_id, cx);
    }

    /// Set or clear the color override for a worktree project
    pub fn set_worktree_color_override(&mut self, project_id: &str, color: Option<FolderColor>, cx: &mut impl WorkspaceCx) {
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
    pub fn reorder_worktree(&mut self, parent_id: &str, worktree_id: &str, new_index: usize, cx: &mut impl WorkspaceCx) {
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
        cx: &mut impl WorkspaceCx,
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
        cx: &mut impl WorkspaceCx,
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
        cx: &mut impl WorkspaceCx,
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
        cx: &mut impl WorkspaceCx,
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
            // When hooks are deferred the worktree directory doesn't exist yet,
            // so use None (no terminals spawned until creation finishes). Otherwise
            // clone the parent's structure; if the parent has NO layout, still seed
            // a single terminal so the new worktree opens with an initial shell
            // instead of an empty project (matches the deferred `fire_worktree_hooks`
            // path). `spawn_uninitialized_terminals` materializes the seeded slot.
            layout: if fire_hooks {
                new_layout.or_else(|| Some(crate::state::LayoutNode::new_terminal()))
            } else {
                None
            },
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
            pinned: false,
            last_activity_at: None,
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
            let runner = cx.hook_runner();
            let monitor = cx.hook_monitor();
            let hook_results = hooks::fire_on_worktree_create(
                &new_project_hooks,
                &id,
                &new_project_name,
                project_path,
                branch,
                folder_id,
                folder_name,
                global_hooks,
                runner.as_ref(),
                monitor.as_ref(),
            );
            self.register_hook_results(hook_results, cx);
        }

        Ok(id)
    }

    /// Finalize a deferred worktree: set the layout from the parent and fire hooks.
    /// Called once the worktree directory exists on disk.
    pub fn fire_worktree_hooks(&mut self, project_id: &str, global_hooks: &HooksConfig, cx: &mut impl WorkspaceCx) {
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
        let runner = cx.hook_runner();
        let monitor = cx.hook_monitor();
        let hook_results = hooks::fire_on_worktree_create(
            &hooks_config,
            project_id,
            &name,
            &path,
            &branch,
            folder_id,
            folder_name,
            global_hooks,
            runner.as_ref(),
            monitor.as_ref(),
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
            pinned: false,
            last_activity_at: None,
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
        // Scrub the child id from its parent's worktree_ids, or the sidebar keeps
        // a dangling phantom child (both for externally-deleted worktrees and the
        // optimistic-create rollback path). `delete_project` already does this; a
        // stale removal must too.
        for parent in &mut self.data.projects {
            parent.worktree_ids.retain(|id| id != project_id);
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

    /// Remove a worktree project and its git worktree (synchronous). Fires the
    /// `on_worktree_close` hook, runs `git worktree remove`, then finalizes state.
    /// Single entry point for in-process / GUI / test callers; the daemon splits
    /// it via [`begin_worktree_removal`](Self::begin_worktree_removal) + an
    /// off-reactor `git worktree remove` + [`finish_worktree_removal`](Self::finish_worktree_removal)
    /// so the (slow, blocking) git call doesn't stall the command loop.
    pub fn remove_worktree_project(&mut self, focus_manager: &mut FocusManager, project_id: &str, force: bool, global_hooks: &HooksConfig, cx: &mut impl WorkspaceCx) -> Result<(), String> {
        let plan = self.begin_worktree_removal(project_id, global_hooks, cx)?;
        okena_git::remove_worktree(&plan.worktree_path, force)
            .map_err(|e| e.to_string())?;
        self.finish_worktree_removal(focus_manager, &plan, global_hooks, cx);
        Ok(())
    }

    /// Phase 1 of worktree removal: validate, snapshot the inputs the finalize
    /// step needs, and fire `on_worktree_close` (which needs the worktree to
    /// still exist for a valid CWD). Returns the plan; the caller then runs
    /// `git worktree remove` (off-reactor on the daemon) and calls
    /// [`finish_worktree_removal`](Self::finish_worktree_removal).
    pub fn begin_worktree_removal(&mut self, project_id: &str, global_hooks: &HooksConfig, cx: &mut impl WorkspaceCx) -> Result<WorktreeRemovalPlan, String> {
        let project = self.project(project_id)
            .ok_or_else(|| "Project not found".to_string())?;
        if project.worktree_info.is_none() {
            return Err("Not a worktree project".to_string());
        }

        // Snapshot everything BEFORE removal, while the project is still in state
        // and its checkout exists on disk (git worktree remove deletes it).
        let folder = self.folder_for_project_or_parent(project_id);
        let folder_id = folder.map(|f| f.id.clone());
        let folder_name = folder.map(|f| f.name.clone());
        let project_hooks = project.hooks.clone();
        let project_name = project.name.clone();
        let project_path = project.path.clone();
        let main_repo_path = self.worktree_parent_path(project_id).unwrap_or_default();
        // For monorepos the project path is a subdirectory inside the checkout;
        // resolve the actual worktree root so `git worktree remove` gets it right.
        let project_pathbuf = std::path::PathBuf::from(&project_path);
        let worktree_path = okena_git::get_repo_root(&project_pathbuf)
            .unwrap_or(project_pathbuf);
        let branch = okena_git::get_current_branch(&worktree_path).unwrap_or_default();

        // Fire on_worktree_close BEFORE removal so the hook has a valid CWD.
        let monitor = cx.hook_monitor();
        hooks::fire_on_worktree_close_with_services(&project_hooks, project_id, &project_name, &project_path, &branch, folder_id.as_deref(), folder_name.as_deref(), global_hooks, monitor.as_ref());

        Ok(WorktreeRemovalPlan {
            project_id: project_id.to_string(),
            worktree_path,
            main_repo_path,
            branch,
            project_hooks,
            project_name,
            folder_id,
            folder_name,
        })
    }

    /// Phase 2 of worktree removal (after `git worktree remove` has run): delete
    /// the project from workspace state (which fires `on_project_close`) and fire
    /// the `worktree_removed` hook from the `plan` snapshot. This is the single
    /// convergence point for every removal route, so the hook fires exactly once;
    /// the checkout is gone, so it runs from `main_repo_path` (OKENA_BRANCH still
    /// carries the removed branch). Results are discarded (fire-and-forget).
    pub fn finish_worktree_removal(&mut self, focus_manager: &mut FocusManager, plan: &WorktreeRemovalPlan, global_hooks: &HooksConfig, cx: &mut impl WorkspaceCx) {
        self.delete_project(focus_manager, &plan.project_id, global_hooks, cx);
        self.fire_worktree_removed_hook(plan, global_hooks, cx);
    }

    /// Fire the `on_worktree_removed` hook. Split out of
    /// [`finish_worktree_removal`](Self::finish_worktree_removal) so the
    /// optimistic deferred-close path can `delete_project` immediately (the
    /// client's row vanishes at once) and fire this only after the physical
    /// directory delete finishes — preserving the hook's "actually removed"
    /// semantics without making the row hang around for the whole `remove_dir_all`.
    pub fn fire_worktree_removed_hook(&self, plan: &WorktreeRemovalPlan, global_hooks: &HooksConfig, cx: &mut impl WorkspaceCx) {
        let runner = cx.hook_runner();
        let monitor = cx.hook_monitor();
        let _ = hooks::fire_worktree_removed(
            &plan.project_hooks,
            global_hooks,
            &plan.project_id,
            &plan.project_name,
            &plan.main_repo_path,
            &plan.branch,
            &plan.main_repo_path,
            plan.folder_id.as_deref(),
            plan.folder_name.as_deref(),
            monitor.as_ref(),
            runner.as_ref(),
        );
    }

    /// Close a worktree project: optionally stash/fetch/rebase/merge/push/
    /// delete-branch, then remove the worktree. Hook integration runs before
    /// the merge step and before the actual removal.
    ///
    /// Daemon-side port of the client `CloseWorktreeDialog::execute` pipeline:
    /// runs synchronously off the UI thread, so there is no `processing`/error
    /// UI state — failures return `Err` with the same message text. The
    /// stash-pop recovery on a failed merge step still runs; a failed recovery
    /// only logs a warning, and the original step error is returned.
    ///
    /// Inputs are recomputed authoritatively from git/state (the client request
    /// only carries the toggle booleans).
    #[allow(clippy::too_many_arguments)] // cohesive close-pipeline toggle flags
    pub fn close_worktree(
        &mut self,
        focus_manager: &mut FocusManager,
        project_id: &str,
        merge: bool,
        stash: bool,
        fetch: bool,
        push: bool,
        delete_branch: bool,
        global_hooks: &HooksConfig,
        cx: &mut impl WorkspaceCx,
    ) -> Result<(), String> {
        // Recompute the git-derived values authoritatively (don't trust the client).
        let project = self.project(project_id)
            .ok_or_else(|| "Project not found".to_string())?;
        let project_name = project.name.clone();
        let project_path = project.path.clone();
        let project_hooks = project.hooks.clone();

        let main_repo_path = self.worktree_parent_path(project_id).unwrap_or_default();
        let branch = okena_git::get_current_branch(std::path::Path::new(&project_path)).unwrap_or_default();
        let default_branch = okena_git::get_default_branch(std::path::Path::new(&main_repo_path)).unwrap_or_default();
        let is_dirty = okena_git::has_uncommitted_changes(std::path::Path::new(&project_path));

        let merge_enabled = merge && (!is_dirty || stash) && !branch.is_empty() && !default_branch.is_empty();
        let stash_enabled = stash && is_dirty;
        let fetch_enabled = fetch;
        let push_enabled = push;
        let delete_branch_enabled = delete_branch;

        let folder = self.folder_for_project_or_parent(project_id);
        let folder_id = folder.map(|f| f.id.clone());
        let folder_name = folder.map(|f| f.name.clone());

        let monitor = cx.hook_monitor();
        let runner = cx.hook_runner();

        // Step 1: If merge enabled, run the merge pipeline (pure git + headless
        // hooks — see `close_worktree_merge_git`; the daemon runs it off-reactor).
        let did_stash = if merge_enabled {
            match close_worktree_merge_git(
                stash_enabled,
                fetch_enabled,
                push_enabled,
                delete_branch_enabled,
                project_id,
                &project_name,
                &project_path,
                &branch,
                &default_branch,
                &main_repo_path,
                &project_hooks,
                global_hooks,
                folder_id.as_deref(),
                folder_name.as_deref(),
                monitor.as_ref(),
                runner.as_ref(),
            ) {
                CloseWorktreeGitOutcome::Ok { did_stash } => did_stash,
                CloseWorktreeGitOutcome::RebaseConflict { error, terminal_actions, hook_results } => {
                    for (cmd, env) in terminal_actions {
                        self.add_terminal_with_command(project_id, &cmd, &env, cx);
                    }
                    self.register_hook_results(hook_results, cx);
                    return Err(error);
                }
                CloseWorktreeGitOutcome::Err(e) => return Err(e),
            }
        } else {
            false
        };

        let force_remove = is_dirty && !did_stash;

        // Step 2: before_worktree_remove hook
        // If the hook exists and we have a runner, fire it as a visible PTY terminal
        // and register a pending close — the actual removal happens when the hook exits.
        // If no hook or no runner, proceed with immediate removal.
        let has_before_remove_hook =
            project_hooks.worktree.before_remove.is_some() || global_hooks.worktree.before_remove.is_some();

        if has_before_remove_hook && runner.is_some() {
            // Fire hook as visible PTY terminal and defer removal
            let hook_results = hooks::fire_before_worktree_remove_async(
                &project_hooks,
                global_hooks,
                project_id,
                &project_name,
                &project_path,
                &branch,
                &main_repo_path,
                folder_id.as_deref(),
                folder_name.as_deref(),
                monitor.as_ref(),
                runner.as_ref(),
            );

            let pending_terminal_id = hook_results.first().map(|r| r.terminal_id.clone());

            if let Some(hook_terminal_id) = pending_terminal_id {
                self.register_hook_results(hook_results, cx);

                // Register pending close — PTY exit handler will complete it
                self.register_pending_worktree_close(PendingWorktreeClose {
                    project_id: project_id.to_string(),
                    hook_terminal_id,
                    branch: branch.clone(),
                    main_repo_path: main_repo_path.clone(),
                });
                Ok(())
            } else {
                // Hook terminal failed to spawn — abort, don't remove
                Err("before_worktree_remove hook failed to start".to_string())
            }
        } else {
            // No hook or no runner — run headlessly then remove immediately
            if has_before_remove_hook
                && let Err(e) = hooks::fire_before_worktree_remove(
                    &project_hooks,
                    global_hooks,
                    project_id,
                    &project_name,
                    &project_path,
                    &branch,
                    &main_repo_path,
                    folder_id.as_deref(),
                    folder_name.as_deref(),
                    monitor.as_ref(),
                    None,
                ) {
                return Err(format!("before_worktree_remove hook failed: {}", e));
            }

            // Fire on_dirty_worktree_close hook when closing dirty worktree without stash
            if force_remove {
                let (terminal_actions, hook_results) = hooks::fire_on_dirty_worktree_close(
                    &project_hooks,
                    global_hooks,
                    project_id,
                    &project_name,
                    &project_path,
                    &branch,
                    folder_id.as_deref(),
                    folder_name.as_deref(),
                    monitor.as_ref(),
                    runner.as_ref(),
                );
                for (cmd, env) in terminal_actions {
                    self.add_terminal_with_command(project_id, &cmd, &env, cx);
                }
                self.register_hook_results(hook_results, cx);
            }

            // remove_worktree_project fires on_worktree_close + removes the git
            // worktree + deletes the project (which fires on_project_close).
            self.remove_worktree_project(focus_manager, project_id, force_remove, global_hooks, cx)
        }
    }
}
