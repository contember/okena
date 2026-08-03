//! GPUI-free git-status polling for the headless daemon.
//!
//! Projects visible in any window, or owning a terminal subscribed by a remote
//! client, stay on the responsive tier: HEAD every 250ms and full status every
//! 5s. Hidden, unsubscribed repositories use bounded fallback cadences (2s HEAD,
//! 30s full status). Explicit actions and detected HEAD changes still trigger an
//! immediate targeted refresh. Cached statuses for projects not selected in a
//! cycle remain published, so tiering changes freshness rather than visibility.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use okena_core::api::ApiGitStatus;
use okena_core::git_poll::GitPollTrigger;
use okena_core::process::{Lane, with_lane};
use okena_git::{self as git, GitStatus, HeadSnapshot};
use okena_workspace::state::Workspace;
use parking_lot::Mutex;
use tokio::sync::{mpsc, watch};

/// Responsive full-status cadence for visible or remotely subscribed projects.
const GIT_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Hidden projects receive a full fallback scan every 6 responsive cycles (30s).
const HIDDEN_GIT_POLL_EVERY_N_CYCLES: u64 = 6;
/// Responsive HEAD cadence for visible or remotely subscribed projects.
const HEAD_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// Hidden projects receive a cheap HEAD fallback scan every 8 ticks (2s).
const HIDDEN_HEAD_POLL_EVERY_N_TICKS: u64 = 8;
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
    ci_checks: HashMap<String, Option<git::CiCheckSummary>>,
}

