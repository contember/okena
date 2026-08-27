//! Drop worktree projects whose checkout directory was deleted behind our back.
//!
//! Worktrees only ever become projects because the user asked for one, so this
//! never adds anything — it only removes rows whose directory is gone (`git
//! worktree remove` from a terminal, a manual `rm -rf`, a cleanup script).
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
    workspace_tick: watch::Sender<u64>,
    hook_runner: Option<HookRunner>,
    hook_monitor: Option<HookMonitor>,
) {
    loop {
        tokio::time::sleep(SWEEP_INTERVAL).await;
        sweep_once(&workspace, &workspace_tick, &hook_runner, &hook_monitor).await;
    }
}

/// Run one sweep. Returns how many worktree rows were removed.
async fn sweep_once(
    workspace: &Arc<Mutex<Workspace>>,
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

    let mut cx = DaemonWorkspaceCx::new(workspace_tick, hook_runner, hook_monitor);
    let mut ws = workspace.lock();
    for id in &stale {
        log::info!("removing worktree project {id}: its checkout no longer exists");
        // Re-checked inside: a project can have started closing between the
        // snapshot and here, and the removal must lose that race.
        ws.remove_stale_worktree(id);
    }
    ws.notify_data(&mut cx);
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
    use okena_state::{ProjectData, WorktreeMetadata};

    fn worktree_project(id: &str, path: &str) -> ProjectData {
        ProjectData {
            worktree_info: Some(WorktreeMetadata {
                parent_project_id: "parent".to_string(),
                color_override: None,
                main_repo_path: String::new(),
                // Deprecated and unset on save, so the checkout path resolves
                // from `project.path` — exactly what the sweep must stat.
                worktree_path: String::new(),
                branch_name: String::new(),
            }),
            ..plain_project(id, path)
        }
    }

    fn plain_project(id: &str, path: &str) -> ProjectData {
        ProjectData {
            id: id.to_string(),
            name: id.to_string(),
            path: path.to_string(),
            layout: None,
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
    fn a_worktree_mid_operation_is_left_alone() {
        // Its directory is legitimately absent while the operation runs, so
        // sweeping it would delete the row out from under the operation.
        let mut workspace = workspace_with(vec![worktree_project("wt", "/tmp/wt")]);
        workspace.lifecycle.mark_worktree_removing("/tmp/wt");

        assert!(sweep_candidates(&workspace).is_empty());
    }
}
