//! Soft-close (grace-period) workspace actions.
//!
//! A "soft close" removes a busy terminal from the layout but keeps its PTY
//! alive for a grace period so the user can undo an accidental close. The
//! desktop layer drives the timer + toast; this module owns the layout
//! bookkeeping: recording the close, restoring it, and finalizing the kill.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use okena_core::soft_close::{encode_action, SOFT_CLOSE_KILL_PREFIX, SOFT_CLOSE_UNDO_PREFIX};
use okena_state::{Toast, ToastAction, ToastActionStyle};
use okena_terminal::backend::TerminalBackend;
use okena_terminal::TerminalsRegistry;

use crate::context::WorkspaceCx;
use crate::focus::FocusManager;
use crate::state::{LayoutNode, PendingClose, RestoredClose, Workspace};

/// Shared `terminal_id -> grace deadline` map for in-flight soft-closes.
///
/// Runtime-agnostic: the daemon-core loop and the headless loop each own one of
/// these and hand a reference to the shared flows below. The command path arms a
/// deadline; the finalizer tick ([`finalize_expired`]) reaps the ones that
/// elapsed; Undo / Close-now remove the deadline first.
pub type SoftCloseDeadlines = Arc<Mutex<HashMap<String, Instant>>>;

/// Probe whether a terminal is "busy" (has a live foreground child) and, if so,
/// what its foreground command is (for the toast label).
///
/// This forks `tmux`/`lsof`/`pgrep` under the hood, so callers must run it OFF
/// their reactor thread (tokio `spawn_blocking` / `smol::unblock`) and hold NO
/// state locks across it. Returns `(busy, command)`.
pub fn probe_busy(backend: &dyn TerminalBackend, terminal_id: &str) -> (bool, Option<String>) {
    let fg = backend.get_foreground_shell_pid(terminal_id);
    let busy = fg
        .map(okena_terminal::terminal::has_child_processes)
        .unwrap_or(false);
    let command = fg.and_then(okena_terminal::terminal::foreground_command);
    (busy, command)
}

/// Build the two-line Undo / Close-now toast for a busy soft-close:
///
///   title:  Closed "make"             — what's closing
///   detail: okena · ~/projects/okena  — project · working directory (muted)
///
/// `command` is the live foreground command (probed off-thread by the caller),
/// used as the title fallback when the terminal has no meaningful display name.
pub fn build_soft_close_toast(
    ws: &Workspace,
    terminals: &TerminalsRegistry,
    project_id: &str,
    terminal_id: &str,
    command: Option<String>,
    toast_id: &str,
    grace: u32,
) -> Toast {
    // Read the live OSC title + cwd under a single registry lock.
    let (osc_title, cwd) = {
        let reg = terminals.lock();
        let term = reg.get(terminal_id);
        (term.and_then(|t| t.title()), term.map(|t| t.current_cwd()))
    };

    let (title, detail) = ws
        .project(project_id)
        .map(|p| {
            // Title label precedence: a meaningful display name (user-set custom
            // name or non-prompt OSC title) wins; else the live foreground
            // command; else a generic "Terminal closed".
            let display = p.terminal_display_name(terminal_id, osc_title);
            let label = if display == p.directory_name() { command } else { Some(display) };
            let title = match label {
                Some(l) => format!("Closed \u{201c}{}\u{201d}", truncate_label(&l)),
                None => "Terminal closed".to_string(),
            };
            // Detail line: project name, plus the cwd when we have one.
            let mut detail = p.name.clone();
            if let Some(cwd) = &cwd {
                detail.push_str(" \u{00b7} ");
                detail.push_str(&shorten_cwd(cwd));
            }
            (title, detail)
        })
        .unwrap_or_else(|| ("Terminal closed".to_string(), String::new()));

    let actions = vec![
        ToastAction::new(
            encode_action(SOFT_CLOSE_UNDO_PREFIX, project_id, terminal_id),
            "Undo",
            ToastActionStyle::Primary,
        ),
        ToastAction::new(
            encode_action(SOFT_CLOSE_KILL_PREFIX, project_id, terminal_id),
            "Close now",
            ToastActionStyle::Danger,
        ),
    ];
    let base = Toast::info(title)
        .with_id(toast_id)
        .with_ttl(Duration::from_secs(grace as u64))
        .with_actions(actions);
    if detail.is_empty() { base } else { base.with_detail(detail) }
}

