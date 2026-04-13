//! Transient remote-sync coordination state.

use std::collections::{HashMap, HashSet};

use okena_core::api::{ApiGitStatus, ApiServiceInfo};

/// Per-project transient remote state populated during state sync.
///
/// Previously these fields lived inside `ProjectData` with `#[serde(skip)]`.
/// Separating them makes persistence semantics obvious at the type level.
#[derive(Clone, Debug, Default)]
pub struct RemoteProjectSnapshot {
    /// Remote service descriptors for this project.
    pub services: Vec<ApiServiceInfo>,
    /// Remote host address (used for port badge URLs).
    pub host: Option<String>,
    /// Last-known git status.
    pub git_status: Option<ApiGitStatus>,
}

/// Transient remote-sync state that lives alongside persistent workspace data.
#[derive(Debug, Default)]
pub struct RemoteSyncState {
    /// Remote project IDs awaiting focus on the next state sync.
    ///
    /// When a CreateTerminal action is dispatched for a remote project, the
    /// project ID is recorded here. On the next sync, we detect the new
    /// terminal and focus it.
    pending_focus: HashSet<String>,
    /// Per-project remote snapshots keyed by project ID.
    snapshots: HashMap<String, RemoteProjectSnapshot>,
}

impl RemoteSyncState {
    pub fn new() -> Self {
        Self::default()
    }

    // === pending focus ===

    pub fn queue_focus(&mut self, project_id: &str) {
        self.pending_focus.insert(project_id.to_string());
    }

    pub fn pending_focus(&self) -> &HashSet<String> {
        &self.pending_focus
    }

    /// Drain all pending focus project IDs.
    pub fn drain_pending_focus(&mut self) -> Vec<String> {
        self.pending_focus.drain().collect()
    }

    // === snapshots ===

    pub fn snapshot(&self, project_id: &str) -> Option<&RemoteProjectSnapshot> {
        self.snapshots.get(project_id)
    }

    pub fn snapshot_mut(&mut self, project_id: &str) -> &mut RemoteProjectSnapshot {
        self.snapshots.entry(project_id.to_string()).or_default()
    }

    pub fn set_snapshot(&mut self, project_id: &str, snapshot: RemoteProjectSnapshot) {
        self.snapshots.insert(project_id.to_string(), snapshot);
    }

    pub fn remove_snapshot(&mut self, project_id: &str) {
        self.snapshots.remove(project_id);
    }

    /// Remove all snapshots whose project ID starts with the given prefix.
    pub fn retain_not_starting_with(&mut self, prefix: &str) {
        self.snapshots.retain(|id, _| !id.starts_with(prefix));
    }
}
