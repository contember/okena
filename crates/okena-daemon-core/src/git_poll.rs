//! GPUI-free git-status poller: the headless analogue of
//! `okena-views-git`'s `GitStatusWatcher` (its `watcher.rs`), minus the GUI.
//!
//! The GUI's watcher polls git status every ~5s for the set of *visible*
//! non-remote projects, caches per-project [`GitStatus`], and pushes a slimmed
//! [`ApiGitStatus`] map into a `tokio::sync::watch` channel (`remote_tx`) that
//! the remote server broadcasts to clients. The daemon reproduces exactly that
//! `watch`-channel output path with `okena-git` directly (NOT `okena-views-git`,
//! which is a GUI crate).
//!
//! ## What is faithfully ported vs. dropped
//!
//! * **Ported:** the 5s cadence, the visible-non-remote project selection (via
//!   [`Workspace::all_visible_project_ids`], which is the union across the main
//!   and all extra windows — the same set the GUI uses), running
//!   [`okena_git::refresh_git_status`] on a blocking pool under [`Lane::Poll`],
//!   building the `HashMap<String, ApiGitStatus>`, and `send_replace`-ing it.
//! * **Dropped (GUI/remote-server concerns not present in daemon-core):** the
//!   `cx.notify()` local-UI push, the `gh` PR/CI fan-out and its adaptive
//!   cadence, the branch-only warmup, and the remotely-subscribed-terminals
//!   augmentation of the poll set (that set lives in the remote server, which
//!   wires the daemon — it can extend the poll set there later). The primary
//!   output is the `git_status_tx` watch, exactly as in the GUI.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use okena_core::api::ApiGitStatus;
use okena_core::process::{with_lane, Lane};
use okena_git::{self as git, GitStatus};
use okena_workspace::state::Workspace;
use parking_lot::Mutex;
use tokio::sync::watch;

/// How often to poll git status. Mirrors the GUI watcher's `GIT_POLL_INTERVAL`.
const GIT_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Project the local [`GitStatus`] onto the slimmer wire type pushed to remote
/// clients. GPUI-free reimplementation of `okena-views-git`'s `to_api`.
fn to_api(s: &GitStatus) -> ApiGitStatus {
    ApiGitStatus {
        branch: s.branch.clone(),
        lines_added: s.lines_added,
        lines_removed: s.lines_removed,
        pr_info: s.pr_info.clone(),
        ci_checks: s.ci_checks.clone(),
        ahead: s.ahead,
        behind: s.behind,
        unpushed: s.unpushed,
    }
}

/// Run the daemon git-status poll loop until the `watch` channel is closed (all
/// receivers dropped → the server is gone).
///
/// Each cycle:
/// 1. Lock the workspace, snapshot the `(id, path)` of the projects to poll
///    (visible, non-remote — see module docs), then DROP the lock.
/// 2. Run [`git::refresh_git_status`] for each on a blocking pool
///    ([`tokio::task::spawn_blocking`], under [`Lane::Poll`]) so the gix dir-walk
///    + diff never stalls the reactor thread.
/// 3. Build the `HashMap<String, ApiGitStatus>` (only projects that produced a
///    status) and `send_replace` it into `git_status_tx`.
///
/// Optionally bumps `state_version` on a real change so a snapshot/broadcast
/// observer can react; the *primary* output is the `git_status_tx` watch.
pub async fn run_git_poll(
    workspace: Arc<Mutex<Workspace>>,
    git_status_tx: Arc<watch::Sender<HashMap<String, ApiGitStatus>>>,
    state_version: watch::Sender<u64>,
) {
    // Last-published per-project statuses, kept across cycles so we only
    // re-broadcast + bump on real change. Keyed by the richer `GitStatus`
    // (which derives `PartialEq`) — the GUI's `commit_statuses` compares the
    // same type. `ApiGitStatus` (the wire projection) has no `PartialEq`.
    let mut last: HashMap<String, GitStatus> = HashMap::new();

    loop {
        // ── 1. Snapshot the projects to poll under the workspace lock ────────
        let projects: Vec<(String, String)> = {
            let ws = workspace.lock();
            // Visible across main + all extra windows (the GUI's poll set), then
            // resolve to (id, path) for non-remote projects only — git status is
            // local-only, so we must never run gix against a remote project.
            let visible = ws.all_visible_project_ids();
            ws.projects()
                .iter()
                .filter(|p| !p.is_remote && visible.contains(&p.id))
                .map(|p| (p.id.clone(), p.path.clone()))
                .collect()
        };

        // ── 2. Refresh each project's git status on the blocking pool ────────
        let mut new_statuses: HashMap<String, GitStatus> = HashMap::new();
        for (id, path) in projects {
            let status = tokio::task::spawn_blocking(move || {
                with_lane(Lane::Poll, || git::refresh_git_status(Path::new(&path)))
            })
            .await;
            match status {
                Ok(Some(status)) => {
                    new_statuses.insert(id, status);
                }
                // No status (not a repo / transient miss with no cache): omit it,
                // matching the GUI's `filter_map` over `Some` statuses.
                Ok(None) => {}
                Err(e) => {
                    log::error!("git status poll task panicked for {id}: {e}");
                }
            }
        }

        // ── 3. Publish the slimmed status map on change ──────────────────────
        // Compare the richer `GitStatus` map (the GUI's `commit_statuses`
        // change check); broadcast the `ApiGitStatus` projection only when it
        // actually changed, then bump the coarse state tick.
        if new_statuses != last {
            last = new_statuses;
            let api_statuses: HashMap<String, ApiGitStatus> =
                last.iter().map(|(id, s)| (id.clone(), to_api(s))).collect();
            git_status_tx.send_replace(api_statuses);
            state_version.send_modify(|v| *v += 1);
        }

        // Stop once every external `watch` receiver is gone (the server is down).
        if git_status_tx.is_closed() {
            return;
        }

        tokio::time::sleep(GIT_POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use okena_state::WorkspaceData;

    fn empty_workspace_data() -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects: Vec::new(),
            project_order: Vec::new(),
            folders: Vec::new(),
            service_panel_heights: Default::default(),
            hook_panel_heights: Default::default(),
            main_window: Default::default(),
            extra_windows: Vec::new(),
        }
    }

    /// With no projects and no external `watch` receiver, the first cycle does
    /// its empty snapshot, publishes nothing (unchanged), detects the closed
    /// channel, and the loop ends — without touching any real repository or
    /// sleeping. Exercises the snapshot → no-change → channel-closed-detection
    /// path.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_git_poll_stops_when_channel_closed() {
        let workspace = Arc::new(Mutex::new(Workspace::new(empty_workspace_data())));
        let (tx, rx) = watch::channel(HashMap::<String, ApiGitStatus>::new());
        let git_status_tx = Arc::new(tx);
        let (state_version, _svrx) = watch::channel(0u64);

        // Drop the only external receiver up front so the first `is_closed()`
        // check returns immediately (no 5s sleep, deterministic).
        drop(rx);

        run_git_poll(workspace, git_status_tx.clone(), state_version).await;

        // No projects → nothing was published; the channel holds the initial map.
        assert!(git_status_tx.borrow().is_empty());
    }
}
