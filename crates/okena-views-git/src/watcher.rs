use gpui::prelude::*;
use gpui::*;
use okena_core::api::ApiGitStatus;
use okena_core::git_poll::{GitPollTrigger, GithubPollSchedule};
use okena_core::process::{Lane, with_lane};
use okena_git::repository::{CiFetch, PrFetch};
use okena_git::{self as git, GitStatus};
use okena_workspace::state::Workspace;
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
        review_base: s.review_base.clone(),
        default_branch: s.default_branch.clone(),
    }
}

/// How often to poll git status (seconds)
const GIT_POLL_INTERVAL: u64 = 5;
/// Cheap HEAD-only cadence used to wake the full status refresh after commits and checkouts.
const HEAD_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Centralized git status poller.
///
/// Polls git status for all locally visible and remotely subscribed (non-remote) projects every 5 seconds.
/// Polls HEAD metadata every 250ms to refresh immediately after commits and checkouts.
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
    /// Per-project `gh` cadence, commit-level result caching and the
    /// rate-limit gate. See [`GithubPollSchedule`].
    schedule: GithubPollSchedule,
    /// Poll cycle counter (one per `GIT_POLL_INTERVAL` tick). Lives on the
    /// entity so out-of-band refreshes schedule against the same clock as the
    /// poll loop.
    cycle: u64,
    /// Invalidates async results captured before a project's HEAD changed.
    head_generations: HashMap<String, u64>,
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
            schedule: GithubPollSchedule::default(),
            cycle: 0,
            head_generations: HashMap::new(),
            remote_tx,
            remote_subscribed_terminals,
        };
        watcher.spawn_branch_warmup(cx);
        watcher.spawn_head_poll(cx);
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
                workspace
                    .read(cx)
                    .projects()
                    .iter()
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
        })
        .detach();
    }

    /// Watch visible repositories for HEAD changes without reading their index or worktree.
    fn spawn_head_poll(&self, cx: &mut Context<Self>) {
        let remote_subscribed_terminals = self.remote_subscribed_terminals.clone();

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let mut previous = HashMap::<String, git::HeadSnapshot>::new();
            loop {
                let projects: Vec<(String, String)> = match this.update(cx, |this, cx| {
                    let ws = this.workspace.read(cx);
                    let mut visible_ids = ws.all_visible_project_ids();
                    if let Ok(remote_terminals) = remote_subscribed_terminals.read() {
                        for terminal_ids in remote_terminals.values() {
                            for terminal_id in terminal_ids {
                                if let Some(project) = ws.find_project_for_terminal(terminal_id)
                                    && !project.is_remote
                                {
                                    visible_ids.insert(project.id.clone());
                                }
                            }
                        }
                    }
                    ws.projects()
                        .iter()
                        .filter(|project| !project.is_remote && visible_ids.contains(&project.id))
                        .map(|project| (project.id.clone(), project.path.clone()))
                        .collect()
                }) {
                    Ok(projects) => projects,
                    Err(_) => break,
                };
                let active_ids: HashSet<String> =
                    projects.iter().map(|(id, _)| id.clone()).collect();
                let snapshots: HashMap<String, git::HeadSnapshot> = smol::unblock(move || {
                    projects
                        .into_iter()
                        .filter_map(|(id, path)| {
                            with_lane(Lane::Poll, || git::get_head_snapshot(Path::new(&path)))
                                .map(|snapshot| (id, snapshot))
                        })
                        .collect()
                })
                .await;
                let changed = update_head_snapshots(&mut previous, &active_ids, snapshots);
                if !changed.is_empty()
                    && this
                        .update(cx, |this, cx| {
                            for (id, reference_changed) in changed {
                                // Bump the generation so results captured before
                                // this change are discarded. The cached CI stays:
                                // checks belong to the last *pushed* commit, which
                                // a local commit doesn't move — dropping it blanked
                                // the badge and forced a refetch on every commit.
                                *this.head_generations.entry(id.clone()).or_default() += 1;
                                if reference_changed {
                                    this.refresh_project(id, cx);
                                } else {
                                    this.refresh_visible_project(id, cx);
                                }
                            }
                        })
                        .is_err()
                {
                    break;
                }
                smol::Timer::after(HEAD_POLL_INTERVAL).await;
            }
        })
        .detach();
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
        self.publish_statuses(cx);
    }

    fn publish_statuses(&self, cx: &mut Context<Self>) {
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

    fn missing_github_stats(&self, project_id: &str) -> bool {
        !(self.pr_infos.contains_key(project_id) && self.ci_checks.contains_key(project_id))
    }

    /// Fold one project's `gh` outcome into the caches and its schedule.
    ///
    /// A refusal parks *all* `gh` traffic rather than being cached as "no PR /
    /// no checks" — otherwise an exhausted rate limit would both blank every
    /// badge and keep the poller hammering a closed door.
    fn apply_github_fetch(&mut self, project_id: &str, pr: Option<PrFetch>, ci: Option<CiFetch>) {
        let cycle = self.cycle;
        let mut rate_limited = false;
        let mut reached_github = false;

        match pr {
            Some(PrFetch::Fetched(info)) => {
                reached_github = true;
                self.schedule.record_pr(project_id, cycle);
                self.pr_infos.insert(project_id.to_string(), info);
            }
            Some(PrFetch::RateLimited) => rate_limited = true,
            None => {}
        }
        match ci {
            Some(CiFetch::Unchanged) => self.schedule.record_ci_unchanged(project_id, cycle),
            Some(CiFetch::Fetched { sha, summary }) => {
                reached_github = true;
                let pending = summary
                    .as_ref()
                    .is_some_and(|summary| summary.status.is_pending());
                self.schedule.record_ci(project_id, cycle, pending, sha);
                self.ci_checks.insert(project_id.to_string(), summary);
            }
            Some(CiFetch::RateLimited) => rate_limited = true,
            None => {}
        }

        if rate_limited {
            self.schedule.note_rate_limited(cycle);
            log::warn!(
                "GitHub API rate limit hit; PR/CI polling paused for {} cycles",
                self.schedule.rate_limit_backoff_cycles()
            );
        } else if reached_github {
            self.schedule.note_request_succeeded();
        }
    }

    /// Trigger an immediate git status refresh for a single project, bypassing
    /// the 5-second polling cadence. Used after explicit user actions like
    /// branch checkout so the UI reflects the new state without waiting for the
    /// next poll cycle.
    pub fn refresh_project(&mut self, project_id: String, cx: &mut Context<Self>) {
        self.refresh_project_with_github(project_id, true, cx);
    }

    /// Trigger an immediate refresh for a project that just became visible. GH
    /// is fetched only when this project has no cached PR/CI result yet.
    pub fn refresh_visible_project(&mut self, project_id: String, cx: &mut Context<Self>) {
        self.refresh_project_with_github(project_id, false, cx);
    }

    /// Drive an immediate refresh off an external [`GitPollTrigger`]. Lets the
    /// single-binary `okena --headless` daemon react to the same wake-ups the
    /// dedicated `okena-daemon` handles in its poll loop: a client showing a
    /// project or requesting git status, a branch checkout, or a client
    /// subscribing to terminals — without waiting for the next poll cycle.
    pub fn apply_poll_trigger(&mut self, trigger: GitPollTrigger, cx: &mut Context<Self>) {
        match trigger.project_id {
            // Branch checkout: cached PR/CI belongs to the old branch — refresh
            // and re-fetch `gh`. Otherwise (project shown / git status asked):
            // refresh, fetching `gh` only when it isn't cached yet.
            Some(project_id) if trigger.invalidate_github => {
                self.refresh_project(project_id, cx);
            }
            Some(project_id) => {
                self.refresh_visible_project(project_id, cx);
            }
            // No project id (a client's terminal subscription changed): fetch
            // `gh` for any newly visible project not yet cached.
            None => self.refresh_visible_projects_missing_github(cx),
        }
    }

    /// Refresh every currently *visible* non-remote project that has no cached
    /// PR/CI yet, so a project entering view gets its badge immediately instead
    /// of on the next `gh` cadence cycle. Mirrors the poll loop's `gh_ids` set —
    /// subscribed-but-hidden projects deliberately excluded, see there.
    fn refresh_visible_projects_missing_github(&mut self, cx: &mut Context<Self>) {
        // A client's subscription set changes often; without the gate this fans
        // `gh` out over the whole workspace while GitHub is already refusing us.
        if self.schedule.is_rate_limited(self.cycle) {
            return;
        }
        let gh_ids = self.workspace.read(cx).all_visible_project_ids();
        let stale: Vec<String> = gh_ids
            .into_iter()
            .filter(|id| !self.statuses.contains_key(id) || self.missing_github_stats(id))
            .collect();
        for id in stale {
            self.refresh_visible_project(id, cx);
        }
    }

    fn refresh_project_with_github(
        &mut self,
        project_id: String,
        invalidate_github: bool,
        cx: &mut Context<Self>,
    ) {
        let path = self
            .workspace
            .read(cx)
            .projects()
            .iter()
            .find(|p| p.id == project_id && !p.is_remote)
            .map(|p| p.path.clone());
        let Some(path) = path else { return };

        if invalidate_github {
            self.pr_infos.remove(&project_id);
            self.ci_checks.remove(&project_id);
            self.schedule.force(&project_id);
            let mut changed = false;
            if let Some(Some(status)) = self.statuses.get_mut(&project_id)
                && (status.pr_info.is_some() || status.ci_checks.is_some())
            {
                status.pr_info = None;
                status.ci_checks = None;
                changed = true;
            }
            if changed {
                self.publish_statuses(cx);
            }
        }

        // Reuse the cached PR base so an immediate post-action refresh measures
        // ahead/behind against the PR's real target, matching the poll loop.
        let pr_base = if invalidate_github {
            None
        } else {
            self.pr_infos
                .get(&project_id)
                .and_then(|p| p.as_ref())
                .and_then(|p| p.base.clone())
        };
        let poll_github = invalidate_github || self.missing_github_stats(&project_id);
        // Skip the wire when the upstream commit still matches a settled result.
        let ci_skip_sha = self.schedule.ci_skip_sha(&project_id, self.cycle);
        let head_generation = self
            .head_generations
            .get(&project_id)
            .copied()
            .unwrap_or_default();

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let status_path = path.clone();
            let new_status = smol::unblock(move || {
                with_lane(Lane::Poll, || {
                    git::refresh_git_status_with_pr_base(
                        Path::new(&status_path),
                        pr_base.as_deref(),
                    )
                })
            })
            .await;

            let applied = this
                .update(cx, |this, cx| {
                    if this
                        .head_generations
                        .get(&project_id)
                        .copied()
                        .unwrap_or_default()
                        != head_generation
                    {
                        return false;
                    }
                    let mut new_status = new_status;
                    if let Some(status) = new_status.as_mut() {
                        status.pr_info = this.pr_infos.get(&project_id).cloned().flatten();
                        status.ci_checks = this.ci_checks.get(&project_id).cloned().flatten();
                    }

                    let changed = this.statuses.get(&project_id) != Some(&new_status);
                    this.statuses.insert(project_id.clone(), new_status);

                    if changed {
                        this.publish_statuses(cx);
                    }
                    true
                })
                .unwrap_or(false);
            if !applied || !poll_github {
                return;
            }

            let (fetched_pr_info, fetched_ci) = smol::unblock(move || {
                with_lane(Lane::Poll, || {
                    let path = Path::new(&path);
                    if !git::repository::has_github_remote(path) {
                        return (
                            PrFetch::Fetched(None),
                            CiFetch::Fetched {
                                sha: None,
                                summary: None,
                            },
                        );
                    }
                    let pr_info = git::repository::fetch_pr_info(path);
                    let pr_number = match &pr_info {
                        PrFetch::Fetched(info) => info.as_ref().map(|info| info.number),
                        PrFetch::RateLimited => None,
                    };
                    let ci = if matches!(pr_info, PrFetch::RateLimited) {
                        CiFetch::RateLimited
                    } else {
                        git::repository::fetch_ci_checks(path, pr_number, ci_skip_sha.as_deref())
                    };
                    (pr_info, ci)
                })
            })
            .await;

            let _ = this.update(cx, |this, cx| {
                if this
                    .head_generations
                    .get(&project_id)
                    .copied()
                    .unwrap_or_default()
                    != head_generation
                {
                    return;
                }
                this.apply_github_fetch(&project_id, Some(fetched_pr_info), Some(fetched_ci));

                let mut changed = false;
                if let Some(Some(status)) = this.statuses.get_mut(&project_id) {
                    let pr_info = this.pr_infos.get(&project_id).cloned().flatten();
                    let ci_checks = this.ci_checks.get(&project_id).cloned().flatten();
                    changed = status.pr_info != pr_info || status.ci_checks != ci_checks;
                    status.pr_info = pr_info;
                    status.ci_checks = ci_checks;
                }
                if changed {
                    this.publish_statuses(cx);
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
                // `gh_ids` is deliberately *narrower* — visible projects only.
                // A client subscribes to every terminal in the workspace, not
                // just the ones it renders, so folding subscriptions into the
                // `gh` set puts projects nobody is looking at on the GitHub
                // cadence and burns the hourly API budget for nothing.
                let (projects, gh_ids): (Vec<(String, String)>, HashSet<String>) =
                    cx.update(|cx| {
                        let ws = workspace.read(cx);

                        let gh_ids = ws.all_visible_project_ids();
                        let mut status_ids = gh_ids.clone();

                        // Projects with remotely subscribed terminals are shown on
                        // a remote client, so keep their local git status fresh.
                        if let Ok(remote_terminals) = remote_subscribed_terminals.read() {
                            for terminal_ids in remote_terminals.values() {
                                for tid in terminal_ids {
                                    if let Some(p) = ws.find_project_for_terminal(tid)
                                        && !p.is_remote
                                    {
                                        status_ids.insert(p.id.clone());
                                    }
                                }
                            }
                        }

                        // Resolve to (id, path) pairs — non-remote only (git status
                        // is local-only; `all_visible_project_ids` may include remote
                        // projects, which we must not run gix against).
                        let projects = ws
                            .projects()
                            .iter()
                            .filter(|p| !p.is_remote && status_ids.contains(&p.id))
                            .map(|p| (p.id.clone(), p.path.clone()))
                            .collect();
                        (projects, gh_ids)
                    });

                // Cycle 0 is startup: the app is already busy spawning terminals
                // and hooks, and git status (phase 1, gix) renders without `gh`.
                // The schedule's own default puts the first fetch on cycle 1.
                let _ = this.update(cx, |this, _| this.cycle = cycle);

                // Snapshot each project's known PR base branch (from the cached
                // PR info fetched on prior `check_prs` cycles). Passing it into
                // the status fetch below re-points ahead/behind at what the PR
                // actually targets (e.g. `develop`) instead of the repo default
                // — recomputed every cycle from local refs, so no extra `gh`.
                let pr_bases: HashMap<String, String> = this
                    .update(cx, |this, _| {
                        this.pr_infos
                            .iter()
                            .filter_map(|(id, pr)| {
                                pr.as_ref()
                                    .and_then(|p| p.base.clone())
                                    .map(|b| (id.clone(), b))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let poll_generations: HashMap<String, u64> = this
                    .update(cx, |this, _| {
                        projects
                            .iter()
                            .map(|(id, _)| {
                                (
                                    id.clone(),
                                    this.head_generations.get(id).copied().unwrap_or_default(),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                // Phase 1: Fetch git status for all projects in parallel
                let status_futures: Vec<_> = projects
                    .iter()
                    .map(|(id, path)| {
                        let id = id.clone();
                        let path = path.clone();
                        let pr_base = pr_bases.get(&id).cloned();
                        async move {
                            let status = smol::unblock(move || {
                                with_lane(Lane::Poll, || {
                                    git::refresh_git_status_with_pr_base(
                                        Path::new(&path),
                                        pr_base.as_deref(),
                                    )
                                })
                            })
                            .await;
                            (id, status)
                        }
                    })
                    .collect();
                let mut new_statuses: HashMap<String, Option<GitStatus>> =
                    futures::future::join_all(status_futures)
                        .await
                        .into_iter()
                        .collect();

                // Commit git status NOW, before the slow `gh` PR/CI calls below.
                // git status comes from gix (fast, in-process); PR/CI come from
                // `gh` (network, and can hang). Committing here means a stuck
                // `gh` can never block the branch/diff badge from appearing —
                // and the poll loop keeps cycling. Inject whatever PR/CI we
                // already have cached so we don't blank those mid-cycle.
                let status_generations = poll_generations.clone();
                match this.update(cx, |this, cx| {
                    new_statuses.retain(|id, _| {
                        this.head_generations.get(id).copied().unwrap_or_default()
                            == status_generations.get(id).copied().unwrap_or_default()
                    });
                    let branch_changes: HashSet<String> = new_statuses
                        .iter()
                        .filter_map(|(id, status)| {
                            let prev = this.statuses.get(id).and_then(|status| status.as_ref())?;
                            let status = status.as_ref()?;
                            (prev.branch != status.branch).then(|| id.clone())
                        })
                        .collect();
                    for id in &branch_changes {
                        this.pr_infos.remove(id);
                        this.ci_checks.remove(id);
                        if let Some(Some(status)) = new_statuses.get_mut(id) {
                            status.pr_info = None;
                            status.ci_checks = None;
                        }
                        // Cached PR/CI described the branch we just left.
                        this.schedule.force(id);
                    }
                    for (id, status) in new_statuses.iter_mut() {
                        if let Some(s) = status.as_mut() {
                            s.pr_info = this.pr_infos.get(id).cloned().flatten();
                            s.ci_checks = this.ci_checks.get(id).cloned().flatten();
                        }
                    }
                    this.commit_statuses(&new_statuses, cx);
                }) {
                    Ok(()) => {}
                    Err(_) => break,
                };

                // Phase 2: `gh` PR/CI, per project and only where due. Each
                // project's PR and CI calls are paired so the CI lookup can use
                // the PR number this pass just fetched.
                let polls: Vec<GithubProjectPoll> = this
                    .update(cx, |this, cx| {
                        // Retain across *all* projects, not just the polled set:
                        // a project scrolling out of view should keep its cached
                        // commit so coming back doesn't cost a fetch.
                        let known: HashSet<String> = this
                            .workspace
                            .read(cx)
                            .projects()
                            .iter()
                            .map(|p| p.id.clone())
                            .collect();
                        this.schedule.retain(&known);
                        if this.schedule.is_rate_limited(cycle) {
                            return Vec::new();
                        }
                        projects
                            .iter()
                            .filter(|(id, _)| gh_ids.contains(id) || this.schedule.is_urgent(id))
                            .filter_map(|(id, path)| {
                                let want_pr = this.schedule.pr_due(id, cycle, true);
                                let want_ci = this.schedule.ci_due(id, cycle, true);
                                (want_pr || want_ci).then(|| GithubProjectPoll {
                                    id: id.clone(),
                                    path: path.clone(),
                                    want_pr,
                                    want_ci,
                                    ci_skip_sha: this.schedule.ci_skip_sha(id, cycle),
                                    cached_pr_number: this
                                        .pr_infos
                                        .get(id)
                                        .and_then(|pr| pr.as_ref())
                                        .map(|pr| pr.number),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                let fetched: Vec<(String, Option<PrFetch>, Option<CiFetch>)> =
                    futures::future::join_all(polls.into_iter().map(|poll| async move {
                        let id = poll.id.clone();
                        let (pr, ci) = smol::unblock(move || fetch_project_github(&poll)).await;
                        (id, pr, ci)
                    }))
                    .await;

                // Merge freshly-fetched PR/CI into the caches and re-commit the
                // statuses with that richer data. `commit_statuses` no-ops when
                // nothing changed since the early commit above.
                let should_continue = this
                    .update(cx, |this, cx| {
                        let is_current = |id: &String| {
                            this.head_generations.get(id).copied().unwrap_or_default()
                                == poll_generations.get(id).copied().unwrap_or_default()
                        };
                        new_statuses.retain(|id, _| is_current(id));
                        let fresh: Vec<_> = fetched
                            .into_iter()
                            .filter(|(id, _, _)| is_current(id))
                            .collect();
                        // Merge into caches rather than replace: when fullscreen narrows
                        // the visible set to a single project, the un-polled projects
                        // should keep their last-known values until they're polled again.
                        for (id, pr, ci) in fresh {
                            this.apply_github_fetch(&id, pr, ci);
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
                    })
                    .unwrap_or(false);

                if !should_continue {
                    break;
                }

                cycle += 1;
                smol::Timer::after(Duration::from_secs(GIT_POLL_INTERVAL)).await;
            }
        })
        .detach();
    }
}

/// One project's slot in a `gh` pass.
struct GithubProjectPoll {
    id: String,
    path: String,
    want_pr: bool,
    want_ci: bool,
    /// Upstream commit whose CI result is already cached; while the branch
    /// still points at it the CI lookup makes no request at all.
    ci_skip_sha: Option<String>,
    /// PR number from a previous pass, used when this pass isn't refetching it.
    cached_pr_number: Option<u32>,
}

/// Run one project's PR and CI lookups on a bus worker, paired so the CI call
/// can use the PR number this pass just fetched.
fn fetch_project_github(poll: &GithubProjectPoll) -> (Option<PrFetch>, Option<CiFetch>) {
    with_lane(Lane::Poll, || {
        let path = Path::new(&poll.path);
        // Skip `gh` entirely for non-GitHub repos.
        if !git::repository::has_github_remote(path) {
            return (
                poll.want_pr.then_some(PrFetch::Fetched(None)),
                poll.want_ci.then_some(CiFetch::Fetched {
                    sha: None,
                    summary: None,
                }),
            );
        }

        let pr = poll.want_pr.then(|| git::repository::fetch_pr_info(path));
        let pr_number = match &pr {
            Some(PrFetch::Fetched(info)) => info.as_ref().map(|info| info.number),
            _ => poll.cached_pr_number,
        };
        // A rate-limited PR call means the CI call would only be refused too.
        let ci = if matches!(pr, Some(PrFetch::RateLimited)) {
            None
        } else {
            poll.want_ci.then(|| {
                git::repository::fetch_ci_checks(path, pr_number, poll.ci_skip_sha.as_deref())
            })
        };
        (pr, ci)
    })
}

fn update_head_snapshots(
    previous: &mut HashMap<String, git::HeadSnapshot>,
    active_ids: &HashSet<String>,
    snapshots: HashMap<String, git::HeadSnapshot>,
) -> Vec<(String, bool)> {
    previous.retain(|id, _| active_ids.contains(id));
    snapshots
        .into_iter()
        .filter_map(|(id, snapshot)| {
            let changed = previous.get(&id).is_some_and(|old| old != &snapshot);
            let reference_changed = previous
                .get(&id)
                .is_some_and(|old| snapshot.reference_changed(old));
            if changed {
                previous.insert(id.clone(), snapshot);
                Some((id, reference_changed))
            } else {
                previous.insert(id, snapshot);
                None
            }
        })
        .collect()
}
