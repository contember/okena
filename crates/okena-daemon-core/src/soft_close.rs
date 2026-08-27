//! Daemon-side grace-period finalizer. The command loop ejects a busy terminal
//! and records a deadline; this loop kills the PTY once the grace elapses.
//! Undo / Close-now (handled in the command loop) remove the deadline first.
//!
//! The finalize logic itself lives in the shared, runtime-agnostic engine
//! ([`okena_workspace::actions::soft_close`]); this module only owns the tokio
//! timer that ticks it. The headless loop drives the same engine off a gpui
//! timer instead.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::watch;

use okena_hooks::{HookMonitor, HookRunner};
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::TerminalBackend;
use okena_workspace::actions::soft_close::finalize_expired;
use okena_workspace::state::Workspace;

use crate::workspace_cx::DaemonWorkspaceCx;

/// Shared `terminal_id -> grace deadline` map for in-flight soft-closes.
///
/// Re-exported from the shared engine so daemon-core callers (`daemon.rs`) stay
/// unchanged now that the type lives in `okena-workspace`.
pub use okena_workspace::actions::soft_close::SoftCloseDeadlines;

const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Periodically finalize soft-closes whose grace period elapsed: drop the
/// pending record (workspace), then kill the PTY + drop it from the registry.
/// The client toast TTLs out on its own.
pub async fn run_soft_close_poll(
    workspace: Arc<Mutex<Workspace>>,
    backend: Arc<dyn TerminalBackend>,
    terminals: TerminalsRegistry,
    workspace_tick: watch::Sender<u64>,
    hook_runner: Option<HookRunner>,
    hook_monitor: Option<HookMonitor>,
    deadlines: SoftCloseDeadlines,
) {
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        finalize_expired_once(
            &workspace,
            &backend,
            &terminals,
            &workspace_tick,
            &hook_runner,
            &hook_monitor,
            &deadlines,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn finalize_expired_once(
    workspace: &Arc<Mutex<Workspace>>,
    backend: &Arc<dyn TerminalBackend>,
    terminals: &TerminalsRegistry,
    workspace_tick: &watch::Sender<u64>,
    hook_runner: &Option<HookRunner>,
    hook_monitor: &Option<HookMonitor>,
    deadlines: &SoftCloseDeadlines,
) {
    let terminal_ids = {
        let mut cx = DaemonWorkspaceCx::new(workspace_tick, hook_runner, hook_monitor);
        let mut ws = workspace.lock();
        finalize_expired(deadlines, &mut ws, &mut cx)
    };
    for terminal_id in terminal_ids {
        backend.kill(&terminal_id);
        terminals.lock().remove(&terminal_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::StubTransport;
    use okena_state::{LayoutNode, ProjectData, WorkspaceData};
    use okena_terminal::shell_config::ShellType;
    use okena_terminal::terminal::TerminalTransport;
    use okena_workspace::focus::FocusManager;
    use std::collections::HashMap;

    struct KillBarrierBackend {
        started: std::sync::mpsc::Sender<String>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl TerminalBackend for KillBarrierBackend {
        fn transport(&self) -> Arc<dyn TerminalTransport> {
            Arc::new(StubTransport)
        }
        fn create_terminal(
            &self,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            anyhow::bail!("kill barrier backend does not create terminals")
        }
        fn reconnect_terminal(
            &self,
            _terminal_id: &str,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            anyhow::bail!("kill barrier backend does not reconnect terminals")
        }
        fn kill(&self, terminal_id: &str) {
            self.started
                .send(terminal_id.to_string())
                .expect("kill waiter remains alive");
            self.release
                .lock()
                .recv_timeout(Duration::from_secs(2))
                .expect("test releases kill barrier");
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

    fn workspace_with_terminal() -> WorkspaceData {
        let project = ProjectData {
            id: "p1".to_string(),
            name: "Project p1".to_string(),
            path: "/tmp".to_string(),
            layout: Some(LayoutNode::Terminal {
                terminal_id: Some("t1".to_string()),
                minimized: false,
                detached: false,
                shell_type: ShellType::Default,
                zoom_level: 1.0,
            }),
            terminal_names: Default::default(),
            hidden_terminals: Default::default(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: Default::default(),
            hooks: Default::default(),
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
        WorkspaceData {
            version: 1,
            projects: vec![project],
            project_order: vec!["p1".to_string()],
            folders: Vec::new(),
            service_panel_heights: Default::default(),
            hook_panel_heights: Default::default(),
            main_window: Default::default(),
            extra_windows: Vec::new(),
        }
    }

    #[test]
    fn expired_soft_close_releases_workspace_before_kill() {
        let workspace = Arc::new(Mutex::new(Workspace::new(workspace_with_terminal())));
        let deadlines: SoftCloseDeadlines = Arc::new(Mutex::new(HashMap::new()));
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(HashMap::new()));
        let (workspace_tick, _workspace_rx) = watch::channel(0u64);
        {
            let mut focus_manager = FocusManager::new();
            let mut cx = DaemonWorkspaceCx::new(&workspace_tick, &None, &None);
            workspace.lock().begin_soft_close(
                &mut focus_manager,
                "p1",
                &[],
                "t1",
                "soft-close:t1",
                &mut cx,
            );
            deadlines.lock().insert(
                "t1".to_string(),
                std::time::Instant::now() - Duration::from_secs(1),
            );
        }

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let backend: Arc<dyn TerminalBackend> = Arc::new(KillBarrierBackend {
            started: started_tx,
            release: Mutex::new(release_rx),
        });
        let task_workspace = workspace.clone();
        let task_backend = backend.clone();
        let task_terminals = terminals.clone();
        let task_deadlines = deadlines.clone();
        let task = std::thread::spawn(move || {
            finalize_expired_once(
                &task_workspace,
                &task_backend,
                &task_terminals,
                &workspace_tick,
                &None,
                &None,
                &task_deadlines,
            );
        });

        assert_eq!(
            started_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("backend kill started"),
            "t1"
        );
        assert!(
            !workspace
                .try_lock()
                .expect("workspace remains available while backend kill waits")
                .has_pending_close("t1")
        );
        assert!(deadlines.lock().is_empty());
        release_tx.send(()).expect("release backend kill");
        task.join().expect("expiry finalizer completes");
    }
}
