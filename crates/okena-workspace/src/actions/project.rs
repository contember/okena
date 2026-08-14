//! Project management workspace actions
//!
//! Actions for creating, modifying, and deleting projects.

use crate::context::WorkspaceCx;
use crate::focus::FocusManager;
use crate::hooks;
use crate::persistence::HooksConfig;
use crate::state::{LayoutNode, ProjectData, WindowId, Workspace};
use okena_core::theme::FolderColor;
use std::collections::{HashMap, HashSet};

/// A fresh, unparented project row — the one place the full `ProjectData`
/// shape is spelled out for newly created projects.
fn new_project_row(
    id: String,
    name: String,
    path: String,
    layout: Option<LayoutNode>,
    default_shell: Option<okena_terminal::shell_config::ShellType>,
) -> ProjectData {
    ProjectData {
        id,
        name,
        path,
        layout,
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
        pinned: false,
        last_activity_at: None,
        is_creating: false,
        is_closing: false,
    }
}

#[derive(Clone)]
pub struct ProjectDirectoryRenamePlan {
    project_id: String,
    old_path: std::path::PathBuf,
    new_name: String,
    translated_paths: Vec<ProjectPathTranslation>,
    move_kind: ProjectDirectoryMove,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectPathTranslation {
    project_id: String,
    old_path: String,
    new_path: String,
    translated_hook_terminal_ids: Vec<String>,
}

impl ProjectPathTranslation {
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn old_path(&self) -> &str {
        &self.old_path
    }

    pub fn new_path(&self) -> &str {
        &self.new_path
    }
}

#[derive(Clone)]
enum ProjectDirectoryMove {
    Directory {
        old_path: std::path::PathBuf,
        new_path: std::path::PathBuf,
    },
    Worktree {
        verified: okena_git::VerifiedWorktree,
        new_path: std::path::PathBuf,
    },
}

pub struct ProjectDirectoryRenameResult {
    moved_worktree_root: Option<String>,
}

impl ProjectDirectoryRenamePlan {
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn old_path(&self) -> &std::path::Path {
        &self.old_path
    }

    pub fn new_path(&self) -> &std::path::Path {
        match &self.move_kind {
            ProjectDirectoryMove::Directory { new_path, .. }
            | ProjectDirectoryMove::Worktree { new_path, .. } => new_path,
        }
    }

    pub fn affected_translations(&self) -> &[ProjectPathTranslation] {
        &self.translated_paths
    }

    pub fn affected_project_ids(&self) -> impl Iterator<Item = &str> {
        self.translated_paths
            .iter()
            .map(|translation| translation.project_id.as_str())
    }

    pub fn execute(&self) -> Result<ProjectDirectoryRenameResult, String> {
        match &self.move_kind {
            ProjectDirectoryMove::Directory { old_path, new_path } => {
                std::fs::rename(old_path, new_path)
                    .map_err(|error| format!("Failed to rename: {error}"))?;
                Ok(ProjectDirectoryRenameResult {
                    moved_worktree_root: None,
                })
            }
            ProjectDirectoryMove::Worktree { verified, new_path } => {
                let moved = okena_git::move_worktree(verified, new_path)
                    .map_err(|error| error.to_string())?;
                Ok(ProjectDirectoryRenameResult {
                    moved_worktree_root: Some(moved.checkout_path().to_string_lossy().into_owned()),
                })
            }
        }
    }
}

/// Pick a replacement focus target after hiding `hidden_id`.
///
/// Walks `visible_before` starting from the hidden project's position to find
/// the closest project that is still visible — preferring the next sibling,
/// then falling back to the previous one.
fn pick_focus_replacement(
    visible_before: &[String],
    visible_after: &[String],
    hidden_id: &str,
) -> Option<String> {
    let idx = visible_before.iter().position(|id| id == hidden_id)?;
    let after_set: std::collections::HashSet<&str> =
        visible_after.iter().map(|s| s.as_str()).collect();
    visible_before
        .iter()
        .skip(idx + 1)
        .find(|id| after_set.contains(id.as_str()))
        .or_else(|| {
            visible_before
                .iter()
                .take(idx)
                .rev()
                .find(|id| after_set.contains(id.as_str()))
        })
        .cloned()
}

/// Expand `~` or `~/...` at the start of a path to the user's home directory.
/// Does not expand `~user/...` syntax (other user's home directories).
fn expand_tilde(path: &str) -> String {
    if (path == "~" || path.starts_with("~/"))
        && let Some(home) = dirs::home_dir()
    {
        let rest = &path[1..]; // "" or "/..."
        return format!("{}{}", home.display(), rest);
    }
    path.to_string()
}

/// Resolve a clone request's `parent_dir` + `directory` into one absolute path.
///
/// `directory` is a NAME, not a path: separators and `..` are rejected so the
/// checkout cannot land outside the parent directory the user actually picked.
/// Runs on the host that will do the cloning, so `~` and the path separator are
/// resolved with that host's conventions — not the calling client's.
pub fn resolve_clone_target(
    parent_dir: &str,
    directory: &str,
) -> Result<std::path::PathBuf, String> {
    let parent = expand_tilde(parent_dir.trim());
    if parent.is_empty() {
        return Err("Parent directory is required".to_string());
    }
    let directory = directory.trim();
    if directory.is_empty() {
        return Err("Directory name is required".to_string());
    }
    let mut components = std::path::Path::new(directory).components();
    let is_plain_name = matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none();
    if directory.contains(['/', '\\']) || !is_plain_name {
        return Err(format!(
            "'{directory}' is not a valid directory name — it must be a single folder name"
        ));
    }
    Ok(std::path::PathBuf::from(parent).join(directory))
}

/// Display name for a cloned project: the caller's, or the directory name when
/// the caller left it blank (`okena project clone` without `--name`).
pub fn clone_project_name(name: &str, directory: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        directory.trim().to_string()
    } else {
        name.to_string()
    }
}