/// Cap a terminal label so the toast stays tidy. OSC titles can be arbitrarily
/// long; truncate on a char boundary with an ellipsis.
fn truncate_label(label: &str) -> String {
    const MAX_CHARS: usize = 42;
    if label.chars().count() <= MAX_CHARS {
        return label.to_string();
    }
    let mut out: String = label.chars().take(MAX_CHARS - 1).collect();
    out.push('\u{2026}');
    out
}

/// Home-relative, tail-preserving working directory for the toast detail line.
/// `~`-collapses the home dir and keeps the *end* of long paths.
fn shorten_cwd(path: &str) -> String {
    let shown = match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path == home => return "~".to_string(),
        Ok(home) if !home.is_empty() && path.starts_with(&format!("{home}/")) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.to_string(),
    };
    const MAX_CHARS: usize = 30;
    if shown.chars().count() <= MAX_CHARS {
        return shown;
    }
    let tail: String = shown
        .chars()
        .rev()
        .take(MAX_CHARS - 1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("\u{2026}{tail}")
}

/// Begin a soft close: eject the busy pane, build its Undo / Close-now toast,
/// and arm the grace deadline for the finalizer tick.
///
/// Returns `Some(toast)` when the terminal was in the layout (the caller pushes
/// it into its own `HookMonitor`). Returns `None` when the terminal has no
/// layout path — the caller should fall back to an immediate close.
///
/// The engine stays `HookMonitor`-free on purpose: it returns the toast rather
/// than pushing it, so each loop wires its own toast surface.
#[allow(clippy::too_many_arguments)]
pub fn begin_soft_close_flow(
    deadlines: &SoftCloseDeadlines,
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    terminals: &TerminalsRegistry,
    project_id: &str,
    terminal_id: &str,
    grace: u32,
    command: Option<String>,
    cx: &mut impl WorkspaceCx,
) -> Option<Toast> {
    let path = ws
        .project(project_id)
        .and_then(|p| p.layout.as_ref())
        .and_then(|l| l.find_terminal_path(terminal_id))?;

    let toast_id = format!("soft-close:{terminal_id}");
    let toast =
        build_soft_close_toast(ws, terminals, project_id, terminal_id, command, &toast_id, grace);
    ws.begin_soft_close(focus_manager, project_id, &path, terminal_id, &toast_id, cx);
    deadlines.lock().insert(
        terminal_id.to_string(),
        Instant::now() + Duration::from_secs(grace as u64),
    );
    Some(toast)
}

/// Undo a soft close: drop the grace deadline and restore the ejected pane (if
/// its PTY is still alive in the registry).
pub fn undo_soft_close_flow(
    deadlines: &SoftCloseDeadlines,
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    terminals: &TerminalsRegistry,
    terminal_id: &str,
    cx: &mut impl WorkspaceCx,
) {
    deadlines.lock().remove(terminal_id);
    // The PTY is restorable only if still in the registry; the loop owns it, so
    // the alive-check happens here.
    let alive = terminals.lock().contains_key(terminal_id);
    ws.undo_soft_close(focus_manager, terminal_id, alive, cx);
}

/// Finalize a soft close now ("Close now"): drop the deadline, finalize the
/// pending record, then kill the PTY + drop it from the registry.
pub fn close_now_flow(
    deadlines: &SoftCloseDeadlines,
    ws: &mut Workspace,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    terminal_id: &str,
    cx: &mut impl WorkspaceCx,
) {
    deadlines.lock().remove(terminal_id);
    ws.finalize_soft_close(terminal_id, cx);
    for id in ws.drain_pending_terminal_kills() {
        backend.kill(&id);
        terminals.lock().remove(&id);
    }
}

/// One finalizer tick: reap every soft-close whose grace period elapsed.
///
/// Collects + removes the expired ids under the deadline lock, finalizes each on
/// the workspace (queues the kills), then drains the kill queue and tears the
/// PTYs down. The client toast TTLs out on its own. Callers drive this on a
/// timer (~200ms).
pub fn finalize_expired(
    deadlines: &SoftCloseDeadlines,
    ws: &mut Workspace,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    cx: &mut impl WorkspaceCx,
) {
    // Collect + remove expired ids under the deadline lock only.
    let expired: Vec<String> = {
        let now = Instant::now();
        let mut d = deadlines.lock();
        let exp: Vec<String> =
            d.iter().filter(|(_, dl)| **dl <= now).map(|(t, _)| t.clone()).collect();
        for t in &exp {
            d.remove(t);
        }
        exp
    };
    if expired.is_empty() {
        return;
    }

    // Finalize on the workspace (queues kills), then drain the kill queue.
    for tid in &expired {
        ws.finalize_soft_close(tid, cx);
    }
    for id in ws.drain_pending_terminal_kills() {
        backend.kill(&id);
        terminals.lock().remove(&id);
    }
}

