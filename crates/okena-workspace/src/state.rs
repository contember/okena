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
    PendingWorktreeClose, ProjectData, WindowBounds, WindowId, WindowState, WorkspaceData,
    WorktreeMetadata,
};

/// Global workspace wrapper for app-wide access (used by quit handler)
#[derive(Clone)]
pub struct GlobalWorkspace(pub Entity<Workspace>);

impl Global for GlobalWorkspace {}

/// GPUI Entity for workspace state.
///
/// Composes focused helper types by ownership. `Workspace` itself is a
/// coordinator — it does not own the raw transient HashSets/HashMaps directly.
///
/// Per slice 03 of the multi-window plan, `FocusManager` is no longer a field
/// here; each `WindowView` owns its own. Action methods that touch focus state
/// take `focus_manager: &mut FocusManager` as a parameter so the focus
/// mutation stays scoped to the window driving the action.
pub struct Workspace {
    pub data: WorkspaceData,
    /// Transient project lifecycle state (creating / closing / removing).
    pub lifecycle: ProjectLifecycleTracker,
    /// Remote-sync coordination state (pending focus, remote snapshots).
    pub remote_sync: RemoteSyncState,
    /// Per-project last-access timestamps, for "recently used" sorting.
    pub access_history: ProjectAccessHistory,
    /// Monotonic counter incremented only on persistent data mutations.
    /// The auto-save observer compares this to skip saves for UI-only changes.
    data_version: u64,
    /// Terminal IDs queued for killing by the app layer (drained by Okena observer).
    pending_terminal_kills: Vec<String>,
}

