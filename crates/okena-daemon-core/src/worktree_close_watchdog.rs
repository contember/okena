//! Daemon-side reconciliation for worktree closes whose before-remove hook PTY
//! disappeared without an authoritative exit event.
//!
//! A current PTY generation is the only liveness signal. There is deliberately
//! no elapsed-time policy: a hook is allowed to run indefinitely until it exits,
//! reports an authoritative result, or its PTY actually vanishes.

use std::sync::Arc;
use std::time::Duration;

use okena_hooks::{HookMonitor, HookRunner};
use okena_terminal::pty_manager::PtyManager;
use okena_workspace::state::Workspace;
use parking_lot::Mutex;
use tokio::sync::watch;

use crate::workspace_cx::DaemonWorkspaceCx;

const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Periodically abort worktree closes whose hook PTY has disappeared. This is a
/// liveness reconciler, not a hook timeout: live PTYs remain pending forever.
pub async fn run_worktree_close_watchdog(
    workspace: Arc<Mutex<Workspace>>,
    pty_manager: Arc<PtyManager>,
    workspace_tick: watch::Sender<u64>,
    hook_runner: Option<HookRunner>,
    hook_monitor: Option<HookMonitor>,
) {
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        reconcile_orphaned_worktree_closes_once(
            &workspace,
            &pty_manager,
            &workspace_tick,
            &hook_runner,
            &hook_monitor,
        );
    }
}

