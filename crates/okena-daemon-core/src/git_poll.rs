//! GPUI-free git-status polling for the headless daemon.
//!
//! Projects visible in any window, or owning a terminal subscribed by a remote
//! client, stay on the responsive tier: HEAD every 250ms and full status every
//! 5s. Hidden, unsubscribed repositories use bounded fallback cadences (2s HEAD,
//! 30s full status). Explicit actions and detected HEAD changes still trigger an
//! immediate targeted refresh. Cached statuses for projects not selected in a
//! cycle remain published, so tiering changes freshness rather than visibility.
//!
//! The `gh` PR/CI fan-out is deliberately *narrower* than the local tier: it
//! covers only projects visible in a window (plus explicitly requested ones),
//! is scheduled per project by [`GithubPollSchedule`], skips any project whose
//! upstream commit hasn't moved since its last settled result, and parks itself
//! when GitHub reports the API rate limit as exhausted.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use okena_core::api::ApiGitStatus;
use okena_core::git_poll::{GitPollTrigger, GithubPollSchedule};
use okena_core::process::{Lane, with_lane};
use okena_git::repository::{CiFetch, PrFetch};
use okena_git::{self as git, GitStatus, HeadSnapshot};
use okena_workspace::state::Workspace;
use parking_lot::Mutex;
use tokio::sync::{Semaphore, mpsc, watch};

/// Responsive full-status cadence for visible or remotely subscribed projects.
const GIT_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Hidden projects receive a full fallback scan every 6 responsive cycles (30s).
const HIDDEN_GIT_POLL_EVERY_N_CYCLES: u64 = 6;
/// Responsive HEAD cadence for visible or remotely subscribed projects.
const HEAD_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// Hidden projects receive a cheap HEAD fallback scan every 8 ticks (2s).
const HIDDEN_HEAD_POLL_EVERY_N_TICKS: u64 = 8;
/// How many projects the `gh` fan-out talks to at once. Matches the process
/// bus's `Lane::Poll` worker count — going wider just queues on the bus, going
/// narrower (the previous strictly sequential loop) made a full pass outlast
/// its own cadence and let passes pile up on top of each other.
const GH_FANOUT_CONCURRENCY: usize = 4;

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

#[derive(Default)]
struct TriggerAccumulator {
    /// HEAD changed locally; invalidates in-flight results from the old commit.
    head_change_ids: HashSet<String>,
    /// Unconditional `gh` refreshes. Used when existing PR/CI cache is invalid.
    force_gh_ids: HashSet<String>,
    /// Conditional refreshes. These become forced only if PR/CI cache is absent.
    candidate_gh_ids: HashSet<String>,
    /// Projects whose cached PR/CI belongs to a previous branch.
    invalidate_gh_ids: HashSet<String>,
}

impl TriggerAccumulator {
    fn record(&mut self, trigger: GitPollTrigger) {
        let Some(project_id) = trigger.project_id else {
            return;
        };
        if trigger.invalidate_github {
            self.invalidate_gh_ids.insert(project_id.clone());
            self.force_gh_ids.insert(project_id);
        } else if trigger.poll_github {
            self.candidate_gh_ids.insert(project_id);
        } else {
            self.head_change_ids.insert(project_id);
        }
    }

    fn local_status_ids(&self) -> HashSet<String> {
        self.head_change_ids
            .iter()
            .chain(&self.force_gh_ids)
            .chain(&self.candidate_gh_ids)
            .cloned()
            .collect()
    }

    fn clear(&mut self) {
        self.head_change_ids.clear();
        self.force_gh_ids.clear();
        self.candidate_gh_ids.clear();
        self.invalidate_gh_ids.clear();
    }
}

struct GithubPollResult {
    head_generations: HashMap<String, u64>,
    branches: HashMap<String, Option<String>>,
    pr_infos: HashMap<String, Option<git::PrInfo>>,
    ci: HashMap<String, CiFetch>,
    /// GitHub refused at least one call because the rate limit is exhausted.
    rate_limited: bool,
    /// At least one call actually reached GitHub, so the rate-limit backoff can
    /// be cleared. A pass of nothing but cache hits proves nothing.
    reached_github: bool,
}

/// One project's slot in a `gh` pass.
struct ProjectPoll {
    id: String,
    path: String,
    want_pr: bool,
    want_ci: bool,
    /// Upstream commit whose CI result the poller already holds; the fetch is
    /// skipped while the branch still points at it.
    ci_skip_sha: Option<String>,
    /// PR number from a previous pass, used when this pass isn't re-fetching it.
    cached_pr_number: Option<u32>,
}

/// Projects the user can currently see. The `gh` fan-out is scoped to these:
/// a badge nobody is looking at is not worth GitHub API budget.
///
/// Two sources, unioned. The workspace's own hidden set covers a daemon driving
/// its own surface, but for a desktop client it is a stale copy that the client
/// never writes to — visibility is client-owned presentation state, persisted
/// client-side in `window-layout.json` under different window ids. So each
/// connected client declares what it renders (`WsInbound::SetVisibleProjects`),
/// and that is the authority for its own viewport. A client that declares
/// nothing (older build, no viewport yet) simply contributes nothing.
fn visible_project_ids(
    workspace: &Workspace,
    remote_visible_projects: &RwLock<HashMap<u64, HashSet<String>>>,
) -> HashSet<String> {
    let mut visible = workspace.all_visible_project_ids();
    if let Ok(declared) = remote_visible_projects.read() {
        for project_ids in declared.values() {
            visible.extend(project_ids.iter().cloned());
        }
    }
    visible
}