impl Workspace {
    pub fn new(data: WorkspaceData) -> Self {
        Self {
            data,
            lifecycle: ProjectLifecycleTracker::new(),
            remote_sync: RemoteSyncState::new(),
            access_history: ProjectAccessHistory::new(),
            data_version: 0,
            pending_terminal_kills: Vec::new(),
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
    pub fn replace_data(&mut self, focus_manager: &mut FocusManager, data: WorkspaceData, cx: &mut Context<Self>) {
        self.data = data;
        focus_manager.clear_all();
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

    /// Current folder filter for the targeted window's viewport.
    ///
    /// Routes through `data.window(window_id)` (the lookup pair on
    /// `WorkspaceData`): `WindowId::Main` always returns the main slot,
    /// `WindowId::Extra(uuid)` walks `extra_windows`. Unknown extra ids
    /// (a paint racing a close) yield `None` -- the same default used when
    /// the targeted window has no folder_filter set. Mirrors the silent
    /// no-op shape of the window-scoped setters.
    pub fn active_folder_filter(&self, window_id: WindowId) -> Option<&String> {
        self.data
            .window(window_id)
            .and_then(|w| w.folder_filter.as_ref())
    }

    /// Set the folder filter on the targeted window.
    ///
    /// Delegates to `data.set_folder_filter`, which writes to the targeted
    /// window's `WindowState::folder_filter`. Unknown extra ids are a silent
    /// no-op (the targeted window was just closed).
    ///
    /// Bumps `data_version` because folder_filter is persisted -- the
    /// auto-save observer must trigger.
    pub fn set_folder_filter(
        &mut self,
        window_id: WindowId,
        folder_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.data.set_folder_filter(window_id, folder_id);
        self.notify_data(cx);
    }

    /// Toggle a project's hidden state in the targeted window.
    ///
    /// Delegates to `data.toggle_hidden`, which inserts the project id into
    /// the targeted window's `hidden_project_ids` if absent and removes it if
    /// present. Unknown extra ids are a silent no-op (the targeted window
    /// was just closed).
    ///
    /// Bumps `data_version` because hidden state is persisted -- the
    /// auto-save observer must trigger.
    pub fn toggle_hidden(
        &mut self,
        window_id: WindowId,
        project_id: &str,
        cx: &mut Context<Self>,
    ) {
        self.data.toggle_hidden(window_id, project_id);
        self.notify_data(cx);
    }

    /// Set a single project's column width on the targeted window.
    ///
    /// Delegates to `data.set_project_width`, which writes the
    /// (project_id, width) pair into the targeted window's
    /// `project_widths` map, overwriting any prior value. Unknown extra
    /// ids are a silent no-op (the targeted window was just closed).
    ///
    /// Bumps `data_version` because project widths are persisted -- the
    /// auto-save observer must trigger.
    pub fn set_project_width(
        &mut self,
        window_id: WindowId,
        project_id: &str,
        width: f32,
        cx: &mut Context<Self>,
    ) {
        self.data.set_project_width(window_id, project_id, width);
        self.notify_data(cx);
    }

    /// Set a folder's collapsed state on the targeted window.
    ///
    /// Delegates to `data.set_folder_collapsed`, which inserts
    /// `(folder_id, true)` into the targeted window's `folder_collapsed`
    /// when `collapsed=true`, or removes any existing entry when
    /// `collapsed=false` (the "absence == expanded" runtime convention).
    /// Unknown extra ids are a silent no-op (the targeted window was just
    /// closed).
    ///
    /// Bumps `data_version` because folder-collapsed state is persisted --
    /// the auto-save observer must trigger.
    pub fn set_folder_collapsed(
        &mut self,
        window_id: WindowId,
        folder_id: &str,
        collapsed: bool,
        cx: &mut Context<Self>,
    ) {
        self.data.set_folder_collapsed(window_id, folder_id, collapsed);
        self.notify_data(cx);
    }

    /// Set the OS window bounds on the targeted window.
    ///
    /// Delegates to `data.set_os_bounds`, which writes the
    /// `Option<WindowBounds>` into the targeted window's `os_bounds` slot.
    /// `Some(bounds)` records the latest OS-reported origin/size so the next
    /// launch can restore the window in the same place; `None` clears the
    /// slot (the next launch falls back to the OS default / cascade-offset).
    /// Unknown extra ids are a silent no-op (the targeted window was just
    /// closed -- a debounced bounds-observer firing after a close lands on
    /// a no-op rather than panicking).
    ///
    /// Bumps `data_version` because os_bounds is persisted -- the auto-save
    /// observer must trigger.
    pub fn set_os_bounds(
        &mut self,
        window_id: WindowId,
        bounds: Option<WindowBounds>,
        cx: &mut Context<Self>,
    ) {
        self.data.set_os_bounds(window_id, bounds);
        self.notify_data(cx);
    }

    /// Spawn a fresh extra window onto `extra_windows` and return its id.
    ///
    /// Delegates to `data.spawn_extra_window`, which appends a new
    /// `WindowState` whose `hidden_project_ids` snapshots every current
    /// project ID (so the spawned window's grid is empty at first render --
    /// the user curates it via the per-window "Show in this window" sidebar
    /// action). The returned `WindowId::Extra(uuid)` is the handle the
    /// observer in `src/app/extras.rs` uses to look the corresponding
    /// `Entity<WindowView>` up in `Okena::extra_windows`.
    ///
    /// `spawning_bounds` carries the live OS bounds of the window that
    /// triggered the spawn (read by the action handler from
    /// `gpui::Window::window_bounds()`). When `Some`, the data layer
    /// seeds the new entry's `os_bounds` with origin shifted by `+30,+30`
    /// (the cascade-offset rule); the observer then passes that
    /// `os_bounds` straight into `cx.open_window`'s `window_bounds` so
    /// the OS positions the new window cascade-offset from its parent.
    /// When `None`, `os_bounds` stays `None` and the OS picks a default
    /// position.
    ///
    /// Bumps `data_version` because the new entry is persisted -- the
    /// auto-save observer must trigger so a freshly-spawned extra survives
    /// a quit-during-spawn race.
    pub fn spawn_extra_window(
        &mut self,
        spawning_bounds: Option<WindowBounds>,
        cx: &mut Context<Self>,
    ) -> WindowId {
        let id = self.data.spawn_extra_window(spawning_bounds);
        self.notify_data(cx);
        id
    }

    /// Drop the extra window entry from `extra_windows`.
    ///
    /// Slice 07 cri 3 lifecycle counterpart to `spawn_extra_window` —
    /// the close-flow in `src/app/extras.rs::open_extra_window`'s
    /// `on_window_should_close` hook calls this when the user closes an
    /// extra OS window so the entry stops being persisted (PRD user
    /// story 22). Delegates to `data.close_extra_window`, which retains
    /// every entry whose `state.id != uuid`.
    ///
    /// `WindowId::Main` is a silent no-op at the data layer (main is
    /// the always-present slot). `WindowId::Extra(uuid)` for an unknown
    /// extra (double-close race) is also a silent no-op.
    ///
    /// Bumps `data_version` because removing an entry shrinks the
    /// persisted state — the auto-save observer must trigger so the
    /// next launch (slice 07 cri 6) does not see the closed extra
    /// reappear.
    pub fn close_extra_window(&mut self, id: WindowId, cx: &mut Context<Self>) {
        self.data.close_extra_window(id);
        self.notify_data(cx);
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

    // === Terminal kill queue ===

    pub fn queue_terminal_kills(&mut self, ids: impl IntoIterator<Item = String>) {
        self.pending_terminal_kills.extend(ids);
    }

    pub fn drain_pending_terminal_kills(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_terminal_kills)
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

    /// Get visible projects in order, expanding folders into their contained projects.
    /// When a folder filter is active, only projects from that folder are shown
    /// (top-level projects are hidden). Focused project override still takes priority.
    ///
    /// Per slice 03 of the multi-window plan, callers pass the focused
    /// project id and individual-mode flag from their per-window
    /// `FocusManager` -- visibility is now scoped to the calling window.
    pub fn visible_projects(
        &self,
        window_id: WindowId,
        focused_project_id: Option<&String>,
        focus_individual: bool,
    ) -> Vec<&ProjectData> {
        // Source folder filter / hidden set / widths / collapse from the
        // calling window's persisted WindowState. Fall back to main_window
        // if the targeted extra has been dropped between caller-resolve and
        // read (drop-race safety).
        let window_state = self.data.window(window_id).unwrap_or(&self.data.main_window);
        compute_visible_projects(
            &self.data,
            focused_project_id,
            focus_individual,
            window_state,
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
    pub fn remove_remote_projects(&mut self, focus_manager: &mut FocusManager, connection_id: &str, cx: &mut Context<Self>) {
        let prefix = format!("remote:{}:", connection_id);

        self.data.projects.retain(|p| !p.id.starts_with(&prefix));
        self.data.folders.retain(|f| !f.id.starts_with(&prefix));
        self.data.project_order.retain(|id| !id.starts_with(&prefix));
        self.data.main_window.project_widths.retain(|id, _| !id.starts_with(&prefix));

        self.remote_sync.retain_not_starting_with(&prefix);

        if let Some(focused) = focus_manager.focused_project_id() {
            if focused.starts_with(&prefix) {
                focus_manager.set_focused_project_id(None);
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
        FolderData, LayoutNode, ProjectData, SplitDirection, WindowId, WindowState, Workspace,
        WorkspaceData, WorktreeMetadata,
    };
    use okena_terminal::shell_config::ShellType;
    use okena_core::theme::FolderColor;
    use crate::settings::HooksConfig;
    use std::collections::HashMap;

    fn make_project(id: &str) -> ProjectData {
        ProjectData {
            id: id.to_string(),
            name: format!("Project {}", id),
            path: "/tmp/test".to_string(),
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
        // Per-window viewport model: hidden state lives on
        // `main_window.hidden_project_ids` and is populated explicitly by
        // tests that exercise hidden-project behavior. The legacy
        // `ProjectData.show_in_overview` shortcut has been removed.
        WorkspaceData {
            version: 1,
            projects,
            project_order: order.into_iter().map(String::from).collect(),
            service_panel_heights: HashMap::new(),
            hook_panel_heights: HashMap::new(),
            folders: Vec::new(),
            main_window: WindowState::default(),
            extra_windows: Vec::new(),
        }
    }

    #[test]
    fn test_visible_projects_filters_hidden() {
        let mut data = make_workspace_data(
            vec![make_project("p1"), make_project("p2"), make_project("p3")],
            vec!["p1", "p2", "p3"],
        );
        data.main_window.hidden_project_ids.insert("p2".to_string());
        let ws = Workspace::new(data);

        let visible = ws.visible_projects(WindowId::Main, None, false);
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].id, "p1");
        assert_eq!(visible[1].id, "p3");
    }

    #[test]
    fn test_visible_projects_with_focused_project() {
        let mut data = make_workspace_data(
            vec![make_project("p1"), make_project("p2"), make_project("p3")],
            vec!["p1", "p2", "p3"],
        );
        data.main_window.hidden_project_ids.insert("p3".to_string());
        let ws = Workspace::new(data);

        let mut fm = crate::focus::FocusManager::new();
        fm.set_focused_project_id(Some("p3".to_string()));

        let visible = ws.visible_projects(WindowId::Main, fm.focused_project_id(), fm.is_focus_individual());
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "p3");
    }

    #[test]
    fn test_visible_projects_with_folder() {
        let mut data = make_workspace_data(
            vec![make_project("p1"), make_project("p2")],
            vec!["f1"],
        );
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["p1".to_string(), "p2".to_string()],
            folder_color: FolderColor::default(),
        }];

        let ws = Workspace::new(data);

        let visible = ws.visible_projects(WindowId::Main, None, false);
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].id, "p1");
        assert_eq!(visible[1].id, "p2");
    }

    #[test]
    fn test_projects_by_recency() {
        let data = make_workspace_data(
            vec![make_project("p1"), make_project("p2"), make_project("p3")],
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
        let mut project = make_project("p1");
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
            vec![make_project("p1"), make_project("p2")],
            vec!["f1", "p2"],
        );
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["p1".to_string()],
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
                make_project("p1"), make_project("p2"),
                make_project("p3"), make_project("p4"),
                make_project("p5"),
            ],
            vec!["f1", "f2", "p5"],
        );
        data.folders = vec![
            FolderData {
                id: "f1".to_string(),
                name: "Folder 1".to_string(),
                project_ids: vec!["p1".to_string(), "p2".to_string()],
                    folder_color: FolderColor::default(),
            },
            FolderData {
                id: "f2".to_string(),
                name: "Folder 2".to_string(),
                project_ids: vec!["p3".to_string(), "p4".to_string()],
                    folder_color: FolderColor::default(),
            },
        ];

        let mut ws = Workspace::new(data);

        assert_eq!(ws.visible_projects(WindowId::Main, None, false).len(), 5);

        ws.data.main_window.folder_filter = Some("f1".to_string());
        let visible = ws.visible_projects(WindowId::Main, None, false);
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].id, "p1");
        assert_eq!(visible[1].id, "p2");

        ws.data.main_window.folder_filter = Some("f2".to_string());
        let visible = ws.visible_projects(WindowId::Main, None, false);
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].id, "p3");
        assert_eq!(visible[1].id, "p4");
    }

    #[test]
    fn test_folder_filter_hides_top_level_projects() {
        let mut data = make_workspace_data(
            vec![
                make_project("p1"), make_project("p2"),
                make_project("p3"),
            ],
            vec!["f1", "p3"],
        );
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["p1".to_string(), "p2".to_string()],
            folder_color: FolderColor::default(),
        }];

        let mut ws = Workspace::new(data);
        ws.data.main_window.folder_filter = Some("f1".to_string());

        let visible = ws.visible_projects(WindowId::Main, None, false);
        assert_eq!(visible.len(), 2);
        assert!(visible.iter().all(|p| p.id != "p3"));
    }

    #[test]
    fn test_visible_projects_worktree_focus() {
        let mut p1 = make_project("p1");
        p1.worktree_ids = vec!["w1".to_string(), "w2".to_string()];
        let mut w1 = make_project("w1");
        w1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "p1".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: "branch-w1".to_string(),
        });
        let mut w2 = make_project("w2");
        w2.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "p1".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt2".to_string(),
            branch_name: "branch-w2".to_string(),
        });

        let data = make_workspace_data(
            vec![p1, w1, w2, make_project("p2")],
            vec!["p1", "p2"],
        );
        let ws = Workspace::new(data);
        let mut fm = crate::focus::FocusManager::new();

        fm.set_focused_project_id(Some("p1".to_string()));
        let visible = ws.visible_projects(WindowId::Main, fm.focused_project_id(), fm.is_focus_individual());
        assert_eq!(visible.len(), 3);
        assert_eq!(visible[0].id, "p1");
        assert_eq!(visible[1].id, "w1");
        assert_eq!(visible[2].id, "w2");

        fm.set_focused_project_id(Some("w1".to_string()));
        let visible = ws.visible_projects(WindowId::Main, fm.focused_project_id(), fm.is_focus_individual());
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "w1");

        fm.set_focused_project_id(None);
        let visible = ws.visible_projects(WindowId::Main, fm.focused_project_id(), fm.is_focus_individual());
        assert_eq!(visible.len(), 4);
    }

    #[test]
    fn test_folder_filter_includes_worktree_children() {
        let mut p1 = make_project("p1");
        p1.worktree_ids = vec!["w1".to_string(), "w2".to_string()];
        let mut w1 = make_project("w1");
        w1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "p1".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: "branch-w1".to_string(),
        });
        let mut w2 = make_project("w2");
        w2.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "p1".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt2".to_string(),
            branch_name: "branch-w2".to_string(),
        });

        let mut data = make_workspace_data(
            vec![p1, w1, w2, make_project("p2")],
            vec!["f1", "p2"],
        );
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["p1".to_string()],
            folder_color: FolderColor::default(),
        }];

        let mut ws = Workspace::new(data);

        assert_eq!(ws.visible_projects(WindowId::Main, None, false).len(), 4);

        ws.data.main_window.folder_filter = Some("f1".to_string());
        let visible = ws.visible_projects(WindowId::Main, None, false);
        assert_eq!(visible.len(), 3);
        assert_eq!(visible[0].id, "p1");
        assert_eq!(visible[1].id, "w1");
        assert_eq!(visible[2].id, "w2");
    }

    #[test]
    fn test_folder_filter_worktree_children_not_duplicated() {
        let mut w1 = make_project("w1");
        w1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "p1".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: "branch-w1".to_string(),
        });

        let mut p1 = make_project("p1");
        p1.worktree_ids = vec!["w1".to_string()];

        let mut data = make_workspace_data(
            vec![p1, w1, make_project("p2")],
            vec!["f1", "w1", "p2"],
        );
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["p1".to_string()],
            folder_color: FolderColor::default(),
        }];

        let mut ws = Workspace::new(data);
        ws.data.main_window.folder_filter = Some("f1".to_string());

        let visible = ws.visible_projects(WindowId::Main, None, false);
        assert_eq!(visible.len(), 2);
        assert_eq!(visible.iter().filter(|p| p.id == "w1").count(), 1);
    }

    #[test]
    fn test_worktree_children_ordered_within_folder_section() {
        let mut w1 = make_project("w1");
        w1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "p1".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: "branch-w1".to_string(),
        });

        let mut p1 = make_project("p1");
        p1.worktree_ids = vec!["w1".to_string()];

        let mut data = make_workspace_data(
            vec![p1, make_project("p2"), w1, make_project("p3")],
            vec!["f1", "w1", "f2", "p3"],
        );
        data.folders = vec![
            FolderData {
                id: "f1".to_string(),
                name: "Folder 1".to_string(),
                project_ids: vec!["p1".to_string()],
                    folder_color: FolderColor::default(),
            },
            FolderData {
                id: "f2".to_string(),
                name: "Folder 2".to_string(),
                project_ids: vec!["p2".to_string()],
                    folder_color: FolderColor::default(),
            },
        ];

        let ws = Workspace::new(data);
        let visible = ws.visible_projects(WindowId::Main, None, false);

        assert_eq!(visible.len(), 4);
        assert_eq!(visible[0].id, "p1");
        assert_eq!(visible[1].id, "w1");
        assert_eq!(visible[2].id, "p2");
        assert_eq!(visible[3].id, "p3");
    }

    #[test]
    fn test_worktree_before_parent_folder_in_project_order() {
        let mut w1 = make_project("w1");
        w1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "p2".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: "branch-w1".to_string(),
        });

        let mut p2 = make_project("p2");
        p2.worktree_ids = vec!["w1".to_string()];

        let mut data = make_workspace_data(
            vec![make_project("p1"), p2, w1],
            vec!["w1", "f1", "f2"],
        );
        data.main_window.hidden_project_ids.insert("p2".to_string());
        data.folders = vec![
            FolderData {
                id: "f1".to_string(),
                name: "Folder 1".to_string(),
                project_ids: vec!["p1".to_string()],
                    folder_color: FolderColor::default(),
            },
            FolderData {
                id: "f2".to_string(),
                name: "Folder 2".to_string(),
                project_ids: vec!["p2".to_string()],
                    folder_color: FolderColor::default(),
            },
        ];

        let ws = Workspace::new(data);
        let visible = ws.visible_projects(WindowId::Main, None, false);

        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].id, "p1");
        assert_eq!(visible[1].id, "w1");
        assert_eq!(visible.iter().filter(|p| p.id == "w1").count(), 1);
    }

    #[test]
    fn test_worktree_children_ordered_when_parent_hidden() {
        let mut w1 = make_project("w1");
        w1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "p1".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: "branch-w1".to_string(),
        });

        let mut p1 = make_project("p1");
        p1.worktree_ids = vec!["w1".to_string()];

        let mut data = make_workspace_data(
            vec![p1, make_project("p2"), w1],
            vec!["f1", "w1", "f2"],
        );
        data.main_window.hidden_project_ids.insert("p1".to_string());
        data.folders = vec![
            FolderData {
                id: "f1".to_string(),
                name: "Folder 1".to_string(),
                project_ids: vec!["p1".to_string()],
                    folder_color: FolderColor::default(),
            },
            FolderData {
                id: "f2".to_string(),
                name: "Folder 2".to_string(),
                project_ids: vec!["p2".to_string()],
                    folder_color: FolderColor::default(),
            },
        ];

        let ws = Workspace::new(data);
        let visible = ws.visible_projects(WindowId::Main, None, false);

        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].id, "w1");
        assert_eq!(visible[1].id, "p2");
    }

    #[test]
    fn test_worktree_child_in_folder_not_duplicated() {
        let mut w1 = make_project("w1");
        w1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "p1".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: "branch-w1".to_string(),
        });

        let mut data = make_workspace_data(
            vec![make_project("p1"), w1, make_project("p2")],
            vec!["f1", "f2"],
        );
        data.folders = vec![
            FolderData {
                id: "f1".to_string(),
                name: "Folder 1".to_string(),
                project_ids: vec!["p1".to_string(), "w1".to_string()],
                    folder_color: FolderColor::default(),
            },
            FolderData {
                id: "f2".to_string(),
                name: "Folder 2".to_string(),
                project_ids: vec!["p2".to_string()],
                    folder_color: FolderColor::default(),
            },
        ];

        let ws = Workspace::new(data);
        let visible = ws.visible_projects(WindowId::Main, None, false);

        assert_eq!(visible.len(), 3);
        assert_eq!(visible[0].id, "p1");
        assert_eq!(visible[1].id, "w1");
        assert_eq!(visible[2].id, "p2");
        assert_eq!(visible.iter().filter(|p| p.id == "w1").count(), 1);
    }

    #[test]
    fn test_orphan_worktree_shown_when_parent_not_in_result() {
        let mut w1 = make_project("w1");
        w1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "p1".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: "branch-w1".to_string(),
        });

        let mut data = make_workspace_data(
            vec![make_project("p1"), w1],
            vec!["p1", "w1"],
        );
        data.main_window.hidden_project_ids.insert("p1".to_string());
        let ws = Workspace::new(data);

        let visible = ws.visible_projects(WindowId::Main, None, false);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "w1");
    }

    #[test]
    fn test_folder_filter_with_focus_override() {
        let mut data = make_workspace_data(
            vec![
                make_project("p1"), make_project("p2"),
                make_project("p3"),
            ],
            vec!["f1", "p3"],
        );
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["p1".to_string(), "p2".to_string()],
            folder_color: FolderColor::default(),
        }];

        let mut ws = Workspace::new(data);
        ws.data.main_window.folder_filter = Some("f1".to_string());

        let mut fm = crate::focus::FocusManager::new();
        fm.set_focused_project_id(Some("p3".to_string()));

        let visible = ws.visible_projects(WindowId::Main, fm.focused_project_id(), fm.is_focus_individual());
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "p3");
    }

    #[test]
    fn test_visible_projects_includes_worktree_children() {
        let mut parent = make_project("parent");
        parent.worktree_ids = vec!["wt1".to_string(), "wt2".to_string()];
        let mut wt1 = make_project("wt1");
        wt1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "parent".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: String::new(),
        });
        let mut wt2 = make_project("wt2");
        wt2.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "parent".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt2".to_string(),
            branch_name: String::new(),
        });
        let data = make_workspace_data(vec![parent, wt1, wt2], vec!["parent"]);
        let ws = Workspace::new(data);

        let visible = ws.visible_projects(WindowId::Main, None, false);
        assert_eq!(visible.len(), 3);
        assert_eq!(visible[0].id, "parent");
        assert_eq!(visible[1].id, "wt1");
        assert_eq!(visible[2].id, "wt2");
    }

    #[test]
    fn test_visible_projects_worktree_children_in_folder() {
        let mut parent = make_project("parent");
        parent.worktree_ids = vec!["wt1".to_string()];
        let mut wt1 = make_project("wt1");
        wt1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "parent".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: String::new(),
        });
        let other = make_project("other");
        let mut data = make_workspace_data(vec![parent, wt1, other], vec!["f1", "other"]);
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["parent".to_string()],
            folder_color: FolderColor::default(),
        }];
        let ws = Workspace::new(data);

        let visible = ws.visible_projects(WindowId::Main, None, false);
        assert_eq!(visible.len(), 3);
        assert_eq!(visible[0].id, "parent");
        assert_eq!(visible[1].id, "wt1");
        assert_eq!(visible[2].id, "other");
    }

    #[test]
    fn test_focus_parent_shows_parent_and_worktrees() {
        let mut parent = make_project("parent");
        parent.worktree_ids = vec!["wt1".to_string(), "wt2".to_string()];
        let mut wt1 = make_project("wt1");
        wt1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "parent".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: String::new(),
        });
        let mut wt2 = make_project("wt2");
        wt2.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "parent".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt2".to_string(),
            branch_name: String::new(),
        });
        let data = make_workspace_data(vec![parent, wt1, wt2], vec!["parent"]);
        let ws = Workspace::new(data);
        let mut fm = crate::focus::FocusManager::new();
        fm.set_focused_project_id(Some("parent".to_string()));

        let visible = ws.visible_projects(WindowId::Main, fm.focused_project_id(), fm.is_focus_individual());
        assert_eq!(visible.len(), 3);
        assert_eq!(visible[0].id, "parent");
        assert_eq!(visible[1].id, "wt1");
        assert_eq!(visible[2].id, "wt2");
    }

    #[test]
    fn test_focus_worktree_shows_only_worktree() {
        let mut parent = make_project("parent");
        parent.worktree_ids = vec!["wt1".to_string(), "wt2".to_string()];
        let mut wt1 = make_project("wt1");
        wt1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "parent".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: String::new(),
        });
        let mut wt2 = make_project("wt2");
        wt2.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "parent".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt2".to_string(),
            branch_name: String::new(),
        });
        let data = make_workspace_data(vec![parent, wt1, wt2], vec!["parent"]);
        let ws = Workspace::new(data);
        let mut fm = crate::focus::FocusManager::new();
        fm.set_focused_project_id(Some("wt1".to_string()));

        let visible = ws.visible_projects(WindowId::Main, fm.focused_project_id(), fm.is_focus_individual());
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "wt1");
    }

    #[test]
    fn test_focus_parent_individual_shows_only_parent() {
        let mut parent = make_project("parent");
        parent.worktree_ids = vec!["wt1".to_string(), "wt2".to_string()];
        let mut wt1 = make_project("wt1");
        wt1.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "parent".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt1".to_string(),
            branch_name: String::new(),
        });
        let mut wt2 = make_project("wt2");
        wt2.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "parent".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/wt2".to_string(),
            branch_name: String::new(),
        });
        let data = make_workspace_data(vec![parent, wt1, wt2], vec!["parent"]);
        let ws = Workspace::new(data);
        let mut fm = crate::focus::FocusManager::new();

        fm.set_focused_project_id_individual(Some("parent".to_string()));
        let visible = ws.visible_projects(WindowId::Main, fm.focused_project_id(), fm.is_focus_individual());
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "parent");

        fm.set_focused_project_id(Some("parent".to_string()));
        let visible = ws.visible_projects(WindowId::Main, fm.focused_project_id(), fm.is_focus_individual());
        assert_eq!(visible.len(), 3);
    }

    #[test]
    fn visible_projects_reads_folder_filter_from_main_window() {
        // visible_projects must source the folder filter from
        // `data.main_window.folder_filter` (the persisted, per-window
        // viewport model). A regression that re-introduces a transient
        // override on the entity would see None and return all 3 projects
        // instead of just f1's 2.
        let mut data = make_workspace_data(
            vec![make_project("p1"), make_project("p2"), make_project("p3")],
            vec!["f1", "p3"],
        );
        data.folders = vec![FolderData {
            id: "f1".to_string(),
            name: "Folder".to_string(),
            project_ids: vec!["p1".to_string(), "p2".to_string()],
            folder_color: FolderColor::default(),
        }];
        data.main_window.folder_filter = Some("f1".to_string());
        let ws = Workspace::new(data);

        let visible = ws.visible_projects(WindowId::Main, None, false);
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].id, "p1");
        assert_eq!(visible[1].id, "p2");
    }
}

