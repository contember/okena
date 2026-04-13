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

    /// Take a pending worktree close for the given hook terminal ID (removes it).
    pub fn take_pending_close(&mut self, hook_terminal_id: &str) -> Option<PendingWorktreeClose> {
        self.pending_worktree_closes.remove(hook_terminal_id)
    }

    /// Cancel a pending worktree close: remove it and unmark the project as closing.
    pub fn cancel_pending_close(&mut self, hook_terminal_id: &str) {
        if let Some(pending) = self.take_pending_close(hook_terminal_id) {
            self.closing.remove(&pending.project_id);
        }
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
}