/// Visible projects plus any owning a terminal a remote client is streaming.
///
/// This is the *local* status tier only. Clients subscribe to every terminal in
/// the daemon's state — not just the ones they render — so folding subscriptions
/// into the `gh` set would put every project that merely owns a terminal on the
/// responsive GitHub cadence, which is how a machine with a few dozen projects
/// burns an hourly API budget without displaying a single extra badge.
fn streaming_project_ids(
    workspace: &Workspace,
    remote_subscribed_terminals: &RwLock<HashMap<u64, HashSet<String>>>,
    remote_visible_projects: &RwLock<HashMap<u64, HashSet<String>>>,
) -> HashSet<String> {
    let mut relevant = visible_project_ids(workspace, remote_visible_projects);
    if let Ok(subscribed) = remote_subscribed_terminals.read() {
        for terminal_ids in subscribed.values() {
            for terminal_id in terminal_ids {
                if let Some(project) = workspace.find_project_for_terminal(terminal_id)
                    && !project.is_remote
                {
                    relevant.insert(project.id.clone());
                }
            }
        }
    }
    relevant
}

fn select_status_poll_ids(
    active_ids: &HashSet<String>,
    relevant_ids: &HashSet<String>,
    forced_ids: &HashSet<String>,
    newly_relevant_ids: &HashSet<String>,
    cadence_due: bool,
    poll_hidden: bool,
) -> HashSet<String> {
    if poll_hidden {
        return active_ids.clone();
    }

    let mut selected = HashSet::new();
    if cadence_due {
        selected.extend(relevant_ids.iter().cloned());
    }
    selected.extend(forced_ids.iter().cloned());
    selected.extend(newly_relevant_ids.iter().cloned());
    selected.retain(|id| active_ids.contains(id));
    selected
}

fn merge_status_results(
    previous: &HashMap<String, GitStatus>,
    active_ids: &HashSet<String>,
    attempted: HashMap<String, Option<GitStatus>>,
) -> HashMap<String, GitStatus> {
    let mut merged = previous.clone();
    merged.retain(|id, _| active_ids.contains(id));
    for (id, status) in attempted {
        match status {
            Some(status) if active_ids.contains(&id) => {
                merged.insert(id, status);
            }
            _ => {
                merged.remove(&id);
            }
        }
    }
    merged
}

/// Poll only each repository's symbolic HEAD and commit id, waking the full
/// status loop when either changes. This never reads the index or worktree.
pub async fn run_git_head_poll(
    workspace: Arc<Mutex<Workspace>>,
    remote_subscribed_terminals: Arc<RwLock<HashMap<u64, HashSet<String>>>>,
    remote_visible_projects: Arc<RwLock<HashMap<u64, HashSet<String>>>>,
    trigger_tx: mpsc::UnboundedSender<GitPollTrigger>,
) {
    let mut previous = HashMap::<String, HeadSnapshot>::new();
    let mut tick = 0u64;
    let mut interval = tokio::time::interval(HEAD_POLL_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;
        if trigger_tx.is_closed() {
            return;
        }

        let (projects, relevant_ids): (Vec<(String, String)>, HashSet<String>) = {
            let workspace = workspace.lock();
            let relevant = streaming_project_ids(
                &workspace,
                &remote_subscribed_terminals,
                &remote_visible_projects,
            );
            let projects = workspace
                .projects()
                .iter()
                .filter(|project| !project.is_remote)
                .map(|project| (project.id.clone(), project.path.clone()))
                .collect();
            (projects, relevant)
        };
        let active_ids: HashSet<String> = projects.iter().map(|(id, _)| id.clone()).collect();
        let poll_hidden = tick.is_multiple_of(HIDDEN_HEAD_POLL_EVERY_N_TICKS);
        tick = tick.wrapping_add(1);
        let projects: Vec<_> = projects
            .into_iter()
            .filter(|(id, _)| poll_hidden || relevant_ids.contains(id))
            .collect();
        let snapshots = tokio::task::spawn_blocking(move || {
            projects
                .into_iter()
                .filter_map(|(id, path)| {
                    with_lane(Lane::Poll, || git::get_head_snapshot(Path::new(&path)))
                        .map(|snapshot| (id, snapshot))
                })
                .collect()
        })
        .await;
        let Ok(snapshots) = snapshots else {
            log::warn!("git HEAD poll task panicked");
            continue;
        };

        // `active_ids` deliberately includes unsampled hidden projects so their
        // prior snapshots survive fast-tier ticks and later changes are detected.
        for id in update_head_snapshots(&mut previous, &active_ids, snapshots) {
            if trigger_tx.send(GitPollTrigger::head_change(id)).is_err() {
                return;
            }
        }
    }
}

fn update_head_snapshots<T: PartialEq>(
    previous: &mut HashMap<String, T>,
    active_ids: &HashSet<String>,
    snapshots: HashMap<String, T>,
) -> Vec<String> {
    previous.retain(|id, _| active_ids.contains(id));
    snapshots
        .into_iter()
        .filter_map(|(id, snapshot)| {
            let changed = previous.get(&id).is_some_and(|old| old != &snapshot);
            if changed {
                previous.insert(id.clone(), snapshot);
                Some(id)
            } else {
                previous.insert(id, snapshot);
                None
            }
        })
        .collect()
}

