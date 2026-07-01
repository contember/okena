use okena_git::{self as git, GitStatus};
use okena_workspace::state::Workspace;
use gpui::prelude::*;
use gpui::*;
use okena_core::api::ApiGitStatus;
use okena_core::process::{with_lane, Lane};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// Project the local `GitStatus` onto the slimmer wire type pushed to remote
/// clients. Carries the GitHub PR/CI rollup and ahead/behind/unpushed counts
/// so a remote workspace renders the same status pill as a local one.
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

/// How often to poll git status (seconds)
const GIT_POLL_INTERVAL: u64 = 5;
/// How many git poll cycles between PR URL checks (~60s)
const PR_POLL_EVERY_N_CYCLES: u64 = 12;
/// How many git poll cycles between CI check polls when checks are pending (~15s)
const CI_PENDING_POLL_EVERY_N_CYCLES: u64 = 3;
/// How many git poll cycles between CI check polls when checks are settled (~60s)
const CI_SETTLED_POLL_EVERY_N_CYCLES: u64 = 12;

/// Centralized git status poller.
///
/// Polls git status for all locally visible and remotely subscribed (non-remote) projects every 5 seconds.
/// Polls PR URLs less frequently (~60 seconds).
/// Pushes changes to:
/// - Local UI via `cx.notify()` (ProjectColumn observes this entity)
/// - Remote clients via `tokio::sync::watch` channel (WS stream handler)
pub struct GitStatusWatcher {
    workspace: Entity<Workspace>,
    statuses: HashMap<String, Option<GitStatus>>,
    /// Cached PR info keyed by project ID
    pr_infos: HashMap<String, Option<okena_git::PrInfo>>,
    /// Cached CI check status keyed by project ID
    ci_checks: HashMap<String, Option<okena_git::CiCheckSummary>>,
    /// Whether any project has pending CI checks (drives adaptive polling)
    any_pending_ci: bool,
    /// Watch channel sender for remote WS push
    remote_tx: Arc<tokio::sync::watch::Sender<HashMap<String, ApiGitStatus>>>,
    /// Per-connection set of subscribed terminal IDs from remote clients
    remote_subscribed_terminals: Arc<RwLock<HashMap<u64, HashSet<String>>>>,
}

impl GitStatusWatcher {
    pub fn new(
        workspace: Entity<Workspace>,
        remote_tx: Arc<tokio::sync::watch::Sender<HashMap<String, ApiGitStatus>>>,
        remote_subscribed_terminals: Arc<RwLock<HashMap<u64, HashSet<String>>>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut watcher = Self {
            workspace,
            statuses: HashMap::new(),
            pr_infos: HashMap::new(),
            ci_checks: HashMap::new(),
            any_pending_ci: false,
            remote_tx,
            remote_subscribed_terminals,
        };
        watcher.spawn_branch_warmup(cx);
        watcher.spawn_refresh(cx);
        watcher
    }

    /// One-shot branch-only warmup for ALL non-remote projects, so consumers
    /// that read the global git cache (project switcher, sidebar worktree
    /// names, ...) see a branch for projects that aren't currently visible
    /// and therefore aren't polled by the steady-state loop.
    fn spawn_branch_warmup(&self, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        cx.spawn(async move |_, cx| {
            let paths: Vec<String> = cx.update(|cx| {
                workspace.read(cx).projects().iter()
                    .filter(|p| !p.is_remote)
                    .map(|p| p.path.clone())
                    .collect()
            });

            let futures = paths.into_iter().map(|path| {
                smol::unblock(move || {
                    with_lane(Lane::Poll, || git::warm_branch_cache(Path::new(&path)))
                })
            });
            futures::future::join_all(futures).await;
        }).detach();
    }

    /// Get cached git status for a project.
    pub fn get(&self, project_id: &str) -> Option<&GitStatus> {
        self.statuses.get(project_id).and_then(|s| s.as_ref())
    }

