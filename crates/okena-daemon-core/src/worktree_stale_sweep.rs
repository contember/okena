//! Drop worktree projects whose checkout directory was deleted behind our back.
//!
//! Worktrees only ever become projects because the user asked for one, so this
//! never adds anything — it only removes rows whose directory is gone (`git
//! worktree remove` from a terminal, a manual `rm -rf`, a cleanup script) and
//! ends their terminals the way deleting the project would.
//!
//! This used to live in the GUI, where it skipped projects materialized from a
//! remote connection. Once the desktop became a thin client of the daemon every
//! project it held was such a projection, so the sweep matched nothing — and
//! the watcher itself stopped being constructed at all. The authoritative state
//! lives here now, which is also the only place that can see the directories.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use okena_hooks::{HookMonitor, HookRunner};
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::TerminalBackend;
use okena_workspace::persistence::worktree_checkout_path;
use okena_workspace::state::Workspace;
use parking_lot::Mutex;
use tokio::sync::watch;

use crate::workspace_cx::DaemonWorkspaceCx;

/// A deleted checkout is not urgent — nothing else depends on noticing it fast,
/// and every pass costs one `exists()` per worktree project.
const SWEEP_INTERVAL: Duration = Duration::from_secs(30);

pub async fn run_worktree_stale_sweep(
    workspace: Arc<Mutex<Workspace>>,
    backend: Arc<dyn TerminalBackend>,
    terminals: TerminalsRegistry,
    workspace_tick: watch::Sender<u64>,
    hook_runner: Option<HookRunner>,
    hook_monitor: Option<HookMonitor>,
) {
    loop {
        tokio::time::sleep(SWEEP_INTERVAL).await;
        sweep_once(
            &workspace,
            &backend,
            &terminals,
            &workspace_tick,
            &hook_runner,
            &hook_monitor,
        )
        .await;
    }
}

/// Run one sweep. Returns how many worktree rows were removed.
async fn sweep_once(
    workspace: &Arc<Mutex<Workspace>>,
    backend: &Arc<dyn TerminalBackend>,
    terminals: &TerminalsRegistry,
    workspace_tick: &watch::Sender<u64>,
    hook_runner: &Option<HookRunner>,
    hook_monitor: &Option<HookMonitor>,
) -> usize {
    let candidates = sweep_candidates(&workspace.lock());
    if candidates.is_empty() {
        return 0;
    }

    // `exists()` per candidate is filesystem work; keep it off the reactor.
    let Ok(stale) = tokio::task::spawn_blocking(move || {
        candidates
            .into_iter()
            .filter(|(_, path)| !path.exists())
            .map(|(id, _)| id)
            .collect::<Vec<_>>()
    })
    .await
    else {
        log::warn!("worktree stale sweep task panicked");
        return 0;
    };

    if stale.is_empty() {
        return 0;
    }

    let terminal_ids = {
        let mut cx = DaemonWorkspaceCx::new(workspace_tick, hook_runner, hook_monitor);
        let mut ws = workspace.lock();
        for id in &stale {
            log::info!("removing worktree project {id}: its checkout no longer exists");
            // Re-checked inside: a project can have started closing between the
            // snapshot and here, and the removal must lose that race.
            ws.remove_stale_worktree(id, &mut cx);
        }
        ws.notify_data(&mut cx);
        ws.drain_pending_terminal_kills()
    };
    // Same teardown as deleting the project, outside the state lock: nothing
    // can reach these terminals once their row is gone.
    for terminal_id in terminal_ids {
        backend.kill(&terminal_id);
        terminals.lock().remove(&terminal_id);
    }
    stale.len()
}

