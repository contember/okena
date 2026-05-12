//! Transient remote-sync coordination state.

use std::collections::HashMap;

use okena_core::api::{ApiGitStatus, ApiServiceInfo};
use okena_state::WindowId;

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
    /// Remote project IDs awaiting focus on the next state sync, scoped to the
    /// originating window.
    ///
    /// When a CreateTerminal action is dispatched for a remote project, the
    /// originating window and project ID are recorded here. On that window's
    /// next sync, we detect the new terminal and focus it in the same window.
    pending_focus: HashMap<WindowId, HashMap<String, Vec<String>>>,
    /// Per-project remote snapshots keyed by project ID.
    snapshots: HashMap<String, RemoteProjectSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingRemoteFocus {
    pub project_id: String,
    pub old_terminal_ids: Vec<String>,
}

impl RemoteSyncState {
    pub fn new() -> Self {
        Self::default()
    }

    // === pending focus ===

    pub fn queue_focus(
        &mut self,
        window_id: WindowId,
        project_id: &str,
        old_terminal_ids: Vec<String>,
    ) {
        self.pending_focus
            .entry(window_id)
            .or_default()
            .entry(project_id.to_string())
            .or_insert(old_terminal_ids);
    }

    /// Drain pending focus project IDs for one window.
    pub fn drain_pending_focus(&mut self, window_id: WindowId) -> Vec<PendingRemoteFocus> {
        self.pending_focus
            .remove(&window_id)
            .map(|projects| {
                projects
                    .into_iter()
                    .map(|(project_id, old_terminal_ids)| PendingRemoteFocus {
                        project_id,
                        old_terminal_ids,
                    })
                    .collect()
            })
            .unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn pending_focus_drains_only_target_window() {
        let extra = WindowId::Extra(Uuid::new_v4());
        let mut sync = RemoteSyncState::new();

        sync.queue_focus(WindowId::Main, "remote:a:p1", vec!["t1".to_string()]);
        sync.queue_focus(extra, "remote:a:p2", vec!["t2".to_string()]);

        let main_pending = sync.drain_pending_focus(WindowId::Main);
        assert_eq!(
            main_pending,
            vec![PendingRemoteFocus {
                project_id: "remote:a:p1".to_string(),
                old_terminal_ids: vec!["t1".to_string()],
            }]
        );

        assert!(sync.drain_pending_focus(WindowId::Main).is_empty());
        assert_eq!(
            sync.drain_pending_focus(extra),
            vec![PendingRemoteFocus {
                project_id: "remote:a:p2".to_string(),
                old_terminal_ids: vec!["t2".to_string()],
            }]
        );
    }
}