    /// Merge freshly-fetched statuses into the cache; on any change, notify
    /// observers (local UI) and push the slimmed status set to remote clients.
    /// No-op when nothing changed, so re-committing the same data is cheap.
    fn commit_statuses(
        &mut self,
        new_statuses: &HashMap<String, Option<GitStatus>>,
        cx: &mut Context<Self>,
    ) {
        let changed = new_statuses
            .iter()
            .any(|(id, s)| self.statuses.get(id) != Some(s));
        if !changed {
            return;
        }
        for (id, status) in new_statuses {
            self.statuses.insert(id.clone(), status.clone());
        }
        cx.notify();
        let api_statuses: HashMap<String, ApiGitStatus> = self
            .statuses
            .iter()
            .filter_map(|(id, status)| status.as_ref().map(|s| (id.clone(), to_api(s))))
            .collect();
        self.remote_tx.send_modify(|current| {
            *current = api_statuses;
        });
    }

    /// Trigger an immediate git status refresh for a single project, bypassing
    /// the 5-second polling cadence. Used after explicit user actions like
    /// branch checkout so the UI reflects the new state without waiting for
    /// the next poll cycle. PR/CI info is preserved from cache and refreshed
    /// by the regular loop.
    pub fn refresh_project(&mut self, project_id: String, cx: &mut Context<Self>) {
        let path = self
            .workspace
            .read(cx)
            .projects()
            .iter()
            .find(|p| p.id == project_id && !p.is_remote)
            .map(|p| p.path.clone());
        let Some(path) = path else { return };

        // Reuse the cached PR base so an immediate post-action refresh measures
        // ahead/behind against the PR's real target, matching the poll loop.
        let pr_base = self
            .pr_infos
            .get(&project_id)
            .and_then(|p| p.as_ref())
            .and_then(|p| p.base.clone());

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let new_status = smol::unblock(move || {
                with_lane(Lane::Poll, || {
                    git::refresh_git_status_with_pr_base(Path::new(&path), pr_base.as_deref())
                })
            })
            .await;

            let _ = this.update(cx, |this, cx| {
                let mut new_status = new_status;
                if let Some(status) = new_status.as_mut() {
                    status.pr_info = this.pr_infos.get(&project_id).cloned().flatten();
                    status.ci_checks = this.ci_checks.get(&project_id).cloned().flatten();
                }

                let changed = this.statuses.get(&project_id) != Some(&new_status);
                this.statuses.insert(project_id, new_status);

                if changed {
                    cx.notify();
                    let api_statuses: HashMap<String, ApiGitStatus> = this
                        .statuses
                        .iter()
                        .filter_map(|(id, status)| status.as_ref().map(|s| (id.clone(), to_api(s))))
                        .collect();
                    this.remote_tx.send_modify(|current| {
                        *current = api_statuses;
                    });
                }
            });
        })
        .detach();
    }

    /// Spawn the async polling loop.
    fn spawn_refresh(&mut self, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let remote_subscribed_terminals = self.remote_subscribed_terminals.clone();

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let mut cycle: u64 = 0;
            loop {
                // Collect the projects to poll: visible in *some* window, plus
                // any with a remotely subscribed terminal — all non-remote.
                //
                // Git status is only fanned out across this set, NOT every local
                // project. Polling all projects every 5s re-walks ~every working
                // tree (gix dir-walk + content hashing) even when nothing
                // changed, which profiling showed to be the single largest idle
                // CPU cost under many projects. Hidden projects keep their last
                // cached status and the branch-only warmup, and refresh on their
                // next poll once they become visible.
                //
                // Multi-window: `all_visible_project_ids()` is the union of
                // visible projects across main + all extra windows, so a project
                // shown ONLY in an extra window is still polled (per PRD rule
                // 3b-ii, projects added from window N are hidden elsewhere).
                //
                // `gh_ids` is the same set; the expensive `gh` PR/CI fan-out is
                // gated further by cycle cadence below.
                let (projects, gh_ids): (Vec<(String, String)>, HashSet<String>) = cx.update(|cx| {
                    let ws = workspace.read(cx);

                    let mut gh_ids = ws.all_visible_project_ids();

                    // Add projects with remotely subscribed terminals — they're
                    // shown on a remote client, so poll their status + PR/CI too.
                    if let Ok(remote_terminals) = remote_subscribed_terminals.read() {
                        for terminal_ids in remote_terminals.values() {
                            for tid in terminal_ids {
                                if let Some(p) = ws.find_project_for_terminal(tid)
                                    && !p.is_remote {
                                        gh_ids.insert(p.id.clone());
                                    }
                            }
                        }
                    }

                    // Resolve to (id, path) pairs — non-remote only (git status
                    // is local-only; `all_visible_project_ids` may include remote
                    // projects, which we must not run gix against).
                    let projects = ws.projects()
                        .iter()
                        .filter(|p| !p.is_remote && gh_ids.contains(&p.id))
                        .map(|p| (p.id.clone(), p.path.clone()))
                        .collect();
                    (projects, gh_ids)
                });

                // Skip the cycle-0 `gh` fan-out: at startup the app is already
                // busy spawning terminals/hooks, and git status (phase 1, gix)
                // renders immediately regardless. PR/CI kick in on cycle 1 (+5s)
                // and then follow their normal cadence — so the badges fill in
                // shortly after launch without a thundering herd of `gh` at the
                // worst moment.
                let ci_poll_interval = if this.update(cx, |this, _| this.any_pending_ci).unwrap_or(false) {
                    CI_PENDING_POLL_EVERY_N_CYCLES
                } else {
                    CI_SETTLED_POLL_EVERY_N_CYCLES
                };
                let check_prs = cycle == 1 || (cycle != 0 && cycle.is_multiple_of(PR_POLL_EVERY_N_CYCLES));
                let check_ci = cycle == 1 || (cycle != 0 && cycle.is_multiple_of(ci_poll_interval));

                // Snapshot each project's known PR base branch (from the cached
                // PR info fetched on prior `check_prs` cycles). Passing it into
                // the status fetch below re-points ahead/behind at what the PR
                // actually targets (e.g. `develop`) instead of the repo default
                // — recomputed every cycle from local refs, so no extra `gh`.
                let pr_bases: HashMap<String, String> = this.update(cx, |this, _| {
                    this.pr_infos
                        .iter()
                        .filter_map(|(id, pr)| {
                            pr.as_ref().and_then(|p| p.base.clone()).map(|b| (id.clone(), b))
                        })
                        .collect()
                }).unwrap_or_default();

                // Phase 1: Fetch git status for all projects in parallel
                let status_futures: Vec<_> = projects.iter().map(|(id, path)| {
                    let id = id.clone();
                    let path = path.clone();
                    let pr_base = pr_bases.get(&id).cloned();
                    async move {
                        let status = smol::unblock(move || {
                            with_lane(Lane::Poll, || {
                                git::refresh_git_status_with_pr_base(Path::new(&path), pr_base.as_deref())
                            })
                        }).await;
                        (id, status)
                    }
                }).collect();
                let mut new_statuses: HashMap<String, Option<GitStatus>> =
                    futures::future::join_all(status_futures).await.into_iter().collect();

                // Commit git status NOW, before the slow `gh` PR/CI calls below.
                // git status comes from gix (fast, in-process); PR/CI come from
                // `gh` (network, and can hang). Committing here means a stuck
                // `gh` can never block the branch/diff badge from appearing —
                // and the poll loop keeps cycling. Inject whatever PR/CI we
                // already have cached so we don't blank those mid-cycle.
                let should_continue = this.update(cx, |this, cx| {
                    for (id, status) in new_statuses.iter_mut() {
                        if let Some(s) = status.as_mut() {
                            s.pr_info = this.pr_infos.get(id).cloned().flatten();
                            s.ci_checks = this.ci_checks.get(id).cloned().flatten();
                        }
                    }
                    this.commit_statuses(&new_statuses, cx);
                    true
                }).unwrap_or(false);
                if !should_continue {
                    break;
                }

                // Phase 2: Fetch PR info in parallel (slower, network calls) — only on PR poll cycles.
                // Runs after all statuses are updated so git status isn't delayed by PR checks.
                let new_pr_infos: HashMap<String, Option<okena_git::PrInfo>> = if check_prs {
                    let pr_futures: Vec<_> = projects.iter()
                        .filter(|(id, _)| gh_ids.contains(id))
                        .map(|(id, path)| {
                        let id = id.clone();
                        let path = path.clone();
                        async move {
                            let pr_info = smol::unblock(move || {
                                with_lane(Lane::Poll, || {
                                    // Skip `gh` entirely for non-GitHub repos.
                                    if !git::repository::has_github_remote(Path::new(&path)) {
                                        return None;
                                    }
                                    git::repository::get_pr_info(Path::new(&path))
                                })
                            }).await;
                            (id, pr_info)
                        }
                    }).collect();
                    futures::future::join_all(pr_futures).await.into_iter().collect()
                } else {
                    HashMap::new()
                };

                // Phase 3: Fetch CI check status — adaptive interval based on pending state.
                // Runs for every project; uses `gh pr checks` when a PR is known,
                // falls back to branch-level `check-runs`/`status` otherwise.
                let new_ci_checks: HashMap<String, Option<okena_git::CiCheckSummary>> = if check_ci {
                    let pr_infos_snapshot: HashMap<String, Option<okena_git::PrInfo>> = if check_prs {
                        // Use freshly fetched PR info
                        new_pr_infos.clone()
                    } else {
                        // Use cached PR info
                        this.update(cx, |this, _| this.pr_infos.clone()).unwrap_or_default()
                    };
                    let ci_futures: Vec<_> = projects.iter()
                        .filter(|(id, _)| gh_ids.contains(id))
                        .map(|(id, path)| {
                            let id = id.clone();
                            let path = path.clone();
                            let pr_number = pr_infos_snapshot.get(&id).and_then(|p| p.as_ref()).map(|p| p.number);
                            async move {
                                let checks = smol::unblock(move || {
                                    with_lane(Lane::Poll, || {
                                        // Skip `gh` entirely for non-GitHub repos.
                                        if !git::repository::has_github_remote(Path::new(&path)) {
                                            return None;
                                        }
                                        git::repository::get_ci_checks(Path::new(&path), pr_number)
                                    })
                                }).await;
                                (id, checks)
                            }
                        }).collect();
                    futures::future::join_all(ci_futures).await.into_iter().collect()
                } else {
                    HashMap::new()
                };

                // Merge freshly-fetched PR/CI into the caches and re-commit the
                // statuses with that richer data. `commit_statuses` no-ops when
                // nothing changed since the early commit above.
                let should_continue = this.update(cx, |this, cx| {
                    // Merge into caches rather than replace: when fullscreen narrows
                    // the visible set to a single project, the un-polled projects
                    // should keep their last-known values until they're polled again.
                    if check_prs {
                        for (id, pr) in new_pr_infos {
                            this.pr_infos.insert(id, pr);
                        }
                    }

                    if check_ci {
                        for (id, checks) in new_ci_checks {
                            this.ci_checks.insert(id, checks);
                        }
                        this.any_pending_ci = this.ci_checks.values()
                            .any(|c| c.as_ref().map(|s| s.status.is_pending()).unwrap_or(false));
                    }

                    // Inject cached PR info + CI checks into statuses
                    for (id, status) in new_statuses.iter_mut() {
                        if let Some(status) = status.as_mut() {
                            status.pr_info = this.pr_infos.get(id).cloned().flatten();
                            status.ci_checks = this.ci_checks.get(id).cloned().flatten();
                        }
                    }

                    this.commit_statuses(&new_statuses, cx);
                    true
                }).unwrap_or(false);

                if !should_continue {
                    break;
                }

                cycle += 1;
                smol::Timer::after(Duration::from_secs(GIT_POLL_INTERVAL)).await;
            }
        }).detach();
    }
}
