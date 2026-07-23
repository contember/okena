//! Transient project lifecycle state.
//!
//! Tracks which projects are currently being created, closed, or removed,
//! plus worktree-close operations waiting on a hook terminal to finish.
//!
//! None of this is persisted — everything resets on restart.

use std::collections::{HashMap, HashSet};

use crate::state::PendingWorktreeClose;

/// Tracks transient "is this project being created/closed/removed" state.
#[derive(Debug, Default)]
pub struct ProjectLifecycleTracker {
    /// Project IDs whose worktree is still being created on disk.
    creating: HashSet<String>,
    /// Project IDs currently being closed (hook running or removal in progress).
    closing: HashSet<String>,
    /// Exact owners for headless runtime quiesce operations.
    runtime_quiesce_owners: HashMap<String, u64>,
    next_runtime_quiesce_generation: u64,
    /// Worktree paths currently being removed in the background.
    /// The sync watcher skips these to avoid re-adding a worktree
    /// whose directory hasn't been fully deleted yet.
    removing_worktree_paths: HashSet<String>,
    /// Pending worktree close operations waiting for a hook terminal to exit.
    /// Keyed by hook terminal_id.
    pending_worktree_closes: HashMap<String, PendingWorktreeClose>,
}

impl ProjectLifecycleTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn has_active_operations(&self) -> bool {
        !self.creating.is_empty()
            || !self.closing.is_empty()
            || !self.runtime_quiesce_owners.is_empty()
            || !self.removing_worktree_paths.is_empty()
            || !self.pending_worktree_closes.is_empty()
    }

    // === creating ===

    pub fn mark_creating(&mut self, project_id: &str) {
        self.creating.insert(project_id.to_string());
    }

    pub fn finish_creating(&mut self, project_id: &str) {
        self.creating.remove(project_id);
    }

    pub fn is_creating(&self, project_id: &str) -> bool {
        self.creating.contains(project_id)
    }

    // === closing ===

    pub fn mark_closing(&mut self, project_id: &str) {
        self.closing.insert(project_id.to_string());
    }

    pub fn finish_closing(&mut self, project_id: &str) {
        self.closing.remove(project_id);
    }

    pub fn is_closing(&self, project_id: &str) -> bool {
        self.closing.contains(project_id)
    }

    /// Atomically claim runtime ownership for every project in one operation.
    pub fn claim_runtime_quiesce(&mut self, project_ids: &[String]) -> Result<u64, String> {
        if let Some(project_id) = project_ids.iter().find(|project_id| {
            self.runtime_quiesce_owners
                .contains_key(project_id.as_str())
        }) {
            return Err(format!("project runtime is already quiesced: {project_id}"));
        }
        let generation = self.next_runtime_quiesce_generation.max(1);
        self.next_runtime_quiesce_generation = generation
            .checked_add(1)
            .ok_or_else(|| "project runtime quiesce generation exhausted".to_string())?;
        for project_id in project_ids {
            self.runtime_quiesce_owners
                .insert(project_id.clone(), generation);
        }
        Ok(generation)
    }

    pub fn owns_runtime_quiesce(&self, project_id: &str, generation: u64) -> bool {
        self.runtime_quiesce_owners.get(project_id) == Some(&generation)
    }

    /// Release only the operation that still owns this project.
    pub fn finish_runtime_quiesce(&mut self, project_id: &str, generation: u64) -> bool {
        if !self.owns_runtime_quiesce(project_id, generation) {
            return false;
        }
        self.runtime_quiesce_owners.remove(project_id);
        true
    }

    /// Prune the closing set to only the given project ids. Used by the client
    /// to reconcile its optimistic closing flags against the daemon's mirror:
    /// projects the mirror no longer reports as closing (or that vanished) drop
    /// their local flag so an aborted close doesn't strand the row "Closing…".
    pub fn retain_closing(&mut self, keep: &HashSet<String>) {
        self.closing.retain(|id| keep.contains(id));
    }

    // === worktree removal ===

    pub fn mark_worktree_removing(&mut self, path: &str) {
        self.removing_worktree_paths.insert(path.to_string());
    }

    pub fn finish_worktree_removing(&mut self, path: &str) {
        self.removing_worktree_paths.remove(path);
    }

    pub fn is_worktree_removing(&self, path: &str) -> bool {
        self.removing_worktree_paths.contains(path)
    }

    // === pending worktree closes ===

    /// Register a pending worktree close and mark the project as closing.
    pub fn register_pending_close(&mut self, pending: PendingWorktreeClose) {
        self.closing.insert(pending.project_id.clone());
        self.pending_worktree_closes
            .insert(pending.hook_terminal_id.clone(), pending);
    }

    /// Snapshot hook terminal IDs with a worktree close awaiting authoritative
    /// completion. Callers must still claim a particular ID through
    /// [`Self::cancel_pending_close`] because the snapshot can become stale.
    pub fn pending_close_terminal_ids(&self) -> Vec<String> {
        self.pending_worktree_closes.keys().cloned().collect()
    }

    /// Take a pending worktree close for the given hook terminal ID (removes it).
    pub fn take_pending_close(&mut self, hook_terminal_id: &str) -> Option<PendingWorktreeClose> {
        self.pending_worktree_closes.remove(hook_terminal_id)
    }

    /// Cancel a pending worktree close: remove it and unmark the project as
    /// closing. Returns the affected project id (if any) so the caller can clear
    /// the wire-facing `is_closing` marker too.
    pub fn cancel_pending_close(&mut self, hook_terminal_id: &str) -> Option<String> {
        let pending = self.take_pending_close(hook_terminal_id)?;
        self.closing.remove(&pending.project_id);
        Some(pending.project_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(project_id: &str, hook_terminal_id: &str) -> PendingWorktreeClose {
        PendingWorktreeClose {
            project_id: project_id.to_string(),
            hook_terminal_id: hook_terminal_id.to_string(),
            branch: "main".to_string(),
            main_repo_path: "/tmp/repo".to_string(),
        }
    }

    #[test]
    fn creating_lifecycle() {
        let mut tracker = ProjectLifecycleTracker::new();
        assert!(!tracker.is_creating("p1"));
        tracker.mark_creating("p1");
        assert!(tracker.is_creating("p1"));
        tracker.finish_creating("p1");
        assert!(!tracker.is_creating("p1"));
    }

    #[test]
    fn closing_lifecycle() {
        let mut tracker = ProjectLifecycleTracker::new();
        tracker.mark_closing("p1");
        assert!(tracker.is_closing("p1"));
        tracker.finish_closing("p1");
        assert!(!tracker.is_closing("p1"));
    }

    #[test]
    fn worktree_removal_lifecycle() {
        let mut tracker = ProjectLifecycleTracker::new();
        tracker.mark_worktree_removing("/tmp/wt");
        assert!(tracker.is_worktree_removing("/tmp/wt"));
        tracker.finish_worktree_removing("/tmp/wt");
        assert!(!tracker.is_worktree_removing("/tmp/wt"));
    }

    #[test]
    fn pending_close_marks_project_closing() {
        let mut tracker = ProjectLifecycleTracker::new();
        tracker.register_pending_close(pending("p1", "hook1"));
        assert!(tracker.is_closing("p1"));
    }

    #[test]
    fn take_pending_close_removes_entry() {
        let mut tracker = ProjectLifecycleTracker::new();
        tracker.register_pending_close(pending("p1", "hook1"));
        let taken = tracker.take_pending_close("hook1");
        assert!(taken.is_some());
        assert!(tracker.take_pending_close("hook1").is_none());
        // closing state is not cleared by take (only by cancel)
        assert!(tracker.is_closing("p1"));
    }

    #[test]
    fn cancel_pending_close_clears_closing() {
        let mut tracker = ProjectLifecycleTracker::new();
        tracker.register_pending_close(pending("p1", "hook1"));
        tracker.cancel_pending_close("hook1");
        assert!(!tracker.is_closing("p1"));
    }

    #[test]
    fn pending_close_terminal_ids_is_a_snapshot() {
        let mut tracker = ProjectLifecycleTracker::new();
        tracker.register_pending_close(pending("p1", "hook1"));
        tracker.register_pending_close(pending("p2", "hook2"));
        let mut ids = tracker.pending_close_terminal_ids();
        ids.sort();
        assert_eq!(ids, ["hook1", "hook2"]);
        tracker.cancel_pending_close("hook1");
        assert_eq!(ids, ["hook1", "hook2"], "snapshot remains independent");
    }

    #[test]
    fn runtime_quiesce_claims_are_atomic_and_generation_fenced() {
        let mut tracker = ProjectLifecycleTracker::new();
        let first = tracker
            .claim_runtime_quiesce(&["p1".to_string(), "p2".to_string()])
            .expect("claim batch");

        assert!(tracker.owns_runtime_quiesce("p1", first));
        assert!(tracker.owns_runtime_quiesce("p2", first));
        assert!(
            tracker
                .claim_runtime_quiesce(&["p2".to_string(), "p3".to_string()])
                .is_err()
        );
        assert!(!tracker.owns_runtime_quiesce("p3", first));

        assert!(tracker.finish_runtime_quiesce("p1", first));
        let second = tracker
            .claim_runtime_quiesce(&["p1".to_string()])
            .expect("reclaim project");
        assert_ne!(first, second);
        assert!(!tracker.finish_runtime_quiesce("p1", first));
        assert!(tracker.owns_runtime_quiesce("p1", second));
    }
}
