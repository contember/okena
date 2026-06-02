//! Soft-close (grace-period) workspace actions.
//!
//! A "soft close" removes a busy terminal from the layout but keeps its PTY
//! alive for a grace period so the user can undo an accidental close. The
//! desktop layer drives the timer + toast; this module owns the layout
//! bookkeeping: recording the close, restoring it, and finalizing the kill.

use crate::focus::FocusManager;
use crate::state::{LayoutNode, PendingClose, Workspace};
use gpui::*;

impl Workspace {
    /// Begin a soft close: snapshot the project's layout, remove the terminal
    /// from the tree (focusing a sibling, exactly like a normal close), and
    /// record the pending close so it can be undone or finalized later.
    ///
    /// The PTY is **not** killed and the terminal is **not** removed from the
    /// registry — that happens in `finalize_soft_close`.
    pub fn begin_soft_close(
        &mut self,
        focus_manager: &mut FocusManager,
        project_id: &str,
        path: &[usize],
        terminal_id: &str,
        toast_id: &str,
        cx: &mut Context<Self>,
    ) {
        let pre_close_layout = self.project(project_id).and_then(|p| p.layout.clone());

        // Remove from the layout + focus a sibling (same as a hard close).
        self.close_terminal_and_focus_sibling(focus_manager, project_id, path, cx);

        let post_close_layout = self.project(project_id).and_then(|p| p.layout.clone());

        self.pending_closes.push(PendingClose {
            terminal_id: terminal_id.to_string(),
            project_id: project_id.to_string(),
            toast_id: toast_id.to_string(),
            pre_close_layout,
            post_close_layout,
        });
    }

    /// True if the terminal is currently waiting out its grace period.
    pub fn has_pending_close(&self, terminal_id: &str) -> bool {
        self.pending_closes
            .iter()
            .any(|p| p.terminal_id == terminal_id)
    }

    fn take_pending_close(&mut self, terminal_id: &str) -> Option<PendingClose> {
        let idx = self
            .pending_closes
            .iter()
            .position(|p| p.terminal_id == terminal_id)?;
        Some(self.pending_closes.remove(idx))
    }

    /// Finalize a soft close: drop the pending record and queue the PTY for the
    /// real teardown (the Okena observer drains `pending_terminal_kills` and
    /// calls `pty_manager.kill` + removes it from the registry). Idempotent —
    /// returns false if there was no pending close for this terminal.
    pub fn finalize_soft_close(&mut self, terminal_id: &str, cx: &mut Context<Self>) -> bool {
        if self.take_pending_close(terminal_id).is_none() {
            return false;
        }
        self.queue_terminal_kills([terminal_id.to_string()]);
        // Plain notify (not notify_data): queuing a kill is transient state, it
        // must not bump data_version / trigger an auto-save. The workspace
        // observer drains the kill queue on this notification.
        cx.notify();
        true
    }

    /// Undo a soft close, bringing the terminal back into the layout.
    ///
    /// `alive` reflects whether the PTY is still in the registry — if the shell
    /// exited on its own during the grace window there is nothing to restore, so
    /// the pending record is simply dropped. Returns true if the terminal was
    /// actually restored.
    pub fn undo_soft_close(
        &mut self,
        focus_manager: &mut FocusManager,
        terminal_id: &str,
        alive: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let pending = match self.take_pending_close(terminal_id) {
            Some(p) => p,
            None => return false,
        };

        if !alive {
            // The shell exited during the grace window — nothing to bring back.
            // The normal exit path has already reaped it.
            return false;
        }

        let project_id = pending.project_id.clone();
        let current = self.project(&project_id).and_then(|p| p.layout.clone());

        if current == pending.post_close_layout {
            // Nothing else touched the tree since the close — restore it exactly.
            if let Some(project) = self.project_mut(&project_id) {
                project.layout = pending.pre_close_layout;
            }
        } else {
            // The tree changed during the grace window. Don't guess a merge —
            // drop the recovered pane into the top-level group.
            let node = pending
                .pre_close_layout
                .as_ref()
                .and_then(|l| l.find_terminal_node(terminal_id))
                .cloned();
            if let Some(mut node) = node {
                if let LayoutNode::Terminal { minimized, detached, .. } = &mut node {
                    *minimized = false;
                    *detached = false;
                }
                if let Some(project) = self.project_mut(&project_id) {
                    match &mut project.layout {
                        Some(root) => root.append_to_root(node),
                        None => project.layout = Some(node),
                    }
                }
            }
        }

        // Focus the restored terminal.
        if let Some(path) = self
            .project(&project_id)
            .and_then(|p| p.layout.as_ref())
            .and_then(|l| l.find_terminal_path(terminal_id))
        {
            self.set_focused_terminal(focus_manager, project_id, path, cx);
        }

        self.notify_data(cx);
        true
    }