/// Outcome of resolving an optimistic close once the (off-thread) busy check
/// has come back. The terminal was already removed from the layout when the
/// close began; this decides what happens to its still-alive PTY.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingDecision {
    /// No pending close remained — the PTY exited on its own (or was undone)
    /// during the check window and the exit path already cleaned up. No-op.
    Raced,
    /// The terminal was idle: the PTY was queued for an immediate kill. The
    /// caller should not show an undo toast.
    Finalized,
    /// The terminal was busy: the pending close stays, and the caller should
    /// post the undo toast + schedule the grace-period teardown.
    KeepForUndo,
}

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
        cx: &mut impl WorkspaceCx,
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

        // A fresh close supersedes any earlier restore-race breadcrumb for this
        // terminal (e.g. close → undo → close again).
        self.restored_closes.retain(|r| r.terminal_id != terminal_id);
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
    pub fn finalize_soft_close(&mut self, terminal_id: &str, cx: &mut impl WorkspaceCx) -> bool {
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

    /// Resolve an optimistic close after the off-thread busy check returns.
    ///
    /// The pane was already ejected from the layout when the close began (so the
    /// UI updated instantly); here we decide the PTY's fate now that we know
    /// whether the terminal was busy. Idle terminals are killed immediately;
    /// busy ones keep their pending record so the caller can offer an undo. If
    /// the pending record is gone (the shell exited during the check window),
    /// this is a no-op — see [`PendingDecision::Raced`].
    pub fn decide_pending_close(
        &mut self,
        terminal_id: &str,
        busy: bool,
        cx: &mut impl WorkspaceCx,
    ) -> PendingDecision {
        if !self.has_pending_close(terminal_id) {
            return PendingDecision::Raced;
        }
        if busy {
            return PendingDecision::KeepForUndo;
        }
        // Idle terminal — no point keeping it alive for an undo. Queue the kill.
        self.finalize_soft_close(terminal_id, cx);
        PendingDecision::Finalized
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
        cx: &mut impl WorkspaceCx,
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
            self.set_focused_terminal(focus_manager, project_id.clone(), path, cx);
        }

        // Leave a breadcrumb: the `alive` check is registry-based and lags the
        // real process exit, so the PTY we just restored might already be dead
        // (its exit event still queued). If that exit lands, the exit handler
        // calls `reap_restored_close` to tear this pane back out — see
        // [`RestoredClose`].
        self.restored_closes.retain(|r| r.terminal_id != terminal_id);
        self.restored_closes.push(RestoredClose {
            terminal_id: terminal_id.to_string(),
            project_id,
        });

        self.notify_data(cx);
        true
    }

    /// A soft-close-restored terminal's shell exited — it was racing the undo and
    /// the PTY is now dead. Consume the breadcrumb and, if the dead terminal is
    /// still sitting in the layout, remove that pane (it can't be reconnected, and
    /// a lingering layout node would respawn a fresh shell on the next render).
    /// Returns true if a breadcrumb was consumed. Idempotent.
    pub fn reap_restored_close(&mut self, terminal_id: &str, cx: &mut impl WorkspaceCx) -> bool {
        let Some(idx) = self
            .restored_closes
            .iter()
            .position(|r| r.terminal_id == terminal_id)
        else {
            return false;
        };
        let restored = self.restored_closes.remove(idx);

        if let Some(path) = self
            .project(&restored.project_id)
            .and_then(|p| p.layout.as_ref())
            .and_then(|l| l.find_terminal_path(terminal_id))
        {
            self.close_terminal(&restored.project_id, &path, cx);
        }
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

    /// Drain pending soft-closes belonging to `project_id`, returning their
    /// terminal ids. Used when a project is deleted mid-grace: the soft-closed
    /// panes are gone from the layout, so the normal teardown wouldn't see them
    /// — kill those PTYs explicitly and drop the now-orphaned pending records.
    pub fn drain_pending_closes_for_project(&mut self, project_id: &str) -> Vec<String> {
        let mut ids = Vec::new();
        self.pending_closes.retain(|p| {
            if p.project_id == project_id {
                ids.push(p.terminal_id.clone());
                false
            } else {
                true
            }
        });
        ids
    }

    /// A soft-closed terminal's shell exited on its own during the grace window.
    /// Drop the pending record (the normal exit path is already reaping the PTY)
    /// and return its toast id so the caller can dismiss the now-useless undo
    /// toast. Returns `None` if the terminal wasn't mid soft-close.
    pub fn cancel_pending_close(&mut self, terminal_id: &str) -> Option<String> {
        self.take_pending_close(terminal_id).map(|p| p.toast_id)
    }
}