/// Worktree projects eligible for a staleness check: everything except the ones
/// an operation is already mid-way through, whose directory is *expected* to
/// come and go.
fn sweep_candidates(workspace: &Workspace) -> Vec<(String, PathBuf)> {
    workspace
        .data()
        .projects
        .iter()
        .filter(|project| project.worktree_info.is_some())
        .filter(|project| !workspace.is_project_closing(&project.id))
        .filter(|project| !workspace.is_creating_project(&project.id))
        .filter(|project| !workspace.lifecycle.is_worktree_removing(&project.path))
        .map(|project| {
            (
                project.id.clone(),
                worktree_checkout_path(project).to_path_buf(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{RecordingBackend, plain_project};
    use okena_state::{LayoutNode, ProjectData, WorktreeMetadata};

    fn worktree_project(id: &str, path: &str) -> ProjectData {
        // Empty checkout root: the pre-persistence shape, where it resolves
        // from `project.path`.
        monorepo_worktree_project(id, path, "")
    }

    fn monorepo_worktree_project(id: &str, path: &str, checkout_root: &str) -> ProjectData {
        ProjectData {
            worktree_info: Some(WorktreeMetadata {
                parent_project_id: "parent".to_string(),
                color_override: None,
                custom_name: false,
                main_repo_path: String::new(),
                worktree_path: checkout_root.to_string(),
                branch_name: String::new(),
            }),
            ..plain_project(id, path)
        }
    }

    fn workspace_with(projects: Vec<ProjectData>) -> Workspace {
        let mut data = crate::test_support::empty_workspace_data();
        data.project_order = projects.iter().map(|p| p.id.clone()).collect();
        data.projects = projects;
        Workspace::new(data)
    }

    #[test]
    fn only_worktree_projects_are_swept() {
        let workspace = workspace_with(vec![
            plain_project("plain", "/tmp/plain"),
            worktree_project("wt", "/tmp/wt"),
        ]);

        let candidates = sweep_candidates(&workspace);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].0, "wt");
        assert_eq!(candidates[0].1, PathBuf::from("/tmp/wt"));
    }

    #[test]
    fn a_monorepo_worktree_is_stated_at_its_checkout_root() {
        // The project path is a package inside the checkout. Statting that
        // instead would call a healthy worktree stale as soon as a branch
        // switch drops the package directory.
        let workspace = workspace_with(vec![monorepo_worktree_project(
            "wt",
            "/tmp/wt/packages/app",
            "/tmp/wt",
        )]);

        let candidates = sweep_candidates(&workspace);

        assert_eq!(candidates[0].1, PathBuf::from("/tmp/wt"));
    }

    #[test]
    fn a_worktree_mid_operation_is_left_alone() {
        // Its directory is legitimately absent while the operation runs, so
        // sweeping it would delete the row out from under the operation.
        let mut workspace = workspace_with(vec![worktree_project("wt", "/tmp/wt")]);
        workspace.lifecycle.mark_worktree_removing("/tmp/wt");

        assert!(sweep_candidates(&workspace).is_empty());
    }

    fn with_terminal(mut project: ProjectData, terminal_id: &str) -> ProjectData {
        let mut layout = LayoutNode::new_terminal();
        if let LayoutNode::Terminal {
            terminal_id: id, ..
        } = &mut layout
        {
            *id = Some(terminal_id.to_string());
        }
        project.layout = Some(layout);
        project
    }

    #[tokio::test]
    async fn a_vanished_worktree_has_its_terminals_ended_and_live_projects_keep_theirs() {
        let live_dir = tempfile::tempdir().expect("live checkout");
        let live = live_dir.path().to_str().expect("utf-8 path");
        let vanished_dir = tempfile::tempdir().expect("vanished checkout");
        let vanished = vanished_dir
            .path()
            .to_str()
            .expect("utf-8 path")
            .to_string();
        drop(vanished_dir);

        let workspace = Arc::new(Mutex::new(workspace_with(vec![
            with_terminal(plain_project("plain", live), "plain-shell"),
            with_terminal(worktree_project("live-wt", live), "live-shell"),
            with_terminal(worktree_project("gone-wt", &vanished), "gone-shell"),
        ])));
        let killed = Arc::new(Mutex::new(Vec::new()));
        let backend: Arc<dyn TerminalBackend> = Arc::new(RecordingBackend {
            killed: killed.clone(),
        });
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));
        let (workspace_tick, _tick_rx) = watch::channel(0u64);

        let removed = sweep_once(
            &workspace,
            &backend,
            &terminals,
            &workspace_tick,
            &None,
            &None,
        )
        .await;

        assert_eq!(removed, 1);
        assert_eq!(*killed.lock(), vec!["gone-shell".to_string()]);
        let ws = workspace.lock();
        assert!(ws.project("gone-wt").is_none());
        assert!(ws.project("live-wt").is_some());
        assert!(ws.project("plain").is_some());
    }
}
