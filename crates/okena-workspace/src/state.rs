//! Workspace GPUI entity — coordinator over persistent data and transient
//! per-session state.
//!
//! Data types (`WorkspaceData`, `ProjectData`, `LayoutNode`, etc.) live in
//! `okena-state` / `okena-layout` and are re-exported here so existing
//! `crate::state::*` imports keep working.

use crate::access_history::ProjectAccessHistory;
use crate::context::WorkspaceCx;
use crate::focus::FocusManager;
use crate::lifecycle::ProjectLifecycleTracker;
use crate::remote_sync::{PendingRemoteFocus, RemoteProjectSnapshot, RemoteSyncState};
use crate::visibility::compute_visible_projects;
#[cfg(feature = "gpui")]
use gpui::*;
use okena_core::theme::FolderColor;
use okena_terminal::backend::TerminalSessionTeardown;
use okena_terminal::session_backend::SessionBackend;
use okena_terminal::shell_config::ShellType;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub use okena_layout::{LayoutNode, SplitDirection};
pub use okena_state::{
    DropZone, FocusedTerminalState, FolderData, HookTerminalEntry, HookTerminalStatus,
    PendingWorktreeClose, ProjectData, ProjectLayoutMode, WindowBounds, WindowId, WindowState,
    WorkspaceData, WorktreeMetadata,
};

/// What a window is focused on, captured before a sync reshapes the layout.
/// See `Workspace::reanchor_focus`.
struct FocusAnchor {
    /// The window's focus at capture time, so a focus target applied during
    /// this sync can be told apart from an untouched window.
    previous: FocusedTerminalState,
    /// Terminal the focused path named — the thing to follow.
    terminal_id: String,
    /// Where focus goes if that terminal is gone once the sync lands, resolved
    /// against the tree it still lived in.
    replacement: Option<String>,
}

/// The terminal slot at `path`: `Some(None)` for a pane whose PTY is still
/// spawning, `None` when the path doesn't name a pane at all.
fn terminal_slot_at<'a>(layout: &'a LayoutNode, path: &[usize]) -> Option<Option<&'a str>> {
    match layout.get_at_path(path) {
        Some(LayoutNode::Terminal { terminal_id, .. }) => Some(terminal_id.as_deref()),
        _ => None,
    }
}

/// Diagnostics returned after atomically aborting a vanished before-remove hook.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbortedWorktreeClose {
    pub project_id: String,
    pub project_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum FilesystemObjectIdentity {
    #[cfg(unix)]
    Unix { device: u64, inode: u64 },
    #[cfg(windows)]
    Windows { volume: u32, file: u64 },
    #[cfg(not(unix))]
    CanonicalPath(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PathComponentIdentity {
    Exact(OsString),
    CaseFolded(String),
}

/// Filesystem-aware path identity used for destructive ownership checks.
///
/// Existing ancestors are identified by their filesystem object IDs, so bind
/// mounts, drive mappings, and UNC aliases converge. Components below the
/// deepest existing ancestor remain an ordered suffix.
#[derive(Clone, Debug)]
pub struct PhysicalPathIdentity {
    ancestors: Vec<FilesystemObjectIdentity>,
    unresolved: Vec<PathComponentIdentity>,
    fallback: PathBuf,
}

impl PhysicalPathIdentity {
    pub fn starts_with(&self, root: &Self) -> bool {
        let Some(root_anchor) = root.ancestors.first() else {
            return self.fallback.starts_with(&root.fallback);
        };

        if root.unresolved.is_empty() {
            return self.ancestors.contains(root_anchor);
        }

        self.ancestors.first() == Some(root_anchor) && self.unresolved.starts_with(&root.unresolved)
    }
}

impl PartialEq for PhysicalPathIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.starts_with(other) && other.starts_with(self)
    }
}

impl Eq for PhysicalPathIdentity {}

#[cfg(unix)]
fn filesystem_object_identity(path: &Path) -> Option<FilesystemObjectIdentity> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = std::fs::metadata(path).ok()?;
    Some(FilesystemObjectIdentity::Unix {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(windows)]
fn filesystem_object_identity(path: &Path) -> Option<FilesystemObjectIdentity> {
    match windows_object_id(path) {
        Some((volume, file)) => Some(FilesystemObjectIdentity::Windows { volume, file }),
        None => std::fs::canonicalize(path)
            .ok()
            .map(normalize_fallback_path)
            .map(FilesystemObjectIdentity::CanonicalPath),
    }
}

/// Volume serial + file index, read straight from a handle because
/// `Metadata::volume_serial_number`/`file_index` are still unstable
/// (rust-lang/rust#63010).
#[cfg(windows)]
fn windows_object_id(path: &Path) -> Option<(u32, u64)> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, GetFileInformationByHandle,
    };

    // No access rights and every share mode, so identifying a path never blocks
    // what the caller does to it next. Directories need BACKUP_SEMANTICS to open
    // at all. The handle closes with `object`.
    let object = std::fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .ok()?;

    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // Safety: the handle is open for the duration of the call and `info` is a
    // live, correctly sized out-parameter.
    if unsafe { GetFileInformationByHandle(object.as_raw_handle(), &mut info) } == 0 {
        return None;
    }
    Some((
        info.dwVolumeSerialNumber,
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    ))
}

#[cfg(not(any(unix, windows)))]
fn filesystem_object_identity(path: &Path) -> Option<FilesystemObjectIdentity> {
    std::fs::canonicalize(path)
        .ok()
        .map(normalize_fallback_path)
        .map(FilesystemObjectIdentity::CanonicalPath)
}

fn normalize_fallback_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(path.to_string_lossy().to_lowercase())
    }
    #[cfg(not(windows))]
    {
        path
    }
}

fn normalize_unresolved_component(
    component: &OsString,
    case_sensitive: bool,
) -> PathComponentIdentity {
    if case_sensitive {
        PathComponentIdentity::Exact(component.clone())
    } else {
        PathComponentIdentity::CaseFolded(component.to_string_lossy().to_lowercase())
    }
}

#[cfg(windows)]
fn filesystem_is_case_sensitive(_path: &Path) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn filesystem_is_case_sensitive(path: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
        return true;
    };
    // `pathconf` reads the volume's case-sensitivity flag for this directory.
    let result = unsafe { libc::pathconf(c_path.as_ptr(), libc::_PC_CASE_SENSITIVE) };
    match result {
        0 => false,
        1 => true,
        _ => probe_case_sensitivity(path).unwrap_or(true),
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn filesystem_is_case_sensitive(path: &Path) -> bool {
    probe_case_sensitivity(path).unwrap_or(true)
}

#[cfg(not(any(unix, windows)))]
fn filesystem_is_case_sensitive(_path: &Path) -> bool {
    true
}

#[cfg(unix)]
fn probe_case_sensitivity(path: &Path) -> Option<bool> {
    if let Some(result) = probe_case_alias(path) {
        return Some(result);
    }
    let entries = std::fs::read_dir(path).ok()?;
    entries
        .filter_map(Result::ok)
        .take(64)
        .find_map(|entry| probe_case_alias(&entry.path()))
}

#[cfg(unix)]
fn probe_case_alias(path: &Path) -> Option<bool> {
    use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};

    let name = path.file_name()?;
    let mut alternate = name.as_bytes().to_vec();
    let byte = alternate
        .iter_mut()
        .find(|byte| byte.is_ascii_alphabetic())?;
    if byte.is_ascii_lowercase() {
        byte.make_ascii_uppercase();
    } else {
        byte.make_ascii_lowercase();
    }
    let alias = path.parent()?.join(OsString::from_vec(alternate));
    let original = filesystem_object_identity(path)?;
    match filesystem_object_identity(&alias) {
        Some(alias) => Some(alias != original),
        None => Some(true),
    }
}

/// Global workspace wrapper for app-wide access (used by quit handler)
#[cfg(feature = "gpui")]
#[derive(Clone)]
pub struct GlobalWorkspace(pub Entity<Workspace>);

#[cfg(feature = "gpui")]
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
    /// Monotonic counter incremented when all workspace data is replaced.
    data_replacement_epoch: u64,
    /// Active live session-backend migration, fenced by its replacement epoch.
    terminal_backend_migration_epoch: Option<u64>,
    /// Terminal IDs queued for killing by the app layer (drained by Okena observer).
    pending_terminal_kills: Vec<String>,
    /// Terminals closed with the grace-period "soft close": removed from the
    /// layout but their PTY is kept alive until the grace timer fires (or the
    /// user undoes / force-closes). Holds the snapshots needed to restore.
    pub(crate) pending_closes: Vec<PendingClose>,
    /// Terminals just brought back by an undo whose PTY might still be racing an
    /// in-flight exit event — see [`RestoredClose`].
    pub(crate) restored_closes: Vec<RestoredClose>,
    /// Ownership retained after a terminal leaves the layout but before its PTY
    /// exit is processed, so daemon lifecycle hooks still have project context.
    pub(crate) closing_terminal_owners: HashMap<String, ClosingTerminalOwner>,
}

/// A terminal that was soft-closed and is waiting out its grace period.
///
/// The PTY is still alive in the registry; only the layout entry was removed.
/// `pre_close_layout` / `post_close_layout` snapshot the owning project's tree
/// before and right after the close so undo can either restore the exact prior
/// tree (when nothing else changed) or fall back to re-appending the pane.
#[derive(Clone, Debug)]
pub struct PendingClose {
    pub terminal_id: String,
    pub project_id: String,
    pub toast_id: String,
    pub pre_close_layout: Option<LayoutNode>,
    pub post_close_layout: Option<LayoutNode>,
}