fn relevant_project_ids(
    workspace: &Workspace,
    remote_subscribed_terminals: &RwLock<HashMap<u64, HashSet<String>>>,
) -> HashSet<String> {
    let mut relevant = workspace.all_visible_project_ids();
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
            let relevant = relevant_project_ids(&workspace, &remote_subscribed_terminals);
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

async fn poll_github(
    projects: Vec<(String, String)>,
    pr_poll_ids: HashSet<String>,
    ci_poll_ids: HashSet<String>,
    cached_pr_infos: HashMap<String, Option<git::PrInfo>>,
    head_generations: HashMap<String, u64>,
    branches: HashMap<String, Option<String>>,
) -> GithubPollResult {
    let mut pr_infos = HashMap::new();
    let mut ci_checks = HashMap::new();

    for (id, path) in &projects {
        if !pr_poll_ids.contains(id) {
            continue;
        }
        let task_id = id.clone();
        let path = path.clone();
        let fetched = tokio::task::spawn_blocking(move || {
            with_lane(Lane::Poll, || {
                if !git::repository::has_github_remote(Path::new(&path)) {
                    return None;
                }
                git::repository::get_pr_info(Path::new(&path))
            })
        })
        .await;
        match fetched {
            Ok(pr) => {
                pr_infos.insert(task_id, pr);
            }
            Err(error) => log::warn!("gh PR info task failed for {task_id}: {error}"),
        }
    }

    for (id, path) in &projects {
        if !ci_poll_ids.contains(id) {
            continue;
        }
        let task_id = id.clone();
        let path = path.clone();
        let pr_number = pr_infos
            .get(id)
            .or_else(|| cached_pr_infos.get(id))
            .and_then(|pr| pr.as_ref())
            .map(|pr| pr.number);
        let fetched = tokio::task::spawn_blocking(move || {
            with_lane(Lane::Poll, || {
                if !git::repository::has_github_remote(Path::new(&path)) {
                    return None;
                }
                git::repository::get_ci_checks(Path::new(&path), pr_number)
            })
        })
        .await;
        match fetched {
            Ok(checks) => {
                ci_checks.insert(task_id, checks);
            }
            Err(error) => log::warn!("gh CI checks task failed for {task_id}: {error}"),
        }
    }

    GithubPollResult {
        head_generations,
        branches,
        pr_infos,
        ci_checks,
    }
}

fn apply_github_result(
    result: GithubPollResult,
    current_head_generations: &HashMap<String, u64>,
    pr_infos: &mut HashMap<String, Option<git::PrInfo>>,
    ci_checks: &mut HashMap<String, Option<git::CiCheckSummary>>,
    last: &mut HashMap<String, GitStatus>,
    git_status_tx: &watch::Sender<HashMap<String, ApiGitStatus>>,
    state_version: &watch::Sender<u64>,
) -> bool {
    let GithubPollResult {
        head_generations,
        branches,
        pr_infos: fetched_pr_infos,
        ci_checks: fetched_ci_checks,
    } = result;
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
            pr_infos.insert(id, pr_info);
        }
    }
    for (id, checks) in fetched_ci_checks {
        if is_current(&id) {
            ci_checks.insert(id, checks);
        }
    }

    let mut enriched = last.clone();
    for (id, status) in &mut enriched {
        status.pr_info = pr_infos.get(id).cloned().flatten();
        status.ci_checks = ci_checks.get(id).cloned().flatten();
    }
    publish(last, &enriched, git_status_tx, state_version);
    has_pending_ci(ci_checks)
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
    // Drives the adaptive CI cadence: faster polling while any check is pending.
    let mut any_pending_ci = false;
    let mut cycle: u64 = 0;
    let mut trigger_acc = TriggerAccumulator::default();
    let mut known_gh_ids: HashSet<String> = HashSet::new();
    let mut known_relevant_ids: HashSet<String> = HashSet::new();
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
        let mut github_cache_changed = false;
        for id in &trigger_acc.head_change_ids {
            *head_generations.entry(id.clone()).or_default() += 1;
            github_cache_changed |= ci_checks.remove(id).is_some();
        }
        github_cache_changed |= clear_github_cache_for_ids(
            &trigger_acc.invalidate_gh_ids,
            &mut pr_infos,
            &mut ci_checks,
        );
        if github_cache_changed {
            any_pending_ci = has_pending_ci(&ci_checks);
        }

        // ── 1. Snapshot relevance and choose this cycle's local work ─────────
        let (projects, relevant_ids): (Vec<(String, String)>, HashSet<String>) = {
            let workspace = workspace.lock();
            let relevant = relevant_project_ids(&workspace, &remote_subscribed_terminals);
            let projects = workspace
                .projects()
                .iter()
                .filter(|project| !project.is_remote)
                .map(|project| (project.id.clone(), project.path.clone()))
                .collect();
            (projects, relevant)
        };
        let active_ids: HashSet<String> = projects.iter().map(|(id, _)| id.clone()).collect();
        pr_infos.retain(|id, _| active_ids.contains(id));
        ci_checks.retain(|id, _| active_ids.contains(id));
        head_generations.retain(|id, _| active_ids.contains(id));
        known_gh_ids.retain(|id| active_ids.contains(id));
        known_relevant_ids.retain(|id| active_ids.contains(id));

        let newly_relevant_ids: HashSet<String> = relevant_ids
            .difference(&known_relevant_ids)
            .cloned()
            .collect();
        known_relevant_ids = relevant_ids.clone();
        let forced_local_ids = trigger_acc.local_status_ids();
        let poll_hidden =
            cycle == 0 || (cadence_due && cycle.is_multiple_of(HIDDEN_GIT_POLL_EVERY_N_CYCLES));
        let status_poll_ids = select_status_poll_ids(
            &active_ids,
            &relevant_ids,
            &forced_local_ids,
            &newly_relevant_ids,
            cadence_due,
            poll_hidden,
        );

        // `gh` stays limited to relevant/explicit projects independently of the
        // slower hidden local-status fallback.
        let mut gh_ids = relevant_ids;
        gh_ids.extend(trigger_acc.force_gh_ids.iter().cloned());
        gh_ids.extend(trigger_acc.candidate_gh_ids.iter().cloned());
        if cycle != 0 || !known_gh_ids.is_empty() {
            trigger_acc.force_gh_ids.extend(missing_github_stats_ids(
                gh_ids.difference(&known_gh_ids),
                &pr_infos,
                &ci_checks,
            ));
        }
        trigger_acc.force_gh_ids.extend(missing_github_stats_ids(
            trigger_acc.candidate_gh_ids.iter(),
            &pr_infos,
            &ci_checks,
        ));
        gh_ids.extend(trigger_acc.force_gh_ids.iter().cloned());
        known_gh_ids = gh_ids.clone();

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
        if clear_github_cache_for_ids(&missing_status_ids, &mut pr_infos, &mut ci_checks) {
            any_pending_ci = has_pending_ci(&ci_checks);
        }
        let mut new_statuses = merge_status_results(&last, &active_ids, attempted);

        let branch_changes = branch_changed_ids(&last, &new_statuses);
        if !branch_changes.is_empty() {
            if clear_github_cache_for_ids(&branch_changes, &mut pr_infos, &mut ci_checks) {
                any_pending_ci = has_pending_ci(&ci_checks);
            }
            for id in &branch_changes {
                if let Some(status) = new_statuses.get_mut(id) {
                    status.pr_info = None;
                    status.ci_checks = None;
                }
            }
            trigger_acc
                .force_gh_ids
                .extend(branch_changes.iter().cloned());
            gh_ids.extend(branch_changes);
        }

        // Skip the cycle-0 `gh` fan-out unless something explicitly forced it:
        // startup still gets basic gix status immediately without a `gh` herd.
        let ci_poll_interval = if any_pending_ci {
            CI_PENDING_POLL_EVERY_N_CYCLES
        } else {
            CI_SETTLED_POLL_EVERY_N_CYCLES
        };
        let pr_cadence = cadence_due
            && (cycle == 1 || (cycle != 0 && cycle.is_multiple_of(PR_POLL_EVERY_N_CYCLES)));
        let ci_cadence =
            cadence_due && (cycle == 1 || (cycle != 0 && cycle.is_multiple_of(ci_poll_interval)));
        let force_gh = !trigger_acc.force_gh_ids.is_empty();
        let check_prs = force_gh || pr_cadence;
        let check_ci = force_gh || ci_cadence;
        let pr_poll_ids = gh_poll_ids(&gh_ids, &trigger_acc.force_gh_ids, force_gh, pr_cadence);
        let ci_poll_ids = gh_poll_ids(&gh_ids, &trigger_acc.force_gh_ids, force_gh, ci_cadence);

        // ── 3. Publish the basic status map on change — BEFORE the slow `gh` ──
        // git status comes from gix (fast, in-process); PR/CI come from `gh`
        // (network, and can hang). Publishing here means a stuck `gh` can never
        // block the branch/diff badge from appearing.
        publish(&mut last, &new_statuses, &git_status_tx, &state_version);

        // Stop once every external `watch` receiver is gone (the server is down).
        if git_status_tx.is_closed() {
            return;
        }

        // ── 4. Start `gh` PR/CI fan-out without blocking local git refreshes ─
        if check_prs || check_ci {
            let result_tx = github_result_tx.clone();
            let poll_generations = projects
                .iter()
                .map(|(id, _)| {
                    (
                        id.clone(),
                        head_generations.get(id).copied().unwrap_or_default(),
                    )
                })
                .collect();
            let poll_branches = new_statuses
                .iter()
                .map(|(id, status)| (id.clone(), status.branch.clone()))
                .collect();
            let cached_pr_infos = pr_infos.clone();
            tokio::spawn(async move {
                let result = poll_github(
                    projects,
                    pr_poll_ids,
                    ci_poll_ids,
                    cached_pr_infos,
                    poll_generations,
                    poll_branches,
                )
                .await;
                let _ = result_tx.send(result);
            });
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
                    any_pending_ci = apply_github_result(
                        result,
                        &head_generations,
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

fn missing_github_stats(
    id: &str,
    pr_infos: &HashMap<String, Option<git::PrInfo>>,
    ci_checks: &HashMap<String, Option<git::CiCheckSummary>>,
) -> bool {
    !(pr_infos.contains_key(id) && ci_checks.contains_key(id))
}

fn missing_github_stats_ids<'a>(
    ids: impl Iterator<Item = &'a String>,
    pr_infos: &HashMap<String, Option<git::PrInfo>>,
    ci_checks: &HashMap<String, Option<git::CiCheckSummary>>,
) -> HashSet<String> {
    ids.filter(|id| missing_github_stats(id, pr_infos, ci_checks))
        .cloned()
        .collect()
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
) -> bool {
    let mut changed = false;
    for id in ids {
        changed |= pr_infos.remove(id).is_some();
        changed |= ci_checks.remove(id).is_some();
    }
    changed
}

fn has_pending_ci(ci_checks: &HashMap<String, Option<git::CiCheckSummary>>) -> bool {
    ci_checks
        .values()
        .any(|c| c.as_ref().map(|s| s.status.is_pending()).unwrap_or(false))
}

fn gh_poll_ids(
    gh_ids: &HashSet<String>,
    forced_gh_ids: &HashSet<String>,
    force_gh: bool,
    cadence: bool,
) -> HashSet<String> {
    match (force_gh, cadence) {
        (true, true) => gh_ids.clone(),
        (true, false) => forced_gh_ids.clone(),
        (false, true) => gh_ids.clone(),
        (false, false) => HashSet::new(),
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
        let (_trigger_tx, trigger_rx) = mpsc::unbounded_channel();
        run_git_poll(
            workspace,
            git_status_tx.clone(),
            state_version,
            subscribed,
            trigger_rx,
        )
        .await;

        // No projects → nothing was published; the channel holds the initial map.
        assert!(git_status_tx.borrow().is_empty());
    }

    #[test]
    fn forced_gh_poll_ids_are_targeted_between_cadence_cycles() {
        let gh_ids: HashSet<String> = ["visible-a".to_string(), "visible-b".to_string()]
            .into_iter()
            .collect();
        let forced: HashSet<String> = ["switched".to_string()].into_iter().collect();

        assert_eq!(gh_poll_ids(&gh_ids, &forced, true, false), forced);
    }

    #[test]
    fn missing_github_stats_requires_both_cache_slots() {
        let mut prs = HashMap::new();
        let mut checks = HashMap::new();

        assert!(missing_github_stats("p1", &prs, &checks));

        prs.insert("p1".to_string(), None);
        assert!(missing_github_stats("p1", &prs, &checks));

        checks.insert("p1".to_string(), None);
        assert!(!missing_github_stats("p1", &prs, &checks));
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

        let changed = clear_github_cache_for_ids(
            &HashSet::from(["p1".to_string(), "missing".to_string()]),
            &mut prs,
            &mut checks,
        );

        assert!(changed);
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

    #[test]
    fn github_results_apply_only_to_the_captured_head() {
        let (git_status_tx, _rx) = watch::channel(HashMap::new());
        let (state_version, _state_rx) = watch::channel(0);
        let mut last = HashMap::from([(
            "p1".to_string(),
            GitStatus {
                branch: Some("main".to_string()),
                ..GitStatus::default()
            },
        )]);
        let mut pr_infos = HashMap::new();
        let mut ci_checks = HashMap::new();
        let current_generations = HashMap::from([("p1".to_string(), 2)]);

        let result = |generation, branch: &str| GithubPollResult {
            head_generations: HashMap::from([("p1".to_string(), generation)]),
            branches: HashMap::from([("p1".to_string(), Some(branch.to_string()))]),
            pr_infos: HashMap::from([("p1".to_string(), None)]),
            ci_checks: HashMap::from([("p1".to_string(), None)]),
        };

        apply_github_result(
            result(1, "main"),
            &current_generations,
            &mut pr_infos,
            &mut ci_checks,
            &mut last,
            &git_status_tx,
            &state_version,
        );
        apply_github_result(
            result(2, "feature"),
            &current_generations,
            &mut pr_infos,
            &mut ci_checks,
            &mut last,
            &git_status_tx,
            &state_version,
        );
        assert!(!pr_infos.contains_key("p1"));
        assert!(!ci_checks.contains_key("p1"));

        apply_github_result(
            result(2, "main"),
            &current_generations,
            &mut pr_infos,
            &mut ci_checks,
            &mut last,
            &git_status_tx,
            &state_version,
        );
        assert!(pr_infos.contains_key("p1"));
        assert!(ci_checks.contains_key("p1"));
    }
}