/// Pick this cycle's `gh` slots.
///
/// Two rules, both of which used to be missing: a project earns a slot only if
/// it is *visible* (or explicitly asked for), and only when its own schedule
/// says it is due — one repo with running CI no longer drags every other repo
/// onto the fast cadence.
fn select_github_polls(
    projects: &[(String, String)],
    visible_ids: &HashSet<String>,
    schedule: &GithubPollSchedule,
    pr_infos: &HashMap<String, Option<git::PrInfo>>,
    cycle: u64,
    cadence_due: bool,
) -> Vec<ProjectPoll> {
    projects
        .iter()
        .filter(|(id, _)| visible_ids.contains(id) || schedule.is_urgent(id))
        .filter_map(|(id, path)| {
            let want_pr = schedule.pr_due(id, cycle, cadence_due);
            let want_ci = schedule.ci_due(id, cycle, cadence_due);
            (want_pr || want_ci).then(|| ProjectPoll {
                id: id.clone(),
                path: path.clone(),
                want_pr,
                want_ci,
                ci_skip_sha: schedule.ci_skip_sha(id, cycle),
                cached_pr_number: pr_infos
                    .get(id)
                    .and_then(|pr| pr.as_ref())
                    .map(|pr| pr.number),
            })
        })
        .collect()
}

/// What one project's `gh` slot produced.
struct ProjectOutcome {
    pr: Option<PrFetch>,
    ci: Option<CiFetch>,
}