/// Run one orphan reconciliation pass. Returns the number of atomically claimed
/// pending closes. A candidate can safely disappear between snapshot and claim:
/// the workspace operation is idempotent and loses races to normal exits.
pub fn reconcile_orphaned_worktree_closes_once(
    workspace: &Arc<Mutex<Workspace>>,
    pty_manager: &PtyManager,
    workspace_tick: &watch::Sender<u64>,
    hook_runner: &Option<HookRunner>,
    hook_monitor: &Option<HookMonitor>,
) -> usize {
    let candidates = workspace.lock().pending_worktree_close_terminal_ids();
    let mut reconciled = 0;

    for terminal_id in candidates {
        // Do not use the terminals registry and never apply an age deadline:
        // only PTY ownership is authoritative for liveness.
        if pty_manager.current_generation(&terminal_id).is_some() {
            continue;
        }

        let aborted = {
            let mut cx = DaemonWorkspaceCx::new(workspace_tick, hook_runner, hook_monitor);
            let mut ws = workspace.lock();
            ws.abort_orphaned_worktree_close(&terminal_id, &mut cx)
        };
        let Some(aborted) = aborted else {
            continue;
        };

        if let Some(monitor) = hook_monitor {
            // `None` records the unknown exit as a failure, decrements the
            // running count, and queues the hook-failure toast exactly once.
            monitor.finish_by_terminal_id(&terminal_id, None);
        }
        log::error!(
            "worktree-close: before-remove hook PTY disappeared; close aborted for \"{}\" ({}) terminal {}",
            aborted.project_name,
            aborted.project_id,
            terminal_id
        );
        reconciled += 1;
    }

    reconciled
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_hooks::HookStatus;
    use okena_state::{HookTerminalEntry, HookTerminalStatus, ProjectData, WorkspaceData};
    use okena_terminal::session_backend::SessionBackend;
    use std::collections::HashMap;

    fn workspace_with_pending_close(terminal_id: &str) -> Workspace {
        let mut project = ProjectData {
            id: "worktree".into(),
            name: "Feature".into(),
            path: std::env::temp_dir().to_string_lossy().into_owned(),
            layout: None,
            terminal_names: HashMap::from([(terminal_id.into(), "Before remove".into())]),
            hidden_terminals: Default::default(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: Default::default(),
            hooks: Default::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: Default::default(),
            default_shell: None,
            hook_terminals: Default::default(),
            pinned: false,
            last_activity_at: None,
            is_creating: false,
            is_closing: false,
            creating_progress: None,
        };
        project.hook_terminals.insert(
            terminal_id.into(),
            HookTerminalEntry {
                label: "Before remove".into(),
                status: HookTerminalStatus::Running,
                hook_type: "before_worktree_remove".into(),
                command: "true".into(),
                cwd: project.path.clone(),
            },
        );
        let mut workspace = Workspace::new(WorkspaceData {
            version: 1,
            projects: vec![project],
            project_order: vec!["worktree".into()],
            folders: Vec::new(),
            service_panel_heights: Default::default(),
            hook_panel_heights: Default::default(),
            main_window: Default::default(),
            extra_windows: Vec::new(),
        });
        workspace.register_pending_worktree_close(okena_state::PendingWorktreeClose {
            project_id: "worktree".into(),
            hook_terminal_id: terminal_id.into(),
            branch: "feature".into(),
            main_repo_path: std::env::temp_dir().to_string_lossy().into_owned(),
        });
        workspace
    }

    #[test]
    fn vanished_hook_pty_aborts_close_heals_once_and_allows_immediate_retry() {
        let workspace = Arc::new(Mutex::new(workspace_with_pending_close("hook-1")));
        let (manager, _events) = PtyManager::new(SessionBackend::None);
        manager
            .create_or_reconnect_terminal(Some("hook-1"), &std::env::temp_dir().to_string_lossy())
            .expect("create hook PTY");
        // Deliberately do not dispatch a PtyEvent::Exit: this reproduces the
        // daemon teardown race where the hook PTY disappears first.
        manager.kill("hook-1");
        manager.flush_teardown();
        let (tick, tick_rx) = watch::channel(0u64);
        let monitor = HookMonitor::new();
        monitor.record_start(
            "before_worktree_remove",
            "true",
            "Feature",
            Some("hook-1".into()),
        );

        assert_eq!(
            reconcile_orphaned_worktree_closes_once(
                &workspace,
                &manager,
                &tick,
                &None,
                &Some(monitor.clone()),
            ),
            1
        );
        assert!(tick_rx.has_changed().expect("tick remains open"));
        let ws = workspace.lock();
        let project = ws.project("worktree").expect("project retained");
        assert!(!ws.is_project_closing("worktree"));
        assert!(!project.is_closing);
        assert!(matches!(
            project.hook_terminals["hook-1"].status,
            HookTerminalStatus::Failed { exit_code: -1 }
        ));
        drop(ws);
        assert!(matches!(
            monitor.history()[0].status,
            HookStatus::Failed { exit_code: -1, .. }
        ));
        assert_eq!(monitor.drain_pending_toasts().len(), 1);
        assert_eq!(
            reconcile_orphaned_worktree_closes_once(
                &workspace,
                &manager,
                &tick,
                &None,
                &Some(monitor.clone()),
            ),
            0,
            "late/duplicate reconciliation must not rewrite status or toast"
        );
        assert!(monitor.drain_pending_toasts().is_empty());
        workspace
            .lock()
            .register_pending_worktree_close(okena_state::PendingWorktreeClose {
                project_id: "worktree".into(),
                hook_terminal_id: "hook-retry".into(),
                branch: "feature".into(),
                main_repo_path: std::env::temp_dir().to_string_lossy().into_owned(),
            });
        assert!(
            workspace.lock().is_project_closing("worktree"),
            "retry can claim close immediately"
        );
    }

    #[test]
    fn live_hook_never_times_out() {
        let workspace = Arc::new(Mutex::new(workspace_with_pending_close("hook-live")));
        let (manager, _events) = PtyManager::new(SessionBackend::None);
        let cwd = std::env::temp_dir().to_string_lossy().into_owned();
        manager
            .create_or_reconnect_terminal(Some("hook-live"), &cwd)
            .expect("create current PTY");
        let (tick, _tick_rx) = watch::channel(0u64);
        let monitor = HookMonitor::new();
        monitor.record_start(
            "before_worktree_remove",
            "sleep 60",
            "Feature",
            Some("hook-live".into()),
        );

        for _ in 0..3 {
            assert_eq!(
                reconcile_orphaned_worktree_closes_once(
                    &workspace,
                    &manager,
                    &tick,
                    &None,
                    &Some(monitor.clone()),
                ),
                0
            );
        }
        let ws = workspace.lock();
        assert!(ws.is_project_closing("worktree"));
        assert!(ws.project("worktree").unwrap().is_closing);
        assert!(matches!(
            ws.project("worktree").unwrap().hook_terminals["hook-live"].status,
            HookTerminalStatus::Running
        ));
        drop(ws);
        assert!(matches!(monitor.history()[0].status, HookStatus::Running));
        manager.kill("hook-live");
    }
}