#[cfg(all(test, feature = "gpui"))]
mod tests {
    use gpui::AppContext as _;

    use super::{
        begin_soft_close_flow, close_now_flow, finalize_expired, undo_soft_close_flow,
        PendingDecision, SoftCloseDeadlines,
    };
    use crate::focus::FocusManager;
    use crate::settings::HooksConfig;
    use crate::state::{LayoutNode, ProjectData, SplitDirection, Workspace, WorkspaceData};
    use okena_core::theme::FolderColor;
    use okena_terminal::backend::TerminalBackend;
    use okena_terminal::shell_config::ShellType;
    use okena_terminal::terminal::TerminalTransport;
    use okena_terminal::TerminalsRegistry;
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

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
            pinned: false,
            last_activity_at: None,
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
    fn decide_keeps_busy_terminal_for_undo(cx: &mut gpui::TestAppContext) {
        let data = workspace_data(hsplit(vec![term("a"), term("b")]));
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = FocusManager::new();
            ws.begin_soft_close(&mut fm, "p1", &[0], "a", "toast-a", cx);

            // Busy → keep the pending record (no kill queued) so the caller can
            // offer an undo.
            assert_eq!(ws.decide_pending_close("a", true, cx), PendingDecision::KeepForUndo);
            assert!(ws.has_pending_close("a"));
            assert!(ws.drain_pending_terminal_kills().is_empty());
        });
    }

    #[gpui::test]
    fn decide_finalizes_idle_terminal(cx: &mut gpui::TestAppContext) {
        let data = workspace_data(hsplit(vec![term("a"), term("b")]));
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = FocusManager::new();
            ws.begin_soft_close(&mut fm, "p1", &[0], "a", "toast-a", cx);

            // Idle → kill immediately, pending record dropped.
            assert_eq!(ws.decide_pending_close("a", false, cx), PendingDecision::Finalized);
            assert!(!ws.has_pending_close("a"));
            assert_eq!(ws.drain_pending_terminal_kills(), vec!["a".to_string()]);
        });
    }

    #[gpui::test]
    fn decide_is_noop_when_pending_already_gone(cx: &mut gpui::TestAppContext) {
        let data = workspace_data(hsplit(vec![term("a"), term("b")]));
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = FocusManager::new();
            ws.begin_soft_close(&mut fm, "p1", &[0], "a", "toast-a", cx);
            // Shell exited during the check window — exit path cancelled the pending close.
            ws.cancel_pending_close("a");

            // Whatever the busy result, there's nothing left to decide.
            assert_eq!(ws.decide_pending_close("a", true, cx), PendingDecision::Raced);
            assert!(ws.drain_pending_terminal_kills().is_empty());
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
    fn reap_restored_close_removes_dead_pane_after_undo(cx: &mut gpui::TestAppContext) {
        let data = workspace_data(hsplit(vec![term("a"), term("b")]));
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = FocusManager::new();
            ws.begin_soft_close(&mut fm, "p1", &[0], "a", "toast-a", cx);
            // Undo with the PTY reading as alive — restored into the layout.
            assert!(ws.undo_soft_close(&mut fm, "a", true, cx));
            assert_eq!(
                ws.project("p1")
                    .unwrap()
                    .layout
                    .as_ref()
                    .unwrap()
                    .find_terminal_path("a"),
                Some(vec![0]),
                "restored into the split"
            );

            // The PTY actually died (it was racing the undo). Reaping tears the
            // dead pane back out instead of leaving it to linger / respawn.
            assert!(ws.reap_restored_close("a", cx));
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            assert!(layout.find_terminal_path("a").is_none(), "dead pane removed");
            assert!(layout.find_terminal_path("b").is_some(), "sibling retained");

            // Idempotent — nothing left to reap.
            assert!(!ws.reap_restored_close("a", cx));
        });
    }

    #[gpui::test]
    fn re_closing_clears_stale_restore_breadcrumb(cx: &mut gpui::TestAppContext) {
        let data = workspace_data(hsplit(vec![term("a"), term("b")]));
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = FocusManager::new();
            ws.begin_soft_close(&mut fm, "p1", &[0], "a", "toast-a", cx);
            assert!(ws.undo_soft_close(&mut fm, "a", true, cx));

            // Soft-closing "a" again supersedes the earlier restore breadcrumb,
            // so a later stray exit can't reap the freshly-closed pane.
            ws.begin_soft_close(&mut fm, "p1", &[0], "a", "toast-a2", cx);
            assert!(!ws.reap_restored_close("a", cx), "breadcrumb cleared by re-close");
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

    #[gpui::test]
    fn cancel_pending_close_returns_toast_and_drops_record(cx: &mut gpui::TestAppContext) {
        let data = workspace_data(hsplit(vec![term("a"), term("b")]));
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = FocusManager::new();
            ws.begin_soft_close(&mut fm, "p1", &[0], "a", "toast-a", cx);

            // Shell exited on its own → cancel returns the toast id to dismiss.
            assert_eq!(ws.cancel_pending_close("a"), Some("toast-a".to_string()));
            assert!(!ws.has_pending_close("a"));
            // Idempotent — nothing left to cancel.
            assert_eq!(ws.cancel_pending_close("a"), None);
        });
    }

    #[gpui::test]
    fn drain_pending_closes_for_project_filters_by_project(cx: &mut gpui::TestAppContext) {
        let data = workspace_data(hsplit(vec![term("a"), term("b")]));
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = FocusManager::new();
            ws.begin_soft_close(&mut fm, "p1", &[0], "a", "toast-a", cx);

            // A different project's pending close must be left untouched.
            assert!(ws.drain_pending_closes_for_project("other").is_empty());
            assert!(ws.has_pending_close("a"));

            assert_eq!(
                ws.drain_pending_closes_for_project("p1"),
                vec!["a".to_string()]
            );
            assert!(!ws.has_pending_close("a"));
        });
    }

    // ── Shared soft-close flow tests ─────────────────────────────────────────

    /// No-op transport for the test backend.
    struct StubTransport;
    impl TerminalTransport for StubTransport {
        fn send_input(&self, _terminal_id: &str, _data: &[u8]) {}
        fn resize(&self, _terminal_id: &str, _cols: u16, _rows: u16) {}
        fn uses_mouse_backend(&self) -> bool {
            false
        }
    }

    /// `TerminalBackend` that records every `kill`ed id. The soft-close flow
    /// tests never spawn PTYs, so the create/reconnect paths are unreachable.
    struct RecordingBackend {
        killed: Arc<Mutex<Vec<String>>>,
    }

    impl TerminalBackend for RecordingBackend {
        fn transport(&self) -> Arc<dyn TerminalTransport> {
            Arc::new(StubTransport)
        }
        fn create_terminal(&self, _cwd: &str, _shell: Option<&ShellType>) -> anyhow::Result<String> {
            anyhow::bail!("stub backend: create_terminal not supported")
        }
        fn reconnect_terminal(
            &self,
            _terminal_id: &str,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            anyhow::bail!("stub backend: reconnect_terminal not supported")
        }
        fn kill(&self, terminal_id: &str) {
            self.killed.lock().push(terminal_id.to_string());
        }
        fn capture_buffer(&self, _terminal_id: &str) -> Option<std::path::PathBuf> {
            None
        }
        fn supports_buffer_capture(&self) -> bool {
            false
        }
        fn is_remote(&self) -> bool {
            false
        }
        fn get_shell_pid(&self, _terminal_id: &str) -> Option<u32> {
            None
        }
        fn get_service_pids(&self, _terminal_id: &str) -> Vec<u32> {
            Vec::new()
        }
    }

    fn empty_deadlines() -> SoftCloseDeadlines {
        Arc::new(Mutex::new(HashMap::new()))
    }

    fn empty_registry() -> TerminalsRegistry {
        Arc::new(Mutex::new(HashMap::new()))
    }

    #[gpui::test]
    fn begin_flow_arms_deadline_and_returns_toast(cx: &mut gpui::TestAppContext) {
        let data = workspace_data(hsplit(vec![term("a"), term("b")]));
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = FocusManager::new();
            let deadlines = empty_deadlines();
            let terminals = empty_registry();

            let toast = begin_soft_close_flow(
                &deadlines, ws, &mut fm, &terminals, "p1", "a", 5, Some("make".into()), cx,
            );

            let toast = toast.expect("terminal in layout → toast returned");
            assert_eq!(toast.id, "soft-close:a");
            assert_eq!(toast.actions.len(), 2, "Undo + Close now");
            assert!(ws.has_pending_close("a"), "pending close recorded");
            assert!(deadlines.lock().contains_key("a"), "deadline armed");
            assert_eq!(
                ws.project("p1").unwrap().layout,
                Some(term("b")),
                "pane ejected from layout"
            );
        });
    }

    #[gpui::test]
    fn begin_flow_returns_none_when_terminal_not_in_layout(cx: &mut gpui::TestAppContext) {
        let data = workspace_data(hsplit(vec![term("a"), term("b")]));
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = FocusManager::new();
            let deadlines = empty_deadlines();
            let terminals = empty_registry();

            // "z" is not in the layout → caller should immediate-close instead.
            let toast = begin_soft_close_flow(
                &deadlines, ws, &mut fm, &terminals, "p1", "z", 5, None, cx,
            );
            assert!(toast.is_none());
            assert!(!ws.has_pending_close("z"));
            assert!(deadlines.lock().is_empty(), "no deadline armed");
        });
    }

    #[gpui::test]
    fn finalize_expired_kills_only_past_deadline_ids(cx: &mut gpui::TestAppContext) {
        let data = workspace_data(hsplit(vec![term("a"), term("b")]));
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = FocusManager::new();
            let killed = Arc::new(Mutex::new(Vec::new()));
            let backend = RecordingBackend { killed: killed.clone() };
            let terminals = empty_registry();
            let deadlines = empty_deadlines();

            // "a" is mid soft-close with an already-expired deadline; "b" is
            // soft-closed but its deadline is far in the future.
            ws.begin_soft_close(&mut fm, "p1", &[0], "a", "toast-a", cx);
            ws.begin_soft_close(&mut fm, "p1", &[0], "b", "toast-b", cx);
            deadlines
                .lock()
                .insert("a".to_string(), Instant::now() - Duration::from_secs(1));
            deadlines
                .lock()
                .insert("b".to_string(), Instant::now() + Duration::from_secs(60));

            finalize_expired(&deadlines, ws, &backend, &terminals, cx);

            assert_eq!(&*killed.lock(), &vec!["a".to_string()], "only past-deadline killed");
            assert!(!deadlines.lock().contains_key("a"), "expired deadline removed");
            assert!(deadlines.lock().contains_key("b"), "future deadline retained");
            assert!(!ws.has_pending_close("a"), "finalized");
            assert!(ws.has_pending_close("b"), "still pending");
        });
    }

    #[gpui::test]
    fn close_now_flow_clears_deadline_and_kills(cx: &mut gpui::TestAppContext) {
        let data = workspace_data(hsplit(vec![term("a"), term("b")]));
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = FocusManager::new();
            let killed = Arc::new(Mutex::new(Vec::new()));
            let backend = RecordingBackend { killed: killed.clone() };
            let terminals = empty_registry();
            let deadlines = empty_deadlines();

            ws.begin_soft_close(&mut fm, "p1", &[0], "a", "toast-a", cx);
            deadlines
                .lock()
                .insert("a".to_string(), Instant::now() + Duration::from_secs(60));

            close_now_flow(&deadlines, ws, &backend, &terminals, "a", cx);

            assert!(!deadlines.lock().contains_key("a"), "deadline cleared");
            assert!(!ws.has_pending_close("a"), "pending finalized");
            assert_eq!(&*killed.lock(), &vec!["a".to_string()], "PTY killed");
        });
    }

    #[gpui::test]
    fn undo_flow_clears_deadline(cx: &mut gpui::TestAppContext) {
        let data = workspace_data(hsplit(vec![term("a"), term("b")]));
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = FocusManager::new();
            let terminals = empty_registry();
            let deadlines = empty_deadlines();

            ws.begin_soft_close(&mut fm, "p1", &[0], "a", "toast-a", cx);
            deadlines
                .lock()
                .insert("a".to_string(), Instant::now() + Duration::from_secs(60));

            // Empty registry → PTY reads as dead, so nothing is restored, but the
            // deadline is always cleared and the pending record dropped.
            undo_soft_close_flow(&deadlines, ws, &mut fm, &terminals, "a", cx);

            assert!(!deadlines.lock().contains_key("a"), "deadline cleared");
            assert!(!ws.has_pending_close("a"), "pending dropped");
        });
    }
}