impl Workspace {
    /// Returns whether a project is hidden in the given window.
    ///
    /// Reads from the targeted `WindowState.hidden_project_ids`. Falls back to
    /// `main_window` if the targeted extra has been dropped between caller
    /// resolution and read (drop-race safety). Missing entry == visible.
    pub fn is_project_hidden(&self, window_id: WindowId, project_id: &str) -> bool {
        let window_state = self
            .data
            .window(window_id)
            .unwrap_or(&self.data.main_window);
        window_state.hidden_project_ids.contains(project_id)
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
    pub fn toggle_project_overview_visibility(
        &mut self,
        focus_manager: &mut FocusManager,
        window_id: WindowId,
        project_id: &str,
        cx: &mut impl WorkspaceCx,
    ) {
        if self.project(project_id).is_none() {
            return;
        }
        let was_hidden = self.is_project_hidden(window_id, project_id);

        // When hiding the project that owns the currently focused terminal,
        // capture the ordered visible list so we can pick a neighbor to focus
        // after the toggle. Otherwise keyboard shortcuts stop working because
        // focus points at a column that's no longer rendered.
        let needs_focus_redirect = !was_hidden
            && focus_manager
                .focused_terminal_state()
                .map(|s| s.project_id)
                .as_deref()
                == Some(project_id);
        let visible_before: Vec<String> = if needs_focus_redirect {
            self.visible_projects(
                window_id,
                focus_manager.focused_project_id(),
                focus_manager.is_focus_individual(),
            )
            .iter()
            .map(|p| p.id.clone())
            .collect()
        } else {
            Vec::new()
        };

        self.toggle_hidden(window_id, project_id, cx);

        if needs_focus_redirect {
            let visible_after: Vec<String> = self
                .visible_projects(
                    window_id,
                    focus_manager.focused_project_id(),
                    focus_manager.is_focus_individual(),
                )
                .iter()
                .map(|p| p.id.clone())
                .collect();
            let replacement = pick_focus_replacement(&visible_before, &visible_after, project_id);
            if focus_manager.is_modal() {
                // The toggle is driven from an open modal (the project
                // switcher): the modal — not a terminal — holds keyboard
                // focus. Refocusing a terminal now via focus_first_terminal_in
                // /clear_focus would drop the Modal context, letting terminal
                // panes steal keyboard focus from the switcher mid-navigation
                // (the user then has to click to keep moving). Instead, rewrite
                // the focus the modal restores when it closes.
                match replacement {
                    Some(next_id) => {
                        if let Some(target) = self.first_terminal_target_in(&next_id) {
                            focus_manager.redirect_modal_focus(Some(target));
                        }
                        // Replacement project has no terminal: leave the saved
                        // focus as-is (matches focus_first_terminal_in's no-op).
                    }
                    None => focus_manager.redirect_modal_focus(None),
                }
            } else {
                match replacement {
                    Some(next_id) => self.focus_first_terminal_in(focus_manager, &next_id),
                    None => focus_manager.clear_focus(),
                }
            }
            cx.notify();
        }
    }

    /// Add a new project
    /// If `with_terminal` is false, creates a bookmark project without a terminal layout.
    ///
    /// `window_id` identifies the spawning window (PRD user story 14:
    /// project lands visible there, hidden everywhere else by default).
    /// After pushing the project onto `data.projects`, the new id is
    /// inserted into every window's `hidden_project_ids` set EXCEPT the
    /// spawning window's via `data.add_project_hide_in_other_windows`. UI
    /// callers pass the originating `WindowView`'s `window_id`; remote-
    /// bridge callers pass the focused window resolved via
    /// `Okena::focus_manager_for_active_window` (slice 05 cri 13). When
    /// only main exists (zero extras), the rule degenerates to a no-op
    /// for the hide-elsewhere step, matching pre-multi-window behavior.
    pub fn add_project(
        &mut self,
        name: String,
        path: String,
        with_terminal: bool,
        global_hooks: &HooksConfig,
        window_id: WindowId,
        cx: &mut impl WorkspaceCx,
    ) -> Result<String, String> {
        let path = expand_tilde(&path);
        self.ensure_project_path_claim_allowed(std::path::Path::new(&path))?;

        // Auto-detect WSL UNC paths and set default shell accordingly
        #[cfg(windows)]
        let default_shell =
            okena_terminal::shell_config::parse_wsl_unc_path(&path).map(|(distro, _)| {
                okena_terminal::shell_config::ShellType::Wsl {
                    distro: Some(distro),
                }
            });
        #[cfg(not(windows))]
        let default_shell: Option<okena_terminal::shell_config::ShellType> = None;

        let id = uuid::Uuid::new_v4().to_string();
        let layout = with_terminal.then(LayoutNode::new_terminal);
        self.data
            .projects
            .push(new_project_row(id.clone(), name, path, layout, default_shell));
        self.data.project_order.push(id.clone());
        self.data.add_project_hide_in_other_windows(&id, window_id);
        self.notify_data(cx);

        self.fire_project_open_hooks(&id, global_hooks, cx);
        Ok(id)
    }

    /// Register a project row whose directory does NOT exist on disk yet.
    ///
    /// The clone counterpart of `register_worktree_project_deferred_hooks`:
    /// the row appears immediately (so the user sees the project while the
    /// clone runs) but gets no layout and fires no hooks, because both would
    /// cd into a directory that is not there. The caller marks it creating,
    /// runs the checkout, then calls `finish_pending_project` — or
    /// `remove_pending_project` when the checkout fails.
    pub fn register_pending_project(
        &mut self,
        name: String,
        path: String,
        window_id: WindowId,
        cx: &mut impl WorkspaceCx,
    ) -> Result<String, String> {
        let path = expand_tilde(&path);
        self.ensure_project_path_claim_allowed(std::path::Path::new(&path))?;

        let id = uuid::Uuid::new_v4().to_string();
        // No layout: `column_content` renders the creating placeholder, and
        // `spawn_uninitialized_terminals` has nothing to spawn into yet. The
        // shell is detected in `finish_pending_project`, once the path is real.
        self.data
            .projects
            .push(new_project_row(id.clone(), name, path, None, None));
        self.data.project_order.push(id.clone());
        self.data.add_project_hide_in_other_windows(&id, window_id);
        self.notify_data(cx);
        Ok(id)
    }

    /// Materialize a pending project once its directory exists: seed the
    /// terminal layout and fire the deferred `on_project_open` hooks.
    ///
    /// Leaves an existing layout alone — a restored session may already carry
    /// one, and re-seeding would drop its terminals.
    pub fn finish_pending_project(
        &mut self,
        project_id: &str,
        global_hooks: &HooksConfig,
        cx: &mut impl WorkspaceCx,
    ) {
        let Some(project) = self.data.projects.iter_mut().find(|p| p.id == project_id) else {
            return;
        };
        if project.layout.is_none() {
            project.layout = Some(LayoutNode::new_terminal());
        }
        // Detect the WSL shell now that the path is real (`add_project` does
        // this up-front from the path string; the check is the same either way).
        #[cfg(windows)]
        {
            if project.default_shell.is_none() {
                project.default_shell =
                    okena_terminal::shell_config::parse_wsl_unc_path(&project.path).map(
                        |(distro, _)| okena_terminal::shell_config::ShellType::Wsl {
                            distro: Some(distro),
                        },
                    );
            }
        }
        self.notify_data(cx);
        self.fire_project_open_hooks(project_id, global_hooks, cx);
    }

    /// Roll back a pending project row whose creation never completed.
    ///
    /// Mirrors `remove_stale_worktree` for plain (non-worktree) projects and
    /// carries the same guard: a row still marked creating or closing belongs
    /// to an in-flight operation and is left alone. Caller calls `notify_data`.
    pub fn remove_pending_project(&mut self, project_id: &str) {
        if self.lifecycle.is_closing(project_id) || self.lifecycle.is_creating(project_id) {
            return;
        }
        // Worktree rows roll back through `remove_stale_worktree`, which also
        // scrubs the parent's `worktree_ids`.
        let is_plain_project = self
            .data
            .projects
            .iter()
            .any(|p| p.id == project_id && p.worktree_info.is_none());
        if !is_plain_project {
            return;
        }

        self.data.projects.retain(|p| p.id != project_id);
        self.data.project_order.retain(|id| id != project_id);
        for folder in &mut self.data.folders {
            folder.project_ids.retain(|id| id != project_id);
        }
        self.data.delete_project_scrub_all_windows(project_id);
    }


    /// Remove hook terminal state restored without a matching live PTY.
    ///
    /// Returns the stale terminal ids so the caller can also tear down a
    /// persistent session backend before the ids become unreachable.
    pub fn clear_stale_hook_terminals(
        &mut self,
        project_id: &str,
        cx: &mut impl WorkspaceCx,
    ) -> Vec<String> {
        let Some(project) = self.project_mut(project_id) else {
            return Vec::new();
        };
        let stale: Vec<String> = project.hook_terminals.keys().cloned().collect();
        if stale.is_empty() {
            return stale;
        }

        let stale_set: HashSet<&str> = stale.iter().map(String::as_str).collect();
        LayoutNode::remove_terminal_ids(&mut project.layout, &stale_set);
        project.hook_terminals.clear();
        project
            .terminal_names
            .retain(|terminal_id, _| !stale_set.contains(terminal_id.as_str()));
        self.notify_data(cx);
        stale
    }

    /// Re-open an ALREADY-EXISTING project (e.g. one restored from
    /// `workspace.json` at daemon boot): drop its stale hook terminals and fire
    /// its `on_project_open` hook, reading the project's stored hooks/name/path.
    ///
    /// `add_project` runs the fire+register step for NEW projects, but restored
    /// projects enter the workspace via `Workspace::new` (never `add_project`),
    /// so without this their `project.on_open` hook — global or per-project —
    /// would never run on restart. The stale-clear matters because
    /// `hook_terminals` is persisted: the entries reloaded from disk point at
    /// PTYs that died with the previous process, so they must be dropped both to
    /// avoid phantom rows (whose rerun/dismiss fail with "hook terminal not
    /// found") and to stop entries accumulating on every restart. No-ops the
    /// fire when no `on_open` hook resolves.
    pub fn fire_project_open_hooks(
        &mut self,
        project_id: &str,
        global_hooks: &HooksConfig,
        cx: &mut impl WorkspaceCx,
    ) {
        let Some(project) = self.project(project_id) else {
            return;
        };
        let project_hooks = project.hooks.clone();
        let name = project.name.clone();
        let path = project.path.clone();
        self.clear_stale_hook_terminals(project_id, cx);
        // Immutable `project` borrow ends here (values cloned); the folder
        // borrow below ends at the fire call, freeing `&mut self` for the
        // mutations that follow.
        let folder = self.folder_for_project_or_parent(project_id);
        let folder_id = folder.map(|f| f.id.as_str());
        let folder_name = folder.map(|f| f.name.as_str());
        let runner = cx.hook_runner();
        let monitor = cx.hook_monitor();
        let hook_results = hooks::fire_on_project_open(
            &project_hooks,
            project_id,
            &name,
            &path,
            folder_id,
            folder_name,
            global_hooks,
            runner.as_ref(),
            monitor.as_ref(),
        );
        self.register_hook_results(hook_results, cx);
    }

    /// Add a new terminal to a project by splitting the root layout
    pub fn add_terminal(
        &mut self,
        focus_manager: &mut FocusManager,
        project_id: &str,
        cx: &mut impl WorkspaceCx,
    ) {
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
        let new_path = self
            .project(project_id)
            .and_then(|p| p.layout.as_ref())
            .and_then(|l| l.find_uninitialized_terminal_path());
        if let Some(path) = new_path {
            self.set_focused_terminal(focus_manager, project_id.to_string(), path, cx);
        }
    }

    /// Add a new terminal running a specific command to a project
    pub fn add_terminal_with_command(
        &mut self,
        project_id: &str,
        command: &str,
        env_vars: &HashMap<String, String>,
        cx: &mut impl WorkspaceCx,
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
    pub fn rename_project(
        &mut self,
        project_id: &str,
        new_name: String,
        cx: &mut impl WorkspaceCx,
    ) {
        self.with_project(project_id, cx, |project| {
            project.name = new_name;
            true
        });
    }

    /// Rename a project's directory path and update the project name to match
    pub fn rename_project_directory(
        &mut self,
        project_id: &str,
        new_path: String,
        new_name: String,
        cx: &mut impl WorkspaceCx,
    ) -> Result<(), String> {
        let plan = self.prepare_project_directory_rename(project_id, new_path, new_name)?;
        let result = plan.execute()?;
        self.finish_project_directory_rename(&plan, result, cx)
    }

    /// Validate and snapshot a directory rename without changing the filesystem.
    pub fn prepare_project_directory_rename(
        &self,
        project_id: &str,
        new_path: String,
        new_name: String,
    ) -> Result<ProjectDirectoryRenamePlan, String> {
        if new_name.is_empty() {
            return Err("name must not be empty".to_string());
        }
        if new_name.contains('/') || new_name.contains('\\') || new_name == "." || new_name == ".."
        {
            return Err("name must not contain path separators".to_string());
        }
        let new_path_buf = std::path::PathBuf::from(&new_path);
        self.ensure_project_path_mutation_allowed(project_id, &new_path_buf)?;
        if new_path_buf.exists() {
            return Err(format!("'{}' already exists", new_name));
        }
        let project = self
            .project(project_id)
            .ok_or_else(|| "Project not found".to_string())?;
        let old_path = std::path::PathBuf::from(&project.path);
        let worktree = project.worktree_info.as_ref().map(|metadata| {
            (
                metadata.parent_project_id.clone(),
                metadata.worktree_path.clone(),
            )
        });

        let Some((parent_project_id, recorded_root)) = worktree else {
            self.ensure_directory_tree_git_topology_safe(&old_path, None)?;
            let translated_paths = self.translate_local_project_paths(&old_path, &new_path_buf)?;
            return Ok(ProjectDirectoryRenamePlan {
                project_id: project_id.to_string(),
                old_path: old_path.clone(),
                new_name,
                translated_paths,
                move_kind: ProjectDirectoryMove::Directory {
                    old_path,
                    new_path: new_path_buf,
                },
            });
        };

        let parent_path = self
            .project(&parent_project_id)
            .filter(|parent| !parent.is_remote)
            .map(|parent| parent.path.clone())
            .ok_or_else(|| "Worktree parent project is not local".to_string())?;
        let checkout_query = if recorded_root.is_empty() {
            old_path.clone()
        } else {
            std::path::PathBuf::from(&recorded_root)
        };
        let verified = okena_git::verify_linked_worktree_fresh(
            std::path::Path::new(&parent_path),
            &checkout_query,
        )
        .map_err(|error| error.to_string())?;
        let root_identity = Self::physical_path_identity(verified.checkout_path());
        let old_identity = Self::physical_path_identity(&old_path);
        if !old_identity.starts_with(&root_identity) {
            return Err("project path is outside its linked worktree root".to_string());
        }

        if old_identity != root_identity {
            if !Self::physical_path_identity(&new_path_buf).starts_with(&root_identity) {
                return Err("renamed project path must stay inside its linked worktree".to_string());
            }
            self.ensure_directory_tree_git_topology_safe(&old_path, None)?;
            let translated_paths = self.translate_local_project_paths(&old_path, &new_path_buf)?;
            return Ok(ProjectDirectoryRenamePlan {
                project_id: project_id.to_string(),
                old_path: old_path.clone(),
                new_name,
                translated_paths,
                move_kind: ProjectDirectoryMove::Directory {
                    old_path,
                    new_path: new_path_buf,
                },
            });
        }

        self.ensure_directory_tree_git_topology_safe(
            verified.checkout_path(),
            Some(verified.checkout_path()),
        )?;
        let translated_paths =
            self.translate_local_project_paths(verified.checkout_path(), &new_path_buf)?;
        Ok(ProjectDirectoryRenamePlan {
            project_id: project_id.to_string(),
            old_path,
            new_name,
            translated_paths,
            move_kind: ProjectDirectoryMove::Worktree {
                verified,
                new_path: new_path_buf,
            },
        })
    }

    /// Publish the state translation after the off-reactor move succeeds.
    pub fn finish_project_directory_rename(
        &mut self,
        plan: &ProjectDirectoryRenamePlan,
        result: ProjectDirectoryRenameResult,
        cx: &mut impl WorkspaceCx,
    ) -> Result<(), String> {
        if self
            .project(&plan.project_id)
            .is_none_or(|project| std::path::Path::new(&project.path) != plan.old_path)
        {
            return Err(format!(
                "project changed while its directory was being renamed: {}",
                plan.project_id
            ));
        }
        self.apply_translated_project_paths(plan.translated_paths.clone());
        if let Some(project) = self
            .data
            .projects
            .iter_mut()
            .find(|project| project.id == plan.project_id)
        {
            project.name = plan.new_name.clone();
            if let Some(moved_root) = result.moved_worktree_root {
                project.path = moved_root.clone();
                if let Some(metadata) = &mut project.worktree_info {
                    metadata.worktree_path = moved_root;
                }
            }
        }
        self.notify_data(cx);
        Ok(())
    }

    fn translate_local_project_paths(
        &self,
        old_root: &std::path::Path,
        new_root: &std::path::Path,
    ) -> Result<Vec<ProjectPathTranslation>, String> {
        let old_identity = Self::physical_path_identity(old_root);
        let canonical_root = std::fs::canonicalize(old_root)
            .map_err(|error| format!("Failed to resolve project directory: {error}"))?;
        let mut translated_paths = Vec::new();
        for descendant in self.projects().iter().filter(|project| !project.is_remote) {
            let descendant_path = std::path::Path::new(&descendant.path);
            if !Self::physical_path_identity(descendant_path).starts_with(&old_identity) {
                continue;
            }
            let canonical_descendant = std::fs::canonicalize(descendant_path).map_err(|error| {
                format!(
                    "Failed to resolve descendant project '{}': {error}",
                    descendant.name
                )
            })?;
            let suffix = canonical_descendant
                .strip_prefix(&canonical_root)
                .map_err(|_| {
                    format!(
                        "Failed to translate descendant project '{}' into moved directory",
                        descendant.name
                    )
                })?;
            let translated_path = if suffix.as_os_str().is_empty() {
                new_root.to_path_buf()
            } else {
                new_root.join(suffix)
            };
            let translated_hook_terminal_ids = descendant
                .hook_terminals
                .iter()
                .filter(|(_, entry)| {
                    Self::physical_path_identity(std::path::Path::new(&entry.cwd))
                        == Self::physical_path_identity(descendant_path)
                })
                .map(|(terminal_id, _)| terminal_id.clone())
                .collect();
            translated_paths.push(ProjectPathTranslation {
                project_id: descendant.id.clone(),
                old_path: descendant.path.clone(),
                new_path: translated_path.to_string_lossy().into_owned(),
                translated_hook_terminal_ids,
            });
        }
        Ok(translated_paths)
    }

    fn apply_translated_project_paths(&mut self, translated_paths: Vec<ProjectPathTranslation>) {
        for translation in translated_paths {
            if let Some(descendant) = self
                .data
                .projects
                .iter_mut()
                .find(|project| project.id == translation.project_id)
            {
                descendant.path = translation.new_path.clone();
                for terminal_id in &translation.translated_hook_terminal_ids {
                    if let Some(entry) = descendant.hook_terminals.get_mut(terminal_id) {
                        entry.cwd = translation.new_path.clone();
                    }
                }
            }
        }
    }

    /// Reject directory moves that would invalidate Git's absolute worktree links.
    fn ensure_directory_tree_git_topology_safe(
        &self,
        old_root: &std::path::Path,
        allowed_worktree_root: Option<&std::path::Path>,
    ) -> Result<(), String> {
        let old_identity = Self::physical_path_identity(old_root);
        let allowed_identity = allowed_worktree_root.map(Self::physical_path_identity);
        let mut repo_roots: Vec<std::path::PathBuf> = Vec::new();

        for project in self.projects().iter().filter(|project| !project.is_remote) {
            let project_path = std::path::Path::new(&project.path);
            let project_identity = Self::physical_path_identity(project_path);
            if project_identity.starts_with(&old_identity)
                && let Some(repo_root) = okena_git::get_repo_root(project_path)
            {
                let repo_identity = Self::physical_path_identity(&repo_root);
                if repo_identity.starts_with(&old_identity) {
                    if !repo_roots
                        .iter()
                        .any(|known| Self::physical_path_identity(known) == repo_identity)
                    {
                        repo_roots.push(repo_root);
                    }
                    if project.worktree_info.is_some()
                        && allowed_identity.as_ref() != Some(&repo_identity)
                    {
                        return Err(format!(
                            "Cannot rename a directory containing linked worktree project '{}'",
                            project.name
                        ));
                    }
                }
            }

            if let Some(metadata) = &project.worktree_info
                && !metadata.worktree_path.is_empty()
            {
                let recorded_identity =
                    Self::physical_path_identity(std::path::Path::new(&metadata.worktree_path));
                if recorded_identity.starts_with(&old_identity)
                    && allowed_identity.as_ref() != Some(&recorded_identity)
                {
                    return Err(format!(
                        "Cannot rename a directory containing recorded worktree root for '{}'",
                        project.name
                    ));
                }
            }
        }

        for repo_root in repo_roots {
            let repo_identity = Self::physical_path_identity(&repo_root);
            if allowed_identity.as_ref() == Some(&repo_identity) {
                continue;
            }
            if !okena_git::list_linked_worktree_paths(&repo_root).is_empty() {
                return Err(
                    "Cannot rename a Git repository while it has linked worktrees; remove them first"
                        .to_string(),
                );
            }
        }
        Ok(())
    }

    /// Set the folder color for a project (also propagates to worktree children without overrides)
    pub fn set_folder_color(
        &mut self,
        project_id: &str,
        color: FolderColor,
        cx: &mut impl WorkspaceCx,
    ) {
        let is_worktree = self
            .project(project_id)
            .and_then(|p| p.worktree_info.as_ref())
            .is_some();

        if is_worktree {
            self.set_worktree_color_override(project_id, Some(color), cx);
        } else {
            // Collect child IDs from the parent's worktree_ids to avoid a full scan
            let child_ids: Vec<String> = self
                .project(project_id)
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
                    let has_override = child
                        .worktree_info
                        .as_ref()
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

    /// Delete a project
    pub fn delete_project(
        &mut self,
        focus_manager: &mut FocusManager,
        project_id: &str,
        global_hooks: &HooksConfig,
        cx: &mut impl WorkspaceCx,
    ) {
        self.delete_project_inner(focus_manager, project_id, Some(global_hooks), cx);
    }

    /// Delete a worktree whose project-close hook already completed before its
    /// checkout was removed.
    pub(crate) fn delete_project_without_project_close(
        &mut self,
        focus_manager: &mut FocusManager,
        project_id: &str,
        cx: &mut impl WorkspaceCx,
    ) {
        self.delete_project_inner(focus_manager, project_id, None, cx);
    }

    fn delete_project_inner(
        &mut self,
        focus_manager: &mut FocusManager,
        project_id: &str,
        global_hooks: Option<&HooksConfig>,
        cx: &mut impl WorkspaceCx,
    ) {
        // Queue all project terminals for killing before removing state.
        // Okena (which owns PtyManager) drains this queue via observer.
        if let Some(project) = self.project(project_id) {
            let hook_terminal_ids: Vec<String> = project.hook_terminals.keys().cloned().collect();
            if let Some(monitor) = cx.hook_monitor() {
                for terminal_id in &hook_terminal_ids {
                    monitor.cancel_by_terminal_id(terminal_id);
                }
            }
            let mut kill_ids: Vec<String> = Vec::new();
            if let Some(layout) = &project.layout {
                kill_ids.extend(layout.collect_terminal_ids());
            }
            kill_ids.extend(hook_terminal_ids);
            kill_ids.extend(project.service_terminals.values().cloned());
            self.queue_terminal_kills(kill_ids);
        }

        // Soft-closed terminals are no longer in the layout but their PTY is
        // still alive waiting out the grace window — kill them too and drop the
        // pending records so the grace timer can't fire against a deleted project.
        let soft_closed = self.drain_pending_closes_for_project(project_id);
        if !soft_closed.is_empty() {
            self.queue_terminal_kills(soft_closed);
        }

        // Capture project info before removal for the hook
        let folder = self.folder_for_project_or_parent(project_id);
        let hook_folder_id = folder.map(|f| f.id.clone());
        let hook_folder_name = folder.map(|f| f.name.clone());
        let hook_info = self.project(project_id).map(|p| {
            (
                p.hooks.clone(),
                p.id.clone(),
                p.name.clone(),
                p.path.clone(),
            )
        });

        // Collect orphaned worktree children (if deleting a parent)
        let orphaned_worktrees: Vec<String> = self
            .project(project_id)
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
            if self.data.projects.iter().any(|p| p.id == wt_id)
                && !self.data.project_order.contains(&wt_id)
            {
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
        if focus_manager.focused_project_id().map(|s| s.as_str()) == Some(project_id) {
            focus_manager.set_focused_project_id(None);
        }
        // Exit fullscreen if this project's terminal was in fullscreen
        if focus_manager.fullscreen_project_id() == Some(project_id) {
            focus_manager.exit_fullscreen();
        }
        self.notify_data(cx);

        if let (Some((project_hooks, id, name, path)), Some(global_hooks)) =
            (hook_info, global_hooks)
        {
            let monitor = cx.hook_monitor();
            hooks::fire_on_project_close(
                &project_hooks,
                &id,
                &name,
                &path,
                hook_folder_id.as_deref(),
                hook_folder_name.as_deref(),
                global_hooks,
                monitor.as_ref(),
            );
        }
    }

    /// Move a project to a new position in the top-level order.
    /// Also removes the project from any folder it may be in.
    /// Worktree children are moved along with their parent.
    pub fn move_project(&mut self, project_id: &str, new_index: usize, cx: &mut impl WorkspaceCx) {
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

    /// Update project column widths on the targeted window.
    ///
    /// Merges the supplied widths into the targeted window's `project_widths`
    /// map. Omitted entries may belong to hidden projects and are preserved.
    ///
    /// Each entry is written via `data.set_project_width(window_id, ...)` so
    /// future changes inherit the per-entry pair-shaped contract automatically.
    ///
    /// Bumps `data_version` exactly once per call (not per entry) -- the data
    /// layer setter does not notify, so the single trailing `notify_data` keeps
    /// the auto-save observer's debounce cadence identical to the pre-migration
    /// body.
    pub fn update_project_widths(
        &mut self,
        window_id: WindowId,
        widths: HashMap<String, f32>,
        cx: &mut impl WorkspaceCx,
    ) {
        for (id, w) in widths {
            self.data.set_project_width(window_id, &id, w);
        }
        self.notify_data(cx);
    }

    /// Update project sizes and the pixel scale used to render them.
    pub fn update_project_widths_with_scale(
        &mut self,
        window_id: WindowId,
        widths: HashMap<String, f32>,
        scale: f32,
        cx: &mut impl WorkspaceCx,
    ) {
        for (id, width) in widths {
            self.data.set_project_width(window_id, &id, width);
        }
        self.data.set_project_width_scale(window_id, scale);
        self.notify_data(cx);
    }

    /// Update service panel height for a project
    pub fn update_service_panel_height(
        &mut self,
        project_id: &str,
        height: f32,
        cx: &mut impl WorkspaceCx,
    ) {
        self.data
            .service_panel_heights
            .insert(project_id.to_string(), height);
        self.notify_data(cx);
    }

    /// Update hook panel height for a project
    pub fn update_hook_panel_height(
        &mut self,
        project_id: &str,
        height: f32,
        cx: &mut impl WorkspaceCx,
    ) {
        self.data
            .hook_panel_heights
            .insert(project_id.to_string(), height);
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
    pub fn get_project_width(
        &self,
        window_id: WindowId,
        project_id: &str,
        visible_count: usize,
    ) -> f32 {
        self.data
            .window(window_id)
            .and_then(|w| w.project_widths.get(project_id).copied())
            .unwrap_or_else(|| 100.0 / visible_count as f32)
    }

    /// Return the persisted pixel scale for project-size weights.
    pub fn get_project_width_scale(&self, window_id: WindowId) -> Option<f32> {
        self.data
            .window(window_id)
            .and_then(|window| window.project_width_scale)
            .filter(|scale| scale.is_finite() && *scale > 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{expand_tilde, pick_focus_replacement};
    use crate::context::WorkspaceCx;
    use crate::settings::HooksConfig;
    use crate::state::*;
    use okena_core::theme::FolderColor;
    use okena_hooks::{HookMonitor, HookRunner};
    use std::collections::HashMap;

    struct TestCx;

    impl WorkspaceCx for TestCx {
        fn notify(&mut self) {}
        fn refresh_views(&mut self) {}
        fn hook_runner(&self) -> Option<HookRunner> {
            None
        }
        fn hook_monitor(&self) -> Option<HookMonitor> {
            None
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
            pinned: false,
            last_activity_at: None,
            is_creating: false,
            is_closing: false,
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
        data.main_window
            .project_widths
            .insert("p1".to_string(), 60.0);
        let ws = Workspace::new(data);
        assert_eq!(ws.get_project_width(WindowId::Main, "p1", 2), 60.0);
    }

    #[test]
    fn get_project_width_reads_from_main_window_project_widths() {
        // Per-window viewport model: WindowId::Main routes through
        // data.window(...) and reads main_window.project_widths.
        let mut data = make_workspace_data();
        data.main_window
            .project_widths
            .insert("p1".to_string(), 75.0);
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

        assert_eq!(
            ws.get_project_width(WindowId::Extra(extra_id), "p1", 2),
            80.0
        );
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
        data.main_window
            .project_widths
            .insert("p1".to_string(), 90.0);
        let ws = Workspace::new(data);

        let unknown = uuid::Uuid::new_v4();
        // Default for visible_count = 4 -> 25.0, NOT 90.0 (main's value).
        assert_eq!(
            ws.get_project_width(WindowId::Extra(unknown), "p1", 4),
            25.0
        );
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

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn pick_focus_replacement_prefers_next() {
        let before = s(&["a", "b", "c", "d"]);
        let after = s(&["a", "b", "d"]);
        assert_eq!(
            pick_focus_replacement(&before, &after, "c").as_deref(),
            Some("d")
        );
    }

    #[test]
    fn pick_focus_replacement_falls_back_to_previous() {
        let before = s(&["a", "b", "c"]);
        let after = s(&["a", "b"]);
        assert_eq!(
            pick_focus_replacement(&before, &after, "c").as_deref(),
            Some("b")
        );
    }

    #[test]
    fn pick_focus_replacement_skips_other_hidden_neighbors() {
        // Hiding "b" while "c" is also no longer visible should jump to "d".
        let before = s(&["a", "b", "c", "d"]);
        let after = s(&["a", "d"]);
        assert_eq!(
            pick_focus_replacement(&before, &after, "b").as_deref(),
            Some("d")
        );
    }

    #[test]
    fn pick_focus_replacement_none_when_alone() {
        let before = s(&["a"]);
        let after: Vec<String> = Vec::new();
        assert_eq!(pick_focus_replacement(&before, &after, "a"), None);
    }

    #[test]
    fn pick_focus_replacement_none_when_id_missing() {
        let before = s(&["a", "b"]);
        let after = s(&["a", "b"]);
        assert_eq!(pick_focus_replacement(&before, &after, "missing"), None);
    }

    #[test]
    fn clear_stale_hook_terminals_clears_metadata_and_legacy_layout_id() {
        let mut project = make_project("p1");
        project.layout = Some(LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![50.0, 50.0],
            children: vec![
                LayoutNode::Terminal {
                    terminal_id: Some("layout-terminal".to_string()),
                    minimized: false,
                    detached: false,
                    shell_type: Default::default(),
                    zoom_level: 1.0,
                },
                LayoutNode::Terminal {
                    terminal_id: Some("stale-hook".to_string()),
                    minimized: true,
                    detached: true,
                    shell_type: Default::default(),
                    zoom_level: 1.0,
                },
            ],
        });
        project.hook_terminals.insert(
            "stale-hook".to_string(),
            HookTerminalEntry {
                label: "on_project_open".to_string(),
                status: HookTerminalStatus::Succeeded,
                hook_type: "on_project_open".to_string(),
                command: "echo old".to_string(),
                cwd: "/tmp".to_string(),
            },
        );
        project
            .terminal_names
            .insert("stale-hook".to_string(), "Old hook".to_string());

        let mut data = make_workspace_data();
        data.projects.push(project);
        data.project_order.push("p1".to_string());
        let mut workspace = Workspace::new(data);
        let stale = workspace.clear_stale_hook_terminals("p1", &mut TestCx);

        assert_eq!(stale, vec!["stale-hook".to_string()]);
        let project = workspace.project("p1").unwrap();
        assert!(project.hook_terminals.is_empty());
        assert!(!project.terminal_names.contains_key("stale-hook"));
        let layout = project.layout.as_ref().unwrap();
        assert!(layout.find_terminal_path("stale-hook").is_none());
        assert!(layout.find_terminal_path("layout-terminal").is_some());
        assert_eq!(layout.collect_terminal_ids(), vec!["layout-terminal"]);
        assert!(matches!(layout, LayoutNode::Terminal { .. }));
    }

    #[test]
    fn clear_stale_hook_terminals_removes_a_legacy_root_leaf() {
        let mut project = make_project("p1");
        project.layout = Some(LayoutNode::Terminal {
            terminal_id: Some("stale-hook".to_string()),
            minimized: false,
            detached: false,
            shell_type: Default::default(),
            zoom_level: 1.0,
        });
        project.hook_terminals.insert(
            "stale-hook".to_string(),
            HookTerminalEntry {
                label: "on_project_open".to_string(),
                status: HookTerminalStatus::Succeeded,
                hook_type: "on_project_open".to_string(),
                command: "echo old".to_string(),
                cwd: "/tmp".to_string(),
            },
        );

        let mut data = make_workspace_data();
        data.projects.push(project);
        data.project_order.push("p1".to_string());
        let mut workspace = Workspace::new(data);

        workspace.clear_stale_hook_terminals("p1", &mut TestCx);

        assert!(workspace.project("p1").unwrap().layout.is_none());
    }
}

#[cfg(all(test, feature = "gpui"))]
mod gpui_tests {
    use crate::focus::FocusManager;
    use crate::settings::HooksConfig;
    use crate::state::{LayoutNode, ProjectData, WindowId, WindowState, Workspace, WorkspaceData};
    use gpui::AppContext as _;
    use okena_core::theme::FolderColor;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::process::Command;

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
            pinned: false,
            last_activity_at: None,
            is_creating: false,
            is_closing: false,
        }
    }

    #[gpui::test]
    fn add_project_main_spawn_with_extra_hides_in_extra_only(cx: &mut gpui::TestAppContext) {
        // Slice 06 + PRD user story 14 entity-level pin: add_project from
        // WindowId::Main with one extra present produces a project that is
        // hidden in the extra and visible (absent from hidden_project_ids)
        // in main. Defends against a regression that drops the WindowId
        // parameter, calls the visibility helper with the wrong target, or
        // skips the helper entirely. Co-located with the data-layer pin
        // `add_project_hide_in_other_windows_main_spawn_inserts_in_extras_only`
        // so the entity layer's threading is verified end-to-end.
        let mut data = make_workspace_data();
        let extra = WindowState::default();
        let extra_id = extra.id;
        data.extra_windows = vec![extra];
        let workspace = cx.new(|_cx| Workspace::new(data));

        let new_id = workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.add_project(
                "p1".to_string(),
                "/tmp/p1".to_string(),
                false,
                &HooksConfig::default(),
                WindowId::Main,
                cx,
            )
            .expect("add project")
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(!ws.data().main_window.hidden_project_ids.contains(&new_id));
            let after = ws.data().window(WindowId::Extra(extra_id)).unwrap();
            assert!(after.hidden_project_ids.contains(&new_id));
        });
    }

    #[gpui::test]
    fn add_project_extra_spawn_hides_in_main_and_other_extras(cx: &mut gpui::TestAppContext) {
        // Slice 06 + PRD user story 14: add_project from
        // WindowId::Extra(spawning) with a second extra present hides the
        // new project in main and the sibling extra, leaves the spawning
        // extra clean. Defends against a regression that always writes to
        // main as the spawning window, or scatters the hide across every
        // extra (including the spawning one). Mirrors the data-layer pin
        // `add_project_hide_in_other_windows_extra_spawn_inserts_in_main_and_other_extras`.
        let mut data = make_workspace_data();
        let extra_a = WindowState::default();
        let extra_a_id = extra_a.id;
        let extra_b = WindowState::default();
        let extra_b_id = extra_b.id;
        data.extra_windows = vec![extra_a, extra_b];
        let workspace = cx.new(|_cx| Workspace::new(data));

        let new_id = workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.add_project(
                "p1".to_string(),
                "/tmp/p1".to_string(),
                false,
                &HooksConfig::default(),
                WindowId::Extra(extra_a_id),
                cx,
            )
            .expect("add project")
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.data().main_window.hidden_project_ids.contains(&new_id));
            let after_a = ws.data().window(WindowId::Extra(extra_a_id)).unwrap();
            assert!(!after_a.hidden_project_ids.contains(&new_id));
            let after_b = ws.data().window(WindowId::Extra(extra_b_id)).unwrap();
            assert!(after_b.hidden_project_ids.contains(&new_id));
        });
    }

    #[gpui::test]
    fn test_add_project_gpui(cx: &mut gpui::TestAppContext) {
        let workspace = cx.new(|_cx| Workspace::new(make_workspace_data()));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.add_project(
                "Test".to_string(),
                "/tmp/test".to_string(),
                true,
                &HooksConfig::default(),
                WindowId::Main,
                cx,
            )
            .expect("add project");
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
            ws.add_project(
                "Bookmark".to_string(),
                "/tmp/bm".to_string(),
                false,
                &HooksConfig::default(),
                WindowId::Main,
                cx,
            )
            .expect("add project");
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
            ws.delete_project(&mut FocusManager::new(), "p1", &HooksConfig::default(), cx);
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
            assert!(ws.is_project_hidden(WindowId::Main, "p1"));
            // Missing entry defaults to visible (not hidden).
            assert!(!ws.is_project_hidden(WindowId::Main, "p2"));
            assert!(!ws.is_project_hidden(WindowId::Main, "missing"));
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
            ws.toggle_project_overview_visibility(
                &mut FocusManager::new(),
                WindowId::Main,
                "p1",
                cx,
            );
        });
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.data().main_window.hidden_project_ids.contains("p1"));
        });

        // Second toggle: hidden -> visible. main_window removes the entry.
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_project_overview_visibility(
                &mut FocusManager::new(),
                WindowId::Main,
                "p1",
                cx,
            );
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
            assert!(
                ws.data()
                    .main_window
                    .hidden_project_ids
                    .contains("unknown_id")
            );
            assert_eq!(ws.data_version(), 1);
        });
    }

    #[gpui::test]
    fn toggle_worktree_visibility_extra_writes_only_to_targeted_window(
        cx: &mut gpui::TestAppContext,
    ) {
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
            ws.toggle_project_overview_visibility(
                &mut FocusManager::new(),
                WindowId::Main,
                "unknown_id",
                cx,
            );
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(
                !ws.data()
                    .main_window
                    .hidden_project_ids
                    .contains("unknown_id")
            );
            assert_eq!(ws.data_version(), 0);
        });
    }

    #[gpui::test]
    fn toggle_project_overview_visibility_extra_writes_only_to_targeted_window(
        cx: &mut gpui::TestAppContext,
    ) {
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
            ws.toggle_project_overview_visibility(
                &mut FocusManager::new(),
                WindowId::Extra(extra_a_id),
                "p1",
                cx,
            );
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
            ws.toggle_project_overview_visibility(
                &mut FocusManager::new(),
                WindowId::Extra(extra_a_id),
                "p1",
                cx,
            );
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(!ws.data().extra_windows[0].hidden_project_ids.contains("p1"));
        });
    }

    #[gpui::test]
    fn toggle_project_overview_visibility_unknown_extra_is_silent_noop(
        cx: &mut gpui::TestAppContext,
    ) {
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
            ws.toggle_project_overview_visibility(
                &mut FocusManager::new(),
                WindowId::Extra(unknown),
                "p1",
                cx,
            );
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
    fn update_project_widths_preserves_unmentioned_entries(cx: &mut gpui::TestAppContext) {
        // Hidden projects are absent from a resize update but must retain their
        // width for when they become visible again.
        let mut data = make_workspace_data();
        data.main_window
            .project_widths
            .insert("p1".to_string(), 0.50);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut widths = HashMap::new();
            widths.insert("p2".to_string(), 0.40);
            ws.update_project_widths(WindowId::Main, widths, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(
                ws.data().main_window.project_widths.get("p1").copied(),
                Some(0.50)
            );
            assert_eq!(
                ws.data().main_window.project_widths.get("p2").copied(),
                Some(0.40)
            );
        });
    }

    #[gpui::test]
    fn update_project_widths_with_scale_persists_both_values(cx: &mut gpui::TestAppContext) {
        let workspace = cx.new(|_cx| Workspace::new(make_workspace_data()));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut widths = HashMap::new();
            widths.insert("p1".to_string(), 31.25);
            ws.update_project_widths_with_scale(WindowId::Main, widths, 16.0, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.get_project_width(WindowId::Main, "p1", 1), 31.25);
            assert_eq!(ws.get_project_width_scale(WindowId::Main), Some(16.0));
            assert_eq!(ws.data_version(), 1);
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
        let extra_a = WindowState::default();
        let extra_a_id = extra_a.id;
        let mut extra_b = WindowState::default();
        let extra_b_id = extra_b.id;
        // Pre-populate sibling state on main + extra_b to verify isolation.
        data.main_window
            .project_widths
            .insert("p1".to_string(), 100.0);
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
            assert_eq!(
                ws.data().extra_windows[1].project_widths.get("p1"),
                Some(&200.0)
            );
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
        data.main_window
            .project_widths
            .insert("p1".to_string(), 50.0);
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
        data.main_window
            .project_widths
            .insert("p1".to_string(), 60.0);
        data.main_window
            .project_widths
            .insert("p2".to_string(), 40.0);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.delete_project(&mut FocusManager::new(), "p1", &HooksConfig::default(), cx);
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
        data.main_window
            .project_widths
            .insert("p1".to_string(), 60.0);
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
            ws.delete_project(&mut FocusManager::new(), "p1", &HooksConfig::default(), cx);
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
        data.main_window
            .project_widths
            .insert("wt1".to_string(), 35.0);
        data.main_window
            .hidden_project_ids
            .insert("wt1".to_string());
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
            assert!(
                !ws.data().extra_windows[0]
                    .project_widths
                    .contains_key("wt1")
            );
            assert!(
                !ws.data().extra_windows[0]
                    .hidden_project_ids
                    .contains("wt1")
            );
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

    fn git(args: &[&str]) {
        let output = Command::new("git").args(args).output().expect("run git");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn path_str(path: &Path) -> &str {
        path.to_str().expect("test path is utf-8")
    }

    #[gpui::test]
    fn removal_rejects_sibling_project_inside_physical_worktree_root(
        cx: &mut gpui::TestAppContext,
    ) {
        let fixture =
            std::env::temp_dir().join(format!("okena-root-owner-test-{}", uuid::Uuid::new_v4()));
        let main_repo = fixture.join("main");
        let worktree = fixture.join("worktree");
        git(&["init", "-b", "main", path_str(&main_repo)]);
        git(&[
            "-C",
            path_str(&main_repo),
            "config",
            "user.email",
            "okena@example.invalid",
        ]);
        git(&[
            "-C",
            path_str(&main_repo),
            "config",
            "user.name",
            "Okena Test",
        ]);
        std::fs::create_dir_all(main_repo.join("packages/a")).unwrap();
        std::fs::create_dir_all(main_repo.join("packages/b")).unwrap();
        std::fs::write(main_repo.join("packages/a/tracked.txt"), "a\n").unwrap();
        std::fs::write(main_repo.join("packages/b/tracked.txt"), "b\n").unwrap();
        git(&["-C", path_str(&main_repo), "add", "packages"]);
        git(&["-C", path_str(&main_repo), "commit", "-m", "base"]);
        git(&[
            "-C",
            path_str(&main_repo),
            "worktree",
            "add",
            "-b",
            "feature",
            path_str(&worktree),
        ]);
        let sentinel = worktree.join("packages/b/uncommitted.txt");
        std::fs::write(&sentinel, "must survive\n").unwrap();

        let mut parent = make_project("parent");
        parent.path = main_repo.to_string_lossy().into_owned();
        parent.worktree_ids = vec!["wt1".to_string()];
        let mut wt = make_worktree_project("wt1", "parent");
        wt.path = worktree.join("packages/a").to_string_lossy().into_owned();
        let metadata = wt.worktree_info.as_mut().unwrap();
        metadata.main_repo_path = main_repo.to_string_lossy().into_owned();
        metadata.worktree_path = worktree.to_string_lossy().into_owned();
        metadata.branch_name = "feature".to_string();
        let mut claimant = make_project("claimant");
        claimant.name = "Sibling".to_string();
        claimant.path = worktree.join("packages/b").to_string_lossy().into_owned();
        let mut data = make_workspace_data();
        data.projects = vec![parent, wt, claimant];
        data.project_order = vec!["parent".to_string(), "claimant".to_string()];
        let workspace = cx.new(|_| Workspace::new(data));

        let result = workspace.update(cx, |ws, cx| {
            ws.remove_worktree_project(
                &mut FocusManager::new(),
                "wt1",
                true,
                &HooksConfig::default(),
                cx,
            )
        });

        assert!(
            result.is_err_and(|error| error.contains("Sibling")),
            "removal must identify the sibling claimant"
        );
        assert!(sentinel.exists(), "uncommitted sibling data must survive");
        assert!(worktree.exists(), "the shared checkout must survive");

        git(&[
            "-C",
            path_str(&main_repo),
            "worktree",
            "remove",
            "--force",
            path_str(&worktree),
        ]);
        let _ = std::fs::remove_dir_all(&fixture);
    }

    /// Build a repo with one linked worktree registered as project "wt1" under
    /// parent "parent". Returns (fixture root, main repo, checkout, workspace data).
    fn worktree_fixture(name: &str) -> (PathBuf, PathBuf, PathBuf, WorkspaceData) {
        let fixture = std::env::temp_dir().join(format!("okena-{name}-{}", uuid::Uuid::new_v4()));
        let main_repo = fixture.join("main");
        let worktree = fixture.join("worktree");
        git(&["init", "-b", "main", path_str(&main_repo)]);
        git(&[
            "-C",
            path_str(&main_repo),
            "config",
            "user.email",
            "okena@example.invalid",
        ]);
        git(&[
            "-C",
            path_str(&main_repo),
            "config",
            "user.name",
            "Okena Test",
        ]);
        std::fs::write(main_repo.join("file.txt"), "tracked\n").unwrap();
        git(&["-C", path_str(&main_repo), "add", "file.txt"]);
        git(&["-C", path_str(&main_repo), "commit", "-m", "base"]);
        git(&[
            "-C",
            path_str(&main_repo),
            "worktree",
            "add",
            "-b",
            "feature",
            path_str(&worktree),
        ]);

        let mut parent = make_project("parent");
        parent.path = main_repo.to_string_lossy().into_owned();
        parent.worktree_ids = vec!["wt1".to_string()];
        let mut wt = make_worktree_project("wt1", "parent");
        wt.path = worktree.to_string_lossy().into_owned();
        let metadata = wt.worktree_info.as_mut().unwrap();
        metadata.main_repo_path = main_repo.to_string_lossy().into_owned();
        metadata.worktree_path = worktree.to_string_lossy().into_owned();
        metadata.branch_name = "feature".to_string();
        let mut data = make_workspace_data();
        data.projects = vec![parent, wt];
        data.project_order = vec!["parent".to_string()];
        (fixture, main_repo, worktree, data)
    }

    /// Orphan the checkout the way a blanket `git worktree prune` does: drop the
    /// main repo's metadata entry and leave the directory with a dangling `.git`.
    fn prune_worktree_metadata(main_repo: &Path) {
        std::fs::remove_dir_all(main_repo.join(".git").join("worktrees").join("worktree"))
            .expect("prune the worktree metadata entry");
    }

    #[gpui::test]
    fn force_remove_clears_an_orphaned_worktree_the_standard_close_cannot(
        cx: &mut gpui::TestAppContext,
    ) {
        let (fixture, main_repo, worktree, data) = worktree_fixture("force-remove-orphan-test");
        prune_worktree_metadata(&main_repo);
        let workspace = cx.new(|_| Workspace::new(data));

        // The standard removal is the dead end users hit: git no longer tracks
        // the checkout, so nothing it can do reaches this directory.
        let standard = workspace.update(cx, |ws, cx| {
            ws.remove_worktree_project(
                &mut FocusManager::new(),
                "wt1",
                true,
                &HooksConfig::default(),
                cx,
            )
        });
        assert!(standard.is_err(), "git cannot remove an orphaned checkout");
        assert!(worktree.exists(), "the failed close leaves the checkout");

        let forced = workspace.update(cx, |ws, cx| {
            assert!(ws.worktree_is_orphaned("wt1"), "checkout reads as orphaned");
            ws.force_remove_worktree_project(
                &mut FocusManager::new(),
                "wt1",
                &HooksConfig::default(),
                cx,
            )
        });

        assert!(forced.is_ok(), "force remove failed: {:?}", forced.err());
        assert!(!worktree.exists(), "the checkout is deleted from disk");
        workspace.read_with(cx, |ws: &Workspace, _| {
            assert!(ws.project("wt1").is_none(), "the project row is dropped");
        });
        let _ = std::fs::remove_dir_all(&fixture);
    }

    #[gpui::test]
    fn force_remove_refuses_a_healthy_worktree(cx: &mut gpui::TestAppContext) {
        let (fixture, main_repo, worktree, data) = worktree_fixture("force-remove-healthy-test");
        let workspace = cx.new(|_| Workspace::new(data));

        let result = workspace.update(cx, |ws, cx| {
            assert!(!ws.worktree_is_orphaned("wt1"), "checkout is still tracked");
            ws.force_remove_worktree_project(
                &mut FocusManager::new(),
                "wt1",
                &HooksConfig::default(),
                cx,
            )
        });

        assert!(
            result.is_err(),
            "a tracked checkout must go through the standard close"
        );
        assert!(worktree.exists(), "the checkout survives");
        workspace.read_with(cx, |ws: &Workspace, _| {
            assert!(ws.project("wt1").is_some(), "the project row survives");
        });

        git(&[
            "-C",
            path_str(&main_repo),
            "worktree",
            "remove",
            "--force",
            path_str(&worktree),
        ]);
        let _ = std::fs::remove_dir_all(&fixture);
    }

    #[gpui::test]
    fn force_remove_refuses_a_checkout_another_project_lives_in(cx: &mut gpui::TestAppContext) {
        let (fixture, main_repo, worktree, mut data) = worktree_fixture("force-remove-claim-test");
        prune_worktree_metadata(&main_repo);
        let sentinel = worktree.join("sibling-data.txt");
        std::fs::write(&sentinel, "must survive\n").unwrap();
        let mut claimant = make_project("claimant");
        claimant.name = "Sibling".to_string();
        claimant.path = worktree.to_string_lossy().into_owned();
        data.projects.push(claimant);
        data.project_order.push("claimant".to_string());
        let workspace = cx.new(|_| Workspace::new(data));

        let result = workspace.update(cx, |ws, cx| {
            ws.force_remove_worktree_project(
                &mut FocusManager::new(),
                "wt1",
                &HooksConfig::default(),
                cx,
            )
        });

        assert!(
            result.is_err_and(|error| error.contains("Sibling")),
            "force remove must identify the sibling claimant"
        );
        assert!(sentinel.exists(), "the sibling's data must survive");
        let _ = std::fs::remove_dir_all(&fixture);
    }

    #[gpui::test]
    fn close_rejects_stale_root_pointing_into_independent_repository(
        cx: &mut gpui::TestAppContext,
    ) {
        let fixture =
            std::env::temp_dir().join(format!("okena-stale-root-test-{}", uuid::Uuid::new_v4()));
        let main_repo = fixture.join("main");
        let registered_worktree = fixture.join("registered-worktree");
        let independent_repo = fixture.join("independent");
        git(&["init", "-b", "main", path_str(&main_repo)]);
        git(&[
            "-C",
            path_str(&main_repo),
            "config",
            "user.email",
            "okena@example.invalid",
        ]);
        git(&[
            "-C",
            path_str(&main_repo),
            "config",
            "user.name",
            "Okena Test",
        ]);
        std::fs::write(main_repo.join("base.txt"), "base\n").unwrap();
        git(&["-C", path_str(&main_repo), "add", "base.txt"]);
        git(&["-C", path_str(&main_repo), "commit", "-m", "base"]);
        git(&[
            "-C",
            path_str(&main_repo),
            "worktree",
            "add",
            "-b",
            "feature",
            path_str(&registered_worktree),
        ]);

        git(&["init", "-b", "main", path_str(&independent_repo)]);
        std::fs::create_dir_all(independent_repo.join("packages/app")).unwrap();
        let sentinel = independent_repo.join("must-survive.txt");
        std::fs::write(&sentinel, "independent data\n").unwrap();

        let mut parent = make_project("parent");
        parent.path = main_repo.to_string_lossy().into_owned();
        parent.worktree_ids = vec!["wt1".to_string()];
        let mut stale = make_worktree_project("wt1", "parent");
        stale.path = independent_repo
            .join("packages/app")
            .to_string_lossy()
            .into_owned();
        let metadata = stale.worktree_info.as_mut().unwrap();
        metadata.main_repo_path = main_repo.to_string_lossy().into_owned();
        metadata.worktree_path = registered_worktree.to_string_lossy().into_owned();
        metadata.branch_name = "feature".to_string();
        let mut data = make_workspace_data();
        data.projects = vec![parent, stale];
        data.project_order = vec!["parent".to_string()];
        let workspace = cx.new(|_| Workspace::new(data));

        let error = workspace.update(cx, |ws, cx| {
            ws.close_worktree(
                &mut FocusManager::new(),
                "wt1",
                false,
                false,
                false,
                false,
                false,
                &HooksConfig::default(),
                cx,
            )
            .unwrap_err()
        });

        assert!(error.contains("does not match its recorded checkout root"));
        assert_eq!(
            std::fs::read_to_string(&sentinel).unwrap(),
            "independent data\n"
        );
        assert!(independent_repo.exists());
        assert!(workspace.read_with(cx, |ws, _| ws.project("wt1").is_some()));

        let legacy_error = workspace.update(cx, |ws, cx| {
            ws.with_project("wt1", cx, |project| {
                project
                    .worktree_info
                    .as_mut()
                    .unwrap()
                    .worktree_path
                    .clear();
                true
            });
            match ws.begin_worktree_removal("wt1", &HooksConfig::default(), cx) {
                Ok(_) => panic!("unregistered legacy worktree must be rejected"),
                Err(error) => error,
            }
        });
        assert!(legacy_error.contains("does not belong to the parent repository"));
        assert!(
            sentinel.exists(),
            "legacy metadata cannot bypass registration"
        );

        let legacy_plan = workspace.update(cx, |ws, cx| {
            ws.with_project("wt1", cx, |project| {
                project.path = registered_worktree.to_string_lossy().into_owned();
                true
            });
            ws.begin_worktree_removal("wt1", &HooksConfig::default(), cx)
                .expect("registered legacy worktree is accepted")
        });
        assert_eq!(
            Workspace::physical_path_identity(&legacy_plan.worktree_path),
            Workspace::physical_path_identity(&registered_worktree)
        );

        git(&[
            "-C",
            path_str(&main_repo),
            "worktree",
            "remove",
            "--force",
            path_str(&registered_worktree),
        ]);
        let _ = std::fs::remove_dir_all(&fixture);
    }

    #[gpui::test]
    fn active_root_lease_rejects_project_and_worktree_registration(cx: &mut gpui::TestAppContext) {
        let fixture =
            std::env::temp_dir().join(format!("okena-root-lease-test-{}", uuid::Uuid::new_v4()));
        let root = fixture.join("checkout");
        std::fs::create_dir_all(&root).unwrap();
        let mut parent = make_project("parent");
        parent.path = fixture.join("main").to_string_lossy().into_owned();
        let mut container = make_project("container");
        container.path = fixture.to_string_lossy().into_owned();
        let mut data = make_workspace_data();
        data.projects = vec![parent, container];
        data.project_order = vec!["parent".to_string(), "container".to_string()];
        let workspace = cx.new(|_| Workspace::new(data));

        let (add_error, register_error, discovered_error, rename_error, released_claim) = workspace
            .update(cx, |ws, cx| {
                let active = ws
                    .register_worktree_project_deferred_hooks(
                        "parent",
                        "active",
                        &fixture.join("main"),
                        path_str(&root),
                        path_str(&root.join("packages/a")),
                        &HooksConfig::default(),
                        WindowId::Main,
                        cx,
                    )
                    .expect("register active worktree");
                let metadata = ws.project(&active).unwrap().worktree_info.as_ref().unwrap();
                assert_eq!(metadata.worktree_path, path_str(&root));
                ws.mark_creating_project(&active);
                let add_error = ws
                    .add_project(
                        "claim".to_string(),
                        root.join("packages/b").to_string_lossy().into_owned(),
                        false,
                        &HooksConfig::default(),
                        WindowId::Main,
                        cx,
                    )
                    .unwrap_err();
                let register_error = ws
                    .register_worktree_project_deferred_hooks(
                        "parent",
                        "other",
                        &fixture.join("main"),
                        path_str(&root.join("nested")),
                        path_str(&root.join("nested/project")),
                        &HooksConfig::default(),
                        WindowId::Main,
                        cx,
                    )
                    .unwrap_err();
                let discovered_error = ws
                    .add_discovered_worktree(
                        path_str(&root.join("discovered")),
                        "discovered",
                        "parent",
                        WindowId::Main,
                    )
                    .unwrap_err();
                let rename_error = ws
                    .ensure_project_path_mutation_allowed(
                        "container",
                        &fixture.with_extension("moved"),
                    )
                    .unwrap_err();
                ws.finish_creating_project(&active);
                let released_claim = ws
                    .add_project(
                        "released claim".to_string(),
                        root.join("packages/b").to_string_lossy().into_owned(),
                        false,
                        &HooksConfig::default(),
                        WindowId::Main,
                        cx,
                    )
                    .expect("claim succeeds after lease release");
                (
                    add_error,
                    register_error,
                    discovered_error,
                    rename_error,
                    released_claim,
                )
            });

        assert!(add_error.contains("active worktree operation"));
        assert!(register_error.contains("overlaps active operation"));
        assert!(discovered_error.contains("overlaps active operation"));
        assert!(rename_error.contains("overlaps active worktree operation"));
        workspace.read_with(cx, |ws, _| {
            assert!(ws.project(&released_claim).is_some());
        });
        let _ = std::fs::remove_dir_all(&fixture);
    }

    #[test]
    fn physical_path_identity_normalizes_relative_paths() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(
            Workspace::physical_path_identity(Path::new("identity-a/../identity-b")),
            Workspace::physical_path_identity(&cwd.join("identity-b"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn physical_path_identity_folds_windows_case_aliases() {
        let path = std::env::temp_dir().join(format!(
            "okena-case-identity-test-{}/Missing",
            uuid::Uuid::new_v4()
        ));
        let upper = std::path::PathBuf::from(path.to_string_lossy().to_uppercase());
        assert_eq!(
            Workspace::physical_path_identity(&path),
            Workspace::physical_path_identity(&upper)
        );
    }

    #[cfg(unix)]
    #[test]
    fn physical_path_identity_follows_dangling_symlink_before_parent_components() {
        use std::os::unix::fs::symlink;

        let fixture =
            std::env::temp_dir().join(format!("okena-path-identity-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&fixture).unwrap();
        let target = fixture.join("not-created/checkout");
        let alias = fixture.join("alias");
        symlink(&target, &alias).unwrap();

        assert_eq!(
            Workspace::physical_path_identity(&alias.join("child/../project")),
            Workspace::physical_path_identity(&target.join("project"))
        );
        let _ = std::fs::remove_dir_all(&fixture);
    }

    #[gpui::test]
    fn test_delete_worktree_removes_from_parent_worktree_ids(cx: &mut gpui::TestAppContext) {
        let mut parent = make_project("parent");
        parent.worktree_ids = vec!["wt1".to_string(), "wt2".to_string()];
        let mut data = make_workspace_data();
        data.projects = vec![
            parent,
            make_worktree_project("wt1", "parent"),
            make_worktree_project("wt2", "parent"),
        ];
        data.project_order = vec!["parent".to_string()];
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.delete_project(&mut FocusManager::new(), "wt1", &HooksConfig::default(), cx);
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
        data.projects = vec![
            parent,
            make_worktree_project("wt1", "parent"),
            make_worktree_project("wt2", "parent"),
        ];
        data.project_order = vec!["parent".to_string()];
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.delete_project(
                &mut FocusManager::new(),
                "parent",
                &HooksConfig::default(),
                cx,
            );
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
        data.projects = vec![
            parent,
            make_worktree_project("wt1", "parent"),
            make_worktree_project("wt2", "parent"),
            make_worktree_project("wt3", "parent"),
        ];
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
    fn test_hide_focused_project_moves_focus_to_next(cx: &mut gpui::TestAppContext) {
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1"), make_project("p2"), make_project("p3")];
        data.project_order = vec!["p1".to_string(), "p2".to_string(), "p3".to_string()];
        let workspace = cx.new(|_cx| Workspace::new(data));

        let mut fm = FocusManager::new();
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.set_focused_terminal(&mut fm, "p2".to_string(), vec![], cx);
            ws.toggle_project_overview_visibility(&mut fm, WindowId::Main, "p2", cx);
        });

        let state = fm.focused_terminal_state().expect("focus should be set");
        assert_eq!(state.project_id, "p3");
    }

    #[gpui::test]
    fn test_hide_focused_last_project_falls_back_to_previous(cx: &mut gpui::TestAppContext) {
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1"), make_project("p2")];
        data.project_order = vec!["p1".to_string(), "p2".to_string()];
        let workspace = cx.new(|_cx| Workspace::new(data));

        let mut fm = FocusManager::new();
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.set_focused_terminal(&mut fm, "p2".to_string(), vec![], cx);
            ws.toggle_project_overview_visibility(&mut fm, WindowId::Main, "p2", cx);
        });

        let state = fm.focused_terminal_state().expect("focus should be set");
        assert_eq!(state.project_id, "p1");
    }

    #[gpui::test]
    fn test_hide_focused_project_from_modal_keeps_modal_and_redirects_restore(
        cx: &mut gpui::TestAppContext,
    ) {
        // Hiding the focused project from the project switcher (a modal) must
        // NOT drop the Modal context — otherwise the switcher loses keyboard
        // focus and the user has to click to keep navigating. The focus the
        // modal restores on close is redirected to the neighbor instead.
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1"), make_project("p2"), make_project("p3")];
        data.project_order = vec!["p1".to_string(), "p2".to_string(), "p3".to_string()];
        let workspace = cx.new(|_cx| Workspace::new(data));

        let mut fm = FocusManager::new();
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.set_focused_terminal(&mut fm, "p2".to_string(), vec![], cx);
            // Open the switcher: clear_focused_terminal -> enter_modal.
            ws.clear_focused_terminal(&mut fm, cx);
            assert!(fm.is_modal());
            ws.toggle_project_overview_visibility(&mut fm, WindowId::Main, "p2", cx);
        });

        // Modal context survives the hide — the switcher keeps keyboard focus.
        assert!(
            fm.is_modal(),
            "switcher must keep keyboard focus after hiding"
        );

        // Closing the switcher restores focus to the neighbor (p3), not the
        // now-hidden p2, and leaves the modal context.
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.restore_focused_terminal(&mut fm, cx);
        });
        assert!(!fm.is_modal());
        let state = fm
            .focused_terminal_state()
            .expect("focus should be set after close");
        assert_eq!(state.project_id, "p3");
    }

    #[gpui::test]
    fn test_hide_unfocused_project_leaves_focus(cx: &mut gpui::TestAppContext) {
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1"), make_project("p2")];
        data.project_order = vec!["p1".to_string(), "p2".to_string()];
        let workspace = cx.new(|_cx| Workspace::new(data));

        let mut fm = FocusManager::new();
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.set_focused_terminal(&mut fm, "p1".to_string(), vec![], cx);
            ws.toggle_project_overview_visibility(&mut fm, WindowId::Main, "p2", cx);
        });

        let state = fm.focused_terminal_state().expect("focus should remain");
        assert_eq!(state.project_id, "p1");
    }

    #[gpui::test]
    fn test_add_terminal_gpui(cx: &mut gpui::TestAppContext) {
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1")];
        data.project_order = vec!["p1".to_string()];
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.add_terminal(&mut FocusManager::new(), "p1", cx);
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

        assert!(
            ws.project("wt1").is_some(),
            "closing project should not be removed"
        );
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

        assert!(
            ws.project("wt1").is_some(),
            "creating project should not be removed"
        );
    }

    #[test]
    fn test_remove_stale_worktree_succeeds_when_not_managed() {
        let mut data = make_workspace_data();
        let wt = make_worktree_project("wt1", "parent");
        data.projects = vec![make_project("parent"), wt];
        data.project_order = vec!["parent".to_string()];
        let mut ws = Workspace::new(data);

        ws.remove_stale_worktree("wt1");

        assert!(
            ws.project("wt1").is_none(),
            "unmanaged stale worktree should be removed"
        );
    }

    #[gpui::test]
    fn begin_worktree_removal_rejected_while_creating(cx: &mut gpui::TestAppContext) {
        // Optimistic worktree create registers the row and returns before its
        // background `git worktree add` finishes; a removal landing in that
        // window must be rejected so it can't race the in-flight checkout and
        // strand an orphaned, git-registered worktree with no workspace row.
        // The row and its creating flag must survive the rejected call intact.
        let mut parent = make_project("parent");
        parent.worktree_ids = vec!["wt1".to_string()];
        let mut data = make_workspace_data();
        data.projects = vec![parent, make_worktree_project("wt1", "parent")];
        data.project_order = vec!["parent".to_string()];
        let workspace = cx.new(|_cx| Workspace::new(data));

        let err = workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.mark_creating_project("wt1");
            ws.begin_worktree_removal("wt1", &HooksConfig::default(), cx)
                .err()
        });

        assert_eq!(err.as_deref(), Some("worktree is still being created"));
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(
                ws.project("wt1").is_some(),
                "row survives the rejected removal"
            );
            assert!(ws.is_creating_project("wt1"), "creating flag untouched");
        });
    }

    #[gpui::test]
    fn close_worktree_rejected_while_creating(cx: &mut gpui::TestAppContext) {
        // The close-entry guard must reject BEFORE close_worktree fires a
        // before_remove hook or registers a pending close. Without the entry
        // guard the flow would still be rejected — by the begin_worktree_removal
        // backstop, with the identical error — but only AFTER the headless
        // before_remove hook ran. So the sharp assertion is the marker file: the
        // project carries a before_remove hook that writes one, and it must not
        // exist after the rejected call. (The project path must be a real dir —
        // the headless hook spawns with cwd = OKENA_PROJECT_PATH.)
        let marker =
            std::env::temp_dir().join(format!("okena_close_guard_marker_{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let mut parent = make_project("parent");
        parent.worktree_ids = vec!["wt1".to_string()];
        let mut wt = make_worktree_project("wt1", "parent");
        wt.path = std::env::temp_dir().to_string_lossy().into_owned();
        wt.hooks.worktree.before_remove = Some(format!("echo x > \"{}\"", marker.display()));
        let mut data = make_workspace_data();
        data.projects = vec![parent, wt];
        data.project_order = vec!["parent".to_string()];
        let workspace = cx.new(|_cx| Workspace::new(data));

        let err = workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.mark_creating_project("wt1");
            ws.close_worktree(
                &mut FocusManager::new(),
                "wt1",
                false, // merge
                false, // stash
                false, // fetch
                false, // push
                false, // delete_branch
                &HooksConfig::default(),
                cx,
            )
            .err()
        });

        assert_eq!(err.as_deref(), Some("worktree is still being created"));
        assert!(
            !marker.exists(),
            "before_remove hook must not fire on a rejected mid-create close",
        );
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(
                ws.project("wt1").is_some(),
                "row survives the rejected close"
            );
            assert!(ws.is_creating_project("wt1"), "creating flag untouched");
            assert!(
                !ws.is_project_closing("wt1"),
                "no pending close registered (tracker)"
            );
            assert!(
                !ws.project("wt1").unwrap().is_closing,
                "no pending close registered (wire-facing closing flag stays clear)",
            );
        });
        let _ = std::fs::remove_file(&marker);
    }

    #[gpui::test]
    fn close_worktree_rejected_while_already_closing(cx: &mut gpui::TestAppContext) {
        let mut parent = make_project("parent");
        parent.worktree_ids = vec!["wt1".to_string()];
        let mut data = make_workspace_data();
        data.projects = vec![parent, make_worktree_project("wt1", "parent")];
        data.project_order = vec!["parent".to_string()];
        let workspace = cx.new(|_cx| Workspace::new(data));

        let err = workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.mark_closing_project_authoritative("wt1");
            ws.close_worktree(
                &mut FocusManager::new(),
                "wt1",
                false,
                false,
                false,
                false,
                false,
                &HooksConfig::default(),
                cx,
            )
            .err()
        });

        assert_eq!(err.as_deref(), Some("worktree is already closing"));
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.project("wt1").is_some());
            assert!(ws.is_project_closing("wt1"));
            assert!(ws.project("wt1").unwrap().is_closing);
        });
    }

    #[gpui::test]
    fn rename_worktree_root_moves_git_registration_and_descendant_projects(
        cx: &mut gpui::TestAppContext,
    ) {
        let fixture =
            std::env::temp_dir().join(format!("okena-root-rename-{}", uuid::Uuid::new_v4()));
        let main_repo = fixture.join("main");
        let worktree = fixture.join("worktree");
        let renamed = fixture.join("renamed-worktree");
        git(&["init", "-b", "main", path_str(&main_repo)]);
        git(&[
            "-C",
            path_str(&main_repo),
            "config",
            "user.email",
            "okena@example.invalid",
        ]);
        git(&[
            "-C",
            path_str(&main_repo),
            "config",
            "user.name",
            "Okena Test",
        ]);
        std::fs::create_dir_all(main_repo.join("packages/child")).unwrap();
        std::fs::write(main_repo.join("packages/child/file.txt"), "tracked\n").unwrap();
        git(&["-C", path_str(&main_repo), "add", "packages"]);
        git(&["-C", path_str(&main_repo), "commit", "-m", "base"]);
        git(&[
            "-C",
            path_str(&main_repo),
            "worktree",
            "add",
            "-b",
            "feature",
            path_str(&worktree),
        ]);

        let mut parent = make_project("parent");
        parent.path = main_repo.to_string_lossy().into_owned();
        parent.worktree_ids = vec!["wt1".to_string()];
        let mut child = make_worktree_project("wt1", "parent");
        child.path = worktree.to_string_lossy().into_owned();
        let metadata = child.worktree_info.as_mut().unwrap();
        metadata.main_repo_path = main_repo.to_string_lossy().into_owned();
        metadata.worktree_path = worktree.to_string_lossy().into_owned();
        let mut descendant = make_project("descendant");
        descendant.path = worktree
            .join("packages/child")
            .to_string_lossy()
            .into_owned();
        let mut data = make_workspace_data();
        data.projects = vec![parent, child, descendant];
        data.project_order = vec!["parent".to_string(), "descendant".to_string()];
        let workspace = cx.new(|_| Workspace::new(data));

        workspace.update(cx, |ws, cx| {
            ws.rename_project_directory(
                "wt1",
                renamed.to_string_lossy().into_owned(),
                "renamed-worktree".to_string(),
                cx,
            )
            .unwrap();
        });

        workspace.read_with(cx, |ws, _| {
            let moved = ws.project("wt1").unwrap();
            assert_eq!(Path::new(&moved.path), renamed);
            assert_eq!(
                Path::new(&moved.worktree_info.as_ref().unwrap().worktree_path),
                renamed
            );
            assert_eq!(
                Path::new(&ws.project("descendant").unwrap().path),
                renamed.join("packages/child")
            );
        });
        assert!(!worktree.exists());
        assert!(okena_git::verify_linked_worktree_fresh(&main_repo, &renamed).is_ok());
        git(&[
            "-C",
            path_str(&main_repo),
            "worktree",
            "remove",
            "--force",
            path_str(&renamed),
        ]);
        let _ = std::fs::remove_dir_all(fixture);
    }

    #[gpui::test]
    fn rename_monorepo_subdirectory_preserves_worktree_root(cx: &mut gpui::TestAppContext) {
        let fixture =
            std::env::temp_dir().join(format!("okena-subdir-rename-{}", uuid::Uuid::new_v4()));
        let main_repo = fixture.join("main");
        let worktree = fixture.join("worktree");
        git(&["init", "-b", "main", path_str(&main_repo)]);
        git(&[
            "-C",
            path_str(&main_repo),
            "config",
            "user.email",
            "okena@example.invalid",
        ]);
        git(&[
            "-C",
            path_str(&main_repo),
            "config",
            "user.name",
            "Okena Test",
        ]);
        std::fs::create_dir_all(main_repo.join("packages/app/nested")).unwrap();
        std::fs::write(main_repo.join("packages/app/file.txt"), "tracked\n").unwrap();
        std::fs::write(main_repo.join("packages/app/nested/file.txt"), "nested\n").unwrap();
        git(&["-C", path_str(&main_repo), "add", "packages"]);
        git(&["-C", path_str(&main_repo), "commit", "-m", "base"]);
        git(&[
            "-C",
            path_str(&main_repo),
            "worktree",
            "add",
            "-b",
            "feature",
            path_str(&worktree),
        ]);
        let old_project_path = worktree.join("packages/app");
        let new_project_path = worktree.join("packages/renamed-app");

        let mut parent = make_project("parent");
        parent.path = main_repo.to_string_lossy().into_owned();
        parent.worktree_ids = vec!["wt1".to_string()];
        let mut child = make_worktree_project("wt1", "parent");
        child.path = old_project_path.to_string_lossy().into_owned();
        let metadata = child.worktree_info.as_mut().unwrap();
        metadata.main_repo_path = main_repo.to_string_lossy().into_owned();
        metadata.worktree_path = worktree.to_string_lossy().into_owned();
        let mut descendant = make_project("descendant");
        descendant.path = old_project_path
            .join("nested")
            .to_string_lossy()
            .into_owned();
        let mut data = make_workspace_data();
        data.projects = vec![parent, child, descendant];
        data.project_order = vec!["parent".to_string(), "descendant".to_string()];
        let workspace = cx.new(|_| Workspace::new(data));

        workspace.update(cx, |ws, cx| {
            ws.rename_project_directory(
                "wt1",
                new_project_path.to_string_lossy().into_owned(),
                "renamed-app".to_string(),
                cx,
            )
            .unwrap();
        });

        workspace.read_with(cx, |ws, _| {
            let moved = ws.project("wt1").unwrap();
            assert_eq!(Path::new(&moved.path), new_project_path);
            assert_eq!(
                Path::new(&moved.worktree_info.as_ref().unwrap().worktree_path),
                worktree
            );
            assert_eq!(
                Path::new(&ws.project("descendant").unwrap().path),
                new_project_path.join("nested")
            );
        });
        assert!(worktree.exists());
        assert!(!old_project_path.exists());
        assert!(new_project_path.exists());
        assert!(okena_git::verify_linked_worktree_fresh(&main_repo, &worktree).is_ok());
        git(&[
            "-C",
            path_str(&main_repo),
            "worktree",
            "remove",
            "--force",
            path_str(&worktree),
        ]);
        let _ = std::fs::remove_dir_all(fixture);
    }

    #[gpui::test]
    fn rename_project_directory_translates_descendant_projects(cx: &mut gpui::TestAppContext) {
        let fixture =
            std::env::temp_dir().join(format!("okena-project-rename-{}", uuid::Uuid::new_v4()));
        let project_path = fixture.join("project");
        let renamed_path = fixture.join("renamed-project");
        let descendant_path = project_path.join("packages/nested");
        std::fs::create_dir_all(&descendant_path).unwrap();

        let mut project = make_project("project");
        project.path = project_path.to_string_lossy().into_owned();
        let mut descendant = make_project("descendant");
        descendant.path = descendant_path.to_string_lossy().into_owned();
        descendant.hook_terminals.insert(
            "completed-descendant-hook".to_string(),
            okena_state::HookTerminalEntry {
                label: "completed".to_string(),
                status: okena_state::HookTerminalStatus::Succeeded,
                hook_type: "project.on_open".to_string(),
                command: "echo done".to_string(),
                cwd: descendant_path.to_string_lossy().into_owned(),
            },
        );
        let mut data = make_workspace_data();
        data.projects = vec![project, descendant];
        data.project_order = vec!["project".to_string(), "descendant".to_string()];
        let workspace = cx.new(|_| Workspace::new(data));

        workspace.update(cx, |ws, cx| {
            let plan = ws
                .prepare_project_directory_rename(
                    "project",
                    renamed_path.to_string_lossy().into_owned(),
                    "renamed-project".to_string(),
                )
                .unwrap();
            assert_eq!(
                plan.affected_project_ids().collect::<Vec<_>>(),
                vec!["project", "descendant"]
            );
            assert_eq!(plan.old_path(), project_path);
            assert_eq!(plan.new_path(), renamed_path);
            ws.rename_project_directory(
                "project",
                renamed_path.to_string_lossy().into_owned(),
                "renamed-project".to_string(),
                cx,
            )
            .unwrap();
        });

        workspace.read_with(cx, |ws, _| {
            assert_eq!(
                Path::new(&ws.project("project").unwrap().path),
                renamed_path
            );
            assert_eq!(
                Path::new(&ws.project("descendant").unwrap().path),
                renamed_path.join("packages/nested")
            );
            assert_eq!(
                Path::new(
                    &ws.project("descendant").unwrap().hook_terminals["completed-descendant-hook"]
                        .cwd
                ),
                renamed_path.join("packages/nested")
            );
        });
        assert!(!project_path.exists());
        assert!(renamed_path.join("packages/nested").exists());
        let _ = std::fs::remove_dir_all(fixture);
    }

    #[gpui::test]
    fn rename_main_repository_requires_linked_worktrees_to_be_removed(
        cx: &mut gpui::TestAppContext,
    ) {
        let fixture =
            std::env::temp_dir().join(format!("okena-main-repo-rename-{}", uuid::Uuid::new_v4()));
        let main_repo = fixture.join("main");
        let linked_worktree = fixture.join("external-worktree");
        let renamed_repo = fixture.join("renamed-main");
        git(&["init", "-b", "main", path_str(&main_repo)]);
        git(&[
            "-C",
            path_str(&main_repo),
            "config",
            "user.email",
            "okena@example.invalid",
        ]);
        git(&[
            "-C",
            path_str(&main_repo),
            "config",
            "user.name",
            "Okena Test",
        ]);
        std::fs::write(main_repo.join("file.txt"), "tracked\n").unwrap();
        git(&["-C", path_str(&main_repo), "add", "file.txt"]);
        git(&["-C", path_str(&main_repo), "commit", "-m", "base"]);
        git(&[
            "-C",
            path_str(&main_repo),
            "worktree",
            "add",
            "-b",
            "external",
            path_str(&linked_worktree),
        ]);

        let mut project = make_project("project");
        project.path = main_repo.to_string_lossy().into_owned();
        let mut data = make_workspace_data();
        data.projects = vec![project];
        data.project_order = vec!["project".to_string()];
        let workspace = cx.new(|_| Workspace::new(data));

        let error = workspace.update(cx, |ws, cx| {
            ws.rename_project_directory(
                "project",
                renamed_repo.to_string_lossy().into_owned(),
                "renamed-main".to_string(),
                cx,
            )
            .expect_err("linked worktree must block repository rename")
        });

        assert!(error.contains("has linked worktrees"));
        assert!(main_repo.exists());
        assert!(!renamed_repo.exists());
        assert!(okena_git::verify_linked_worktree_fresh(&main_repo, &linked_worktree).is_ok());

        git(&[
            "-C",
            path_str(&main_repo),
            "worktree",
            "remove",
            path_str(&linked_worktree),
        ]);
        workspace.update(cx, |ws, cx| {
            ws.rename_project_directory(
                "project",
                renamed_repo.to_string_lossy().into_owned(),
                "renamed-main".to_string(),
                cx,
            )
            .expect("repository without linked worktrees can be renamed");
        });

        assert!(!main_repo.exists());
        assert!(renamed_repo.join(".git").exists());
        workspace.read_with(cx, |ws, _| {
            assert_eq!(
                Path::new(&ws.project("project").unwrap().path),
                renamed_repo
            );
        });
        let _ = std::fs::remove_dir_all(fixture);
    }

    #[gpui::test]
    fn rename_ancestor_rejects_descendant_repository_with_external_worktree(
        cx: &mut gpui::TestAppContext,
    ) {
        let fixture = std::env::temp_dir().join(format!(
            "okena-ancestor-repo-rename-{}",
            uuid::Uuid::new_v4()
        ));
        let ancestor = fixture.join("workspace");
        let main_repo = ancestor.join("packages/repo");
        let linked_worktree = fixture.join("external-worktree");
        let renamed = fixture.join("renamed-workspace");
        git(&["init", "-b", "main", path_str(&main_repo)]);
        git(&[
            "-C",
            path_str(&main_repo),
            "config",
            "user.email",
            "okena@example.invalid",
        ]);
        git(&[
            "-C",
            path_str(&main_repo),
            "config",
            "user.name",
            "Okena Test",
        ]);
        std::fs::write(main_repo.join("file.txt"), "tracked\n").unwrap();
        git(&["-C", path_str(&main_repo), "add", "file.txt"]);
        git(&["-C", path_str(&main_repo), "commit", "-m", "base"]);
        git(&[
            "-C",
            path_str(&main_repo),
            "worktree",
            "add",
            "-b",
            "external",
            path_str(&linked_worktree),
        ]);

        let mut parent = make_project("ancestor");
        parent.path = ancestor.to_string_lossy().into_owned();
        let mut descendant = make_project("repo");
        descendant.path = main_repo.to_string_lossy().into_owned();
        let mut data = make_workspace_data();
        data.projects = vec![parent, descendant];
        data.project_order = vec!["ancestor".to_string(), "repo".to_string()];
        let workspace = cx.new(|_| Workspace::new(data));

        let error = workspace.update(cx, |ws, _| {
            ws.prepare_project_directory_rename(
                "ancestor",
                renamed.to_string_lossy().into_owned(),
                "renamed-workspace".to_string(),
            )
            .err()
            .expect("descendant repository registration must block ancestor move")
        });
        assert!(error.contains("has linked worktrees"));

        git(&[
            "-C",
            path_str(&main_repo),
            "worktree",
            "remove",
            "--force",
            path_str(&linked_worktree),
        ]);
        let _ = std::fs::remove_dir_all(fixture);
    }

    #[gpui::test]
    fn rename_ancestor_rejects_descendant_worktree_metadata_root(cx: &mut gpui::TestAppContext) {
        let fixture = std::env::temp_dir().join(format!(
            "okena-ancestor-worktree-rename-{}",
            uuid::Uuid::new_v4()
        ));
        let main_repo = fixture.join("main");
        let ancestor = fixture.join("workspace");
        let worktree = ancestor.join("linked-checkout");
        let renamed = fixture.join("renamed-workspace");
        git(&["init", "-b", "main", path_str(&main_repo)]);
        git(&[
            "-C",
            path_str(&main_repo),
            "config",
            "user.email",
            "okena@example.invalid",
        ]);
        git(&[
            "-C",
            path_str(&main_repo),
            "config",
            "user.name",
            "Okena Test",
        ]);
        std::fs::write(main_repo.join("file.txt"), "tracked\n").unwrap();
        git(&["-C", path_str(&main_repo), "add", "file.txt"]);
        git(&["-C", path_str(&main_repo), "commit", "-m", "base"]);
        git(&[
            "-C",
            path_str(&main_repo),
            "worktree",
            "add",
            "-b",
            "feature",
            path_str(&worktree),
        ]);

        let mut main = make_project("main");
        main.path = main_repo.to_string_lossy().into_owned();
        let mut outer = make_project("ancestor");
        outer.path = ancestor.to_string_lossy().into_owned();
        let mut child = make_worktree_project("worktree", "main");
        child.path = worktree.to_string_lossy().into_owned();
        child.worktree_info.as_mut().unwrap().worktree_path =
            worktree.to_string_lossy().into_owned();
        let mut data = make_workspace_data();
        data.projects = vec![main, outer, child];
        data.project_order = vec!["main".to_string(), "ancestor".to_string()];
        let workspace = cx.new(|_| Workspace::new(data));

        let error = workspace.update(cx, |ws, _| {
            ws.prepare_project_directory_rename(
                "ancestor",
                renamed.to_string_lossy().into_owned(),
                "renamed-workspace".to_string(),
            )
            .err()
            .expect("registered descendant checkout must block plain directory move")
        });
        assert!(error.contains("linked worktree project"));

        git(&[
            "-C",
            path_str(&main_repo),
            "worktree",
            "remove",
            "--force",
            path_str(&worktree),
        ]);
        let _ = std::fs::remove_dir_all(fixture);
    }

    #[gpui::test]
    fn root_claim_succeeds_after_create_finishes(cx: &mut gpui::TestAppContext) {
        // Once finalize clears the create lease, paths below the checkout can
        // be claimed again rather than remaining wedged forever.
        let mut parent = make_project("parent");
        parent.worktree_ids = vec!["wt1".to_string()];
        let mut data = make_workspace_data();
        data.projects = vec![parent, make_worktree_project("wt1", "parent")];
        data.project_order = vec!["parent".to_string()];
        let workspace = cx.new(|_cx| Workspace::new(data));

        let result = workspace.update(cx, |ws: &mut Workspace, _cx| {
            ws.mark_creating_project("wt1");
            ws.finish_creating_project("wt1");
            ws.ensure_project_path_claim_allowed(Path::new("/tmp/worktrees/wt1/packages/app"))
        });

        assert!(
            result.is_ok(),
            "guard should release once create finishes, got {:?}",
            result.err(),
        );
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(!ws.is_creating_project("wt1"), "creating flag cleared");
        });
    }

    #[test]
    fn clone_target_joins_the_parent_and_the_directory() {
        let target = super::resolve_clone_target("/home/user/projects", "okena").unwrap();
        assert_eq!(target, Path::new("/home/user/projects/okena"));
        // Surrounding whitespace is the user's, not part of the path.
        let target = super::resolve_clone_target("  /home/user/projects ", " okena ").unwrap();
        assert_eq!(target, Path::new("/home/user/projects/okena"));
    }

    #[test]
    fn clone_target_rejects_a_directory_that_is_not_a_plain_name() {
        // A separator or `..` would put the checkout outside the parent the
        // user picked.
        for directory in ["../escape", "nested/dir", "a\\b", ".", "..", "", "   "] {
            assert!(
                super::resolve_clone_target("/home/user", directory).is_err(),
                "expected rejection for {directory:?}"
            );
        }
        assert!(super::resolve_clone_target("", "okena").is_err());
        #[cfg(windows)]
        assert!(super::resolve_clone_target(r"C:\parent", "D:").is_err());
    }

    #[test]
    fn clone_name_falls_back_to_the_directory() {
        assert_eq!(super::clone_project_name("My Repo", "okena"), "My Repo");
        assert_eq!(super::clone_project_name("   ", "okena"), "okena");
        assert_eq!(super::clone_project_name("", " okena "), "okena");
    }

    #[gpui::test]
    fn pending_project_gets_no_layout_until_it_is_finished(cx: &mut gpui::TestAppContext) {
        let workspace = cx.new(|_cx| Workspace::new(make_workspace_data()));

        let id = workspace.update(cx, |ws: &mut Workspace, cx| {
            let id = ws
                .register_pending_project(
                    "Okena".to_string(),
                    "/tmp/okena-clone-target".to_string(),
                    WindowId::Main,
                    cx,
                )
                .expect("registers");
            ws.mark_creating_project(&id);
            id
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let project = ws.project(&id).expect("project exists");
            assert!(project.layout.is_none(), "no layout while the clone runs");
            assert!(project.is_creating, "creating flag mirrored onto the row");
        });

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.finish_pending_project(&id, &HooksConfig::default(), cx);
            ws.finish_creating_project(&id);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let project = ws.project(&id).expect("project exists");
            assert!(project.layout.is_some(), "layout seeded once the dir exists");
            assert!(!project.is_creating);
        });
    }

    #[gpui::test]
    fn rolling_back_a_pending_project_drops_it_everywhere(cx: &mut gpui::TestAppContext) {
        let mut data = make_workspace_data();
        data.folders = vec![crate::state::FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec![],
            folder_color: FolderColor::default(),
        }];
        let workspace = cx.new(|_cx| Workspace::new(data));

        let id = workspace.update(cx, |ws: &mut Workspace, cx| {
            let id = ws
                .register_pending_project(
                    "Okena".to_string(),
                    "/tmp/okena-clone-rollback".to_string(),
                    WindowId::Main,
                    cx,
                )
                .expect("registers");
            ws.mark_creating_project(&id);
            ws.move_project_to_folder(&id, "f1", None, cx);
            id
        });

        // Still creating: the row belongs to an in-flight clone and stays put.
        workspace.update(cx, |ws: &mut Workspace, _cx| ws.remove_pending_project(&id));
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.project(&id).is_some(), "creating rows are not removed");
        });

        workspace.update(cx, |ws: &mut Workspace, _cx| {
            ws.finish_creating_project(&id);
            ws.remove_pending_project(&id);
        });
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.project(&id).is_none());
            assert!(!ws.data().project_order.contains(&id));
            assert!(ws.data().folders[0].project_ids.is_empty());
        });
    }

    #[gpui::test]
    fn a_pending_project_cannot_claim_a_path_reserved_by_a_worktree_create(
        cx: &mut gpui::TestAppContext,
    ) {
        // A clone must inherit the same path-claim guard as any other project:
        // a checkout that is still being created owns its root, so a clone
        // cannot land inside it and race the checkout on the same directory.
        let mut parent = make_project("parent");
        parent.worktree_ids = vec!["wt1".to_string()];
        let mut data = make_workspace_data();
        data.projects = vec![parent, make_worktree_project("wt1", "parent")];
        data.project_order = vec!["parent".to_string()];
        let workspace = cx.new(|_cx| Workspace::new(data));

        let result = workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.mark_creating_project("wt1");
            ws.register_pending_project(
                "Clash".to_string(),
                "/tmp/worktrees/wt1/nested".to_string(),
                WindowId::Main,
                cx,
            )
        });

        assert!(
            result.is_err(),
            "clone target inside an in-flight worktree must be rejected"
        );
    }
}