    /// Drain all pending soft-closes, returning their terminal ids. Used on
    /// quit to make sure soft-closed PTYs are actually torn down (and don't
    /// leak as orphaned session-backend sessions).
    pub fn drain_pending_closes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_closes)
            .into_iter()
            .map(|p| p.terminal_id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use gpui::AppContext as _;

    use crate::focus::FocusManager;
    use crate::settings::HooksConfig;
    use crate::state::{LayoutNode, ProjectData, SplitDirection, Workspace, WorkspaceData};
    use okena_core::theme::FolderColor;
    use okena_terminal::shell_config::ShellType;
    use std::collections::HashMap;

    fn term(id: &str) -> LayoutNode {
        LayoutNode::Terminal {
            terminal_id: Some(id.to_string()),
            minimized: false,
            detached: false,
            shell_type: ShellType::Default,
            zoom_level: 1.0,
        }
    }

    fn project_with(layout: LayoutNode) -> ProjectData {
        ProjectData {
            id: "p1".to_string(),
            name: "Project p1".to_string(),
            path: "/tmp/test".to_string(),
            layout: Some(layout),
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

    fn workspace_data(layout: LayoutNode) -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects: vec![project_with(layout)],
            project_order: vec!["p1".to_string()],
            service_panel_heights: HashMap::new(),
            hook_panel_heights: HashMap::new(),
            folders: vec![],
            main_window: crate::state::WindowState::default(),
            extra_windows: Vec::new(),
        }
    }

    fn hsplit(children: Vec<LayoutNode>) -> LayoutNode {
        let n = children.len();
        LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![1.0 / n as f32; n],
            children,
        }
    }

    #[gpui::test]
    fn undo_restores_exact_tree_when_unchanged(cx: &mut gpui::TestAppContext) {
        let data = workspace_data(hsplit(vec![term("a"), term("b")]));
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = FocusManager::new();
            // Close "a" (path [0]) softly. The split collapses to just "b".
            ws.begin_soft_close(&mut fm, "p1", &[0], "a", "toast-a", cx);
            assert!(ws.has_pending_close("a"));
            assert_eq!(
                ws.project("p1").unwrap().layout,
                Some(term("b")),
                "split collapses to the sibling after close"
            );

            // Undo with the PTY still alive — exact restore.
            assert!(ws.undo_soft_close(&mut fm, "a", true, cx));
            assert!(!ws.has_pending_close("a"));
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            assert_eq!(layout.find_terminal_path("a"), Some(vec![0]));
            assert_eq!(layout.find_terminal_path("b"), Some(vec![1]));
        });
    }

    #[gpui::test]
    fn undo_with_dead_pty_drops_pending_without_restoring(cx: &mut gpui::TestAppContext) {
        let data = workspace_data(hsplit(vec![term("a"), term("b")]));
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = FocusManager::new();
            ws.begin_soft_close(&mut fm, "p1", &[0], "a", "toast-a", cx);

            // PTY exited during the grace window (alive = false) — nothing to bring back.
            assert!(!ws.undo_soft_close(&mut fm, "a", false, cx));
            assert!(!ws.has_pending_close("a"), "pending record is dropped");
            assert_eq!(
                ws.project("p1").unwrap().layout,
                Some(term("b")),
                "layout stays collapsed — not restored"
            );
        });
    }

    #[gpui::test]
    fn finalize_queues_kill_and_drops_pending(cx: &mut gpui::TestAppContext) {
        let data = workspace_data(hsplit(vec![term("a"), term("b")]));
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = FocusManager::new();
            ws.begin_soft_close(&mut fm, "p1", &[0], "a", "toast-a", cx);

            assert!(ws.finalize_soft_close("a", cx));
            assert!(!ws.has_pending_close("a"));
            assert_eq!(ws.drain_pending_terminal_kills(), vec!["a".to_string()]);

            // Idempotent — second finalize is a no-op.
            assert!(!ws.finalize_soft_close("a", cx));
        });
    }

    #[gpui::test]
    fn undo_appends_to_root_when_layout_changed(cx: &mut gpui::TestAppContext) {
        let data = workspace_data(hsplit(vec![term("a"), term("b")]));
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = FocusManager::new();
            ws.begin_soft_close(&mut fm, "p1", &[0], "a", "toast-a", cx);
            // Tree now changed since the close: wrap the survivor in a tab group.
            ws.add_tab(&mut fm, "p1", &[], cx);

            // Undo can't restore the exact spot — it appends "a" to the root group.
            assert!(ws.undo_soft_close(&mut fm, "a", true, cx));
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            assert!(layout.find_terminal_path("a").is_some(), "a is back in the tree");
            assert!(layout.find_terminal_path("b").is_some(), "b retained");
        });
    }

    #[gpui::test]
    fn drain_pending_closes_returns_all_ids(cx: &mut gpui::TestAppContext) {
        let data = workspace_data(hsplit(vec![term("a"), term("b")]));
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = FocusManager::new();
            ws.begin_soft_close(&mut fm, "p1", &[0], "a", "toast-a", cx);
            let drained = ws.drain_pending_closes();
            assert_eq!(drained, vec!["a".to_string()]);
            assert!(!ws.has_pending_close("a"));
        });
    }
}