/// A terminal brought back by `undo_soft_close` whose PTY may still be racing an
/// in-flight exit event.
///
/// The `alive` check the undo path uses is registry-based, and the registry only
/// drops a terminal once the app *processes* its exit event — so a shell that has
/// already exited can still read as "alive", letting undo restore a doomed pane.
/// If that exit then lands, `reap_restored_close` tears the now-dead pane back out
/// of the layout (it can't be reconnected) instead of leaving it to linger — or to
/// silently respawn a fresh shell on the next render.
#[derive(Clone, Debug)]
pub struct RestoredClose {
    pub terminal_id: String,
    pub project_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalMigrationSlot {
    pub project_id: String,
    pub path: Vec<usize>,
    pub terminal_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalBackendMigration {
    pub epoch: u64,
    pub project_ids: Vec<String>,
    pub ordinary_slots: Vec<TerminalMigrationSlot>,
    pub teardown_sessions: Vec<TerminalSessionTeardown>,
    pub hook_terminal_ids: Vec<String>,
}

/// Terminal ownership detached from one project while its directory is moved
/// or removed by the headless daemon.
#[derive(Clone, Debug)]
pub struct ProjectRuntimeQuiesce {
    pub project_id: String,
    pub data_replacement_epoch: u64,
    pub runtime_quiesce_generation: u64,
    pub project_path: String,
    pub teardown_sessions: Vec<TerminalSessionTeardown>,
    /// Running hook owners that must be cancelled and removed from the registry.
    pub hook_terminal_ids: Vec<String>,
    /// Completed hooks whose scrollback remains registered after session teardown.
    pub preserved_registry_terminal_ids: Vec<String>,
    pub pending_close_terminal_ids: Vec<String>,
    layout_slots: Vec<ProjectRuntimeSlot>,
}

#[derive(Clone, Debug)]
struct ProjectRuntimeSlot {
    path: Vec<usize>,
    terminal_name: Option<String>,
    hidden: Option<bool>,
}

/// Transient ownership metadata for a terminal awaiting its PTY exit event.
#[derive(Clone, Debug)]
pub(crate) struct ClosingTerminalOwner {
    pub project_id: String,
    pub terminal_name: Option<String>,
}

#[allow(clippy::too_many_arguments)]
fn take_layout_terminal_ownership(
    layout: &mut LayoutNode,
    project_id: &str,
    project_default_shell: Option<&ShellType>,
    global_default_shell: &ShellType,
    backend_preference: SessionBackend,
    path: &mut Vec<usize>,
    ordinary_slots: &mut Vec<TerminalMigrationSlot>,
    teardown_sessions: &mut Vec<TerminalSessionTeardown>,
) {
    match layout {
        LayoutNode::Terminal {
            terminal_id,
            shell_type,
            ..
        } => {
            let Some(terminal_id) = terminal_id.take() else {
                return;
            };
            teardown_sessions.push(TerminalSessionTeardown {
                terminal_id: terminal_id.clone(),
                route: crate::persistence::teardown_route(
                    shell_type,
                    project_default_shell,
                    global_default_shell,
                    backend_preference,
                ),
            });
            ordinary_slots.push(TerminalMigrationSlot {
                project_id: project_id.to_string(),
                path: path.clone(),
                terminal_id,
            });
        }
        LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
            for (index, child) in children.iter_mut().enumerate() {
                path.push(index);
                take_layout_terminal_ownership(
                    child,
                    project_id,
                    project_default_shell,
                    global_default_shell,
                    backend_preference,
                    path,
                    ordinary_slots,
                    teardown_sessions,
                );
                path.pop();
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn take_project_layout_runtime(
    layout: &mut LayoutNode,
    project_default_shell: Option<&ShellType>,
    global_default_shell: &ShellType,
    backend_preference: SessionBackend,
    terminal_names: &mut HashMap<String, String>,
    hidden_terminals: &mut HashMap<String, bool>,
    path: &mut Vec<usize>,
    slots: &mut Vec<ProjectRuntimeSlot>,
    teardown_sessions: &mut Vec<TerminalSessionTeardown>,
) {
    match layout {
        LayoutNode::Terminal {
            terminal_id,
            shell_type,
            ..
        } => {
            let Some(terminal_id) = terminal_id.take() else {
                return;
            };
            teardown_sessions.push(TerminalSessionTeardown {
                terminal_id: terminal_id.clone(),
                route: crate::persistence::teardown_route(
                    shell_type,
                    project_default_shell,
                    global_default_shell,
                    backend_preference,
                ),
            });
            slots.push(ProjectRuntimeSlot {
                path: path.clone(),
                terminal_name: terminal_names.remove(&terminal_id),
                hidden: hidden_terminals.remove(&terminal_id),
            });
        }
        LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
            for (index, child) in children.iter_mut().enumerate() {
                path.push(index);
                take_project_layout_runtime(
                    child,
                    project_default_shell,
                    global_default_shell,
                    backend_preference,
                    terminal_names,
                    hidden_terminals,
                    path,
                    slots,
                    teardown_sessions,
                );
                path.pop();
            }
        }
    }
}

impl Workspace {
    pub fn new(data: WorkspaceData) -> Self {
        Self {
            data,
            lifecycle: ProjectLifecycleTracker::new(),
            remote_sync: RemoteSyncState::new(),
            access_history: ProjectAccessHistory::new(),
            data_version: 0,
            data_replacement_epoch: 0,
            terminal_backend_migration_epoch: None,
            pending_terminal_kills: Vec::new(),
            pending_closes: Vec::new(),
            restored_closes: Vec::new(),
            closing_terminal_owners: HashMap::new(),
        }
    }

    /// Seed desktop-owned project presentation before the first daemon snapshot.
    pub fn seed_client_project_layouts(&mut self, layouts: HashMap<String, LayoutNode>) {
        self.remote_sync.seed_project_layouts(layouts);
    }

    /// Snapshot all desktop-owned layouts, including projects temporarily
    /// absent while their daemon connection is reconnecting.
    pub fn client_project_layouts(&self) -> HashMap<String, LayoutNode> {
        let mut layouts = self.remote_sync.preserved_project_layouts().clone();
        layouts.extend(self.data.projects.iter().filter_map(|project| {
            project
                .layout
                .clone()
                .map(|layout| (project.id.clone(), layout))
        }));
        layouts
    }

    /// Current data version (incremented on persistent data mutations)
    pub fn data_version(&self) -> u64 {
        self.data_version
    }

    /// Current wholesale data replacement epoch.
    pub fn data_replacement_epoch(&self) -> u64 {
        self.data_replacement_epoch
    }

    /// Detach every runtime that can retain a project's working directory.
    /// The layout remains authoritative with empty terminal slots so it can be
    /// materialized again after a failed removal or a directory move.
    pub fn begin_project_runtime_quiesce(
        &mut self,
        project_id: &str,
        global_default_shell: &ShellType,
        backend_preference: SessionBackend,
        reject_running_hooks: bool,
        cx: &mut impl WorkspaceCx,
    ) -> Result<ProjectRuntimeQuiesce, String> {
        let mut snapshots = self.begin_project_runtimes_quiesce(
            &[project_id.to_string()],
            global_default_shell,
            backend_preference,
            reject_running_hooks,
            cx,
        )?;
        snapshots
            .pop()
            .ok_or_else(|| "project runtime quiesce produced no owner".to_string())
    }

    /// Atomically detach every runtime for a set of projects.
    pub fn begin_project_runtimes_quiesce(
        &mut self,
        project_ids: &[String],
        global_default_shell: &ShellType,
        backend_preference: SessionBackend,
        reject_running_hooks: bool,
        cx: &mut impl WorkspaceCx,
    ) -> Result<Vec<ProjectRuntimeQuiesce>, String> {
        let mut unique_ids = Vec::new();
        let mut seen = HashSet::new();
        for project_id in project_ids {
            if seen.insert(project_id.clone()) {
                unique_ids.push(project_id.clone());
            }
        }
        if unique_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Validate the full batch before claiming or mutating any project.
        for project_id in &unique_ids {
            if self.lifecycle.is_creating(project_id) {
                return Err(format!("project is still being created: {project_id}"));
            }
            if self.lifecycle.is_closing(project_id) {
                return Err(format!(
                    "project operation is already in progress: {project_id}"
                ));
            }
            let project = self
                .project(project_id)
                .ok_or_else(|| format!("Project not found: {project_id}"))?;
            if project.is_remote {
                return Err(format!(
                    "remote project directories cannot be changed locally: {project_id}"
                ));
            }
            if reject_running_hooks
                && project
                    .hook_terminals
                    .values()
                    .any(|entry| entry.status == HookTerminalStatus::Running)
            {
                return Err(format!(
                    "cannot move a project while a lifecycle hook is running: {project_id}"
                ));
            }
        }

        let generation = self.lifecycle.claim_runtime_quiesce(&unique_ids)?;
        let data_replacement_epoch = self.data_replacement_epoch;
        let mut snapshots = Vec::with_capacity(unique_ids.len());
        for project_id in &unique_ids {
            let project_path = self
                .project(project_id)
                .map(|project| project.path.clone())
                .ok_or_else(|| format!("Project not found: {project_id}"))?;
            let mut teardown_sessions = Vec::new();
            let mut hook_terminal_ids = Vec::new();
            let mut preserved_registry_terminal_ids = Vec::new();
            let mut layout_slots = Vec::new();
            {
                let project = self
                    .project_mut(project_id)
                    .ok_or_else(|| format!("Project not found: {project_id}"))?;
                if let Some(layout) = &mut project.layout {
                    take_project_layout_runtime(
                        layout,
                        project.default_shell.as_ref(),
                        global_default_shell,
                        backend_preference,
                        &mut project.terminal_names,
                        &mut project.hidden_terminals,
                        &mut Vec::new(),
                        &mut layout_slots,
                        &mut teardown_sessions,
                    );
                }
                teardown_sessions.extend(
                    project
                        .service_terminals
                        .drain()
                        .map(|(_, terminal_id)| TerminalSessionTeardown::host(terminal_id)),
                );
                let all_hook_ids: Vec<String> = project.hook_terminals.keys().cloned().collect();
                for terminal_id in &all_hook_ids {
                    let is_running = project
                        .hook_terminals
                        .get(terminal_id)
                        .is_some_and(|entry| entry.status == HookTerminalStatus::Running);
                    if is_running {
                        hook_terminal_ids.push(terminal_id.clone());
                        project.hook_terminals.remove(terminal_id);
                        project.terminal_names.remove(terminal_id);
                        project.hidden_terminals.remove(terminal_id);
                    } else {
                        preserved_registry_terminal_ids.push(terminal_id.clone());
                    }
                }
                teardown_sessions
                    .extend(all_hook_ids.into_iter().map(TerminalSessionTeardown::host));
            }
            let pending_close_terminal_ids = self.drain_pending_closes_for_project(project_id);
            teardown_sessions.extend(
                pending_close_terminal_ids
                    .iter()
                    .cloned()
                    .map(TerminalSessionTeardown::host),
            );
            teardown_sessions.sort_by(|a, b| a.terminal_id.cmp(&b.terminal_id));
            teardown_sessions.dedup_by(|a, b| a.terminal_id == b.terminal_id);
            hook_terminal_ids.sort();
            preserved_registry_terminal_ids.sort();
            self.mark_closing_project_authoritative(project_id);
            snapshots.push(ProjectRuntimeQuiesce {
                project_id: project_id.clone(),
                data_replacement_epoch,
                runtime_quiesce_generation: generation,
                project_path,
                teardown_sessions,
                hook_terminal_ids,
                preserved_registry_terminal_ids,
                pending_close_terminal_ids,
                layout_slots,
            });
        }
        self.notify_data(cx);
        Ok(snapshots)
    }

    pub fn project_runtime_quiesce_is_current(&self, snapshot: &ProjectRuntimeQuiesce) -> bool {
        self.project_runtime_quiesce_is_current_at(snapshot, &snapshot.project_path)
    }

    pub fn project_runtime_quiesce_is_current_at(
        &self,
        snapshot: &ProjectRuntimeQuiesce,
        project_path: &str,
    ) -> bool {
        self.data_replacement_epoch == snapshot.data_replacement_epoch
            && self
                .lifecycle
                .owns_runtime_quiesce(&snapshot.project_id, snapshot.runtime_quiesce_generation)
            && self.lifecycle.is_closing(&snapshot.project_id)
            && self
                .project(&snapshot.project_id)
                .is_some_and(|project| project.path == project_path)
    }

    /// Restore terminal metadata after empty layout slots have been materialized.
    pub fn finish_project_runtime_recovery(
        &mut self,
        snapshot: &ProjectRuntimeQuiesce,
        cx: &mut impl WorkspaceCx,
    ) {
        if self.data_replacement_epoch != snapshot.data_replacement_epoch
            || !self
                .lifecycle
                .owns_runtime_quiesce(&snapshot.project_id, snapshot.runtime_quiesce_generation)
        {
            return;
        }
        let Some(project) = self.project_mut(&snapshot.project_id) else {
            return;
        };
        for slot in &snapshot.layout_slots {
            let Some(terminal_id) = project
                .layout
                .as_ref()
                .and_then(|layout| layout.get_at_path(&slot.path))
                .and_then(|node| match node {
                    LayoutNode::Terminal { terminal_id, .. } => terminal_id.clone(),
                    _ => None,
                })
            else {
                continue;
            };
            if let Some(name) = &slot.terminal_name {
                project
                    .terminal_names
                    .insert(terminal_id.clone(), name.clone());
            }
            if let Some(hidden) = slot.hidden {
                project.hidden_terminals.insert(terminal_id, hidden);
            }
        }
        if !self
            .lifecycle
            .finish_runtime_quiesce(&snapshot.project_id, snapshot.runtime_quiesce_generation)
        {
            return;
        }
        self.finish_closing_project(&snapshot.project_id);
        self.notify_data(cx);
    }

    /// Clear PTYs created by a failed rematerialization attempt so recovery can
    /// fail without publishing a half-restored layout or leaking registry ids.
    pub fn drain_partial_project_runtime_recovery(
        &mut self,
        snapshot: &ProjectRuntimeQuiesce,
        global_default_shell: &ShellType,
        backend_preference: SessionBackend,
        cx: &mut impl WorkspaceCx,
    ) -> Vec<TerminalSessionTeardown> {
        let Some(project) = self.project_mut(&snapshot.project_id) else {
            return Vec::new();
        };
        let mut teardown_sessions = Vec::new();
        let mut discarded_slots = Vec::new();
        if let Some(layout) = &mut project.layout {
            take_project_layout_runtime(
                layout,
                project.default_shell.as_ref(),
                global_default_shell,
                backend_preference,
                &mut project.terminal_names,
                &mut project.hidden_terminals,
                &mut Vec::new(),
                &mut discarded_slots,
                &mut teardown_sessions,
            );
        }
        if !teardown_sessions.is_empty() {
            self.notify_data(cx);
        }
        teardown_sessions
    }

    /// Snapshot and atomically clear all local terminal ownership for migration.
    pub fn begin_terminal_backend_migration(
        &mut self,
        backend_preference: SessionBackend,
        global_default_shell: &ShellType,
    ) -> Result<TerminalBackendMigration, String> {
        if self.terminal_backend_migration_epoch.is_some() {
            return Err("terminal backend migration already in progress".to_string());
        }
        if self.lifecycle.has_active_operations()
            || !self.pending_closes.is_empty()
            || !self.restored_closes.is_empty()
            || !self.closing_terminal_owners.is_empty()
            || !self.pending_terminal_kills.is_empty()
        {
            return Err(
                "cannot switch terminal backend while a workspace operation is active".to_string(),
            );
        }
        self.data_replacement_epoch = self
            .data_replacement_epoch
            .checked_add(1)
            .ok_or_else(|| "workspace replacement epoch exhausted".to_string())?;
        let epoch = self.data_replacement_epoch;
        self.terminal_backend_migration_epoch = Some(epoch);

        let mut project_ids = Vec::new();
        let mut ordinary_slots = Vec::new();
        let mut teardown_sessions = Vec::new();
        let mut hook_terminal_ids = Vec::new();
        for project in self
            .data
            .projects
            .iter_mut()
            .filter(|project| !project.is_remote)
        {
            project_ids.push(project.id.clone());
            if let Some(layout) = &mut project.layout {
                take_layout_terminal_ownership(
                    layout,
                    &project.id,
                    project.default_shell.as_ref(),
                    global_default_shell,
                    backend_preference,
                    &mut Vec::new(),
                    &mut ordinary_slots,
                    &mut teardown_sessions,
                );
            }
            teardown_sessions.extend(
                project
                    .service_terminals
                    .drain()
                    .map(|(_, terminal_id)| TerminalSessionTeardown::host(terminal_id)),
            );
            let project_hook_ids: Vec<String> =
                project.hook_terminals.drain().map(|(id, _)| id).collect();
            for terminal_id in &project_hook_ids {
                project.terminal_names.remove(terminal_id);
                project.hidden_terminals.remove(terminal_id);
            }
            teardown_sessions.extend(
                project_hook_ids
                    .iter()
                    .cloned()
                    .map(TerminalSessionTeardown::host),
            );
            hook_terminal_ids.extend(project_hook_ids);
        }
        teardown_sessions.sort_by(|a, b| a.terminal_id.cmp(&b.terminal_id));
        teardown_sessions.dedup_by(|a, b| a.terminal_id == b.terminal_id);
        project_ids.sort();
        ordinary_slots.sort_by(|a, b| a.terminal_id.cmp(&b.terminal_id));
        hook_terminal_ids.sort();

        Ok(TerminalBackendMigration {
            epoch,
            project_ids,
            ordinary_slots,
            teardown_sessions,
            hook_terminal_ids,
        })
    }

    /// Return the active migration epoch, if terminal ownership is provisional.
    pub fn terminal_backend_migration_epoch(&self) -> Option<u64> {
        self.terminal_backend_migration_epoch
    }

    /// Atomically publish replacement data behind the transient terminal gate.
    ///
    /// Callers finish terminal teardown/materialization before releasing the
    /// gate, so observers never persist partially materialized ownership.
    pub fn begin_workspace_replacement_transition(
        &mut self,
        focus_manager: &mut FocusManager,
        data: WorkspaceData,
    ) -> Result<u64, String> {
        if self.terminal_backend_migration_epoch.is_some() {
            return Err("a terminal ownership transition is already in progress".to_string());
        }
        self.data_replacement_epoch = self
            .data_replacement_epoch
            .checked_add(1)
            .ok_or_else(|| "workspace replacement epoch exhausted".to_string())?;
        let epoch = self.data_replacement_epoch;
        self.terminal_backend_migration_epoch = Some(epoch);
        self.data = data;
        for project in &mut self.data.projects {
            project.is_closing = false;
        }
        self.lifecycle = ProjectLifecycleTracker::new();
        self.pending_closes.clear();
        self.restored_closes.clear();
        self.closing_terminal_owners.clear();
        focus_manager.clear_all();
        self.data_version = self.data_version.wrapping_add(1);
        Ok(epoch)
    }

    /// Publish one consolidated change after replacement materialization.
    pub fn finish_workspace_replacement_transition(
        &mut self,
        epoch: u64,
        cx: &mut impl WorkspaceCx,
    ) -> bool {
        self.finish_terminal_backend_migration(epoch, cx)
    }

    /// Clear only the migration that owns `epoch` and wake skipped observers.
    pub fn finish_terminal_backend_migration(
        &mut self,
        epoch: u64,
        cx: &mut impl WorkspaceCx,
    ) -> bool {
        if self.terminal_backend_migration_epoch != Some(epoch) {
            return false;
        }
        self.terminal_backend_migration_epoch = None;
        self.notify_data(cx);
        true
    }

    /// Restore ordinary logical IDs before reconnecting them on the selected backend.
    pub fn restore_terminal_backend_migration_slots(
        &mut self,
        migration: &TerminalBackendMigration,
    ) -> Result<(), String> {
        if self.terminal_backend_migration_epoch != Some(migration.epoch) {
            return Err("stale terminal backend migration completion".to_string());
        }
        for slot in &migration.ordinary_slots {
            let project = self.project_mut(&slot.project_id).ok_or_else(|| {
                format!("project disappeared during migration: {}", slot.project_id)
            })?;
            let node = project
                .layout
                .as_mut()
                .and_then(|layout| layout.get_at_path_mut(&slot.path))
                .ok_or_else(|| {
                    format!(
                        "terminal slot disappeared during migration: {}",
                        slot.terminal_id
                    )
                })?;
            match node {
                LayoutNode::Terminal { terminal_id, .. }
                    if terminal_id
                        .as_ref()
                        .is_none_or(|terminal_id| terminal_id == &slot.terminal_id) =>
                {
                    *terminal_id = Some(slot.terminal_id.clone());
                }
                LayoutNode::Terminal { .. } => {
                    return Err(format!(
                        "terminal slot was claimed during migration: {}",
                        slot.terminal_id
                    ));
                }
                LayoutNode::Split { .. } | LayoutNode::Tabs { .. } => {
                    return Err(format!(
                        "terminal slot changed during migration: {}",
                        slot.terminal_id
                    ));
                }
            }
        }
        Ok(())
    }

    /// Resolve a path for physical ownership checks, including mount/drive
    /// aliases, symlinked ancestors, and relative/nonexistent descendants.
    pub fn physical_path_identity(path: &Path) -> PhysicalPathIdentity {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        };

        fn resolve_symlink_components(path: &Path, depth: usize) -> PathBuf {
            let mut resolved = PathBuf::new();
            for component in path.components() {
                match component {
                    std::path::Component::CurDir => {}
                    std::path::Component::ParentDir => {
                        resolved.pop();
                    }
                    _ => {
                        resolved.push(component.as_os_str());
                        if depth >= 40
                            || !std::fs::symlink_metadata(&resolved)
                                .is_ok_and(|metadata| metadata.file_type().is_symlink())
                        {
                            continue;
                        }
                        let Ok(target) = std::fs::read_link(&resolved) else {
                            continue;
                        };
                        resolved.pop();
                        let target = if target.is_absolute() {
                            target
                        } else {
                            resolved.join(target)
                        };
                        resolved = resolve_symlink_components(&target, depth + 1);
                    }
                }
            }
            okena_git::repository::normalize_path(&resolved)
        }

        // Walk components in filesystem order so `link/..` backs out of the
        // symlink target, not the lexical directory containing the link.
        let normalized = resolve_symlink_components(&absolute, 0);
        let mut cursor = normalized.as_path();
        let mut unresolved = Vec::new();

        let (existing_path, resolved) = loop {
            if let Ok(mut existing) = std::fs::canonicalize(cursor) {
                for component in unresolved.iter().rev() {
                    existing.push(component);
                }
                break (
                    Some(cursor.to_path_buf()),
                    okena_git::repository::normalize_path(&existing),
                );
            }
            let Some(name) = cursor.file_name() else {
                break (None, normalized);
            };
            unresolved.push(name.to_os_string());
            let Some(parent) = cursor.parent() else {
                break (None, normalized);
            };
            cursor = parent;
        };

        let fallback = normalize_fallback_path(resolved);
        let Some(existing_path) = existing_path else {
            return PhysicalPathIdentity {
                ancestors: Vec::new(),
                unresolved: Vec::new(),
                fallback,
            };
        };

        let case_sensitive = filesystem_is_case_sensitive(&existing_path);
        let unresolved = unresolved
            .iter()
            .rev()
            .map(|component| normalize_unresolved_component(component, case_sensitive))
            .collect();
        let mut ancestors = Vec::new();
        let mut ancestor = Some(existing_path.as_path());
        while let Some(path) = ancestor {
            if let Some(identity) = filesystem_object_identity(path)
                && ancestors.last() != Some(&identity)
            {
                ancestors.push(identity);
            }
            ancestor = path.parent();
        }

        PhysicalPathIdentity {
            ancestors,
            unresolved,
            fallback,
        }
    }

    fn worktree_root_identity(&self, project: &ProjectData) -> Option<PhysicalPathIdentity> {
        let metadata = project.worktree_info.as_ref()?;
        let project_path = Path::new(&project.path);
        let root = okena_git::get_repo_root(project_path).unwrap_or_else(|| {
            if metadata.worktree_path.is_empty() {
                project_path.to_path_buf()
            } else {
                PathBuf::from(&metadata.worktree_path)
            }
        });
        Some(Self::physical_path_identity(&root))
    }

    /// Reject a project path that would enter a worktree root currently being
    /// created, closed, merged, or removed.
    pub fn ensure_project_path_claim_allowed(&self, path: &Path) -> Result<(), String> {
        let candidate = Self::physical_path_identity(path);
        for project in self.projects().iter().filter(|project| !project.is_remote) {
            if !(self.is_creating_project(&project.id) || self.is_project_closing(&project.id)) {
                continue;
            }
            let Some(root) = self.worktree_root_identity(project) else {
                continue;
            };
            if candidate.starts_with(&root) {
                return Err(format!(
                    "path is reserved by active worktree operation for '{}'",
                    project.name
                ));
            }
        }
        Ok(())
    }

    /// Reject a worktree target whose physical root overlaps an active root in
    /// either direction.
    pub fn ensure_worktree_target_claim_allowed(&self, root: &Path) -> Result<(), String> {
        let candidate = Self::physical_path_identity(root);
        for project in self.projects().iter().filter(|project| !project.is_remote) {
            if !(self.is_creating_project(&project.id) || self.is_project_closing(&project.id)) {
                continue;
            }
            let Some(active_root) = self.worktree_root_identity(project) else {
                continue;
            };
            if candidate.starts_with(&active_root) || active_root.starts_with(&candidate) {
                return Err(format!(
                    "worktree target overlaps active operation for '{}'",
                    project.name
                ));
            }
        }
        Ok(())
    }

    /// A worktree checkout may only be removed when no other local project owns
    /// its root or a descendant path.
    pub fn ensure_worktree_root_exclusively_owned(
        &self,
        owner_project_id: &str,
        root: &Path,
    ) -> Result<(), String> {
        let root = Self::physical_path_identity(root);
        if let Some(claimant) = self.projects().iter().find(|project| {
            project.id != owner_project_id
                && !project.is_remote
                && Self::physical_path_identity(Path::new(&project.path)).starts_with(&root)
        }) {
            return Err(format!(
                "worktree checkout is also used by project '{}'",
                claimant.name
            ));
        }
        Ok(())
    }

    pub fn ensure_project_path_mutation_allowed(
        &self,
        project_id: &str,
        new_path: &Path,
    ) -> Result<(), String> {
        if self.is_creating_project(project_id) {
            return Err("worktree is still being created".to_string());
        }
        if self.is_project_closing(project_id) {
            return Err("worktree is already closing".to_string());
        }
        let existing = self
            .project(project_id)
            .map(|project| Self::physical_path_identity(Path::new(&project.path)))
            .ok_or_else(|| "Project not found".to_string())?;
        let candidate = Self::physical_path_identity(new_path);
        for project in self.projects().iter().filter(|project| !project.is_remote) {
            if !(self.is_creating_project(&project.id) || self.is_project_closing(&project.id)) {
                continue;
            }
            let Some(root) = self.worktree_root_identity(project) else {
                continue;
            };
            let overlaps =
                |path: &PhysicalPathIdentity| path.starts_with(&root) || root.starts_with(path);
            if overlaps(&candidate) || overlaps(&existing) {
                return Err(format!(
                    "path overlaps active worktree operation for '{}'",
                    project.name
                ));
            }
        }
        Ok(())
    }

    /// Read-only access to persistent workspace data.
    pub fn data(&self) -> &WorkspaceData {
        &self.data
    }

    /// Notify that persistent data changed. Bumps version, calls cx.notify(),
    /// and refreshes all windows to bypass `.cached()` view wrappers.
    /// Use this instead of cx.notify() when mutating `self.data`.
    ///
    /// `refresh_windows()` is the heavy hammer: it bypasses EVERY `.cached()`
    /// view (project columns, sidebar — and the terminal grids), forcing them
    /// all to re-render. That's necessary so cached chrome reflects structural
    /// data changes, but it means callers fired in a hot loop will re-shape
    /// every visible terminal grid each time. Keep such callers rare or
    /// throttled (see `bump_activity`).
    pub fn notify_data(&mut self, cx: &mut impl WorkspaceCx) {
        self.data_version += 1;
        cx.notify();
        cx.refresh_views();
    }

    fn mutate_data(&mut self, cx: &mut impl WorkspaceCx, f: impl FnOnce(&mut WorkspaceData)) {
        f(&mut self.data);
        self.notify_data(cx);
    }

    /// Replace workspace data wholesale (e.g. by loading a named session).
    ///
    /// A replacement becomes the daemon's active restart state, so it is a
    /// persistent mutation even though its source was another file.
    pub fn replace_data(
        &mut self,
        focus_manager: &mut FocusManager,
        data: WorkspaceData,
        cx: &mut impl WorkspaceCx,
    ) {
        self.data = data;
        for project in &mut self.data.projects {
            project.is_closing = false;
        }
        self.data_replacement_epoch += 1;
        self.lifecycle = ProjectLifecycleTracker::new();
        // Snapshots in pending_closes refer to the old data — drop them so an
        // undo can't restore into a wholesale-replaced workspace. The
        // restore-race breadcrumbs refer to the old layout too.
        self.pending_closes.clear();
        self.restored_closes.clear();
        self.closing_terminal_owners.clear();
        focus_manager.clear_all();
        self.notify_data(cx);
    }

    /// Record that a project was accessed (for sorting by recency)
    pub fn touch_project(&mut self, project_id: &str) {
        self.access_history.touch(project_id);
    }

    /// Record meaningful activity for a project: stamp `last_activity_at` with
    /// the current unix-millis and persist it (drives the activity-sorted
    /// sidebar view). Called on focus, a finished command (OSC 133 ;D), and a
    /// bell/notification from one of the project's terminals — deliberately NOT
    /// on raw terminal output, since output volume is not "activity". A no-op
    /// for an unknown project id. Uses `notify_data` so the change is persisted
    /// (debounced) and the sidebar re-renders to reorder.
    pub fn bump_activity(&mut self, project_id: &str, cx: &mut impl WorkspaceCx) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        if let Some(project) = self.project_mut(project_id) {
            // Throttle per project. Activity stamping drives only the
            // recency-sorted sidebar, which doesn't need sub-second precision —
            // but it's hit on every command finish (OSC 133 ;D), focus, and
            // bell. Each unthrottled stamp calls `notify_data` →
            // `refresh_windows()`, which bypasses every `.cached()` view and
            // re-shapes ALL visible terminal grids. A terminal finishing
            // commands rapidly therefore pinned the renderer at full tilt
            // (render-stats showed ~18 full-window refreshes/s re-rendering
            // every pane). One stamp/sec per project keeps ordering correct
            // while collapsing the refresh storm. The throttle is per project,
            // so a different project's activity still re-sorts promptly.
            const ACTIVITY_STAMP_MIN_INTERVAL_MS: u64 = 1000;
            if let Some(prev) = project.last_activity_at
                && now.saturating_sub(prev) < ACTIVITY_STAMP_MIN_INTERVAL_MS
            {
                return;
            }
            project.last_activity_at = Some(now);
            self.notify_data(cx);
        }
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
        cx: &mut impl WorkspaceCx,
    ) {
        self.mutate_data(cx, |data| data.set_folder_filter(window_id, folder_id));
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
        cx: &mut impl WorkspaceCx,
    ) {
        self.mutate_data(cx, |data| data.toggle_hidden(window_id, project_id));
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
        cx: &mut impl WorkspaceCx,
    ) {
        self.mutate_data(cx, |data| {
            data.set_project_width(window_id, project_id, width)
        });
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
        cx: &mut impl WorkspaceCx,
    ) {
        self.mutate_data(cx, |data| {
            data.set_folder_collapsed(window_id, folder_id, collapsed);
        });
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
        cx: &mut impl WorkspaceCx,
    ) {
        self.mutate_data(cx, |data| data.set_os_bounds(window_id, bounds));
    }

    /// Set sidebar open/closed state for the targeted window. Persisted
    /// so each window remembers its own chrome layout across launches.
    pub fn set_sidebar_open(&mut self, window_id: WindowId, open: bool, cx: &mut impl WorkspaceCx) {
        self.mutate_data(cx, |data| data.set_sidebar_open(window_id, open));
    }

    /// Read the project-grid orientation for the targeted window. Falls back
    /// to the default (`Columns`) for an unknown window id.
    pub fn project_layout_mode(&self, window_id: WindowId) -> ProjectLayoutMode {
        self.data
            .window(window_id)
            .map(|w| w.project_layout)
            .unwrap_or_default()
    }

    /// Flip the targeted window's project grid between columns and rows.
    ///
    /// Terminal splits are transposed at render time for this window. Their
    /// canonical daemon-owned directions stay untouched, so another window can
    /// present the same project with a different orientation and state syncs
    /// cannot undo the local choice.
    ///
    /// Weights in `project_widths` are axis-agnostic, so relative grid
    /// sizing is preserved across the flip. The pixel scale is not — it is
    /// pixels per weight unit along the *current* axis, so it is dropped and
    /// recomputed from the new axis' viewport. Persisted via `notify_data`.
    pub fn toggle_project_layout_mode(&mut self, window_id: WindowId, cx: &mut impl WorkspaceCx) {
        if self.data.window(window_id).is_none() {
            return;
        }

        if let Some(w) = self.data.window_mut(window_id) {
            w.project_layout = w.project_layout.toggled();
            w.project_width_scale = None;
        }
        self.notify_data(cx);
    }

    /// Flip the sidebar project sort mode (manual ↔ activity) for a window.
    /// Persisted via `notify_data`.
    pub fn toggle_project_sort_mode(&mut self, window_id: WindowId, cx: &mut impl WorkspaceCx) {
        if self.data.toggle_project_sort_mode(window_id).is_some() {
            self.notify_data(cx);
        }
    }

    /// Flip the "needs attention" section opt-in for a window's manual view.
    /// Persisted via `notify_data`.
    pub fn toggle_show_attention_section(
        &mut self,
        window_id: WindowId,
        cx: &mut impl WorkspaceCx,
    ) {
        if self.data.toggle_show_attention_section(window_id).is_some() {
            self.notify_data(cx);
        }
    }

    /// Toggle whether a project is pinned to the top of the activity-sorted
    /// view. No-op for an unknown project id. Persisted via `notify_data`.
    pub fn toggle_project_pinned(&mut self, project_id: &str, cx: &mut impl WorkspaceCx) {
        if let Some(project) = self.project_mut(project_id) {
            project.pinned = !project.pinned;
            self.notify_data(cx);
        }
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
        cx: &mut impl WorkspaceCx,
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
    pub fn close_extra_window(&mut self, id: WindowId, cx: &mut impl WorkspaceCx) {
        self.data.close_extra_window(id);
        self.notify_data(cx);
    }

    // === ProjectLifecycleTracker conveniences ===

    pub fn is_creating_project(&self, project_id: &str) -> bool {
        self.lifecycle.is_creating(project_id)
    }

    pub fn mark_creating_project(&mut self, project_id: &str) {
        self.lifecycle.mark_creating(project_id);
        // Mirror onto the persisted/wire-facing marker so the flag survives a
        // daemon restart and reaches clients (the in-memory tracker does neither).
        if let Some(p) = self.data.projects.iter_mut().find(|p| p.id == project_id) {
            p.is_creating = true;
        }
    }

    pub fn finish_creating_project(&mut self, project_id: &str) {
        self.lifecycle.finish_creating(project_id);
        if let Some(p) = self.data.projects.iter_mut().find(|p| p.id == project_id) {
            p.is_creating = false;
            // Progress describes the operation, not the project: leaving the
            // last percentage behind would keep claiming a clone is running.
            p.creating_progress = None;
        }
    }

    /// Record how far the in-flight create for `project_id` has got, e.g.
    /// `Receiving objects: 42%`.
    ///
    /// Returns whether anything actually changed, so a caller driven by a
    /// chatty progress stream can skip broadcasting an identical snapshot.
    /// Ignores a project that is no longer being created: progress lines are
    /// delivered from a reader thread and can land after the operation ended,
    /// and one arriving late must not resurrect the placeholder.
    pub fn set_creating_progress(&mut self, project_id: &str, summary: String) -> bool {
        let Some(p) = self.data.projects.iter_mut().find(|p| p.id == project_id) else {
            return false;
        };
        if !p.is_creating || p.creating_progress.as_deref() == Some(summary.as_str()) {
            return false;
        }
        p.creating_progress = Some(summary);
        true
    }

    pub fn mark_worktree_removing(&mut self, path: &str) {
        self.lifecycle.mark_worktree_removing(path);
    }

    pub fn finish_worktree_removing(&mut self, path: &str) {
        self.lifecycle.finish_worktree_removing(path);
    }

    /// Client-side optimistic mark: sets only the in-memory tracker so the
    /// initiating client dims the row instantly. The authoritative daemon flag
    /// arrives via the mirror (`ProjectData::is_closing`); do NOT set the
    /// wire-facing marker here or a stale local set could out-live the mirror.
    pub fn mark_closing_project(&mut self, project_id: &str) {
        self.lifecycle.mark_closing(project_id);
    }

    /// Daemon-owned closing mark mirrored to thin clients.
    pub fn mark_closing_project_authoritative(&mut self, project_id: &str) {
        self.lifecycle.mark_closing(project_id);
        self.set_project_closing_flag(project_id, true);
    }

    pub fn finish_closing_project(&mut self, project_id: &str) {
        self.lifecycle.finish_closing(project_id);
        // Clear the wire-facing marker too: on the daemon this is the abort path
        // (before-remove hook failed), and mirroring the cleared flag is what
        // heals the client's "Closing…" row.
        self.set_project_closing_flag(project_id, false);
    }

    /// Mirror the closing lifecycle state onto the wire-facing `is_closing`
    /// marker on `ProjectData` so the daemon's authoritative closing state
    /// reaches thin clients (the in-memory lifecycle tracker is neither
    /// persisted nor mirrored).
    fn set_project_closing_flag(&mut self, project_id: &str, closing: bool) {
        if let Some(p) = self.data.projects.iter_mut().find(|p| p.id == project_id) {
            p.is_closing = closing;
        }
    }

    // === Terminal kill queue ===

    pub fn queue_terminal_kills(&mut self, ids: impl IntoIterator<Item = String>) {
        self.pending_terminal_kills.extend(ids);
    }

    pub fn drain_pending_terminal_kills(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_terminal_kills)
    }

    // === RemoteSyncState conveniences ===

    pub fn queue_pending_remote_focus(
        &mut self,
        window_id: WindowId,
        project_id: &str,
        old_terminal_ids: Vec<String>,
    ) {
        self.remote_sync
            .queue_focus(window_id, project_id, old_terminal_ids);
    }

    pub fn drain_pending_remote_focus(&mut self, window_id: WindowId) -> Vec<PendingRemoteFocus> {
        self.remote_sync.drain_pending_focus(window_id)
    }

    /// Record where a window's focus should land once the daemon applies a
    /// close, so keyboard focus survives the round-trip.
    ///
    /// Closing is not a local mutation: the client sends the action and the new
    /// layout comes back in a state sync. Nothing re-anchors the window's focus
    /// path across that sync, so without this a close either strands focus on a
    /// pane that no longer exists at that path or drops it entirely. The intent
    /// is captured here — while the pre-close tree is still available to reason
    /// about — and resolved by terminal id after the sync.
    ///
    /// `focused` is the window's current focus. Focus in another project is
    /// left alone; focus on a terminal that survives the close is re-anchored
    /// to that same terminal (its path may shift); focus on a closing terminal
    /// (or no focus at all) moves to the neighbour that takes its place.
    pub fn queue_focus_after_close(
        &mut self,
        window_id: WindowId,
        project_id: &str,
        closing_terminal_ids: &[String],
        focused: Option<&FocusedTerminalState>,
    ) {
        let Some(layout) = self.project(project_id).and_then(|p| p.layout.as_ref()) else {
            return;
        };
        let closing: HashSet<&str> = closing_terminal_ids.iter().map(String::as_str).collect();

        let focused_id = focused
            .filter(|f| f.project_id == project_id)
            .and_then(|f| layout.get_at_path(&f.layout_path))
            .and_then(|node| match node {
                LayoutNode::Terminal { terminal_id, .. } => terminal_id.clone(),
                _ => None,
            });

        let next = match focused_id {
            Some(id) if !closing.contains(id.as_str()) => Some(id),
            focused_id => {
                // Focus is on one of the closing terminals, or the window has no
                // terminal focus at all and this close is its chance to recover.
                if focused_id.is_none() && focused.is_some_and(|f| f.project_id != project_id) {
                    return;
                }
                let neighbour = focused_id
                    .or_else(|| closing_terminal_ids.first().cloned())
                    .and_then(|id| layout.find_terminal_path(&id))
                    .and_then(|path| layout.terminal_to_focus_after_closing(&path))
                    .filter(|id| !closing.contains(id.as_str()));
                // A bulk close (close others / close to the right) can swallow
                // the neighbour too — fall back to whatever the pruned tree
                // leaves visible.
                neighbour.or_else(|| {
                    let mut remaining = Some(layout.clone());
                    LayoutNode::remove_terminal_ids(&mut remaining, &closing);
                    remaining.and_then(|layout| layout.visible_terminal_id())
                })
            }
        };

        self.remote_sync.queue_close_focus(
            window_id,
            project_id,
            closing_terminal_ids.to_vec(),
            next,
        );
    }

    pub fn queue_pending_remote_project_visibility(
        &mut self,
        window_id: WindowId,
        connection_id: &str,
        name: &str,
        path: Option<&str>,
    ) {
        self.remote_sync
            .queue_project_visibility(window_id, connection_id, name, path);
    }

    pub fn take_pending_remote_project_visibility(
        &mut self,
        connection_id: &str,
        name: &str,
        path: &str,
    ) -> Option<WindowId> {
        self.remote_sync
            .take_project_visibility(connection_id, name, path)
    }

    pub fn remote_snapshot(&self, project_id: &str) -> Option<&RemoteProjectSnapshot> {
        self.remote_sync.snapshot(project_id)
    }

    /// Update the saved service terminal IDs for a project.
    /// Called by the ServiceManager observer to persist terminal IDs across restarts.
    pub fn sync_service_terminals(
        &mut self,
        project_id: &str,
        terminals: HashMap<String, String>,
        cx: &mut impl WorkspaceCx,
    ) {
        if let Some(project) = self.project_mut(project_id)
            && project.service_terminals != terminals
        {
            project.service_terminals = terminals;
            self.notify_data(cx);
        }
    }

    pub fn register_hook_terminal(
        &mut self,
        project_id: &str,
        terminal_id: &str,
        entry: HookTerminalEntry,
        cx: &mut impl WorkspaceCx,
    ) {
        if let Some(project) = self.project_mut(project_id) {
            let label = entry.label.clone();
            project
                .hook_terminals
                .insert(terminal_id.to_string(), entry);

            // Hook terminals are displayed in the dedicated HookPanel (not in the layout tree).
            // Set the terminal name so the panel can display it.
            project
                .terminal_names
                .insert(terminal_id.to_string(), label);

            self.notify_data(cx);
        }
    }

    /// Register hook terminal results from a hook execution.
    /// Convenience wrapper that converts `HookTerminalResult`s into `HookTerminalEntry`s.
    pub fn register_hook_results(
        &mut self,
        results: Vec<crate::hooks::HookTerminalResult>,
        cx: &mut impl WorkspaceCx,
    ) {
        for result in results {
            self.register_hook_terminal(
                &result.project_id,
                &result.terminal_id,
                HookTerminalEntry {
                    label: result.label,
                    status: HookTerminalStatus::Running,
                    hook_type: result.hook_type.to_string(),
                    command: result.command,
                    cwd: result.cwd,
                },
                cx,
            );
        }
    }

    pub fn update_hook_terminal_status(
        &mut self,
        terminal_id: &str,
        status: HookTerminalStatus,
        cx: &mut impl WorkspaceCx,
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

    pub fn remove_hook_terminal(&mut self, terminal_id: &str, cx: &mut impl WorkspaceCx) {
        for project in &mut self.data.projects {
            if project.hook_terminals.remove(terminal_id).is_some() {
                if let Some(ref layout) = project.layout
                    && let Some(path) = layout.find_terminal_path(terminal_id)
                {
                    if path.is_empty() {
                        project.layout = None;
                    } else if let Some(ref mut layout) = project.layout {
                        layout.remove_at_path(&path);
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
            p.layout
                .as_ref()
                .is_some_and(|l| l.find_terminal_path(terminal_id).is_some())
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
        cx: &mut impl WorkspaceCx,
    ) {
        let Some(project) = self.project_mut(project_id) else {
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
        // Set the wire-facing marker so clients render "Closing…" authoritatively
        // for the whole before-remove hook window (not just off their optimistic
        // flag), and so an abort that clears it heals the row.
        self.set_project_closing_flag(&pending.project_id, true);
        self.lifecycle.register_pending_close(pending);
    }

    /// Take a pending worktree close for the given terminal ID (removes it).
    pub fn take_pending_worktree_close(
        &mut self,
        terminal_id: &str,
    ) -> Option<PendingWorktreeClose> {
        self.lifecycle.take_pending_close(terminal_id)
    }

    /// Cancel a pending worktree close: remove it and unmark the project as closing.
    pub fn cancel_pending_worktree_close(&mut self, terminal_id: &str) {
        if let Some(project_id) = self.lifecycle.cancel_pending_close(terminal_id) {
            self.set_project_closing_flag(&project_id, false);
        }
    }

    /// Snapshot before-remove hook terminal IDs awaiting authoritative completion.
    ///
    /// The returned IDs are only candidates; callers must use
    /// [`Self::abort_orphaned_worktree_close`] to atomically claim an orphan.
    pub fn pending_worktree_close_terminal_ids(&self) -> Vec<String> {
        self.lifecycle.pending_close_terminal_ids()
    }

    /// Abort a pending close whose before-remove hook PTY vanished without an
    /// authoritative exit result. This is intentionally state-only: it retains
    /// the project and worktree, does not run removal hooks, and is idempotent.
    ///
    /// The lifecycle record, in-memory closing marker, wire-facing closing flag,
    /// and still-running hook entry are healed in one workspace mutation. A
    /// caller that sees `None` lost the race to a normal exit, rerun, data
    /// replacement, or another watchdog pass.
    pub fn abort_orphaned_worktree_close(
        &mut self,
        terminal_id: &str,
        cx: &mut impl WorkspaceCx,
    ) -> Option<AbortedWorktreeClose> {
        let project_id = self.lifecycle.cancel_pending_close(terminal_id)?;
        let project = self.data.projects.iter_mut().find(|p| p.id == project_id);
        let project_name = project
            .as_ref()
            .map(|project| project.name.clone())
            .unwrap_or_else(|| project_id.clone());
        if let Some(project) = project {
            project.is_closing = false;
            if let Some(entry) = project.hook_terminals.get_mut(terminal_id)
                && entry.status == HookTerminalStatus::Running
            {
                entry.status = HookTerminalStatus::Failed { exit_code: -1 };
            }
        }
        cx.notify();
        Some(AbortedWorktreeClose {
            project_id,
            project_name,
        })
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
        let window_state = self
            .data
            .window(window_id)
            .unwrap_or(&self.data.main_window);
        compute_visible_projects(
            &self.data,
            focused_project_id,
            focus_individual,
            window_state,
        )
    }

    /// Union of project IDs visible in *any* window (main + extras).
    ///
    /// Uses each window's persistent visibility (folder filter + hidden set)
    /// but deliberately *not* the transient fullscreen-focus narrowing
    /// (`focus_individual`), so the set stays stable as focus moves around.
    /// Used by the git-status watcher to scope expensive `gh` PR/CI polling to
    /// projects the user can actually see somewhere.
    pub fn all_visible_project_ids(&self) -> std::collections::HashSet<String> {
        let mut ids = std::collections::HashSet::new();
        for window in std::iter::once(&self.data.main_window).chain(self.data.extra_windows.iter())
        {
            for p in compute_visible_projects(&self.data, None, false, window) {
                ids.insert(p.id.clone());
            }
        }
        ids
    }

    /// Get IDs of worktree children for a given parent project.
    pub fn worktree_child_ids(&self, parent_id: &str) -> Vec<String> {
        self.data
            .projects
            .iter()
            .filter(|p| {
                p.worktree_info
                    .as_ref()
                    .is_some_and(|w| w.parent_project_id == parent_id)
            })
            .map(|p| p.id.clone())
            .collect()
    }

    /// Get a project by ID
    pub fn project(&self, id: &str) -> Option<&ProjectData> {
        self.data.projects.iter().find(|p| p.id == id)
    }

    /// True when the project is served by the co-located local daemon (shared
    /// filesystem) — i.e. local paths are openable on this machine. A project
    /// mirrored from a user-added remote connection returns false. A project
    /// with no connection is treated as local (legacy / non-headless).
    pub fn is_local_daemon_project(&self, project_id: &str) -> bool {
        match self
            .project(project_id)
            .and_then(|p| p.connection_id.as_deref())
        {
            Some(id) => id == okena_transport::client::LOCAL_DAEMON_CONNECTION_ID,
            None => true,
        }
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
        self.data
            .folders
            .iter()
            .find(|f| f.project_ids.contains(&project_id.to_string()))
    }

    /// Find folder for a project, falling back to the parent project's folder for worktrees.
    pub fn folder_for_project_or_parent(&self, project_id: &str) -> Option<&FolderData> {
        self.folder_for_project(project_id).or_else(|| {
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
    pub fn remove_remote_projects(
        &mut self,
        focus_manager: &mut FocusManager,
        connection_id: &str,
        cx: &mut impl WorkspaceCx,
    ) {
        let prefix = format!("remote:{}:", connection_id);

        let removed_project_ids: Vec<String> = self
            .data
            .projects
            .iter()
            .filter(|p| p.id.starts_with(&prefix))
            .map(|p| p.id.clone())
            .collect();
        let removed_folder_ids: Vec<String> = self
            .data
            .folders
            .iter()
            .filter(|f| f.id.starts_with(&prefix))
            .map(|f| f.id.clone())
            .collect();

        self.data.projects.retain(|p| !p.id.starts_with(&prefix));
        self.data.folders.retain(|f| !f.id.starts_with(&prefix));
        self.data
            .project_order
            .retain(|id| !id.starts_with(&prefix));

        for project_id in &removed_project_ids {
            self.data.delete_project_scrub_all_windows(project_id);
        }
        for folder_id in &removed_folder_ids {
            self.data.delete_folder_scrub_all_windows(folder_id);
        }

        for project_id in self.remote_sync.retain_not_starting_with(&prefix) {
            self.data.delete_project_scrub_all_windows(&project_id);
        }

        if let Some(focused) = focus_manager.focused_project_id()
            && focused.starts_with(&prefix)
        {
            focus_manager.set_focused_project_id(None);
        }

        cx.notify();
    }

    /// Notify UI without bumping data_version (for remote state changes that shouldn't trigger auto-save).
    pub fn notify_ui_only(&mut self, cx: &mut impl WorkspaceCx) {
        cx.notify();
    }

    /// Reconcile remote connection snapshots into the workspace.
    ///
    /// Runs the pure reconciliation core (`remote_apply::apply_remote_snapshot`)
    /// on `self.data`/`self.remote_sync`, then applies the GPUI side-effects:
    /// focuses any newly-created terminals and notifies the UI without bumping
    /// `data_version` (remote changes shouldn't trigger auto-save).
    pub fn apply_remote_snapshot(
        &mut self,
        snapshots: &[crate::remote_apply::RemoteSnapshot],
        window_id: WindowId,
        focus_manager: &mut FocusManager,
        cx: &mut impl WorkspaceCx,
    ) {
        // Capture what this window is looking at before reconciliation reshapes
        // the tree — see `reanchor_focus`.
        let anchor = self.capture_focus_anchor(focus_manager);

        let outcome = crate::remote_apply::apply_remote_snapshot(
            &mut self.data,
            &mut self.remote_sync,
            snapshots,
            window_id,
        );

        // Heal the optimistic client-side "closing" flag against the daemon's
        // authoritative mirror. The dialog marks a project closing locally before
        // dispatch for instant feedback; the daemon then sets `is_closing` on the
        // mirrored project while the before-remove hook runs and clears it on
        // abort. Keep the local flag only while the mirror still reports closing —
        // any project the mirror reports as not-closing (hook aborted) or that
        // vanished when the close completed drops its local flag, so an aborted
        // close no longer strands the row dimmed "Closing…" forever.
        let still_closing: std::collections::HashSet<String> = self
            .data
            .projects
            .iter()
            .filter(|p| p.is_closing)
            .map(|p| p.id.clone())
            .collect();
        self.lifecycle.retain_closing(&still_closing);

        for target in outcome.focus_targets {
            self.set_focused_terminal(focus_manager, target.project_id, target.layout_path, cx);
        }

        self.reanchor_focus(focus_manager, anchor.as_ref());

        // Notify UI without bumping data_version (remote changes shouldn't trigger auto-save)
        self.notify_ui_only(cx);
    }

    /// Record which terminal a window is focused on, before a sync reshapes the
    /// layout under it. `None` when the window has no focus, or its path no
    /// longer names a pane (already orphaned — nothing to follow).
    fn capture_focus_anchor(&self, focus_manager: &FocusManager) -> Option<FocusAnchor> {
        let previous = focus_manager.focused_terminal_state()?;
        let layout = self.project(&previous.project_id)?.layout.as_ref()?;
        let terminal_id = terminal_slot_at(layout, &previous.layout_path)??.to_string();
        // Computed on the pre-sync tree, while the focused pane's position is
        // still known: where focus goes if this sync took that pane away.
        let replacement = layout.terminal_to_focus_after_closing(&previous.layout_path);
        Some(FocusAnchor {
            previous,
            terminal_id,
            replacement,
        })
    }

    /// Keep a window pointed at the terminal it was on across a sync.
    ///
    /// Focus is a layout *path*, and any change to the tree can move it: a pane
    /// closed in another window, a shell that exited on its own, a daemon-side
    /// move. The path then silently addresses a different terminal, or nothing
    /// at all — leaving the window with no keyboard focus. Windows that issued
    /// the change get an explicit target (`queue_focus_after_close`); this is
    /// the passive counterpart every window gets for changes it didn't make, so
    /// it defers to a target that already moved focus this pass.
    ///
    /// Ordinary focus changes (clicks, navigation) are not activity in the
    /// project sense, so this deliberately bypasses `set_focused_terminal`: no
    /// `bump_activity`, no sidebar re-sort, no debounced save.
    fn reanchor_focus(&self, focus_manager: &mut FocusManager, anchor: Option<&FocusAnchor>) {
        // A modal owns focus and restores its own target when it closes.
        if focus_manager.is_modal() {
            return;
        }
        let Some(current) = focus_manager.focused_terminal_state() else {
            return;
        };
        // An explicit focus target already moved this window — it wins.
        if anchor.is_some_and(|anchor| anchor.previous != current) {
            return;
        }
        let Some(layout) = self
            .project(&current.project_id)
            .and_then(|p| p.layout.as_ref())
        else {
            // A project with no layout keeps focus on the project itself.
            return;
        };

        let replacement = match anchor {
            Some(anchor) => {
                if terminal_slot_at(layout, &current.layout_path)
                    == Some(Some(anchor.terminal_id.as_str()))
                {
                    return; // Still on the same terminal.
                }
                // Follow it if it merely moved; otherwise take the neighbour
                // resolved against the tree it lived in.
                layout.find_terminal_path(&anchor.terminal_id).or_else(|| {
                    anchor
                        .replacement
                        .as_ref()
                        .and_then(|id| layout.find_terminal_path(id))
                })
            }
            // Focus was already orphaned before this sync, so there is nothing
            // to follow — but if the path names a pane again, leave it be.
            None if terminal_slot_at(layout, &current.layout_path).is_some() => return,
            None => None,
        };

        let path = replacement.unwrap_or_else(|| layout.find_visible_terminal_path());
        // A degenerate tree (an empty container) has no terminal to offer, and
        // `find_visible_terminal_path` would hand back the container's own path
        // — leaving focus just as orphaned and re-running this every sync.
        if path != current.layout_path && terminal_slot_at(layout, &path).is_some() {
            focus_manager.focus_terminal(current.project_id, path);
        }
    }

    /// Helper to mutate a layout node at a path, with automatic notify.
    /// Returns true if the mutation was applied.
    pub fn with_layout_node<F>(
        &mut self,
        project_id: &str,
        path: &[usize],
        cx: &mut impl WorkspaceCx,
        f: F,
    ) -> bool
    where
        F: FnOnce(&mut LayoutNode) -> bool,
    {
        if let Some(project) = self.project_mut(project_id)
            && let Some(ref mut layout) = project.layout
            && let Some(node) = layout.get_at_path_mut(path)
            && f(node)
        {
            self.notify_data(cx);
            return true;
        }
        false
    }

    /// Helper to mutate a project, with automatic notify.
    /// Returns true if the mutation was applied.
    pub fn with_project<F>(&mut self, project_id: &str, cx: &mut impl WorkspaceCx, f: F) -> bool
    where
        F: FnOnce(&mut ProjectData) -> bool,
    {
        if let Some(project) = self.project_mut(project_id)
            && f(project)
        {
            self.notify_data(cx);
            return true;
        }
        false
    }
}

#[cfg(test)]
mod workspace_tests {
    use super::normalize_unresolved_component;
    use crate::context::WorkspaceCx;
    use crate::settings::HooksConfig;
    use crate::state::{
        FolderData, HookTerminalEntry, HookTerminalStatus, LayoutNode, ProjectData, SplitDirection,
        WindowId, WindowState, Workspace, WorkspaceData, WorktreeMetadata,
    };
    use okena_core::theme::FolderColor;
    use okena_terminal::session_backend::SessionBackend;
    use okena_terminal::shell_config::ShellType;
    use std::collections::HashMap;

    #[derive(Default)]
    struct RecordingCx {
        notifications: usize,
    }

    impl WorkspaceCx for RecordingCx {
        fn notify(&mut self) {
            self.notifications += 1;
        }

        fn refresh_views(&mut self) {}

        fn hook_runner(&self) -> Option<okena_hooks::HookRunner> {
            None
        }

        fn hook_monitor(&self) -> Option<okena_hooks::HookMonitor> {
            None
        }
    }

    #[test]
    fn orphaned_worktree_close_aborts_atomically_and_idempotently() {
        let mut project = make_project("wt1");
        project.name = "Feature".into();
        project.hook_terminals.insert(
            "hook-1".into(),
            HookTerminalEntry {
                label: "Before remove".into(),
                status: HookTerminalStatus::Running,
                hook_type: "before_worktree_remove".into(),
                command: "true".into(),
                cwd: "/tmp".into(),
            },
        );
        let mut workspace = Workspace::new(make_workspace_data(vec![project], vec!["wt1"]));
        workspace.register_pending_worktree_close(crate::state::PendingWorktreeClose {
            project_id: "wt1".into(),
            hook_terminal_id: "hook-1".into(),
            branch: "feature".into(),
            main_repo_path: "/tmp".into(),
        });
        assert!(workspace.is_project_closing("wt1"));
        assert!(workspace.project("wt1").unwrap().is_closing);

        let mut cx = RecordingCx::default();
        assert_eq!(
            workspace.abort_orphaned_worktree_close("hook-1", &mut cx),
            Some(crate::state::AbortedWorktreeClose {
                project_id: "wt1".into(),
                project_name: "Feature".into(),
            })
        );
        assert!(!workspace.is_project_closing("wt1"));
        let project = workspace.project("wt1").expect("project retained");
        assert!(!project.is_closing);
        assert!(matches!(
            project.hook_terminals["hook-1"].status,
            HookTerminalStatus::Failed { exit_code: -1 }
        ));
        assert_eq!(cx.notifications, 1);
        assert!(workspace.pending_worktree_close_terminal_ids().is_empty());
        assert!(
            workspace
                .abort_orphaned_worktree_close("hook-1", &mut cx)
                .is_none()
        );
        assert_eq!(cx.notifications, 1, "second claim is a no-op");
    }

    #[test]
    fn terminal_backend_migration_gate_is_exclusive_and_epoch_fenced() {
        let mut workspace = Workspace::new(WorkspaceData::empty());
        let initial_epoch = workspace.data_replacement_epoch();
        let migration = workspace
            .begin_terminal_backend_migration(SessionBackend::None, &ShellType::Default)
            .expect("begin migration");
        let migration_epoch = migration.epoch;

        assert_eq!(migration_epoch, initial_epoch + 1);
        assert_eq!(
            workspace.terminal_backend_migration_epoch(),
            Some(migration_epoch)
        );
        assert!(
            workspace
                .begin_terminal_backend_migration(SessionBackend::None, &ShellType::Default)
                .is_err()
        );

        let mut cx = RecordingCx::default();
        assert!(!workspace.finish_terminal_backend_migration(migration_epoch + 1, &mut cx));
        assert_eq!(cx.notifications, 0);
        assert!(workspace.finish_terminal_backend_migration(migration_epoch, &mut cx));
        assert_eq!(workspace.terminal_backend_migration_epoch(), None);
        assert_eq!(cx.notifications, 1);
    }

    #[test]
    fn backend_migration_clears_and_restores_only_ordinary_terminal_ownership() {
        let mut project = make_project("p1");
        project
            .service_terminals
            .insert("web".to_string(), "service-1".to_string());
        project.hook_terminals.insert(
            "hook-1".to_string(),
            HookTerminalEntry {
                label: "hook".to_string(),
                status: HookTerminalStatus::Succeeded,
                hook_type: "project.on_open".to_string(),
                command: "echo hook".to_string(),
                cwd: "/tmp/test".to_string(),
            },
        );
        project
            .terminal_names
            .insert("term_p1".to_string(), "ordinary".to_string());
        project
            .terminal_names
            .insert("hook-1".to_string(), "hook".to_string());
        if let Some(LayoutNode::Terminal {
            minimized,
            detached,
            ..
        }) = &mut project.layout
        {
            *minimized = true;
            *detached = true;
        }
        let mut workspace = Workspace::new(make_workspace_data(vec![project], vec!["p1"]));

        let migration = workspace
            .begin_terminal_backend_migration(SessionBackend::None, &ShellType::Default)
            .expect("begin migration");

        let project = workspace.project("p1").expect("project");
        assert!(matches!(
            project.layout.as_ref(),
            Some(LayoutNode::Terminal {
                terminal_id: None,
                minimized: true,
                detached: true,
                ..
            })
        ));
        assert!(project.service_terminals.is_empty());
        assert!(project.hook_terminals.is_empty());
        assert_eq!(
            project.terminal_names.get("term_p1").map(String::as_str),
            Some("ordinary")
        );
        assert!(!project.terminal_names.contains_key("hook-1"));
        assert_eq!(migration.hook_terminal_ids, vec!["hook-1"]);
        assert_eq!(
            migration
                .teardown_sessions
                .iter()
                .map(|session| session.terminal_id.as_str())
                .collect::<Vec<_>>(),
            vec!["hook-1", "service-1", "term_p1"]
        );

        workspace
            .restore_terminal_backend_migration_slots(&migration)
            .expect("restore slots");
        assert!(matches!(
            workspace.project("p1").and_then(|project| project.layout.as_ref()),
            Some(LayoutNode::Terminal {
                terminal_id: Some(terminal_id),
                minimized: true,
                detached: true,
                ..
            }) if terminal_id == "term_p1"
        ));
    }

    #[test]
    fn case_insensitive_unresolved_components_share_identity() {
        assert_eq!(
            normalize_unresolved_component(&"NewWorktree".into(), false),
            normalize_unresolved_component(&"newworktree".into(), false)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn nonexistent_suffix_follows_volume_case_semantics() {
        let fixture =
            std::env::temp_dir().join(format!("okena-case-volume-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&fixture).unwrap();
        if !super::filesystem_is_case_sensitive(&fixture) {
            assert_eq!(
                Workspace::physical_path_identity(&fixture.join("NewWorktree/project")),
                Workspace::physical_path_identity(&fixture.join("newworktree/PROJECT"))
            );
        }
        let _ = std::fs::remove_dir_all(fixture);
    }

    #[cfg(windows)]
    #[test]
    fn subst_aliases_share_filesystem_identity() {
        struct SubstGuard(String);

        impl Drop for SubstGuard {
            fn drop(&mut self) {
                let _ = std::process::Command::new("subst")
                    .args([&self.0, "/D"])
                    .status();
            }
        }

        let fixture = std::env::temp_dir().join(format!(
            "okena-subst-identity-test-{}",
            uuid::Uuid::new_v4()
        ));
        let root = fixture.join("worktree");
        std::fs::create_dir_all(&root).unwrap();
        let fixture_text = fixture.to_string_lossy().into_owned();
        let Some(drive) = (b'D'..=b'Z').rev().find_map(|letter| {
            let drive = format!("{}:", char::from(letter));
            std::process::Command::new("subst")
                .args([&drive, &fixture_text])
                .status()
                .ok()
                .filter(|status| status.success())
                .map(|_| drive)
        }) else {
            let _ = std::fs::remove_dir_all(fixture);
            return;
        };
        let guard = SubstGuard(drive.clone());
        let mapped_root = std::path::PathBuf::from(format!("{drive}\\worktree"));

        let real = Workspace::physical_path_identity(&root);
        let mapped = Workspace::physical_path_identity(&mapped_root);
        let mapped_child = Workspace::physical_path_identity(&mapped_root.join("packages/app"));
        assert_eq!(real, mapped);
        assert!(mapped_child.starts_with(&real));

        drop(guard);
        let _ = std::fs::remove_dir_all(fixture);
    }

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
            pinned: false,
            last_activity_at: None,
            is_creating: false,
            is_closing: false,
            creating_progress: None,
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

    /// Project whose layout is a tab group over `terminal_ids`.
    fn project_with_tabs(id: &str, terminal_ids: &[&str], active_tab: usize) -> ProjectData {
        let mut project = make_project(id);
        project.layout = Some(LayoutNode::Tabs {
            children: terminal_ids
                .iter()
                .map(|tid| LayoutNode::Terminal {
                    terminal_id: Some((*tid).to_string()),
                    minimized: false,
                    detached: false,
                    shell_type: ShellType::Default,
                    zoom_level: 1.0,
                })
                .collect(),
            active_tab,
        });
        project
    }

    fn focused(project_id: &str, layout_path: Vec<usize>) -> crate::state::FocusedTerminalState {
        crate::state::FocusedTerminalState {
            project_id: project_id.to_string(),
            layout_path,
        }
    }

    /// Queue a close intent and read back the terminal it resolved to.
    fn close_intent(
        workspace: &mut Workspace,
        closing: &[&str],
        focus: Option<crate::state::FocusedTerminalState>,
    ) -> Option<String> {
        let closing: Vec<String> = closing.iter().map(|id| (*id).to_string()).collect();
        workspace.queue_focus_after_close(WindowId::Main, "p1", &closing, focus.as_ref());
        workspace
            .remote_sync
            .take_close_focus(WindowId::Main)
            .expect("close intent queued")
            .next_terminal_id
    }

    #[test]
    fn closing_the_focused_tab_hands_focus_to_the_tab_that_becomes_visible() {
        let mut workspace = Workspace::new(make_workspace_data(
            vec![project_with_tabs("p1", &["t1", "t2", "t3"], 0)],
            vec!["p1"],
        ));

        assert_eq!(
            close_intent(&mut workspace, &["t1"], Some(focused("p1", vec![0]))),
            Some("t2".to_string())
        );
    }

    #[test]
    fn closing_a_background_tab_re_anchors_focus_onto_the_same_terminal() {
        // t2 is focused and survives; its path shifts from [1] to [0] when t1
        // goes away, so the intent must name t2 rather than keep the path.
        let mut workspace = Workspace::new(make_workspace_data(
            vec![project_with_tabs("p1", &["t1", "t2", "t3"], 1)],
            vec!["p1"],
        ));

        assert_eq!(
            close_intent(&mut workspace, &["t1"], Some(focused("p1", vec![1]))),
            Some("t2".to_string())
        );
    }

    #[test]
    fn closing_the_last_terminal_leaves_focus_on_the_project() {
        let mut workspace =
            Workspace::new(make_workspace_data(vec![make_project("p1")], vec!["p1"]));

        assert_eq!(
            close_intent(&mut workspace, &["term_p1"], Some(focused("p1", vec![]))),
            None
        );
    }

    #[test]
    fn bulk_close_that_swallows_the_neighbour_falls_back_to_a_survivor() {
        // "Close other tabs" from t3: t1 (focused) and t2 both go, so the
        // neighbouring-tab rule is useless and only t3 is left to focus.
        let mut workspace = Workspace::new(make_workspace_data(
            vec![project_with_tabs("p1", &["t1", "t2", "t3"], 0)],
            vec!["p1"],
        ));

        assert_eq!(
            close_intent(&mut workspace, &["t1", "t2"], Some(focused("p1", vec![0]))),
            Some("t3".to_string())
        );
    }

    #[test]
    fn closing_a_terminal_in_another_project_leaves_focus_alone() {
        let mut workspace = Workspace::new(make_workspace_data(
            vec![
                project_with_tabs("p1", &["t1", "t2"], 0),
                make_project("p2"),
            ],
            vec!["p1", "p2"],
        ));

        let closing = vec!["t1".to_string()];
        workspace.queue_focus_after_close(
            WindowId::Main,
            "p1",
            &closing,
            Some(&focused("p2", vec![])),
        );

        assert!(
            workspace
                .remote_sync
                .take_close_focus(WindowId::Main)
                .is_none()
        );
    }

    #[test]
    fn closing_with_nothing_focused_recovers_focus_into_the_project() {
        let mut workspace = Workspace::new(make_workspace_data(
            vec![project_with_tabs("p1", &["t1", "t2"], 0)],
            vec!["p1"],
        ));

        assert_eq!(
            close_intent(&mut workspace, &["t1"], None),
            Some("t2".to_string())
        );
    }

    #[test]
    fn project_runtime_quiesce_batch_is_all_or_nothing() {
        let mut blocked = make_project("blocked");
        blocked.is_remote = true;
        let mut workspace = Workspace::new(make_workspace_data(
            vec![make_project("ready"), blocked],
            vec!["ready", "blocked"],
        ));
        let mut cx = RecordingCx::default();

        let error = workspace
            .begin_project_runtimes_quiesce(
                &["ready".to_string(), "blocked".to_string()],
                &ShellType::Default,
                SessionBackend::None,
                true,
                &mut cx,
            )
            .expect_err("remote descendant rejects the full batch");

        assert!(error.contains("remote project"));
        assert!(matches!(
            workspace.project("ready").and_then(|project| project.layout.as_ref()),
            Some(LayoutNode::Terminal {
                terminal_id: Some(terminal_id),
                ..
            }) if terminal_id == "term_ready"
        ));
        assert!(!workspace.is_project_closing("ready"));
        assert_eq!(cx.notifications, 0);
    }

    #[test]
    fn project_runtime_quiesce_preserves_completed_hooks_and_fences_aba() {
        let mut project = make_project("p1");
        project.hook_terminals.insert(
            "completed-hook".to_string(),
            HookTerminalEntry {
                label: "completed".to_string(),
                status: HookTerminalStatus::Succeeded,
                hook_type: "project.on_open".to_string(),
                command: "echo done".to_string(),
                cwd: "/tmp/test".to_string(),
            },
        );
        project.hook_terminals.insert(
            "running-hook".to_string(),
            HookTerminalEntry {
                label: "running".to_string(),
                status: HookTerminalStatus::Running,
                hook_type: "project.on_open".to_string(),
                command: "sleep 10".to_string(),
                cwd: "/tmp/test".to_string(),
            },
        );
        project
            .terminal_names
            .insert("completed-hook".to_string(), "completed".to_string());
        project
            .terminal_names
            .insert("running-hook".to_string(), "running".to_string());
        let mut workspace = Workspace::new(make_workspace_data(vec![project], vec!["p1"]));
        let mut cx = RecordingCx::default();

        let first = workspace
            .begin_project_runtime_quiesce(
                "p1",
                &ShellType::Default,
                SessionBackend::None,
                false,
                &mut cx,
            )
            .expect("quiesce project");
        assert_eq!(first.hook_terminal_ids, vec!["running-hook"]);
        assert_eq!(
            first.preserved_registry_terminal_ids,
            vec!["completed-hook"]
        );
        let project = workspace.project("p1").expect("project");
        assert!(project.hook_terminals.contains_key("completed-hook"));
        assert!(!project.hook_terminals.contains_key("running-hook"));
        assert!(project.terminal_names.contains_key("completed-hook"));
        assert!(!project.terminal_names.contains_key("running-hook"));

        workspace.finish_project_runtime_recovery(&first, &mut cx);
        let second = workspace
            .begin_project_runtime_quiesce(
                "p1",
                &ShellType::Default,
                SessionBackend::None,
                false,
                &mut cx,
            )
            .expect("quiesce project again");
        assert_ne!(
            first.runtime_quiesce_generation,
            second.runtime_quiesce_generation
        );
        workspace.finish_project_runtime_recovery(&first, &mut cx);
        assert!(workspace.project_runtime_quiesce_is_current(&second));
        assert!(workspace.is_project_closing("p1"));

        let mut focus = crate::focus::FocusManager::new();
        workspace.replace_data(
            &mut focus,
            make_workspace_data(vec![make_project("p1")], vec!["p1"]),
            &mut cx,
        );
        let replacement = workspace
            .begin_project_runtime_quiesce(
                "p1",
                &ShellType::Default,
                SessionBackend::None,
                false,
                &mut cx,
            )
            .expect("quiesce replacement project");
        assert_eq!(
            first.runtime_quiesce_generation, replacement.runtime_quiesce_generation,
            "replacement tracker may reuse generations"
        );
        workspace.finish_project_runtime_recovery(&first, &mut cx);
        assert!(workspace.project_runtime_quiesce_is_current(&replacement));
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

        let visible = ws.visible_projects(
            WindowId::Main,
            fm.focused_project_id(),
            fm.is_focus_individual(),
        );
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "p3");
    }

    #[test]
    fn test_visible_projects_with_folder() {
        let mut data =
            make_workspace_data(vec![make_project("p1"), make_project("p2")], vec!["f1"]);
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
                make_project("p1"),
                make_project("p2"),
                make_project("p3"),
                make_project("p4"),
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
            vec![make_project("p1"), make_project("p2"), make_project("p3")],
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

        let data = make_workspace_data(vec![p1, w1, w2, make_project("p2")], vec!["p1", "p2"]);
        let ws = Workspace::new(data);
        let mut fm = crate::focus::FocusManager::new();

        fm.set_focused_project_id(Some("p1".to_string()));
        let visible = ws.visible_projects(
            WindowId::Main,
            fm.focused_project_id(),
            fm.is_focus_individual(),
        );
        assert_eq!(visible.len(), 3);
        assert_eq!(visible[0].id, "p1");
        assert_eq!(visible[1].id, "w1");
        assert_eq!(visible[2].id, "w2");

        fm.set_focused_project_id(Some("w1".to_string()));
        let visible = ws.visible_projects(
            WindowId::Main,
            fm.focused_project_id(),
            fm.is_focus_individual(),
        );
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "w1");

        fm.set_focused_project_id(None);
        let visible = ws.visible_projects(
            WindowId::Main,
            fm.focused_project_id(),
            fm.is_focus_individual(),
        );
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

        let mut data = make_workspace_data(vec![p1, w1, w2, make_project("p2")], vec!["f1", "p2"]);
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

        let mut data =
            make_workspace_data(vec![p1, w1, make_project("p2")], vec!["f1", "w1", "p2"]);
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

        let mut data =
            make_workspace_data(vec![make_project("p1"), p2, w1], vec!["w1", "f1", "f2"]);
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

        let mut data =
            make_workspace_data(vec![p1, make_project("p2"), w1], vec!["f1", "w1", "f2"]);
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

        let mut data = make_workspace_data(vec![make_project("p1"), w1], vec!["p1", "w1"]);
        data.main_window.hidden_project_ids.insert("p1".to_string());
        let ws = Workspace::new(data);

        let visible = ws.visible_projects(WindowId::Main, None, false);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "w1");
    }

    #[test]
    fn test_folder_filter_with_focus_override() {
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

        let mut ws = Workspace::new(data);
        ws.data.main_window.folder_filter = Some("f1".to_string());

        let mut fm = crate::focus::FocusManager::new();
        fm.set_focused_project_id(Some("p3".to_string()));

        let visible = ws.visible_projects(
            WindowId::Main,
            fm.focused_project_id(),
            fm.is_focus_individual(),
        );
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

        let visible = ws.visible_projects(
            WindowId::Main,
            fm.focused_project_id(),
            fm.is_focus_individual(),
        );
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

        let visible = ws.visible_projects(
            WindowId::Main,
            fm.focused_project_id(),
            fm.is_focus_individual(),
        );
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
        let visible = ws.visible_projects(
            WindowId::Main,
            fm.focused_project_id(),
            fm.is_focus_individual(),
        );
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "parent");

        fm.set_focused_project_id(Some("parent".to_string()));
        let visible = ws.visible_projects(
            WindowId::Main,
            fm.focused_project_id(),
            fm.is_focus_individual(),
        );
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

#[cfg(all(test, feature = "gpui"))]
mod gpui_tests {
    use crate::settings::HooksConfig;
    use crate::state::{
        HookTerminalEntry, HookTerminalStatus, LayoutNode, ProjectData, ProjectLayoutMode,
        WindowBounds, WindowId, WindowState, Workspace, WorkspaceData,
    };
    use gpui::AppContext as _;
    use okena_core::theme::FolderColor;
    use okena_terminal::shell_config::ShellType;
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
            pinned: false,
            last_activity_at: None,
            is_creating: false,
            is_closing: false,
            creating_progress: None,
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

    fn pane(terminal_id: &str) -> LayoutNode {
        LayoutNode::Terminal {
            terminal_id: Some(terminal_id.to_string()),
            minimized: false,
            detached: false,
            shell_type: ShellType::Default,
            zoom_level: 1.0,
        }
    }

    fn split_of(terminal_ids: &[&str]) -> LayoutNode {
        LayoutNode::Split {
            direction: crate::state::SplitDirection::Horizontal,
            sizes: vec![100.0 / terminal_ids.len() as f32; terminal_ids.len()],
            children: terminal_ids.iter().map(|tid| pane(tid)).collect(),
        }
    }

    fn tabs_of(terminal_ids: &[&str], active_tab: usize) -> LayoutNode {
        LayoutNode::Tabs {
            children: terminal_ids.iter().map(|tid| pane(tid)).collect(),
            active_tab,
        }
    }

    /// Project whose layout is a two-pane horizontal split.
    fn project_with_split(id: &str, terminal_ids: [&str; 2]) -> ProjectData {
        let mut project = make_project(id);
        project.layout = Some(split_of(&terminal_ids));
        project
    }

    /// Focus `path` in `p1`, then replay what a sync brings: capture the anchor,
    /// swap in the layout the sync produced, resolve. Reports where focus landed
    /// and which terminal is there.
    fn reanchor_across(
        cx: &mut gpui::TestAppContext,
        before: LayoutNode,
        focus_path: Vec<usize>,
        after: LayoutNode,
    ) -> (Vec<usize>, Option<String>) {
        let mut project = make_project("p1");
        project.layout = Some(before);
        let workspace =
            cx.new(|_cx| Workspace::new(make_workspace_data(vec![project], vec!["p1"])));
        let mut fm = crate::focus::FocusManager::new();
        fm.focus_terminal("p1".to_string(), focus_path);

        workspace.update(cx, |ws: &mut Workspace, _cx| {
            let anchor = ws.capture_focus_anchor(&fm);
            ws.project_mut("p1").unwrap().layout = Some(after);
            ws.reanchor_focus(&mut fm, anchor.as_ref());
        });

        let focused = fm.focused_terminal_state().expect("focus retained");
        let terminal = workspace.read_with(cx, |ws: &Workspace, _cx| {
            ws.project("p1")
                .and_then(|p| p.layout.as_ref())
                .and_then(|layout| match layout.get_at_path(&focused.layout_path) {
                    Some(LayoutNode::Terminal { terminal_id, .. }) => terminal_id.clone(),
                    _ => None,
                })
        });
        (focused.layout_path, terminal)
    }

    #[gpui::test]
    fn focus_follows_its_terminal_when_another_window_reshapes_the_layout(
        cx: &mut gpui::TestAppContext,
    ) {
        // Focused on t3, the third pane; another window closes t1, shifting
        // every surviving pane one slot left. Focus has to follow t3 to [1] —
        // not fall back to whatever is first on screen.
        let (path, terminal) = reanchor_across(
            cx,
            split_of(&["t1", "t2", "t3"]),
            vec![2],
            split_of(&["t2", "t3"]),
        );

        assert_eq!(path, vec![1]);
        assert_eq!(terminal.as_deref(), Some("t3"));
    }

    #[gpui::test]
    fn focus_moves_to_the_neighbour_when_another_window_closes_its_terminal(
        cx: &mut gpui::TestAppContext,
    ) {
        // Focused on t3; another window closes it. The neighbour is resolved
        // against the tree t3 lived in — the same rule this window's own close
        // would have used — landing on t2, not on whatever is first on screen.
        let (path, terminal) = reanchor_across(
            cx,
            split_of(&["t1", "t2", "t3"]),
            vec![2],
            split_of(&["t1", "t2"]),
        );

        assert_eq!(path, vec![1]);
        assert_eq!(terminal.as_deref(), Some("t2"));
    }

    #[gpui::test]
    fn reanchoring_defers_to_a_focus_target_applied_this_sync(cx: &mut gpui::TestAppContext) {
        // Cmd+T: the sync brings a new tab and an explicit target focuses it.
        // The terminal this window was on is still very much alive, so a
        // re-anchor that followed it blindly would yank focus straight back
        // out of the tab the user just opened.
        let mut project = make_project("p1");
        project.layout = Some(tabs_of(&["t1", "t2"], 0));
        let workspace =
            cx.new(|_cx| Workspace::new(make_workspace_data(vec![project], vec!["p1"])));
        let mut fm = crate::focus::FocusManager::new();
        fm.focus_terminal("p1".to_string(), vec![0]);

        workspace.update(cx, |ws: &mut Workspace, _cx| {
            let anchor = ws.capture_focus_anchor(&fm);
            ws.project_mut("p1").unwrap().layout = Some(tabs_of(&["t1", "t2", "t3"], 2));
            fm.focus_terminal("p1".to_string(), vec![2]);
            ws.reanchor_focus(&mut fm, anchor.as_ref());
        });

        assert_eq!(
            fm.focused_terminal_state().map(|f| f.layout_path),
            Some(vec![2])
        );
    }

    /// Run a no-op remote sync (no connections) and report where focus ended up.
    fn sync_and_read_focus(
        workspace: &gpui::Entity<Workspace>,
        fm: &mut crate::focus::FocusManager,
        cx: &mut gpui::TestAppContext,
    ) -> Option<Vec<usize>> {
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.apply_remote_snapshot(&[], WindowId::Main, fm, cx);
        });
        fm.focused_terminal_state().map(|f| f.layout_path)
    }

    #[gpui::test]
    fn sync_re_anchors_focus_orphaned_by_a_close_this_window_did_not_make(
        cx: &mut gpui::TestAppContext,
    ) {
        // A shell exits on its own (or another window closes the pane): the
        // split collapses and the focused path stops naming a terminal.
        let workspace = cx.new(|_cx| {
            Workspace::new(make_workspace_data(
                vec![project_with_split("p1", ["t1", "t2"])],
                vec!["p1"],
            ))
        });
        let mut fm = crate::focus::FocusManager::new();
        fm.focus_terminal("p1".to_string(), vec![1]);

        workspace.update(cx, |ws: &mut Workspace, _cx| {
            ws.project_mut("p1").unwrap().layout = Some(LayoutNode::Terminal {
                terminal_id: Some("t1".to_string()),
                minimized: false,
                detached: false,
                shell_type: ShellType::Default,
                zoom_level: 1.0,
            });
        });

        assert_eq!(sync_and_read_focus(&workspace, &mut fm, cx), Some(vec![]));
    }

    #[gpui::test]
    fn sync_leaves_a_focus_path_that_still_names_a_terminal_alone(cx: &mut gpui::TestAppContext) {
        let workspace = cx.new(|_cx| {
            Workspace::new(make_workspace_data(
                vec![project_with_split("p1", ["t1", "t2"])],
                vec!["p1"],
            ))
        });
        let mut fm = crate::focus::FocusManager::new();
        fm.focus_terminal("p1".to_string(), vec![1]);

        assert_eq!(sync_and_read_focus(&workspace, &mut fm, cx), Some(vec![1]));
    }

    #[gpui::test]
    fn sync_does_not_re_anchor_focus_onto_a_non_terminal(cx: &mut gpui::TestAppContext) {
        // A degenerate tree has no terminal to offer. Re-anchoring onto the
        // empty container would leave focus just as orphaned and re-run the
        // heal — with its notify + debounced save — on every later sync.
        let workspace = cx.new(|_cx| {
            Workspace::new(make_workspace_data(
                vec![project_with_split("p1", ["t1", "t2"])],
                vec!["p1"],
            ))
        });
        let mut fm = crate::focus::FocusManager::new();
        fm.focus_terminal("p1".to_string(), vec![1]);

        workspace.update(cx, |ws: &mut Workspace, _cx| {
            ws.project_mut("p1").unwrap().layout = Some(LayoutNode::Split {
                direction: crate::state::SplitDirection::Horizontal,
                sizes: Vec::new(),
                children: Vec::new(),
            });
        });

        assert_eq!(sync_and_read_focus(&workspace, &mut fm, cx), Some(vec![1]));
        let version = workspace.read_with(cx, |ws: &Workspace, _cx| ws.data_version());
        // Still inert on the next sync — no repeated activity bumps.
        assert_eq!(sync_and_read_focus(&workspace, &mut fm, cx), Some(vec![1]));
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data_version(), version);
        });
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

        let mut new_data = make_workspace_data(vec![make_project("p2")], vec!["p2"]);
        new_data.projects[0].is_closing = true;
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.mark_creating_project("p2");
            ws.mark_closing_project("p2");
            ws.mark_worktree_removing("/tmp/p2");
            ws.replace_data(&mut fm, new_data, cx);
        });

        assert!(fm.focused_project_id().is_none());
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data().projects.len(), 1);
            assert_eq!(ws.data().projects[0].id, "p2");
            assert_eq!(ws.data_version(), 1);
            assert_eq!(ws.data_replacement_epoch(), 1);
            assert!(!ws.is_creating_project("p2"));
            assert!(!ws.is_project_closing("p2"));
            assert!(!ws.data().projects[0].is_closing);
            assert!(!ws.lifecycle.is_worktree_removing("/tmp/p2"));
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
            ws.toggle_project_overview_visibility(
                &mut crate::focus::FocusManager::new(),
                WindowId::Main,
                "p1",
                cx,
            );
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

            assert!(
                !ws.data
                    .project_order
                    .contains(&"remote:conn1:folder1".to_string())
            );
            assert!(
                ws.data
                    .project_order
                    .contains(&"remote:conn2:folder2".to_string())
            );
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
        data.main_window
            .hidden_project_ids
            .insert("remote:conn1:p2".to_string());
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
            assert!(matches!(
                p.layout.as_ref().unwrap(),
                LayoutNode::Terminal { .. }
            ));
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
            assert!(
                ws.project("p1")
                    .unwrap()
                    .hook_terminals
                    .contains_key("hook-1")
            );
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
            ws.register_hook_terminal(
                "p1",
                "hook-1",
                HookTerminalEntry {
                    label: "on_project_open (feature/foo)".to_string(),
                    status: HookTerminalStatus::Running,
                    hook_type: "on_project_open".to_string(),
                    command: "echo test".to_string(),
                    cwd: ".".to_string(),
                },
                cx,
            );
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let name = ws
                .project("p1")
                .unwrap()
                .terminal_names
                .get("hook-1")
                .unwrap();
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
            assert_eq!(
                ws.active_folder_filter(WindowId::Main).map(|s| s.as_str()),
                Some("f1")
            );
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
            assert_eq!(
                ws.data().main_window.project_widths.get("p1").copied(),
                Some(0.42)
            );
            assert_eq!(ws.data_version(), 1);
        });
    }

    #[gpui::test]
    fn toggle_project_layout_mode_flips_and_persists(cx: &mut gpui::TestAppContext) {
        // Per-window orientation defaults to Columns, flips to Rows on the
        // first toggle and back on the second. Each flip bumps data_version
        // so the auto-save observer persists the new orientation.
        use crate::state::ProjectLayoutMode;
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(
                ws.project_layout_mode(WindowId::Main),
                ProjectLayoutMode::Columns
            );
        });

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_project_layout_mode(WindowId::Main, cx);
        });
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(
                ws.project_layout_mode(WindowId::Main),
                ProjectLayoutMode::Rows
            );
            assert_eq!(ws.data_version(), 1);
        });

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_project_layout_mode(WindowId::Main, cx);
        });
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(
                ws.project_layout_mode(WindowId::Main),
                ProjectLayoutMode::Columns
            );
            assert_eq!(ws.data_version(), 2);
        });
    }

    #[gpui::test]
    fn toggle_project_layout_mode_keeps_weights_but_drops_scale(cx: &mut gpui::TestAppContext) {
        // Weights are axis-agnostic and survive the flip; the pixel scale is
        // pixels per unit along the old axis, so carrying it over would size
        // stacked rows by the grid's *width*.
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut widths = HashMap::new();
            widths.insert("p1".to_string(), 23.5);
            ws.update_project_widths_with_scale(WindowId::Main, widths, 35.4, cx);
            ws.toggle_project_layout_mode(WindowId::Main, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.get_project_width(WindowId::Main, "p1", 1), 23.5);
            assert_eq!(ws.get_project_width_scale(WindowId::Main), None);
        });
    }

    #[gpui::test]
    fn toggle_project_layout_mode_is_scoped_to_its_window(cx: &mut gpui::TestAppContext) {
        let extra = WindowState::default();
        let extra_id = extra.id;
        let mut data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        data.extra_windows.push(extra);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_project_layout_mode(WindowId::Main, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(
                ws.project_layout_mode(WindowId::Main),
                ProjectLayoutMode::Rows
            );
            assert_eq!(
                ws.project_layout_mode(WindowId::Extra(extra_id)),
                ProjectLayoutMode::Columns
            );
        });
    }

    #[gpui::test]
    fn toggle_project_layout_mode_keeps_canonical_splits_unchanged(cx: &mut gpui::TestAppContext) {
        // Orientation is per-window presentation; the shared daemon mirror must
        // remain canonical so a state snapshot and another window cannot fight it.
        use crate::state::SplitDirection;

        let nested = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![0.6, 0.4],
            children: vec![
                LayoutNode::new_terminal(),
                LayoutNode::Split {
                    direction: SplitDirection::Vertical,
                    sizes: vec![0.5, 0.5],
                    children: vec![LayoutNode::new_terminal(), LayoutNode::new_terminal()],
                },
            ],
        };
        let mut p1 = make_project("p1");
        p1.layout = Some(nested);

        let data = make_workspace_data(vec![p1], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.toggle_project_layout_mode(WindowId::Main, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            let LayoutNode::Split {
                direction,
                sizes,
                children,
            } = layout
            else {
                panic!("expected outer split");
            };
            assert_eq!(
                *direction,
                SplitDirection::Horizontal,
                "outer stays canonical"
            );
            assert_eq!(sizes, &vec![0.6, 0.4], "sizes preserved");
            let LayoutNode::Split {
                direction: inner, ..
            } = &children[1]
            else {
                panic!("expected nested split");
            };
            assert_eq!(*inner, SplitDirection::Vertical, "nested stays canonical");
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
            assert_eq!(
                ws.data().main_window.project_widths.get("p1").copied(),
                Some(0.75)
            );
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
            assert_eq!(
                ws.data().main_window.folder_collapsed.get("f1"),
                Some(&true)
            );
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
        data.main_window
            .folder_collapsed
            .insert("f1".to_string(), true);
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

        let returned =
            workspace.update(cx, |ws: &mut Workspace, cx| ws.spawn_extra_window(None, cx));

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

        let id = workspace.update(cx, |ws: &mut Workspace, cx| ws.spawn_extra_window(None, cx));

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
            assert_eq!(
                ws.active_folder_filter(WindowId::Main).map(|s| s.as_str()),
                Some("f1")
            );
        });
    }

    #[gpui::test]
    fn active_folder_filter_extra_reads_targeted_extras_folder_filter(
        cx: &mut gpui::TestAppContext,
    ) {
        // Per-window viewport model: targeting `WindowId::Extra(uuid)` reads
        // from that extra's `WindowState::folder_filter` (NOT main's). The
        // fixture pre-populates main + a sibling extra with their own
        // distinct filters so a regression that ignores window_id and
        // unconditionally returns main's filter, scatters across extras,
        // or routes through the wrong slot would surface here.
        let mut data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        data.main_window.folder_filter = Some("main_folder".to_string());
        let extra_a = WindowState {
            folder_filter: Some("extra_a_folder".to_string()),
            ..Default::default()
        };
        let extra_a_id = extra_a.id;
        let extra_b = WindowState {
            folder_filter: Some("extra_b_folder".to_string()),
            ..Default::default()
        };
        let extra_b_id = extra_b.id;
        data.extra_windows = vec![extra_a, extra_b];
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(
                ws.active_folder_filter(WindowId::Extra(extra_a_id))
                    .map(|s| s.as_str()),
                Some("extra_a_folder"),
            );
            assert_eq!(
                ws.active_folder_filter(WindowId::Extra(extra_b_id))
                    .map(|s| s.as_str()),
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
