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
    /// Remote project creates waiting for the created project to appear in a
    /// state sync. The server assigns the project ID, so the client matches
    /// the first newly materialized project by connection/name/path.
    pending_project_visibility: Vec<PendingRemoteProjectVisibility>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingRemoteFocus {
    pub project_id: String,
    pub old_terminal_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingRemoteProjectVisibility {
    connection_id: String,
    name: String,
    path: Option<String>,
    window_id: WindowId,
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

    // === pending project visibility ===

    pub fn queue_project_visibility(
        &mut self,
        window_id: WindowId,
        connection_id: &str,
        name: &str,
        path: Option<&str>,
    ) {
        self.pending_project_visibility.push(PendingRemoteProjectVisibility {
            connection_id: connection_id.to_string(),
            name: name.to_string(),
            path: path.map(|path| path.to_string()),
            window_id,
        });
    }

    /// Take the spawning window for a newly materialized remote project.
    ///
    /// The exact path is preferred. A unique same-name pending create on the
    /// same connection is accepted only when the path is unknown (remote
    /// worktree create) or may be server-normalized (`~` expansion).
    /// Ambiguous duplicate names stay queued rather than applying visibility
    /// to the wrong project.
    pub fn take_project_visibility(
        &mut self,
        connection_id: &str,
        name: &str,
        path: &str,
    ) -> Option<WindowId> {
        let index = self
            .pending_project_visibility
            .iter()
            .position(|pending| {
                pending.connection_id == connection_id
                    && pending.name == name
                    && pending.path.as_deref() == Some(path)
            })
            .or_else(|| self.unique_project_visibility_name_match(connection_id, name))?;

        Some(self.pending_project_visibility.remove(index).window_id)
    }

    fn unique_project_visibility_name_match(
        &self,
        connection_id: &str,
        name: &str,
    ) -> Option<usize> {
        let mut matches = self
            .pending_project_visibility
            .iter()
            .enumerate()
            .filter(|(_, pending)| {
                pending.connection_id == connection_id
                    && pending.name == name
                    && pending.allows_name_only_match()
            });
        let (index, _) = matches.next()?;
        if matches.next().is_none() {
            Some(index)
        } else {
            None
        }
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

impl PendingRemoteProjectVisibility {
    fn allows_name_only_match(&self) -> bool {
        match self.path.as_deref() {
            None => true,
            Some("~") => true,
            Some(path) => path.starts_with("~/"),
        }
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

    #[test]
    fn pending_project_visibility_drains_matching_create() {
        let extra = WindowId::Extra(Uuid::new_v4());
        let mut sync = RemoteSyncState::new();

        sync.queue_project_visibility(extra, "conn-a", "Project", Some("/repo/project"));

        assert_eq!(
            sync.take_project_visibility("conn-a", "Project", "/repo/project"),
            Some(extra)
        );
        assert_eq!(
            sync.take_project_visibility("conn-a", "Project", "/repo/project"),
            None
        );
    }

    #[test]
    fn pending_project_visibility_accepts_unique_name_match_when_path_changes() {
        let extra = WindowId::Extra(Uuid::new_v4());
        let mut sync = RemoteSyncState::new();

        sync.queue_project_visibility(extra, "conn-a", "Project", Some("~/project"));

        assert_eq!(
            sync.take_project_visibility("conn-a", "Project", "/home/user/project"),
            Some(extra)
        );
    }

    #[test]
    fn pending_project_visibility_keeps_ambiguous_name_matches_queued() {
        let extra_a = WindowId::Extra(Uuid::new_v4());
        let extra_b = WindowId::Extra(Uuid::new_v4());
        let mut sync = RemoteSyncState::new();

        sync.queue_project_visibility(extra_a, "conn-a", "Project", Some("~/a"));
        sync.queue_project_visibility(extra_b, "conn-a", "Project", Some("~/b"));

        assert_eq!(
            sync.take_project_visibility("conn-a", "Project", "/home/user/project"),
            None
        );
        assert_eq!(
            sync.take_project_visibility("conn-a", "Project", "~/a"),
            Some(extra_a)
        );
    }

    #[test]
    fn pending_project_visibility_rejects_name_match_for_absolute_path_mismatch() {
        let extra = WindowId::Extra(Uuid::new_v4());
        let mut sync = RemoteSyncState::new();

        sync.queue_project_visibility(extra, "conn-a", "Project", Some("/repo/a"));

        assert_eq!(
            sync.take_project_visibility("conn-a", "Project", "/repo/b"),
            None
        );
    }

    #[test]
    fn pending_project_visibility_allows_name_match_for_unknown_worktree_path() {
        let extra = WindowId::Extra(Uuid::new_v4());
        let mut sync = RemoteSyncState::new();

        sync.queue_project_visibility(extra, "conn-a", "feature", None);

        assert_eq!(
            sync.take_project_visibility("conn-a", "feature", "/repo/.worktrees/feature"),
            Some(extra)
        );
    }
}