/// Run one project's PR and CI lookups back to back on a bus worker.
///
/// Paired rather than run as two separate passes so the CI call can use the PR
/// number this pass just fetched, and so a project costs one blocking task
/// instead of two.
fn poll_one_project(poll: &ProjectPoll) -> ProjectOutcome {
    with_lane(Lane::Poll, || {
        let path = Path::new(&poll.path);
        // Repos with no GitHub remote can never have PRs or checks; skipping
        // them here keeps the whole `gh` machinery off non-GitHub projects.
        if !git::repository::has_github_remote(path) {
            return ProjectOutcome {
                pr: poll.want_pr.then_some(PrFetch::Fetched(None)),
                ci: poll.want_ci.then_some(CiFetch::Fetched {
                    sha: None,
                    summary: None,
                }),
            };
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

        ProjectOutcome { pr, ci }
    })
}

async fn poll_github(
    polls: Vec<ProjectPoll>,
    head_generations: HashMap<String, u64>,
    branches: HashMap<String, Option<String>>,
) -> GithubPollResult {
    let mut pr_infos = HashMap::new();
    let mut ci = HashMap::new();
    let mut rate_limited = false;
    let mut reached_github = false;

    let permits = Arc::new(Semaphore::new(GH_FANOUT_CONCURRENCY));
    let mut tasks = tokio::task::JoinSet::new();
    for poll in polls {
        let permits = permits.clone();
        tasks.spawn(async move {
            // Bounded so a large workspace doesn't queue dozens of blocking
            // tasks that all end up waiting on the same four bus workers.
            let _permit = permits.acquire_owned().await;
            let id = poll.id.clone();
            let outcome = tokio::task::spawn_blocking(move || poll_one_project(&poll)).await;
            (id, outcome)
        });
    }

    while let Some(joined) = tasks.join_next().await {
        let Ok((id, outcome)) = joined else {
            continue;
        };
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                log::warn!("gh poll task failed for {id}: {error}");
                continue;
            }
        };

        match outcome.pr {
            Some(PrFetch::Fetched(info)) => {
                reached_github = true;
                pr_infos.insert(id.clone(), info);
            }
            Some(PrFetch::RateLimited) => rate_limited = true,
            None => {}
        }
        match outcome.ci {
            Some(CiFetch::RateLimited) => rate_limited = true,
            Some(fetch) => {
                reached_github |= matches!(fetch, CiFetch::Fetched { .. });
                ci.insert(id, fetch);
            }
            None => {}
        }
    }

    GithubPollResult {
        head_generations,
        branches,
        pr_infos,
        ci,
        rate_limited,
        reached_github,
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_github_result(
    result: GithubPollResult,
    cycle: u64,
    current_head_generations: &HashMap<String, u64>,
    schedule: &mut GithubPollSchedule,
    pr_infos: &mut HashMap<String, Option<git::PrInfo>>,
    ci_checks: &mut HashMap<String, Option<git::CiCheckSummary>>,
    last: &mut HashMap<String, GitStatus>,
    git_status_tx: &watch::Sender<HashMap<String, ApiGitStatus>>,
    state_version: &watch::Sender<u64>,
) {
    let GithubPollResult {
        head_generations,
        branches,
        pr_infos: fetched_pr_infos,
        ci: fetched_ci,
        rate_limited,
        reached_github,
    } = result;

    if rate_limited {
        schedule.note_rate_limited(cycle);
        log::warn!(
            "GitHub API rate limit hit; PR/CI polling paused for {} cycles",
            schedule.rate_limit_backoff_cycles()
        );
    } else if reached_github {
        schedule.note_request_succeeded();
    }

    let is_current = |id: &str| {
        let expected_generation = head_generations.get(id).copied().unwrap_or_default();
        let current_generation = current_head_generations
            .get(id)
            .copied()
            .unwrap_or_default();
        let expected_branch = branches.get(id);
        let current_branch = last.get(id).map(|status| &status.branch);
        expected_generation == current_generation && expected_branch == current_branch
    };

    for (id, pr_info) in fetched_pr_infos {
        if is_current(&id) {
            schedule.record_pr(&id, cycle);
            pr_infos.insert(id, pr_info);
        }
    }
    for (id, fetch) in fetched_ci {
        if !is_current(&id) {
            continue;
        }
        match fetch {
            CiFetch::Unchanged => schedule.record_ci_unchanged(&id, cycle),
            CiFetch::Fetched { sha, summary } => {
                let pending = summary
                    .as_ref()
                    .is_some_and(|summary| summary.status.is_pending());
                schedule.record_ci(&id, cycle, pending, sha);
                ci_checks.insert(id, summary);
            }
            // Refusals never make it this far — they set `rate_limited` instead.
            CiFetch::RateLimited => {}
        }
    }

    let mut enriched = last.clone();
    for (id, status) in &mut enriched {
        status.pr_info = pr_infos.get(id).cloned().flatten();
        status.ci_checks = ci_checks.get(id).cloned().flatten();
    }
    publish(last, &enriched, git_status_tx, state_version);
}

/// Run the daemon git-status poll loop until the `watch` channel is closed (all
/// receivers dropped → the server is gone).
///
/// Each cycle snapshots all local projects and their current relevance, selects
/// only due or explicitly triggered repositories, and runs their gix work on the
/// blocking pool. Results merge into the prior cache so skipped hidden projects
/// remain published. The independent wall-clock interval keeps 5s/30s deadlines
/// stable even when targeted triggers wake the loop between cadence ticks.
/// PR/CI lookups retain their existing visible-project adaptive cadence.
///
/// Bumps `state_version` on a real change so a snapshot/broadcast observer can
/// react; the *primary* output is the `git_status_tx` watch.
pub async fn run_git_poll(
    workspace: Arc<Mutex<Workspace>>,
    git_status_tx: Arc<watch::Sender<HashMap<String, ApiGitStatus>>>,
    state_version: watch::Sender<u64>,
    remote_subscribed_terminals: Arc<RwLock<HashMap<u64, HashSet<String>>>>,
    remote_visible_projects: Arc<RwLock<HashMap<u64, HashSet<String>>>>,
    mut trigger_rx: mpsc::UnboundedReceiver<GitPollTrigger>,
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
    // Per-project `gh` cadence, commit-level result caching and the rate-limit
    // gate. Replaces the old global "is anything pending?" flag, which put every
    // project on the fast cadence as soon as one repo had CI running.
    let mut schedule = GithubPollSchedule::default();
    // A `gh` pass is running. Passes used to be spawned unconditionally, so a
    // fan-out slower than its own cadence stacked copies of itself.
    let mut github_in_flight = false;
    let mut cycle: u64 = 0;
    let mut trigger_acc = TriggerAccumulator::default();
    let mut known_streaming_ids: HashSet<String> = HashSet::new();
    let mut trigger_rx_closed = false;
    let mut head_generations: HashMap<String, u64> = HashMap::new();
    let (github_result_tx, mut github_result_rx) = mpsc::unbounded_channel();
    // Consume `interval`'s immediate first tick. Subsequent ticks stay anchored
    // to wall time, so targeted wakes cannot postpone periodic refreshes.
    let mut cadence = tokio::time::interval(GIT_POLL_INTERVAL);
    cadence.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    cadence.tick().await;
    let mut cadence_due = true;

    loop {
        drain_git_poll_triggers(&mut trigger_rx, &mut trigger_acc, &mut trigger_rx_closed);
        for id in &trigger_acc.head_change_ids {
            // Bump the generation so in-flight results for the old commit are
            // discarded. The cached CI summary is deliberately *not* dropped:
            // checks belong to the last pushed commit, which a local commit
            // doesn't move — and dropping it both blanked the badge and forced
            // a refetch on every commit.
            *head_generations.entry(id.clone()).or_default() += 1;
        }
        clear_github_cache_for_ids(
            &trigger_acc.invalidate_gh_ids,
            &mut pr_infos,
            &mut ci_checks,
        );

        // ── 1. Snapshot relevance and choose this cycle's local work ─────────
        let (projects, visible_ids, streaming_ids) = {
            let workspace = workspace.lock();
            let visible = visible_project_ids(&workspace, &remote_visible_projects);
            let streaming = streaming_project_ids(
                &workspace,
                &remote_subscribed_terminals,
                &remote_visible_projects,
            );
            let projects: Vec<(String, String)> = workspace
                .projects()
                .iter()
                .filter(|project| !project.is_remote)
                .map(|project| (project.id.clone(), project.path.clone()))
                .collect();
            (projects, visible, streaming)
        };
        let active_ids: HashSet<String> = projects.iter().map(|(id, _)| id.clone()).collect();
        pr_infos.retain(|id, _| active_ids.contains(id));
        ci_checks.retain(|id, _| active_ids.contains(id));
        head_generations.retain(|id, _| active_ids.contains(id));
        known_streaming_ids.retain(|id| active_ids.contains(id));
        schedule.retain(&active_ids);

        let newly_relevant_ids: HashSet<String> = streaming_ids
            .difference(&known_streaming_ids)
            .cloned()
            .collect();
        known_streaming_ids = streaming_ids.clone();
        let forced_local_ids = trigger_acc.local_status_ids();
        let poll_hidden =
            cycle == 0 || (cadence_due && cycle.is_multiple_of(HIDDEN_GIT_POLL_EVERY_N_CYCLES));
        let status_poll_ids = select_status_poll_ids(
            &active_ids,
            &streaming_ids,
            &forced_local_ids,
            &newly_relevant_ids,
            cadence_due,
            poll_hidden,
        );

        // Explicit actions steer the `gh` schedule: a branch switch invalidates
        // what we hold, while merely showing a project is only worth a fetch
        // when we hold no PR/CI result for it yet.
        for id in &trigger_acc.invalidate_gh_ids {
            schedule.force(id);
        }
        for id in &trigger_acc.candidate_gh_ids {
            let has_cached_result = pr_infos.contains_key(id) && ci_checks.contains_key(id);
            schedule.force_if_unfetched(id, has_cached_result);
        }

        // ── 2. Refresh selected statuses and merge into the published cache ──
        let mut attempted: HashMap<String, Option<GitStatus>> = HashMap::new();
        for (id, path) in projects
            .iter()
            .filter(|(id, _)| status_poll_ids.contains(id))
        {
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
                    attempted.insert(id, Some(status));
                }
                Ok(None) => {
                    attempted.insert(id, None);
                }
                Err(error) => {
                    // Preserve the last published value on a panicked blocking
                    // task; the next cadence or targeted trigger retries it.
                    log::error!("git status poll task panicked for {id}: {error}");
                }
            }
        }
        let missing_status_ids: HashSet<String> = attempted
            .iter()
            .filter_map(|(id, status)| status.is_none().then_some(id.clone()))
            .collect();
        clear_github_cache_for_ids(&missing_status_ids, &mut pr_infos, &mut ci_checks);
        let mut new_statuses = merge_status_results(&last, &active_ids, attempted);

        let branch_changes = branch_changed_ids(&last, &new_statuses);
        if !branch_changes.is_empty() {
            clear_github_cache_for_ids(&branch_changes, &mut pr_infos, &mut ci_checks);
            for id in &branch_changes {
                if let Some(status) = new_statuses.get_mut(id) {
                    status.pr_info = None;
                    status.ci_checks = None;
                }
                // Cached PR/CI described the branch we just left.
                schedule.force(id);
            }
        }

        // ── 3. Publish the basic status map on change — BEFORE the slow `gh` ──
        // git status comes from gix (fast, in-process); PR/CI come from `gh`
        // (network, and can hang). Publishing here means a stuck `gh` can never
        // block the branch/diff badge from appearing.
        publish(&mut last, &new_statuses, &git_status_tx, &state_version);

        // Stop once every external `watch` receiver is gone (the server is down).
        if git_status_tx.is_closed() {
            log::trace!("git poll loop exiting: no status receivers left");
            return;
        }

        // ── 4. Start `gh` PR/CI fan-out without blocking local git refreshes ─
        // Only visible projects (plus anything explicitly asked for) and only
        // while no pass is already running and GitHub isn't refusing us.
        if !github_in_flight && !schedule.is_rate_limited(cycle) {
            let polls = select_github_polls(
                &projects,
                &visible_ids,
                &schedule,
                &pr_infos,
                cycle,
                cadence_due,
            );

            log::trace!(
                "gh poll cycle={cycle}: {} projects, {} visible, {} due",
                projects.len(),
                visible_ids.len(),
                polls.len()
            );
            if !polls.is_empty() {
                // Push each project's next due cycle forward before the pass
                // leaves, so the cycles it spans don't queue it again.
                for poll in &polls {
                    if poll.want_pr {
                        schedule.pr_dispatched(&poll.id, cycle);
                    }
                    if poll.want_ci {
                        schedule.ci_dispatched(&poll.id, cycle);
                    }
                }
                let poll_generations = polls
                    .iter()
                    .map(|poll| {
                        (
                            poll.id.clone(),
                            head_generations.get(&poll.id).copied().unwrap_or_default(),
                        )
                    })
                    .collect();
                let poll_branches = polls
                    .iter()
                    .map(|poll| {
                        (
                            poll.id.clone(),
                            new_statuses
                                .get(&poll.id)
                                .and_then(|status| status.branch.clone()),
                        )
                    })
                    .collect();
                let result_tx = github_result_tx.clone();
                github_in_flight = true;
                tokio::spawn(async move {
                    let result = poll_github(polls, poll_generations, poll_branches).await;
                    let _ = result_tx.send(result);
                });
            }
        }

        trigger_acc.clear();
        if cadence_due {
            cycle = cycle.wrapping_add(1);
        }
        cadence_due = false;
        loop {
            tokio::select! {
                biased;
                _ = cadence.tick() => {
                    cadence_due = true;
                    break;
                }
                trigger = trigger_rx.recv(), if !trigger_rx_closed => {
                    match trigger {
                        Some(trigger) => {
                            trigger_acc.record(trigger);
                            break;
                        }
                        None => trigger_rx_closed = true,
                    }
                }
                Some(result) = github_result_rx.recv() => {
                    github_in_flight = false;
                    apply_github_result(
                        result,
                        cycle,
                        &head_generations,
                        &mut schedule,
                        &mut pr_infos,
                        &mut ci_checks,
                        &mut last,
                        &git_status_tx,
                        &state_version,
                    );
                }
            }
        }
    }
}

