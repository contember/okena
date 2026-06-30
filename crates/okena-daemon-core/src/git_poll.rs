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
//!   Also ported: the `gh` PR/CI fan-out and its adaptive cadence
//!   ([`okena_git::repository::get_pr_info`] / [`get_ci_checks`], gated by
//!   [`has_github_remote`]), with the across-cycle `pr_infos`/`ci_checks` caches
//!   and the two-phase publish — basic gix status is published first so the
//!   branch/diff badge never waits on a (possibly hanging) `gh` call, then the
//!   richer PR/CI data is merged in and published as a follow-up.
//! * **Dropped (GUI/remote-server concerns not present in daemon-core):** the
//!   `cx.notify()` local-UI push, the branch-only warmup, and the
//!   remotely-subscribed-terminals augmentation of the poll set (that set lives
//!   in the remote server, which wires the daemon — it can extend the poll set
//!   there later). The primary output is the `git_status_tx` watch, exactly as
//!   in the GUI.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use okena_core::api::ApiGitStatus;
use okena_core::process::{with_lane, Lane};
use okena_git::{self as git, GitStatus};
use okena_workspace::state::Workspace;
use parking_lot::Mutex;
use tokio::sync::watch;

/// How often to poll git status. Mirrors the GUI watcher's `GIT_POLL_INTERVAL`.
const GIT_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// How many git poll cycles between PR URL checks (~60s). Mirrors the GUI
/// watcher's `PR_POLL_EVERY_N_CYCLES`.
const PR_POLL_EVERY_N_CYCLES: u64 = 12;
/// How many git poll cycles between CI check polls when checks are pending
/// (~15s). Mirrors the GUI watcher's `CI_PENDING_POLL_EVERY_N_CYCLES`.
const CI_PENDING_POLL_EVERY_N_CYCLES: u64 = 3;
/// How many git poll cycles between CI check polls when checks are settled
/// (~60s). Mirrors the GUI watcher's `CI_SETTLED_POLL_EVERY_N_CYCLES`.
const CI_SETTLED_POLL_EVERY_N_CYCLES: u64 = 12;

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
        review_base: s.review_base.clone(),
        default_branch: s.default_branch.clone(),
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
///    and diff never stall the reactor thread. Merge in any cached PR/CI so the
///    badges don't blank mid-cycle, then build + `send_replace` the slimmed
///    `HashMap<String, ApiGitStatus>` on change — BEFORE the slow `gh` calls.
/// 3. On the PR/CI cadence (skip cycle 0; first check at cycle 1), fan out the
///    `gh` PR/CI lookups — also on the blocking pool, gated by `has_github_remote`
///    — merge the results into the across-cycle caches, then re-publish the
///    statuses with the richer PR/CI data as a follow-up update.
///
/// Bumps `state_version` on a real change so a snapshot/broadcast observer can
/// react; the *primary* output is the `git_status_tx` watch.
pub async fn run_git_poll(
    workspace: Arc<Mutex<Workspace>>,
    git_status_tx: Arc<watch::Sender<HashMap<String, ApiGitStatus>>>,
    state_version: watch::Sender<u64>,
    remote_subscribed_terminals: Arc<RwLock<HashMap<u64, HashSet<String>>>>,
) {
    // Last-published per-project statuses, kept across cycles so we only
    // re-broadcast + bump on real change. Keyed by the richer `GitStatus`
    // (which derives `PartialEq`) — the GUI's `commit_statuses` compares the
    // same type. `ApiGitStatus` (the wire projection) has no `PartialEq`.
    let mut last: HashMap<String, GitStatus> = HashMap::new();

    // Across-cycle PR/CI caches keyed by project ID, mirroring the GUI watcher's
    // `pr_infos` / `ci_checks`. The expensive `gh` fan-out only runs on the
    // cadence below; between those cycles the cached values are merged into every
    // status so the badges don't blank. Merge (not replace) on update so a
    // project that drops out of the visible set keeps its last-known PR/CI.
    let mut pr_infos: HashMap<String, Option<git::PrInfo>> = HashMap::new();
    let mut ci_checks: HashMap<String, Option<git::CiCheckSummary>> = HashMap::new();
    // Drives the adaptive CI cadence: faster polling while any check is pending.
    let mut any_pending_ci = false;
    let mut cycle: u64 = 0;

    loop {
        // ── 1. Snapshot the projects to poll under the workspace lock ────────
        // git status (gix, cheap, local-only) is polled for EVERY non-remote
        // project: the daemon's own window visibility is synthetic and must not
        // gate it, or a project a CLIENT views (but the daemon's window hides)
        // would show no branch/diff/PR badge. The expensive `gh` PR/CI fan-out
        // (network) is instead gated to `gh_ids` — projects visible in some
        // window PLUS any with a remotely-subscribed terminal (i.e. what a
        // client is actually looking at). Mirrors the GUI watcher's split.
        let (projects, gh_ids): (Vec<(String, String)>, HashSet<String>) = {
            let ws = workspace.lock();
            let projects = ws
                .projects()
                .iter()
                .filter(|p| !p.is_remote)
                .map(|p| (p.id.clone(), p.path.clone()))
                .collect();

            let mut gh_ids = ws.all_visible_project_ids();
            if let Ok(subscribed) = remote_subscribed_terminals.read() {
                for terminal_ids in subscribed.values() {
                    for tid in terminal_ids {
                        if let Some(p) = ws.find_project_for_terminal(tid)
                            && !p.is_remote
                        {
                            gh_ids.insert(p.id.clone());
                        }
                    }
                }
            }
            (projects, gh_ids)
        };

        // Skip the cycle-0 `gh` fan-out: at startup the basic gix status renders
        // immediately regardless, and we don't want a thundering herd of `gh` at
        // the worst moment. PR/CI kick in on cycle 1 (+5s), then settle into
        // their normal cadence. Mirrors the GUI watcher's `check_prs`/`check_ci`.
        let ci_poll_interval = if any_pending_ci {
            CI_PENDING_POLL_EVERY_N_CYCLES
        } else {
            CI_SETTLED_POLL_EVERY_N_CYCLES
        };
        let check_prs = cycle == 1 || (cycle != 0 && cycle.is_multiple_of(PR_POLL_EVERY_N_CYCLES));
        let check_ci = cycle == 1 || (cycle != 0 && cycle.is_multiple_of(ci_poll_interval));

        // ── 2. Refresh each project's git status on the blocking pool ────────
        let mut new_statuses: HashMap<String, GitStatus> = HashMap::new();
        for (id, path) in &projects {
            let id = id.clone();
            let path = path.clone();
            let status = tokio::task::spawn_blocking(move || {
                with_lane(Lane::Poll, || git::refresh_git_status(Path::new(&path)))
            })
            .await;
            match status {
                Ok(Some(mut status)) => {
                    // Inject whatever PR/CI we already have cached so a still-fresh
                    // badge doesn't blank between `gh` cadence cycles.
                    status.pr_info = pr_infos.get(&id).cloned().flatten();
                    status.ci_checks = ci_checks.get(&id).cloned().flatten();
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

        // ── 3. Publish the basic status map on change — BEFORE the slow `gh` ──
        // git status comes from gix (fast, in-process); PR/CI come from `gh`
        // (network, and can hang). Publishing here means a stuck `gh` can never
        // block the branch/diff badge from appearing.
        publish(
            &mut last,
            &new_statuses,
            &git_status_tx,
            &state_version,
        );

        // Stop once every external `watch` receiver is gone (the server is down).
        if git_status_tx.is_closed() {
            return;
        }

        // ── 4. `gh` PR/CI fan-out (network, can hang) on the blocking pool ───
        // Only on the PR/CI cadence. Each `gh` call runs under `spawn_blocking`
        // so it never stalls the reactor or the basic-status publish above. A
        // failed/missing call leaves that project's cache value untouched (or
        // None) — we never panic and never blank existing data on error.
        if check_prs {
            for (id, path) in &projects {
                // Only fan out `gh` for projects a client is actually viewing.
                if !gh_ids.contains(id) {
                    continue;
                }
                let id = id.clone();
                let path = path.clone();
                let fetched = tokio::task::spawn_blocking(move || {
                    with_lane(Lane::Poll, || {
                        // Skip `gh` entirely for non-GitHub repos.
                        if !git::repository::has_github_remote(Path::new(&path)) {
                            return None;
                        }
                        git::repository::get_pr_info(Path::new(&path))
                    })
                })
                .await;
                match fetched {
                    Ok(pr) => {
                        pr_infos.insert(id, pr);
                    }
                    Err(e) => {
                        log::warn!("gh PR info task failed for {id}: {e}");
                    }
                }
            }
        }

        if check_ci {
            for (id, path) in &projects {
                // Only fan out `gh` for projects a client is actually viewing.
                if !gh_ids.contains(id) {
                    continue;
                }
                let id = id.clone();
                let path = path.clone();
                // Use the freshest PR number we have (just-fetched or cached) so
                // `gh pr checks` targets the right PR; falls back to branch-level
                // checks when no PR is known.
                let pr_number = pr_infos.get(&id).and_then(|p| p.as_ref()).map(|p| p.number);
                let fetched = tokio::task::spawn_blocking(move || {
                    with_lane(Lane::Poll, || {
                        // Skip `gh` entirely for non-GitHub repos.
                        if !git::repository::has_github_remote(Path::new(&path)) {
                            return None;
                        }
                        git::repository::get_ci_checks(Path::new(&path), pr_number)
                    })
                })
                .await;
                match fetched {
                    Ok(checks) => {
                        ci_checks.insert(id, checks);
                    }
                    Err(e) => {
                        log::warn!("gh CI checks task failed for {id}: {e}");
                    }
                }
            }
            any_pending_ci = ci_checks
                .values()
                .any(|c| c.as_ref().map(|s| s.status.is_pending()).unwrap_or(false));
        }

        // ── 5. Re-publish with the richer PR/CI merged in (follow-up update) ─
        // `publish` no-ops when nothing changed since the basic publish above,
        // so when `gh` was skipped or returned the same data this is free.
        if check_prs || check_ci {
            for (id, status) in new_statuses.iter_mut() {
                status.pr_info = pr_infos.get(id).cloned().flatten();
                status.ci_checks = ci_checks.get(id).cloned().flatten();
            }
            publish(
                &mut last,
                &new_statuses,
                &git_status_tx,
                &state_version,
            );
        }

        cycle += 1;
        tokio::time::sleep(GIT_POLL_INTERVAL).await;
    }
}

/// Broadcast the slimmed `ApiGitStatus` map into `git_status_tx` and bump
/// `state_version`, but only on a real change. `last` holds the previously
/// published richer `GitStatus` map (the GUI's `commit_statuses` change check);
/// no-ops when `new_statuses` equals it, so re-committing the same data is free.
fn publish(
    last: &mut HashMap<String, GitStatus>,
    new_statuses: &HashMap<String, GitStatus>,
    git_status_tx: &watch::Sender<HashMap<String, ApiGitStatus>>,
    state_version: &watch::Sender<u64>,
) {
    if new_statuses == last {
        return;
    }
    *last = new_statuses.clone();
    let api_statuses: HashMap<String, ApiGitStatus> =
        last.iter().map(|(id, s)| (id.clone(), to_api(s))).collect();
    git_status_tx.send_replace(api_statuses);
    state_version.send_modify(|v| *v += 1);
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

        let subscribed = Arc::new(RwLock::new(HashMap::new()));
        run_git_poll(workspace, git_status_tx.clone(), state_version, subscribed).await;

        // No projects → nothing was published; the channel holds the initial map.
        assert!(git_status_tx.borrow().is_empty());
    }
}