#[cfg(test)]
mod gpui_tests {
    use gpui::AppContext as _;
    use crate::state::{HookTerminalEntry, HookTerminalStatus, LayoutNode, ProjectData, WindowBounds, WindowId, WindowState, Workspace, WorkspaceData};
    use crate::settings::HooksConfig;
    use okena_terminal::shell_config::ShellType;
    use okena_core::theme::FolderColor;
    use std::collections::HashMap;

    fn make_project(id: &str) -> ProjectData {
        ProjectData {
            id: id.to_string(),
            name: format!("Project {}", id),
            path: "/tmp/test".to_string(),
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
        // Per-window viewport model: hidden state lives on
        // `main_window.hidden_project_ids` and is set explicitly by tests
        // that exercise hidden-project behavior.
        WorkspaceData {
            version: 1,
            projects,
            project_order: order.into_iter().map(String::from).collect(),
            service_panel_heights: HashMap::new(),
            hook_panel_heights: HashMap::new(),
            folders: vec![],
            main_window: WindowState::default(),
            extra_windows: Vec::new(),
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
        use crate::focus::FocusManager;

        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));
        let mut fm = FocusManager::new();

        fm.set_focused_project_id(Some("p1".to_string()));
        assert!(fm.focused_project_id().is_some());

        let new_data = make_workspace_data(vec![make_project("p2")], vec!["p2"]);
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.replace_data(&mut fm, new_data, cx);
        });

        assert!(fm.focused_project_id().is_none());
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data().projects.len(), 1);
            assert_eq!(ws.data().projects[0].id, "p2");
        });
    }

    #[gpui::test]
    fn test_visible_projects_gpui(cx: &mut gpui::TestAppContext) {
        let p1 = make_project("p1");
        let p2 = make_project("p2");
        let p3 = make_project("p3");
        let mut data = make_workspace_data(vec![p1, p2, p3], vec!["p1", "p2", "p3"]);
        data.main_window.hidden_project_ids.insert("p1".to_string());
        data.main_window.hidden_project_ids.insert("p3".to_string());
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let visible = ws.visible_projects(WindowId::Main, None, false);
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0].id, "p2");
        });

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_project_overview_visibility(WindowId::Main, "p1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let visible = ws.visible_projects(WindowId::Main, None, false);
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
            folder_color: FolderColor::default(),
        });
        data.folders.push(FolderData {
            id: "remote:conn2:folder2".to_string(),
            name: "Server 2".to_string(),
            project_ids: vec!["remote:conn2:p1".to_string()],
            folder_color: FolderColor::default(),
        });

        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.remove_remote_projects(&mut crate::focus::FocusManager::new(), "conn1", cx);
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
        let remote1 = make_remote_project("remote:conn1:p1", "conn1");
        let remote2 = make_remote_project("remote:conn1:p2", "conn1");

        let mut data = make_workspace_data(
            vec![local, remote1, remote2],
            vec!["local1", "remote:conn1:folder1"],
        );
        data.main_window.hidden_project_ids.insert("remote:conn1:p2".to_string());
        data.folders.push(FolderData {
            id: "remote:conn1:folder1".to_string(),
            name: "Server 1".to_string(),
            project_ids: vec!["remote:conn1:p1".to_string(), "remote:conn1:p2".to_string()],
            folder_color: FolderColor::default(),
        });

        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let visible = ws.visible_projects(WindowId::Main, None, false);
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

    #[gpui::test]
    fn set_folder_filter_main_writes_to_data(cx: &mut gpui::TestAppContext) {
        // Window-scoped entity setter: WindowId::Main writes to
        // data.main_window.folder_filter (the persisted source of truth).
        // data_version bumps because folder_filter is persisted -- the
        // auto-save observer must trigger.
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.set_folder_filter(WindowId::Main, Some("f1".to_string()), cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data().main_window.folder_filter.as_deref(), Some("f1"));
            assert_eq!(ws.active_folder_filter(WindowId::Main).map(|s| s.as_str()), Some("f1"));
            assert_eq!(ws.data_version(), 1);
        });
    }

    #[gpui::test]
    fn set_folder_filter_main_clears_with_none(cx: &mut gpui::TestAppContext) {
        // Passing None must clear the data-layer filter. Without this,
        // callers wanting to exit folder-filter mode (e.g. ClearFocus) would
        // have no API path -- the setter would be write-only.
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.set_folder_filter(WindowId::Main, Some("f1".to_string()), cx);
            ws.set_folder_filter(WindowId::Main, None, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.data().main_window.folder_filter.is_none());
            assert!(ws.active_folder_filter(WindowId::Main).is_none());
        });
    }

    #[gpui::test]
    fn set_folder_filter_extra_writes_only_to_targeted_window(cx: &mut gpui::TestAppContext) {
        // Targeting an extra window writes to that extra's WindowState only.
        // The main window's filter is untouched. Defends against a regression
        // that ignores the WindowId and writes to main, or scatters the write
        // across all windows.
        let mut data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let extra = WindowState::default();
        let extra_id = extra.id;
        data.extra_windows.push(extra);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.set_folder_filter(WindowId::Extra(extra_id), Some("f1".to_string()), cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let extra_w = ws.data().window(WindowId::Extra(extra_id)).unwrap();
            assert_eq!(extra_w.folder_filter.as_deref(), Some("f1"));
            assert!(ws.data().main_window.folder_filter.is_none());
            assert!(ws.active_folder_filter(WindowId::Main).is_none());
        });
    }

    #[gpui::test]
    fn set_folder_filter_unknown_extra_is_silent_noop(cx: &mut gpui::TestAppContext) {
        // The "targeted window was just closed" race: the entity setter
        // delegates to data.set_folder_filter, which silently no-ops on a
        // missing extra id. Pin the contract so a future refactor that swaps
        // the data layer to a panicking variant fails here loudly.
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let unknown = uuid::Uuid::new_v4();
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.set_folder_filter(WindowId::Extra(unknown), Some("f1".to_string()), cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.data().main_window.folder_filter.is_none());
            assert!(ws.active_folder_filter(WindowId::Main).is_none());
            assert!(ws.data().extra_windows.is_empty());
        });
    }

    #[gpui::test]
    fn toggle_hidden_main_inserts_when_absent(cx: &mut gpui::TestAppContext) {
        // Window-scoped entity setter: WindowId::Main + previously-visible
        // project lands the project's id in main_window.hidden_project_ids.
        // data_version bumps because hidden state is persisted -- the
        // auto-save observer must trigger.
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_hidden(WindowId::Main, "p1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.data().main_window.hidden_project_ids.contains("p1"));
            assert_eq!(ws.data_version(), 1);
        });
    }

    #[gpui::test]
    fn toggle_hidden_main_removes_when_present(cx: &mut gpui::TestAppContext) {
        // The "Show Project" leg: a previously-hidden project becomes visible
        // again after toggling. Pinned separately from the insert leg because
        // a future refactor that always-inserts would leave projects stuck
        // hidden after the user clicks "Show Project".
        let mut data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        data.main_window.hidden_project_ids.insert("p1".to_string());
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_hidden(WindowId::Main, "p1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(!ws.data().main_window.hidden_project_ids.contains("p1"));
            assert_eq!(ws.data_version(), 1);
        });
    }

    #[gpui::test]
    fn toggle_hidden_extra_writes_only_to_targeted_window(cx: &mut gpui::TestAppContext) {
        // Targeting an extra window writes to that extra's WindowState only.
        // Main and the sibling extra are untouched. Defends against a
        // regression that ignores the WindowId, scatters the write across
        // every window, or always writes to main.
        let mut data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let extra_a = WindowState::default();
        let extra_a_id = extra_a.id;
        let extra_b = WindowState::default();
        let extra_b_id = extra_b.id;
        data.extra_windows.push(extra_a);
        data.extra_windows.push(extra_b);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_hidden(WindowId::Extra(extra_a_id), "p1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let a = ws.data().window(WindowId::Extra(extra_a_id)).unwrap();
            let b = ws.data().window(WindowId::Extra(extra_b_id)).unwrap();
            assert!(a.hidden_project_ids.contains("p1"));
            assert!(!b.hidden_project_ids.contains("p1"));
            assert!(!ws.data().main_window.hidden_project_ids.contains("p1"));
        });
    }

    #[gpui::test]
    fn toggle_hidden_unknown_extra_is_silent_noop(cx: &mut gpui::TestAppContext) {
        // The "targeted window was just closed" race: the entity setter
        // delegates to data.toggle_hidden, which silently no-ops on a
        // missing extra id. Pin the contract so a future refactor that swaps
        // the data layer to a panicking variant fails here loudly.
        let mut data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let extra = WindowState::default();
        let extra_id = extra.id;
        data.extra_windows.push(extra);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let unknown = uuid::Uuid::new_v4();
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_hidden(WindowId::Extra(unknown), "p1", cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.data().main_window.hidden_project_ids.is_empty());
            let kept = ws.data().window(WindowId::Extra(extra_id)).unwrap();
            assert!(kept.hidden_project_ids.is_empty());
        });
    }

    #[gpui::test]
    fn set_project_width_main_writes_to_data(cx: &mut gpui::TestAppContext) {
        // Window-scoped entity setter: WindowId::Main writes the
        // (project_id, width) pair into data.main_window.project_widths
        // (the persisted source of truth). data_version bumps because
        // project widths are persisted -- the auto-save observer must
        // trigger.
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.set_project_width(WindowId::Main, "p1", 0.42, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data().main_window.project_widths.get("p1").copied(), Some(0.42));
            assert_eq!(ws.data_version(), 1);
        });
    }

    #[gpui::test]
    fn set_project_width_main_overwrites_existing_value(cx: &mut gpui::TestAppContext) {
        // Re-setting a width for the same project must replace the prior
        // value, not silently keep the first write. Without this, every
        // column-resize after the first would be a silent no-op (the user
        // would see the column "snap back" once they tried to resize the
        // same column twice). Pinned via two consecutive sets.
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.set_project_width(WindowId::Main, "p1", 0.25, cx);
            ws.set_project_width(WindowId::Main, "p1", 0.75, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data().main_window.project_widths.get("p1").copied(), Some(0.75));
            assert_eq!(ws.data_version(), 2);
        });
    }

    #[gpui::test]
    fn set_project_width_extra_writes_only_to_targeted_window(cx: &mut gpui::TestAppContext) {
        // Targeting an extra window writes to that extra's WindowState only.
        // Main and the sibling extra are untouched. Defends against a
        // regression that ignores the WindowId, scatters the write across
        // every window, or always writes to main.
        let mut data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let extra_a = WindowState::default();
        let extra_a_id = extra_a.id;
        let extra_b = WindowState::default();
        let extra_b_id = extra_b.id;
        data.extra_windows.push(extra_a);
        data.extra_windows.push(extra_b);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.set_project_width(WindowId::Extra(extra_a_id), "p1", 0.42, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let a = ws.data().window(WindowId::Extra(extra_a_id)).unwrap();
            let b = ws.data().window(WindowId::Extra(extra_b_id)).unwrap();
            assert_eq!(a.project_widths.get("p1").copied(), Some(0.42));
            assert!(b.project_widths.is_empty());
            assert!(ws.data().main_window.project_widths.is_empty());
        });
    }

    #[gpui::test]
    fn set_project_width_unknown_extra_is_silent_noop(cx: &mut gpui::TestAppContext) {
        // The "targeted window was just closed" race: the entity setter
        // delegates to data.set_project_width, which silently no-ops on a
        // missing extra id. Pin the contract so a future refactor that swaps
        // the data layer to a panicking variant fails here loudly.
        let mut data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let extra = WindowState::default();
        let extra_id = extra.id;
        data.extra_windows.push(extra);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let unknown = uuid::Uuid::new_v4();
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.set_project_width(WindowId::Extra(unknown), "p1", 0.42, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.data().main_window.project_widths.is_empty());
            let kept = ws.data().window(WindowId::Extra(extra_id)).unwrap();
            assert!(kept.project_widths.is_empty());
        });
    }

    #[gpui::test]
    fn set_folder_collapsed_main_inserts_when_true(cx: &mut gpui::TestAppContext) {
        // Window-scoped entity setter: WindowId::Main + collapsed=true inserts
        // (folder_id, true) into data.main_window.folder_collapsed (the
        // persisted source of truth). data_version bumps because
        // folder-collapsed state is persisted -- the auto-save observer must
        // trigger.
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.set_folder_collapsed(WindowId::Main, "f1", true, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data().main_window.folder_collapsed.get("f1"), Some(&true));
            assert_eq!(ws.data_version(), 1);
        });
    }

    #[gpui::test]
    fn set_folder_collapsed_main_removes_when_false(cx: &mut gpui::TestAppContext) {
        // The "absence == expanded" runtime convention: collapsed=false on a
        // previously-collapsed folder removes the entry, NOT inserts
        // Some(false). Defends against a regression that uses unconditional
        // insert (which would leave Some(false) tombstones bloating the on-
        // disk shape over time).
        let mut data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        data.main_window.folder_collapsed.insert("f1".to_string(), true);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.set_folder_collapsed(WindowId::Main, "f1", false, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(!ws.data().main_window.folder_collapsed.contains_key("f1"));
            assert_eq!(ws.data_version(), 1);
        });
    }

    #[gpui::test]
    fn set_folder_collapsed_extra_writes_only_to_targeted_window(cx: &mut gpui::TestAppContext) {
        // Targeting an extra window writes to that extra's WindowState only.
        // Main and the sibling extra are untouched. Defends against a
        // regression that ignores the WindowId, scatters the write across
        // every window, or always writes to main.
        let mut data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let extra_a = WindowState::default();
        let extra_a_id = extra_a.id;
        let extra_b = WindowState::default();
        let extra_b_id = extra_b.id;
        data.extra_windows.push(extra_a);
        data.extra_windows.push(extra_b);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.set_folder_collapsed(WindowId::Extra(extra_a_id), "f1", true, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let a = ws.data().window(WindowId::Extra(extra_a_id)).unwrap();
            let b = ws.data().window(WindowId::Extra(extra_b_id)).unwrap();
            assert_eq!(a.folder_collapsed.get("f1"), Some(&true));
            assert!(b.folder_collapsed.is_empty());
            assert!(ws.data().main_window.folder_collapsed.is_empty());
        });
    }

    #[gpui::test]
    fn set_folder_collapsed_unknown_extra_is_silent_noop(cx: &mut gpui::TestAppContext) {
        // The "targeted window was just closed" race: the entity setter
        // delegates to data.set_folder_collapsed, which silently no-ops on a
        // missing extra id. Pin the contract so a future refactor that swaps
        // the data layer to a panicking variant fails here loudly.
        let mut data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let extra = WindowState::default();
        let extra_id = extra.id;
        data.extra_windows.push(extra);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let unknown = uuid::Uuid::new_v4();
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.set_folder_collapsed(WindowId::Extra(unknown), "f1", true, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.data().main_window.folder_collapsed.is_empty());
            let kept = ws.data().window(WindowId::Extra(extra_id)).unwrap();
            assert!(kept.folder_collapsed.is_empty());
        });
    }

    #[gpui::test]
    fn set_os_bounds_main_writes_to_data(cx: &mut gpui::TestAppContext) {
        // Window-scoped entity setter: WindowId::Main + Some(bounds) writes
        // to data.main_window.os_bounds (the persisted source of truth).
        // data_version bumps because os_bounds is persisted -- the auto-save
        // observer must trigger.
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let bounds = WindowBounds {
            origin_x: 100.0,
            origin_y: 50.0,
            width: 1280.0,
            height: 800.0,
        };
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.set_os_bounds(WindowId::Main, Some(bounds), cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data().main_window.os_bounds, Some(bounds));
            assert_eq!(ws.data_version(), 1);
        });
    }

    #[gpui::test]
    fn set_os_bounds_main_clears_with_none(cx: &mut gpui::TestAppContext) {
        // Passing None must clear the bounds. Without this leg, callers
        // wanting to forget a window's last position would have no API path
        // through the entity. Pinned at the entity layer because the
        // asymmetric set/clear contract is part of the integration surface
        // runtime code touches; data_version bumps even on the clear.
        let mut data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        data.main_window.os_bounds = Some(WindowBounds {
            origin_x: 0.0,
            origin_y: 0.0,
            width: 800.0,
            height: 600.0,
        });
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.set_os_bounds(WindowId::Main, None, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.data().main_window.os_bounds.is_none());
            assert_eq!(ws.data_version(), 1);
        });
    }

    #[gpui::test]
    fn set_os_bounds_extra_writes_only_to_targeted_window(cx: &mut gpui::TestAppContext) {
        // Targeting an extra window writes to that extra's WindowState only.
        // Main and the sibling extra are untouched. Defends against a
        // regression that ignores the WindowId, scatters the write across
        // every window, or always writes to main.
        let mut data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let extra_a = WindowState::default();
        let extra_a_id = extra_a.id;
        let extra_b = WindowState::default();
        let extra_b_id = extra_b.id;
        data.extra_windows.push(extra_a);
        data.extra_windows.push(extra_b);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let bounds = WindowBounds {
            origin_x: 200.0,
            origin_y: 150.0,
            width: 1024.0,
            height: 768.0,
        };
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.set_os_bounds(WindowId::Extra(extra_a_id), Some(bounds), cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let a = ws.data().window(WindowId::Extra(extra_a_id)).unwrap();
            let b = ws.data().window(WindowId::Extra(extra_b_id)).unwrap();
            assert_eq!(a.os_bounds, Some(bounds));
            assert!(b.os_bounds.is_none());
            assert!(ws.data().main_window.os_bounds.is_none());
        });
    }

    #[gpui::test]
    fn set_os_bounds_unknown_extra_is_silent_noop(cx: &mut gpui::TestAppContext) {
        // The "targeted window was just closed" race: the entity setter
        // delegates to data.set_os_bounds, which silently no-ops on a
        // missing extra id. Pin the contract so a future refactor that swaps
        // the data layer to a panicking variant fails here loudly.
        let mut data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let extra = WindowState::default();
        let extra_id = extra.id;
        data.extra_windows.push(extra);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let unknown = uuid::Uuid::new_v4();
        let bounds = WindowBounds {
            origin_x: 1.0,
            origin_y: 2.0,
            width: 3.0,
            height: 4.0,
        };
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.set_os_bounds(WindowId::Extra(unknown), Some(bounds), cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.data().main_window.os_bounds.is_none());
            let kept = ws.data().window(WindowId::Extra(extra_id)).unwrap();
            assert!(kept.os_bounds.is_none());
        });
    }

    #[gpui::test]
    fn spawn_extra_window_pushes_entry_and_bumps_version(cx: &mut gpui::TestAppContext) {
        // Wrapper contract: a single call pushes exactly one entry onto
        // `extra_windows`, returns a `WindowId::Extra(uuid)` whose uuid
        // matches the pushed entry's `state.id`, and bumps `data_version`
        // by one so the auto-save observer triggers. Pinned at the entity
        // layer because both halves -- the data-layer push and the version
        // bump -- are part of the spawn contract the upcoming `NewWindow`
        // action handler relies on.
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let returned = workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.spawn_extra_window(None, cx)
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data().extra_windows.len(), 1);
            let pushed = &ws.data().extra_windows[0];
            assert_eq!(returned, WindowId::Extra(pushed.id));
            assert_eq!(ws.data_version(), 1);
        });
    }

    #[gpui::test]
    fn spawn_extra_window_snapshot_hides_every_current_project(cx: &mut gpui::TestAppContext) {
        // Wrapper-boundary regression defense: a future refactor that
        // re-implemented the wrapper inline (instead of delegating to
        // `data.spawn_extra_window`) could drop the snapshot semantic and
        // produce a window whose grid renders every project on first
        // open -- defeating PRD line 26 ("a new window to start empty"). Pin
        // the snapshot contract at the entity layer too so a stale wrapper
        // surfaces here, not just in the data-layer test.
        let data = make_workspace_data(
            vec![make_project("p1"), make_project("p2"), make_project("p3")],
            vec!["p1", "p2", "p3"],
        );
        let workspace = cx.new(|_cx| Workspace::new(data));

        let id = workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.spawn_extra_window(None, cx)
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let spawned = ws.data().window(id).unwrap();
            assert!(spawned.hidden_project_ids.contains("p1"));
            assert!(spawned.hidden_project_ids.contains("p2"));
            assert!(spawned.hidden_project_ids.contains("p3"));
            assert_eq!(spawned.hidden_project_ids.len(), 3);
        });
    }

    #[gpui::test]
    fn spawn_extra_window_two_calls_produce_distinct_extras_and_two_version_bumps(
        cx: &mut gpui::TestAppContext,
    ) {
        // Per-call distinct ids + per-call data_version bumps. Pins the
        // "Cmd+Shift+N twice opens two windows" contract at the entity
        // layer: defends against (a) a hypothetical wrapper that coalesces
        // duplicate spawns by hidden-set contents (two windows that both
        // start fully hidden are still two distinct windows), and (b) a
        // wrapper that lazily defers the version bump (which would let the
        // auto-save observer miss the second spawn until something else
        // mutated the data).
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let (first, second) = workspace.update(cx, |ws: &mut Workspace, cx| {
            let a = ws.spawn_extra_window(None, cx);
            let b = ws.spawn_extra_window(None, cx);
            (a, b)
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_ne!(first, second);
            assert_eq!(ws.data().extra_windows.len(), 2);
            assert!(ws.data().window(first).is_some());
            assert!(ws.data().window(second).is_some());
            assert_eq!(ws.data_version(), 2);
        });
    }

    #[gpui::test]
    fn spawn_extra_window_threads_spawning_bounds_into_cascade_offset(
        cx: &mut gpui::TestAppContext,
    ) {
        // Wrapper threads `spawning_bounds: Option<WindowBounds>` into the
        // data layer, which seeds os_bounds with the +30,+30 cascade. This
        // test pins the entity-layer threading -- a future refactor that
        // dropped the parameter (e.g. went back to the no-args wrapper)
        // would surface here as a missing os_bounds on the spawned entry,
        // independent of the data-layer's `spawn_extra_window_with_
        // spawning_bounds_cascades_origin_by_30_30_preserves_size` test.
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));
        let spawning = WindowBounds {
            origin_x: 50.0,
            origin_y: 75.0,
            width: 1024.0,
            height: 768.0,
        };

        let id = workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.spawn_extra_window(Some(spawning), cx)
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let spawned = ws.data().window(id).unwrap();
            let bounds = spawned.os_bounds.expect("cascade-offset os_bounds");
            assert_eq!(bounds.origin_x, 80.0);
            assert_eq!(bounds.origin_y, 105.0);
            assert_eq!(bounds.width, 1024.0);
            assert_eq!(bounds.height, 768.0);
        });
    }

    #[gpui::test]
    fn close_extra_window_drops_targeted_entry_and_bumps_version(cx: &mut gpui::TestAppContext) {
        // Slice 07 cri 3: the entity wrapper for close-extra delegates to
        // `data.close_extra_window` and bumps `data_version` so the auto-
        // save observer captures the shrunk `extra_windows` Vec. Without
        // the version bump, a closed extra would reappear on the next
        // launch (cri 6 would silently regress). Pin both halves: the
        // targeted entry is gone AND the version moved.
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let (id_a, id_b) = workspace.update(cx, |ws: &mut Workspace, cx| {
            let a = ws.spawn_extra_window(None, cx);
            let b = ws.spawn_extra_window(None, cx);
            (a, b)
        });
        let after_spawn_version = workspace.read_with(cx, |ws: &Workspace, _cx| ws.data_version());

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.close_extra_window(id_a, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.data().window(id_a).is_none(), "closed entry is gone");
            assert!(ws.data().window(id_b).is_some(), "sibling survives");
            assert_eq!(ws.data().extra_windows.len(), 1);
            assert_eq!(
                ws.data_version(),
                after_spawn_version + 1,
                "version bumps so auto-save fires"
            );
        });
    }

    #[gpui::test]
    fn close_extra_window_main_does_not_remove_main_state(cx: &mut gpui::TestAppContext) {
        // PRD line 53: main is the always-present slot; closing main quits
        // the app via `LastWindowClosed`, it does not delete persisted
        // main state. Targeting `WindowId::Main` at the wrapper must
        // leave main_window's per-window state intact even if a future
        // caller routes a close event through here unconditionally.
        let mut data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        data.main_window.hidden_project_ids.insert("p1".to_string());
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.close_extra_window(WindowId::Main, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.data().main_window.hidden_project_ids.contains("p1"));
        });
    }

    #[gpui::test]
    fn active_folder_filter_main_reads_main_windows_folder_filter(cx: &mut gpui::TestAppContext) {
        // Source-of-truth contract: targeting `WindowId::Main` reads from
        // `data.main_window.folder_filter` (the persisted, per-window model).
        // This fixture writes the filter directly to main_window via a
        // WorkspaceData mutation -- never through the entity setter -- and
        // asserts the getter surfaces it. Defends against a regression that
        // re-introduces a transient cache field on the entity.
        let mut data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        data.main_window.folder_filter = Some("f1".to_string());
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.active_folder_filter(WindowId::Main).map(|s| s.as_str()), Some("f1"));
        });
    }

    #[gpui::test]
    fn active_folder_filter_extra_reads_targeted_extras_folder_filter(cx: &mut gpui::TestAppContext) {
        // Per-window viewport model: targeting `WindowId::Extra(uuid)` reads
        // from that extra's `WindowState::folder_filter` (NOT main's). The
        // fixture pre-populates main + a sibling extra with their own
        // distinct filters so a regression that ignores window_id and
        // unconditionally returns main's filter, scatters across extras,
        // or routes through the wrong slot would surface here.
        let mut data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        data.main_window.folder_filter = Some("main_folder".to_string());
        let mut extra_a = WindowState::default();
        extra_a.folder_filter = Some("extra_a_folder".to_string());
        let extra_a_id = extra_a.id;
        let mut extra_b = WindowState::default();
        extra_b.folder_filter = Some("extra_b_folder".to_string());
        let extra_b_id = extra_b.id;
        data.extra_windows = vec![extra_a, extra_b];
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(
                ws.active_folder_filter(WindowId::Extra(extra_a_id)).map(|s| s.as_str()),
                Some("extra_a_folder"),
            );
            assert_eq!(
                ws.active_folder_filter(WindowId::Extra(extra_b_id)).map(|s| s.as_str()),
                Some("extra_b_folder"),
            );
            // Main is unchanged by the extras' reads.
            assert_eq!(
                ws.active_folder_filter(WindowId::Main).map(|s| s.as_str()),
                Some("main_folder"),
            );
        });
    }

    #[gpui::test]
    fn active_folder_filter_unknown_extra_returns_none(cx: &mut gpui::TestAppContext) {
        // Close-race contract: a fresh uuid that does not match any extra
        // returns `None` (no panic, no fallback to main's filter). Pre-
        // populate main with a filter to ensure the unknown-extra path does
        // NOT silently surface main's value as a default. Mirrors the
        // silent-no-op shape of the window-scoped setters.
        let mut data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        data.main_window.folder_filter = Some("main_folder".to_string());
        let workspace = cx.new(|_cx| Workspace::new(data));
        let unknown = uuid::Uuid::new_v4();

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.active_folder_filter(WindowId::Extra(unknown)).is_none());
            // Main's filter is still readable via its own id.
            assert_eq!(
                ws.active_folder_filter(WindowId::Main).map(|s| s.as_str()),
                Some("main_folder"),
            );
        });
    }
}