fn drain_git_poll_triggers(
    trigger_rx: &mut mpsc::UnboundedReceiver<GitPollTrigger>,
    trigger_acc: &mut TriggerAccumulator,
    trigger_rx_closed: &mut bool,
) {
    if *trigger_rx_closed {
        return;
    }
    loop {
        match trigger_rx.try_recv() {
            Ok(trigger) => {
                trigger_acc.record(trigger);
            }
            Err(mpsc::error::TryRecvError::Empty) => break,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                *trigger_rx_closed = true;
                break;
            }
        }
    }
}

fn branch_changed_ids(
    last: &HashMap<String, GitStatus>,
    new_statuses: &HashMap<String, GitStatus>,
) -> HashSet<String> {
    new_statuses
        .iter()
        .filter_map(|(id, status)| {
            last.get(id)
                .filter(|prev| prev.branch != status.branch)
                .map(|_| id.clone())
        })
        .collect()
}

fn clear_github_cache_for_ids(
    ids: &HashSet<String>,
    pr_infos: &mut HashMap<String, Option<git::PrInfo>>,
    ci_checks: &mut HashMap<String, Option<git::CiCheckSummary>>,
) {
    for id in ids {
        pr_infos.remove(id);
        ci_checks.remove(id);
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
    use crate::test_support::empty_workspace_data;

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
        let client_visible = Arc::new(RwLock::new(HashMap::new()));
        let (_trigger_tx, trigger_rx) = mpsc::unbounded_channel();
        run_git_poll(
            workspace,
            git_status_tx.clone(),
            state_version,
            subscribed,
            client_visible,
            trigger_rx,
        )
        .await;

        // No projects → nothing was published; the channel holds the initial map.
        assert!(git_status_tx.borrow().is_empty());
    }

    #[test]
    fn branch_changed_ids_only_reports_existing_branch_changes() {
        let mut last = HashMap::new();
        last.insert(
            "same".to_string(),
            GitStatus {
                branch: Some("main".to_string()),
                ..GitStatus::default()
            },
        );
        last.insert(
            "changed".to_string(),
            GitStatus {
                branch: Some("main".to_string()),
                ..GitStatus::default()
            },
        );

        let mut new_statuses = HashMap::new();
        new_statuses.insert(
            "same".to_string(),
            GitStatus {
                branch: Some("main".to_string()),
                ..GitStatus::default()
            },
        );
        new_statuses.insert(
            "changed".to_string(),
            GitStatus {
                branch: Some("feature".to_string()),
                ..GitStatus::default()
            },
        );
        new_statuses.insert(
            "new".to_string(),
            GitStatus {
                branch: Some("main".to_string()),
                ..GitStatus::default()
            },
        );

        let changed = branch_changed_ids(&last, &new_statuses);
        assert_eq!(changed, HashSet::from(["changed".to_string()]));
    }

    #[test]
    fn clear_github_cache_for_ids_removes_pr_and_ci_entries() {
        let mut prs = HashMap::from([("p1".to_string(), None), ("p2".to_string(), None)]);
        let mut checks = HashMap::from([("p1".to_string(), None), ("p3".to_string(), None)]);

        clear_github_cache_for_ids(
            &HashSet::from(["p1".to_string(), "missing".to_string()]),
            &mut prs,
            &mut checks,
        );

        assert!(!prs.contains_key("p1"));
        assert!(prs.contains_key("p2"));
        assert!(!checks.contains_key("p1"));
        assert!(checks.contains_key("p3"));
    }

    #[test]
    fn trigger_accumulator_keeps_visible_projects_conditional() {
        let mut acc = TriggerAccumulator::default();
        acc.record(GitPollTrigger::head_change("committed".to_string()));
        acc.record(GitPollTrigger::project_visible("visible".to_string()));
        acc.record(GitPollTrigger::branch_change("switched".to_string()));
        acc.record(GitPollTrigger::visibility_changed());

        assert!(acc.candidate_gh_ids.contains("visible"));
        assert!(acc.force_gh_ids.contains("switched"));
        assert!(acc.invalidate_gh_ids.contains("switched"));
        assert!(acc.head_change_ids.contains("committed"));
        assert!(!acc.force_gh_ids.contains("committed"));
        assert!(!acc.force_gh_ids.contains("visible"));
        assert_eq!(
            acc.local_status_ids(),
            HashSet::from([
                "committed".to_string(),
                "visible".to_string(),
                "switched".to_string(),
            ])
        );
    }

    #[test]
    fn status_poll_selection_respects_tiers_and_targeted_wakes() {
        let active = HashSet::from(["visible".to_string(), "hidden".to_string()]);
        let relevant = HashSet::from(["visible".to_string()]);
        let hidden = HashSet::from(["hidden".to_string()]);
        let empty = HashSet::new();

        assert_eq!(
            select_status_poll_ids(&active, &relevant, &empty, &empty, true, true),
            active,
            "startup and hidden fallback cycles scan every active project"
        );
        assert_eq!(
            select_status_poll_ids(&active, &relevant, &empty, &empty, true, false),
            relevant,
            "ordinary cadence scans only relevant projects"
        );
        assert_eq!(
            select_status_poll_ids(&active, &relevant, &hidden, &empty, false, false),
            hidden,
            "targeted hidden refreshes do not wait for fallback cadence"
        );
        assert_eq!(
            select_status_poll_ids(&active, &empty, &empty, &hidden, false, false),
            hidden,
            "promotion to the relevant tier refreshes immediately"
        );
    }

    #[test]
    fn merging_targeted_statuses_retains_unpolled_and_prunes_deleted() {
        let previous = HashMap::from([
            (
                "visible".to_string(),
                GitStatus {
                    branch: Some("main".to_string()),
                    ..GitStatus::default()
                },
            ),
            (
                "hidden".to_string(),
                GitStatus {
                    branch: Some("main".to_string()),
                    ..GitStatus::default()
                },
            ),
            ("deleted".to_string(), GitStatus::default()),
        ]);
        let active = HashSet::from([
            "visible".to_string(),
            "hidden".to_string(),
            "not-a-repo".to_string(),
        ]);
        let attempted = HashMap::from([
            (
                "hidden".to_string(),
                Some(GitStatus {
                    branch: Some("feature".to_string()),
                    ..GitStatus::default()
                }),
            ),
            ("not-a-repo".to_string(), None),
        ]);

        let merged = merge_status_results(&previous, &active, attempted);
        assert_eq!(
            merged
                .get("visible")
                .and_then(|status| status.branch.as_deref()),
            Some("main"),
            "unpolled active status stays published"
        );
        assert_eq!(
            merged
                .get("hidden")
                .and_then(|status| status.branch.as_deref()),
            Some("feature")
        );
        assert!(!merged.contains_key("deleted"));
        assert!(!merged.contains_key("not-a-repo"));
    }

    #[test]
    fn unsampled_head_snapshots_survive_fast_tier_ticks() {
        let mut previous = HashMap::from([
            ("hidden".to_string(), "old".to_string()),
            ("deleted".to_string(), "old".to_string()),
        ]);
        let active = HashSet::from(["hidden".to_string()]);

        assert!(update_head_snapshots(&mut previous, &active, HashMap::new()).is_empty());
        assert_eq!(previous.get("hidden").map(String::as_str), Some("old"));
        assert!(!previous.contains_key("deleted"));

        let changed = update_head_snapshots(
            &mut previous,
            &active,
            HashMap::from([("hidden".to_string(), "new".to_string())]),
        );
        assert_eq!(changed, vec!["hidden".to_string()]);
    }

    fn workspace_with_hidden_project(id: &str) -> Workspace {
        let mut data = empty_workspace_data();
        data.projects.push(okena_state::ProjectData {
            id: id.to_string(),
            name: "Project".to_string(),
            path: "/tmp".to_string(),
            layout: None,
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: Default::default(),
            hooks: Default::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            default_shell: None,
            hook_terminals: HashMap::new(),
            pinned: false,
            last_activity_at: None,
            is_creating: false,
            is_closing: false,
        });
        data.project_order.push(id.to_string());
        data.main_window.hidden_project_ids.insert(id.to_string());
        Workspace::new(data)
    }

    /// The regression this whole path exists for: a desktop client keeps its
    /// own visibility (client-side window ids, `window-layout.json`) and never
    /// writes to the daemon's copy, so a project hidden here can be the very
    /// one on screen. The client's declaration has to win.
    #[test]
    fn client_declared_projects_enter_the_gh_scope() {
        let workspace = workspace_with_hidden_project("on-screen");

        let nothing_declared = RwLock::new(HashMap::new());
        assert!(!visible_project_ids(&workspace, &nothing_declared).contains("on-screen"));

        let declared = RwLock::new(HashMap::from([(
            7u64,
            HashSet::from(["on-screen".to_string()]),
        )]));
        assert!(visible_project_ids(&workspace, &declared).contains("on-screen"));
    }

    #[test]
    fn every_clients_viewport_counts() {
        let workspace = Workspace::new(empty_workspace_data());
        let declared = RwLock::new(HashMap::from([
            (1u64, HashSet::from(["desktop".to_string()])),
            (2u64, HashSet::from(["phone".to_string()])),
        ]));
        let visible = visible_project_ids(&workspace, &declared);
        assert!(visible.contains("desktop") && visible.contains("phone"));
    }

    /// Build an `apply_github_result` fixture: one project, one CI outcome.
    fn github_result(generation: u64, branch: &str, ci: CiFetch) -> GithubPollResult {
        GithubPollResult {
            head_generations: HashMap::from([("p1".to_string(), generation)]),
            branches: HashMap::from([("p1".to_string(), Some(branch.to_string()))]),
            pr_infos: HashMap::from([("p1".to_string(), None)]),
            ci: HashMap::from([("p1".to_string(), ci)]),
            rate_limited: false,
            reached_github: true,
        }
    }

    fn fetched(sha: &str) -> CiFetch {
        CiFetch::Fetched {
            sha: Some(sha.to_string()),
            summary: None,
        }
    }

    struct ApplyHarness {
        schedule: GithubPollSchedule,
        pr_infos: HashMap<String, Option<git::PrInfo>>,
        ci_checks: HashMap<String, Option<git::CiCheckSummary>>,
        last: HashMap<String, GitStatus>,
        git_status_tx: watch::Sender<HashMap<String, ApiGitStatus>>,
        state_version: watch::Sender<u64>,
        _rx: watch::Receiver<HashMap<String, ApiGitStatus>>,
        _state_rx: watch::Receiver<u64>,
    }

    impl ApplyHarness {
        fn new() -> Self {
            let (git_status_tx, _rx) = watch::channel(HashMap::new());
            let (state_version, _state_rx) = watch::channel(0);
            Self {
                schedule: GithubPollSchedule::default(),
                pr_infos: HashMap::new(),
                ci_checks: HashMap::new(),
                last: HashMap::from([(
                    "p1".to_string(),
                    GitStatus {
                        branch: Some("main".to_string()),
                        ..GitStatus::default()
                    },
                )]),
                git_status_tx,
                state_version,
                _rx,
                _state_rx,
            }
        }

        fn apply(
            &mut self,
            result: GithubPollResult,
            cycle: u64,
            generations: &HashMap<String, u64>,
        ) {
            apply_github_result(
                result,
                cycle,
                generations,
                &mut self.schedule,
                &mut self.pr_infos,
                &mut self.ci_checks,
                &mut self.last,
                &self.git_status_tx,
                &self.state_version,
            );
        }
    }

    #[test]
    fn github_results_apply_only_to_the_captured_head() {
        let mut harness = ApplyHarness::new();
        let current_generations = HashMap::from([("p1".to_string(), 2)]);

        harness.apply(
            github_result(1, "main", fetched("abc")),
            5,
            &current_generations,
        );
        harness.apply(
            github_result(2, "feature", fetched("abc")),
            5,
            &current_generations,
        );
        assert!(!harness.pr_infos.contains_key("p1"));
        assert!(!harness.ci_checks.contains_key("p1"));

        harness.apply(
            github_result(2, "main", fetched("abc")),
            5,
            &current_generations,
        );
        assert!(harness.pr_infos.contains_key("p1"));
        assert!(harness.ci_checks.contains_key("p1"));
    }

    #[test]
    fn settled_result_arms_the_commit_skip_for_the_next_poll() {
        let mut harness = ApplyHarness::new();
        let generations = HashMap::from([("p1".to_string(), 0)]);

        harness.apply(github_result(0, "main", fetched("abc")), 5, &generations);

        assert_eq!(
            harness.schedule.ci_skip_sha("p1", 6).as_deref(),
            Some("abc")
        );
        // Settled → back on the slow cadence, not the pending one.
        assert!(!harness.schedule.ci_due("p1", 10, true));
        assert!(harness.schedule.ci_due("p1", 17, true));
    }

    #[test]
    fn a_skipped_fetch_keeps_the_cached_summary() {
        let mut harness = ApplyHarness::new();
        let generations = HashMap::from([("p1".to_string(), 0)]);
        harness.ci_checks.insert(
            "p1".to_string(),
            Some(git::CiCheckSummary {
                status: git::CiStatus::Success,
                passed: 1,
                failed: 0,
                pending: 0,
                total: 1,
                checks: Vec::new(),
            }),
        );

        harness.apply(
            github_result(0, "main", CiFetch::Unchanged),
            5,
            &generations,
        );

        assert!(
            harness.ci_checks.get("p1").is_some_and(Option::is_some),
            "an unchanged commit must not blank the badge"
        );
    }

    fn projects() -> Vec<(String, String)> {
        vec![
            ("visible".to_string(), "/tmp/visible".to_string()),
            ("hidden".to_string(), "/tmp/hidden".to_string()),
        ]
    }

    #[test]
    fn hidden_projects_never_earn_a_gh_slot() {
        let visible = HashSet::from(["visible".to_string()]);
        let schedule = GithubPollSchedule::default();

        let polls = select_github_polls(&projects(), &visible, &schedule, &HashMap::new(), 1, true);

        assert_eq!(polls.len(), 1);
        assert_eq!(polls[0].id, "visible");
    }

    #[test]
    fn an_explicit_request_reaches_a_hidden_project() {
        let visible = HashSet::new();
        let mut schedule = GithubPollSchedule::default();
        schedule.force("hidden");

        // Off-cadence too: an explicit action shouldn't wait for the next tick.
        let polls =
            select_github_polls(&projects(), &visible, &schedule, &HashMap::new(), 4, false);

        assert_eq!(polls.len(), 1);
        assert_eq!(polls[0].id, "hidden");
    }

    #[test]
    fn a_settled_project_carries_its_commit_so_the_fetch_can_be_skipped() {
        let visible = HashSet::from(["visible".to_string()]);
        let mut schedule = GithubPollSchedule::default();
        schedule.record_pr("visible", 1);
        schedule.record_ci("visible", 1, false, Some("abc".to_string()));

        // Nothing due yet on the settled cadence…
        assert!(
            select_github_polls(&projects(), &visible, &schedule, &HashMap::new(), 5, true)
                .is_empty()
        );

        // …and when it is, the cached commit rides along.
        let polls =
            select_github_polls(&projects(), &visible, &schedule, &HashMap::new(), 13, true);
        assert_eq!(polls.len(), 1);
        assert_eq!(polls[0].ci_skip_sha.as_deref(), Some("abc"));
    }

    #[test]
    fn one_busy_repo_does_not_speed_up_the_others() {
        let visible = HashSet::from(["visible".to_string(), "hidden".to_string()]);
        let mut schedule = GithubPollSchedule::default();
        schedule.record_pr("visible", 1);
        schedule.record_pr("hidden", 1);
        schedule.record_ci("visible", 1, true, None); // CI running
        schedule.record_ci("hidden", 1, false, Some("abc".to_string())); // settled

        let polls = select_github_polls(&projects(), &visible, &schedule, &HashMap::new(), 4, true);

        assert_eq!(polls.len(), 1, "only the repo with running CI is due");
        assert_eq!(polls[0].id, "visible");
        assert!(polls[0].want_ci && !polls[0].want_pr);
    }

    #[test]
    fn rate_limited_pass_parks_further_polling() {
        let mut harness = ApplyHarness::new();
        let generations = HashMap::from([("p1".to_string(), 0)]);
        let mut result = github_result(0, "main", fetched("abc"));
        result.rate_limited = true;
        result.reached_github = false;

        harness.apply(result, 5, &generations);

        assert!(harness.schedule.is_rate_limited(6));
    }
}
