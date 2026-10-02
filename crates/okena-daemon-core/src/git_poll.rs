//! GPUI-free git-status polling for the headless daemon.
//!
//! Projects in a client's declared viewport (`SetVisibleProjects`), or owning a
//! terminal streamed by a client that never declared one, stay on the
//! responsive tier: HEAD every 250ms and full status every 5s. A declared
//! viewport is the whole truth for that client — the desktop subscribes to
//! every terminal it mirrors, so its subscriptions say nothing about what is
//! on screen — and any declared viewport supersedes the workspace's own window
//! state: visibility is client-owned (`window-layout.json`), so the daemon's
//! copy is a stale legacy set, consulted only while nobody has declared one.
//! Everything else uses bounded fallback cadences (2s HEAD, 30s full status).
//! Explicit actions and detected HEAD changes still trigger an immediate
//! targeted refresh. Cached statuses for projects not selected in a cycle
//! remain published, so tiering changes freshness rather than visibility.
//!
//! The GitHub PR/CI fan-out is deliberately *narrower* than the local tier: it
//! covers only that visible set (plus explicitly requested ones), is scheduled
//! per project by [`GithubPollSchedule`], skips any project whose upstream
//! commit hasn't moved since its last settled result, and parks itself when
//! GitHub reports the API rate limit as exhausted.

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

/// Responsive full-status cadence for projects on the responsive tier.
const GIT_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Hidden projects receive a full fallback scan every 6 responsive cycles (30s).
const HIDDEN_GIT_POLL_EVERY_N_CYCLES: u64 = 6;
/// Responsive HEAD cadence for projects on the responsive tier.
const HEAD_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// Hidden projects receive a cheap HEAD fallback scan every 8 ticks (2s).
const HIDDEN_HEAD_POLL_EVERY_N_TICKS: u64 = 8;
/// How many projects the GitHub fan-out talks to at once. Going wider only
/// parks more blocking-pool threads on the network; going narrower (the
/// previous strictly sequential loop) made a full pass outlast its own cadence
/// and let passes pile up on top of each other.
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
    /// Unconditional GitHub refreshes. Used when existing PR/CI cache is invalid.
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

/// A message from a running GitHub pass back to the poll loop.
enum GithubPassMessage {
    /// One project's outcome, sent the moment that project finishes. A pass
    /// used to publish nothing until its slowest repo returned, so a 0.4s PR
    /// lookup could sit behind another project's 15s request timeout.
    Project(Box<GithubPollResult>),
    /// The pass is over; carries the ids it held so they can be polled again.
    Finished(HashSet<String>),
}

struct GithubPollResult {
    repositories: HashMap<String, git::repository::GithubRepo>,
    failed_ids: HashSet<String>,
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

/// One project's slot in a GitHub pass.
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
    /// The cached PR number and CI SHA are reusable only within this repository.
    cached_repository: Option<git::repository::GithubRepo>,
}

/// Union of declared viewports (`SetVisibleProjects`). The workspace's own window
/// state counts only while no connection has declared one: visibility is
/// client-owned (`window-layout.json`), so the daemon's copy is a stale legacy set.
fn visible_project_ids(
    workspace: &Workspace,
    remote_visible_projects: &RwLock<HashMap<u64, HashSet<String>>>,
) -> HashSet<String> {
    match remote_visible_projects.read() {
        Ok(declared) if !declared.is_empty() => declared.values().flatten().cloned().collect(),
        _ => workspace.all_visible_project_ids(),
    }
}

/// Visible projects plus any owning a terminal streamed by a connection that
/// has not declared a viewport. A declared viewport is the whole truth for that
/// connection, so its subscriptions are ignored (see the module docs).
fn streaming_project_ids(
    workspace: &Workspace,
    remote_subscribed_terminals: &RwLock<HashMap<u64, HashSet<String>>>,
    remote_visible_projects: &RwLock<HashMap<u64, HashSet<String>>>,
) -> HashSet<String> {
    let mut relevant = visible_project_ids(workspace, remote_visible_projects);
    let (Ok(subscribed), Ok(declared)) = (
        remote_subscribed_terminals.read(),
        remote_visible_projects.read(),
    ) else {
        return relevant;
    };
    let undeclared = subscribed
        .iter()
        .filter(|(connection_id, _)| !declared.contains_key(connection_id));
    for (_, terminal_ids) in undeclared {
        for terminal_id in terminal_ids {
            if let Some(project) = workspace.find_project_for_terminal(terminal_id) {
                relevant.insert(project.id.clone());
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

/// Pick this cycle's GitHub slots.
///
/// Rules: a project earns a slot only if it is *visible* (or explicitly asked
/// for), only when its own schedule says it is due — one repo with running CI
/// no longer drags every other repo onto the fast cadence — and only when no
/// running pass already covers it.
///
/// `urgent_only` is set while another pass is still running. A cadence pass
/// waits its turn (passes used to stack copies of themselves), but a project
/// someone explicitly forced — a branch switch — jumps straight out rather than
/// waiting for that pass plus the next cadence tick.
#[allow(clippy::too_many_arguments)]
fn select_github_polls(
    projects: &[(String, String)],
    visible_ids: &HashSet<String>,
    schedule: &GithubPollSchedule,
    pr_infos: &HashMap<String, Option<git::PrInfo>>,
    cycle: u64,
    cadence_due: bool,
    in_flight: &HashSet<String>,
    urgent_only: bool,
) -> Vec<ProjectPoll> {
    projects
        .iter()
        .filter(|(id, _)| visible_ids.contains(id) || schedule.is_urgent(id))
        .filter(|(id, _)| !in_flight.contains(id))
        .filter(|(id, _)| !urgent_only || schedule.is_urgent(id))
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
                cached_repository: None,
            })
        })
        .collect()
}

/// What one project's GitHub slot produced.
struct ProjectOutcome {
    repository: Option<git::repository::GithubRepo>,
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
        // them here keeps the whole GitHub machinery off non-GitHub projects.
        if !git::repository::has_github_remote(path) {
            return ProjectOutcome {
                repository: None,
                pr: poll.want_pr.then_some(PrFetch::Fetched(None)),
                ci: poll.want_ci.then_some(CiFetch::Fetched {
                    sha: None,
                    summary: None,
                }),
            };
        }

        let repository = git::repository::resolve_base_repo(path);
        let repository_changed = repository != poll.cached_repository;
        let pr = if poll.want_pr {
            Some(git::repository::fetch_pr_info(path))
        } else {
            repository_changed.then_some(PrFetch::Failed)
        };
        let pr_number = match &pr {
            Some(PrFetch::Fetched(info)) => info.as_ref().map(|info| info.number),
            Some(PrFetch::Failed) => None,
            _ => poll.cached_pr_number,
        };
        let pr_identity_changed =
            matches!(pr, Some(PrFetch::Fetched(_))) && pr_number != poll.cached_pr_number;

        // A rate-limited PR call means the CI call would only be refused too.
        let ci = if matches!(pr, Some(PrFetch::RateLimited)) {
            None
        } else if poll.want_ci {
            Some(git::repository::fetch_ci_checks(
                path,
                pr_number,
                if repository_changed || pr_identity_changed {
                    None
                } else {
                    poll.ci_skip_sha.as_deref()
                },
            ))
        } else {
            repository_changed.then_some(CiFetch::Failed {
                sha: None,
                summary: None,
            })
        };

        if repository != git::repository::resolve_base_repo(path) {
            return ProjectOutcome {
                repository: None,
                pr: Some(PrFetch::Failed),
                ci: None,
            };
        }
        ProjectOutcome { repository, pr, ci }
    })
}

/// Run one GitHub pass, emitting each project's outcome on `result_tx` as soon as
/// that project returns and a [`GithubPassMessage::Finished`] when all have.
///
/// Streaming rather than returning one aggregate is deliberate: a GitHub round
/// trip per repo ranges from well under a second to the 15s request cap, and
/// an aggregate held every badge in the pass hostage to its slowest repo.
async fn poll_github(
    polls: Vec<ProjectPoll>,
    head_generations: HashMap<String, u64>,
    branches: HashMap<String, Option<String>>,
    result_tx: mpsc::UnboundedSender<GithubPassMessage>,
) {
    let pass_ids: HashSet<String> = polls.iter().map(|poll| poll.id.clone()).collect();

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
                log::warn!("GitHub poll task failed for {id}: {error}");
                ProjectOutcome {
                    repository: None,
                    pr: Some(PrFetch::Failed),
                    ci: None,
                }
            }
        };

        let mut pr_infos = HashMap::new();
        let mut ci = HashMap::new();
        let mut rate_limited = false;
        let mut reached_github = false;
        let mut failed_ids = HashSet::new();

        match outcome.pr {
            Some(PrFetch::Fetched(info)) => {
                reached_github = true;
                pr_infos.insert(id.clone(), info);
            }
            Some(PrFetch::RateLimited) => rate_limited = true,
            Some(PrFetch::Failed) => {
                failed_ids.insert(id.clone());
                pr_infos.insert(id.clone(), None);
            }
            None => {}
        }
        match outcome.ci {
            Some(CiFetch::RateLimited) => rate_limited = true,
            Some(fetch) => {
                if matches!(fetch, CiFetch::Failed { .. }) {
                    failed_ids.insert(id.clone());
                }
                reached_github |= matches!(fetch, CiFetch::Fetched { .. });
                ci.insert(id.clone(), fetch);
            }
            None => {}
        }

        // The staleness guard on the receiving side looks both up by id, so a
        // single-entry map carries everything this project's result needs. The
        // entries must exist even when empty — a missing branch reads as a
        // mismatch and the result would be dropped.
        let result = GithubPollResult {
            repositories: outcome
                .repository
                .into_iter()
                .map(|repo| (id.clone(), repo))
                .collect(),
            failed_ids,
            head_generations: HashMap::from([(
                id.clone(),
                head_generations.get(&id).copied().unwrap_or_default(),
            )]),
            branches: HashMap::from([(id.clone(), branches.get(&id).cloned().flatten())]),
            pr_infos,
            ci,
            rate_limited,
            reached_github,
        };
        if result_tx
            .send(GithubPassMessage::Project(Box::new(result)))
            .is_err()
        {
            return;
        }
    }

    let _ = result_tx.send(GithubPassMessage::Finished(pass_ids));
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
    let current_ids: HashSet<_> = result
        .branches
        .keys()
        .filter(|id| github_result_is_current(&result, id, current_head_generations, last))
        .cloned()
        .collect();
    let GithubPollResult {
        repositories: _,
        failed_ids: _,
        head_generations: _,
        branches: _,
        pr_infos: fetched_pr_infos,
        ci: fetched_ci,
        rate_limited,
        reached_github,
    } = result;

    // Guarded because results now arrive per project: without it, five refused
    // projects in one pass would double the backoff five times over instead of
    // once. One step per cycle keeps the doubling tied to elapsed time.
    if rate_limited {
        for id in &current_ids {
            schedule.require_fresh_ci(id);
        }
        if !schedule.is_rate_limited(cycle) {
            schedule.note_rate_limited(cycle);
            log::warn!(
                "GitHub API rate limit hit; PR/CI polling paused for {} cycles",
                schedule.rate_limit_backoff_cycles()
            );
        }
    } else if reached_github {
        schedule.note_request_succeeded();
    }

    for (id, pr_info) in fetched_pr_infos {
        if current_ids.contains(&id) {
            let identity_changed =
                match (pr_infos.get(&id).and_then(Option::as_ref), pr_info.as_ref()) {
                    (Some(old), Some(new)) => !same_pr_identity(old, new),
                    (None, None) => false,
                    _ => true,
                };
            if identity_changed {
                schedule.require_fresh_ci(&id);
                ci_checks.remove(&id);
            }
            schedule.record_pr(&id, cycle);
            pr_infos.insert(id, pr_info);
        }
    }
    for (id, fetch) in fetched_ci {
        if !current_ids.contains(&id) {
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
            CiFetch::Failed { summary, .. } => {
                schedule.require_fresh_ci(&id);
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

fn same_pr_identity(a: &git::PrInfo, b: &git::PrInfo) -> bool {
    a.number == b.number && a.url == b.url
}

fn github_result_is_current(
    result: &GithubPollResult,
    id: &str,
    generations: &HashMap<String, u64>,
    statuses: &HashMap<String, GitStatus>,
) -> bool {
    result.head_generations.get(id).copied().unwrap_or_default()
        == generations.get(id).copied().unwrap_or_default()
        && result.branches.get(id) == statuses.get(id).map(|status| &status.branch)
}

fn invalidate_mission_prs(
    ids: &HashSet<String>,
    workspace: &Arc<Mutex<Workspace>>,
    workspace_tick: &watch::Sender<u64>,
) {
    let mut ws = workspace.lock();
    let mut cx = crate::workspace_cx::DaemonWorkspaceCx::new(workspace_tick, &None, &None);
    for id in ids {
        if ws.data().missions.iter().any(|mission| {
            mission
                .pull_requests
                .iter()
                .any(|pr| pr.source_project_ids.contains(id))
        }) {
            let _ = ws.record_mission_pr(id, None, &mut cx);
        }
    }
}

fn record_mission_observations(
    result: &GithubPollResult,
    generations: &HashMap<String, u64>,
    statuses: &HashMap<String, GitStatus>,
    failed_ci: &mut HashSet<String>,
    workspace: &Arc<Mutex<Workspace>>,
    workspace_tick: &watch::Sender<u64>,
) {
    use okena_core::mission::{MissionPullRequest, PullRequestIdentity};
    use okena_workspace::missions::MissionPrObservation;
    let mut ws = workspace.lock();
    let mut cx = crate::workspace_cx::DaemonWorkspaceCx::new(workspace_tick, &None, &None);
    for id in result.branches.keys() {
        if !github_result_is_current(result, id, generations, statuses) {
            continue;
        }
        match result.ci.get(id) {
            Some(CiFetch::Failed { .. } | CiFetch::RateLimited) => {
                failed_ci.insert(id.clone());
            }
            Some(CiFetch::Fetched { .. }) => {
                failed_ci.remove(id);
            }
            _ if result.rate_limited => {
                failed_ci.insert(id.clone());
            }
            _ => {}
        }
        if !ws.data().missions.iter().any(|mission| {
            mission.repository_ids.contains(id)
                || mission.worktree_ids.contains(id)
                || mission
                    .pull_requests
                    .iter()
                    .any(|pr| pr.source_project_ids.contains(id))
        }) {
            continue;
        }
        let available =
            !result.rate_limited && !result.failed_ids.contains(id) && !failed_ci.contains(id);
        let pr_fetched = result.pr_infos.get(id).is_some_and(Option::is_some);
        let ci_fetched = matches!(result.ci.get(id), Some(CiFetch::Fetched { .. }));
        if !pr_fetched && !ci_fetched && available && !result.pr_infos.contains_key(id) {
            continue;
        }
        let observation = if pr_fetched || ci_fetched {
            result.repositories.get(id).and_then(|repository| {
                let cached = statuses.get(id)?;
                let info = result
                    .pr_infos
                    .get(id)
                    .unwrap_or(&cached.pr_info)
                    .as_ref()?
                    .clone();
                let ci = match result.ci.get(id) {
                    Some(CiFetch::Fetched { summary, .. }) => summary.clone(),
                    _ if cached
                        .pr_info
                        .as_ref()
                        .is_some_and(|old| same_pr_identity(old, &info)) =>
                    {
                        cached.ci_checks.clone()
                    }
                    _ => None,
                };
                Some(MissionPrObservation {
                    snapshot: MissionPullRequest {
                        identity: PullRequestIdentity {
                            host: repository.host.clone(),
                            repository: format!("{}/{}", repository.owner, repository.name)
                                .to_ascii_lowercase(),
                            number: info.number,
                        },
                        info,
                        ci,
                        observed_at: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                        available,
                        source_project_ids: Vec::new(),
                    },
                    pr_fetched,
                    ci_fetched,
                })
            })
        } else {
            None
        };
        if observation.is_none()
            && !ws.data().missions.iter().any(|mission| {
                mission
                    .pull_requests
                    .iter()
                    .any(|pr| pr.source_project_ids.contains(id))
            })
        {
            continue;
        }
        if let Err(error) = ws.record_mission_pr(id, observation, &mut cx) {
            log::debug!("mission PR observation for {id} rejected: {error}");
            let _ = ws.record_mission_pr(id, None, &mut cx);
        }
    }
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
    workspace_tick: watch::Sender<u64>,
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
    // `pr_infos` / `ci_checks`. The expensive GitHub fan-out only runs on the
    // cadence below; between those cycles the cached values are merged into every
    // status so the badges don't blank. Merge (not replace) on update so a
    // project that drops out of the visible set keeps its last-known PR/CI.
    let mut pr_infos: HashMap<String, Option<git::PrInfo>> = HashMap::new();
    let mut ci_checks: HashMap<String, Option<git::CiCheckSummary>> = HashMap::new();
    // Per-project GitHub cadence, commit-level result caching and the rate-limit
    // gate. Replaces the old global "is anything pending?" flag, which put every
    // project on the fast cadence as soon as one repo had CI running.
    let mut schedule = GithubPollSchedule::default();
    // Projects a running GitHub pass currently holds. Passes used to be spawned
    // unconditionally, so a fan-out slower than its own cadence stacked copies
    // of itself; tracking the ids (rather than a bare flag) keeps that
    // protection while letting an explicitly forced project start its own pass
    // instead of waiting out the one in progress.
    let mut github_in_flight: HashSet<String> = HashSet::new();
    let mut cycle: u64 = 0;
    let mut trigger_acc = TriggerAccumulator::default();
    let mut known_streaming_ids: HashSet<String> = HashSet::new();
    let mut trigger_rx_closed = false;
    let mut head_generations: HashMap<String, u64> = HashMap::new();
    let mut failed_mission_ci = HashSet::new();
    let mut repository_cache = HashMap::new();
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
        invalidate_mission_prs(&trigger_acc.invalidate_gh_ids, &workspace, &workspace_tick);
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
                .map(|project| (project.id.clone(), project.path.clone()))
                .collect();
            (projects, visible, streaming)
        };
        let active_ids: HashSet<String> = projects.iter().map(|(id, _)| id.clone()).collect();
        pr_infos.retain(|id, _| active_ids.contains(id));
        ci_checks.retain(|id, _| active_ids.contains(id));
        head_generations.retain(|id, _| active_ids.contains(id));
        failed_mission_ci.retain(|id| active_ids.contains(id));
        repository_cache.retain(|id, _| active_ids.contains(id));
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

        // Explicit actions steer the GitHub schedule: a branch switch invalidates
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
        let mut failed_status_ids = HashSet::new();
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
                    // badge doesn't blank between GitHub cadence cycles.
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
                    failed_status_ids.insert(id);
                }
            }
        }
        let missing_status_ids: HashSet<String> = attempted
            .iter()
            .filter_map(|(id, status)| status.is_none().then_some(id.clone()))
            .collect();
        clear_github_cache_for_ids(&missing_status_ids, &mut pr_infos, &mut ci_checks);
        invalidate_mission_prs(&missing_status_ids, &workspace, &workspace_tick);
        invalidate_mission_prs(&failed_status_ids, &workspace, &workspace_tick);
        let mut new_statuses = merge_status_results(&last, &active_ids, attempted);

        let branch_changes = branch_changed_ids(&last, &new_statuses);
        if !branch_changes.is_empty() {
            invalidate_mission_prs(&branch_changes, &workspace, &workspace_tick);
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

        // ── 3. Publish the basic status map on change — BEFORE the slow GitHub calls
        // git status comes from gix (fast, in-process); PR/CI come from the GitHub
        // API (network, and can stall). Publishing here means a stuck request can
        // never block the branch/diff badge from appearing.
        publish(&mut last, &new_statuses, &git_status_tx, &state_version);

        // Stop once every external `watch` receiver is gone (the server is down).
        if git_status_tx.is_closed() {
            log::trace!("git poll loop exiting: no status receivers left");
            return;
        }

        // ── 4. Start GitHub PR/CI fan-out without blocking local git refreshes ─
        // Only visible projects (plus anything explicitly asked for) and only
        // while no pass is already running and GitHub isn't refusing us.
        if !schedule.is_rate_limited(cycle) {
            // While a pass runs, only forced projects earn a second one — a
            // branch switch shouldn't have to wait out the pass in progress
            // and then the next cadence tick on top of it.
            let urgent_only = !github_in_flight.is_empty();
            let mut polls = select_github_polls(
                &projects,
                &visible_ids,
                &schedule,
                &pr_infos,
                cycle,
                cadence_due,
                &github_in_flight,
                urgent_only,
            );
            for poll in &mut polls {
                poll.cached_repository = repository_cache.get(&poll.id).cloned();
            }

            log::trace!(
                "GitHub poll cycle={cycle}: {} projects, {} visible, {} due",
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
                github_in_flight.extend(polls.iter().map(|poll| poll.id.clone()));
                tokio::spawn(poll_github(
                    polls,
                    poll_generations,
                    poll_branches,
                    result_tx,
                ));
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
                Some(message) = github_result_rx.recv() => {
                    match message {
                        // Applied and published the moment it lands, so a badge
                        // never waits on the rest of its pass.
                        GithubPassMessage::Project(result) => {
                            for (id, repository) in &result.repositories {
                                if result.pr_infos.contains_key(id) && result.ci.contains_key(id)
                                    && github_result_is_current(&result, id, &head_generations, &last) {
                                    repository_cache.insert(id.clone(), repository.clone());
                                }
                            }
                            record_mission_observations(&result, &head_generations, &last,
                                &mut failed_mission_ci, &workspace, &workspace_tick);
                            apply_github_result(
                            *result,
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
                        GithubPassMessage::Finished(ids) => {
                            for id in &ids {
                                github_in_flight.remove(id);
                            }
                            // A force this pass was covering (a branch switch
                            // detected mid-pass) is now dispatchable — go round
                            // instead of idling until the next cadence tick.
                            if schedule.has_urgent() {
                                break;
                            }
                        }
                    }
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
            watch::channel(0u64).0,
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

    fn project_with_terminal(id: &str, terminal_id: &str) -> okena_state::ProjectData {
        okena_state::ProjectData {
            id: id.to_string(),
            name: "Project".to_string(),
            path: "/tmp".to_string(),
            layout: Some(okena_state::LayoutNode::Terminal {
                terminal_id: Some(terminal_id.to_string()),
                pending_agent_resume: None,
                mission_id: None,
                mission_excluded: false,
                minimized: false,
                detached: false,
                shell_type: Default::default(),
                zoom_level: 1.0,
                show_name_when_inactive: false,
            }),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: Default::default(),
            hooks: Default::default(),
            connection_id: None,
            service_terminals: HashMap::new(),
            agent_sessions: Default::default(),
            default_shell: None,
            hook_terminals: HashMap::new(),
            pinned: false,
            last_activity_at: None,
            is_creating: false,
            is_closing: false,
            creating_progress: None,
        }
    }

    fn workspace_data_with_projects(projects: &[(&str, &str)]) -> okena_state::WorkspaceData {
        let mut data = empty_workspace_data();
        for (id, terminal_id) in projects {
            data.projects.push(project_with_terminal(id, terminal_id));
            data.project_order.push(id.to_string());
        }
        data
    }

    /// Every project is hidden in the daemon's own window, so relevance comes
    /// only from what connected clients declare or subscribe to.
    fn workspace_with_hidden_projects(projects: &[(&str, &str)]) -> Workspace {
        let mut data = workspace_data_with_projects(projects);
        for (id, _) in projects {
            data.main_window.hidden_project_ids.insert(id.to_string());
        }
        Workspace::new(data)
    }

    /// Every project is visible in the daemon's own persisted window state —
    /// the legacy copy a desktop client never updates.
    fn workspace_with_daemon_visible_projects(projects: &[(&str, &str)]) -> Workspace {
        Workspace::new(workspace_data_with_projects(projects))
    }

    /// The regression this whole path exists for: a desktop client keeps its
    /// own visibility (client-side window ids, `window-layout.json`) and never
    /// writes to the daemon's copy, so a project hidden here can be the very
    /// one on screen. The client's declaration has to win.
    #[test]
    fn client_declared_projects_enter_the_gh_scope() {
        let workspace = workspace_with_hidden_projects(&[("on-screen", "t-on-screen")]);

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

    /// A desktop client's hides live in its own `window-layout.json`; the
    /// daemon's persisted window state is a stale copy that must not widen the
    /// responsive tier once any client has said what it renders.
    #[test]
    fn a_declared_viewport_supersedes_the_daemons_own_window_state() {
        let workspace =
            workspace_with_daemon_visible_projects(&[("stale", "t-stale"), ("shown", "t-shown")]);
        let declared = RwLock::new(HashMap::from([(
            1u64,
            HashSet::from(["shown".to_string()]),
        )]));

        assert_eq!(
            visible_project_ids(&workspace, &declared),
            HashSet::from(["shown".to_string()])
        );
        let no_subscriptions = RwLock::new(HashMap::new());
        assert_eq!(
            streaming_project_ids(&workspace, &no_subscriptions, &declared),
            HashSet::from(["shown".to_string()])
        );

        declared.write().unwrap().insert(1, HashSet::new());
        assert!(visible_project_ids(&workspace, &declared).is_empty());
    }

    /// A headless daemon serving clients that never declare a viewport (TUI,
    /// CLI, nobody at all) still has only its own window state to go by.
    #[test]
    fn the_daemons_own_window_state_counts_while_nobody_has_declared() {
        let workspace = workspace_with_daemon_visible_projects(&[("stale", "t-stale")]);
        let nothing_declared = RwLock::new(HashMap::new());

        assert!(visible_project_ids(&workspace, &nothing_declared).contains("stale"));
    }

    #[test]
    fn the_gh_fan_out_follows_the_declared_viewport() {
        let workspace =
            workspace_with_daemon_visible_projects(&[("stale", "t-stale"), ("shown", "t-shown")]);
        let declared = RwLock::new(HashMap::from([(
            1u64,
            HashSet::from(["shown".to_string()]),
        )]));
        let visible = visible_project_ids(&workspace, &declared);
        let projects: Vec<(String, String)> = workspace
            .projects()
            .iter()
            .map(|project| (project.id.clone(), project.path.clone()))
            .collect();

        let polls = select_github_polls(
            &projects,
            &visible,
            &GithubPollSchedule::default(),
            &HashMap::new(),
            1,
            true,
            &HashSet::new(),
            false,
        );

        let polled: Vec<&str> = polls.iter().map(|poll| poll.id.as_str()).collect();
        assert_eq!(polled, ["shown"]);
    }

    /// The desktop subscribes to every terminal it mirrors, so its
    /// subscriptions must not drag hidden projects onto the responsive tier.
    #[test]
    fn a_declared_viewport_silences_that_connections_subscriptions() {
        let workspace =
            workspace_with_hidden_projects(&[("shown", "t-shown"), ("background", "t-background")]);
        let subscribed = RwLock::new(HashMap::from([(
            1u64,
            HashSet::from(["t-shown".to_string(), "t-background".to_string()]),
        )]));
        let declared = RwLock::new(HashMap::from([(
            1u64,
            HashSet::from(["shown".to_string()]),
        )]));

        let relevant = streaming_project_ids(&workspace, &subscribed, &declared);
        assert_eq!(relevant, HashSet::from(["shown".to_string()]));
    }

    #[test]
    fn an_empty_declared_viewport_still_counts_as_declared() {
        let workspace = workspace_with_hidden_projects(&[("background", "t-background")]);
        let subscribed = RwLock::new(HashMap::from([(
            1u64,
            HashSet::from(["t-background".to_string()]),
        )]));
        let declared = RwLock::new(HashMap::from([(1u64, HashSet::new())]));

        assert!(streaming_project_ids(&workspace, &subscribed, &declared).is_empty());
    }

    /// TUI/CLI streaming clients have no viewport to declare; what they stream
    /// is what they show.
    #[test]
    fn subscriptions_promote_projects_for_undeclared_connections() {
        let workspace =
            workspace_with_hidden_projects(&[("shown", "t-shown"), ("background", "t-background")]);
        let subscribed = RwLock::new(HashMap::from([
            (
                1u64,
                HashSet::from(["t-shown".to_string(), "t-background".to_string()]),
            ),
            (2u64, HashSet::from(["t-background".to_string()])),
        ]));
        let declared = RwLock::new(HashMap::from([(
            1u64,
            HashSet::from(["shown".to_string()]),
        )]));

        let relevant = streaming_project_ids(&workspace, &subscribed, &declared);
        assert_eq!(
            relevant,
            HashSet::from(["shown".to_string(), "background".to_string()])
        );
    }

    #[test]
    fn a_project_entering_a_viewport_is_fetched_off_cadence() {
        let workspace = workspace_with_hidden_projects(&[("shown", "t-shown")]);
        let active = HashSet::from(["shown".to_string()]);
        let subscribed = RwLock::new(HashMap::from([(
            1u64,
            HashSet::from(["t-shown".to_string()]),
        )]));
        let declared = RwLock::new(HashMap::from([(1u64, HashSet::new())]));

        let known = streaming_project_ids(&workspace, &subscribed, &declared);
        assert!(known.is_empty());

        declared
            .write()
            .unwrap()
            .insert(1, HashSet::from(["shown".to_string()]));
        let relevant = streaming_project_ids(&workspace, &subscribed, &declared);
        let newly_relevant: HashSet<String> = relevant.difference(&known).cloned().collect();
        let empty = HashSet::new();
        assert_eq!(
            select_status_poll_ids(&active, &relevant, &empty, &newly_relevant, false, false),
            HashSet::from(["shown".to_string()])
        );
    }

    /// Build an `apply_github_result` fixture: one project, one CI outcome.
    fn github_result(generation: u64, branch: &str, ci: CiFetch) -> GithubPollResult {
        GithubPollResult {
            repositories: HashMap::new(),
            failed_ids: HashSet::new(),
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

    fn mission_workspace(projects: &[(&str, &str)]) -> (Arc<Mutex<Workspace>>, watch::Sender<u64>) {
        use okena_core::mission::{MissionCommand, MissionMember};
        let workspace = Arc::new(Mutex::new(Workspace::new(workspace_data_with_projects(
            projects,
        ))));
        let (tick, _) = watch::channel(0);
        let mut cx = crate::workspace_cx::DaemonWorkspaceCx::new(&tick, &None, &None);
        let mission = workspace
            .lock()
            .execute_mission(
                MissionCommand::Create {
                    title: "Outputs".into(),
                    goal: None,
                    home_project_id: None,
                    member: None,
                },
                &mut cx,
            )
            .unwrap();
        for (id, _) in projects {
            workspace
                .lock()
                .execute_mission(
                    MissionCommand::Attach {
                        mission_id: mission.clone(),
                        member: MissionMember::Repository {
                            project_id: (*id).into(),
                        },
                    },
                    &mut cx,
                )
                .unwrap();
        }
        (workspace, tick)
    }

    #[test]
    fn mission_repository_change_does_not_reuse_another_repositories_pr_number() {
        let path =
            std::env::temp_dir().join(format!("okena-mission-repository-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        for args in [
            vec!["init", "-q"],
            vec![
                "remote",
                "add",
                "origin",
                "https://github.com/owner/new.git",
            ],
        ] {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(&path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let outcome = poll_one_project(&ProjectPoll {
            id: "p1".into(),
            path: path.to_string_lossy().into_owned(),
            want_pr: false,
            want_ci: true,
            ci_skip_sha: Some("old-sha".into()),
            cached_pr_number: Some(7),
            cached_repository: Some(git::repository::GithubRepo {
                host: "github.com".into(),
                owner: "owner".into(),
                name: "old".into(),
            }),
        });
        assert_eq!(outcome.pr, Some(PrFetch::Failed));
        assert_eq!(
            outcome.ci,
            Some(CiFetch::Fetched {
                sha: None,
                summary: None
            }),
            "an unpushed new repository has no checks; the old PR must not trigger a network lookup"
        );
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn mission_observations_deduplicate_identity_and_retain_unavailable_sources() {
        let projects = [("a", "ta"), ("b", "tb"), ("c", "tc"), ("d", "td")];
        let (workspace, tick) = mission_workspace(&projects);
        let mut cx = crate::workspace_cx::DaemonWorkspaceCx::new(&tick, &None, &None);
        let mut result = GithubPollResult {
            repositories: HashMap::new(),
            failed_ids: HashSet::new(),
            head_generations: HashMap::new(),
            branches: HashMap::new(),
            pr_infos: HashMap::new(),
            ci: HashMap::new(),
            rate_limited: false,
            reached_github: true,
        };
        let mut statuses = HashMap::new();
        for (id, host, name) in [
            ("a", "github.com", "one"),
            ("b", "github.com", "ONE"),
            ("c", "github.com", "two"),
            ("d", "git.example.com", "one"),
        ] {
            result.repositories.insert(
                id.into(),
                git::repository::GithubRepo {
                    host: host.into(),
                    owner: "owner".into(),
                    name: name.into(),
                },
            );
            result.branches.insert(id.into(), Some("feature".into()));
            result.pr_infos.insert(
                id.into(),
                Some(git::PrInfo {
                    number: 7,
                    url: format!("https://{host}/owner/{name}/pull/7"),
                    state: okena_core::api::PrState::Open,
                    base: Some("main".into()),
                }),
            );
            result.ci.insert(id.into(), fetched("sha"));
            statuses.insert(
                id.into(),
                GitStatus {
                    branch: Some("feature".into()),
                    ..Default::default()
                },
            );
        }
        let mut failed_ci = HashSet::new();
        let observe = |result: &GithubPollResult,
                       failed_ci: &mut HashSet<String>,
                       statuses: &HashMap<String, GitStatus>| {
            record_mission_observations(
                result,
                &HashMap::new(),
                statuses,
                failed_ci,
                &workspace,
                &tick,
            );
        };
        observe(&result, &mut failed_ci, &statuses);
        {
            let ws = workspace.lock();
            let prs = &ws.data().missions[0].pull_requests;
            assert_eq!(prs.len(), 3);
            assert_eq!(
                prs.iter()
                    .find(|pr| pr.identity.host == "github.com"
                        && pr.identity.repository == "owner/one")
                    .unwrap()
                    .source_project_ids
                    .len(),
                2
            );
            assert!(prs.iter().all(|pr| pr.available));
        }
        result.failed_ids.insert("a".into());
        observe(&result, &mut failed_ci, &statuses);
        {
            let ws = workspace.lock();
            let shared = ws.data().missions[0]
                .pull_requests
                .iter()
                .find(|pr| {
                    pr.identity.host == "github.com" && pr.identity.repository == "owner/one"
                })
                .unwrap();
            assert!(shared.available);
            assert_eq!(shared.source_project_ids, vec!["b"]);
        }
        result.failed_ids.insert("b".into());
        result.ci.insert(
            "c".into(),
            CiFetch::Failed {
                sha: Some("sha".into()),
                summary: None,
            },
        );
        observe(&result, &mut failed_ci, &statuses);
        {
            let ws = workspace.lock();
            let prs = &ws.data().missions[0].pull_requests;
            assert_eq!(prs.len(), 3);
            assert!(
                prs.iter()
                    .filter(|pr| pr.identity.host == "github.com")
                    .all(|pr| !pr.available)
            );
            assert!(
                prs.iter()
                    .all(|pr| pr.info.state == okena_core::api::PrState::Open)
            );
        }
        result.ci.insert("c".into(), CiFetch::Unchanged);
        observe(&result, &mut failed_ci, &statuses);
        assert!(
            !workspace.lock().data().missions[0]
                .pull_requests
                .iter()
                .find(|pr| pr.identity.repository == "owner/two")
                .unwrap()
                .available
        );
        result.failed_ids.clear();
        result.ci.insert("c".into(), fetched("sha"));
        observe(&result, &mut failed_ci, &statuses);
        invalidate_mission_prs(&HashSet::from(["a".into(), "b".into()]), &workspace, &tick);
        statuses.get_mut("a").unwrap().branch = Some("new-branch".into());
        statuses.get_mut("b").unwrap().branch = Some("new-branch".into());
        observe(&result, &mut failed_ci, &statuses);
        assert!(!workspace.lock().data().missions[0].pull_requests.iter().find(|pr| pr.identity.host == "github.com" && pr.identity.repository == "owner/one").unwrap().available);
        workspace.lock().delete_project(
            &mut okena_workspace::focus::FocusManager::new(),
            "d",
            &Default::default(),
            &mut cx,
        );
        observe(&result, &mut failed_ci, &statuses);
        let ws = workspace.lock();
        let retained = ws.data().missions[0]
            .pull_requests
            .iter()
            .find(|pr| pr.identity.host == "git.example.com")
            .unwrap();
        assert!(!retained.available);
        assert!(retained.source_project_ids.is_empty());
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

    fn mission_pr_result(
        id: &str,
        state: git::PrState,
        ci: Option<git::CiCheckSummary>,
    ) -> GithubPollResult {
        GithubPollResult {
            repositories: HashMap::from([(
                id.into(),
                git::repository::GithubRepo {
                    host: "github.com".into(),
                    owner: "owner".into(),
                    name: "repo".into(),
                },
            )]),
            failed_ids: HashSet::new(),
            head_generations: HashMap::new(),
            branches: HashMap::from([(id.into(), Some("main".into()))]),
            pr_infos: HashMap::from([(
                id.into(),
                Some(git::PrInfo {
                    number: 7,
                    url: "https://github.com/owner/repo/pull/7".into(),
                    state,
                    base: Some("main".into()),
                }),
            )]),
            ci: HashMap::from([(
                id.into(),
                CiFetch::Fetched {
                    sha: Some("sha".into()),
                    summary: ci,
                },
            )]),
            rate_limited: false,
            reached_github: true,
        }
    }

    #[test]
    fn mission_interleaved_sources_merge_only_fetched_components() {
        let (workspace, tick) = mission_workspace(&[("a", "ta"), ("b", "tb")]);
        let pending = git::CiCheckSummary {
            status: git::CiStatus::Pending,
            passed: 0,
            failed: 0,
            pending: 1,
            total: 1,
            checks: Vec::new(),
        };
        let success = git::CiCheckSummary {
            status: git::CiStatus::Success,
            passed: 1,
            failed: 0,
            pending: 0,
            total: 1,
            checks: Vec::new(),
        };
        let initial = mission_pr_result("a", git::PrState::Open, Some(pending.clone()));
        let statuses = HashMap::from([
            (
                "a".into(),
                GitStatus {
                    branch: Some("main".into()),
                    pr_info: initial.pr_infos["a"].clone(),
                    ci_checks: Some(pending),
                    ..Default::default()
                },
            ),
            (
                "b".into(),
                GitStatus {
                    branch: Some("main".into()),
                    pr_info: initial.pr_infos["a"].clone(),
                    ci_checks: Some(success.clone()),
                    ..Default::default()
                },
            ),
        ]);
        let mut failed_ci = HashSet::new();
        let mut observe = |result: &GithubPollResult| {
            record_mission_observations(
                result,
                &HashMap::new(),
                &statuses,
                &mut failed_ci,
                &workspace,
                &tick,
            )
        };
        observe(&initial);
        observe(&mission_pr_result(
            "b",
            git::PrState::Merged,
            Some(success.clone()),
        ));
        let mut ci_only = mission_pr_result("a", git::PrState::Open, None);
        ci_only.pr_infos.clear();
        observe(&ci_only);
        {
            let ws = workspace.lock();
            let pr = &ws.data().missions[0].pull_requests[0];
            assert_eq!(
                pr.info.state,
                git::PrState::Merged,
                "cached open must not overwrite freshly merged"
            );
            assert_eq!(
                pr.ci, None,
                "freshly verified absence of checks must clear the old summary"
            );
        }
        let mut pr_only = mission_pr_result("b", git::PrState::Closed, Some(success));
        pr_only.ci.clear();
        observe(&pr_only);
        let observed = workspace.lock().data().missions[0].pull_requests[0].clone();
        assert_eq!(observed.info.state, git::PrState::Closed);
        assert_eq!(
            observed.ci, None,
            "cached successful checks must not restore cleared checks"
        );
        ci_only.ci.insert("a".into(), CiFetch::Unchanged);
        let previous_tick = *tick.borrow();
        observe(&ci_only);
        assert_eq!(
            workspace.lock().data().missions[0].pull_requests[0],
            observed
        );
        assert_eq!(
            *tick.borrow(),
            previous_tick,
            "cache hits are not new observations"
        );
        let mut partial = mission_pr_result("a", git::PrState::Merged, None);
        partial.ci.insert(
            "a".into(),
            CiFetch::Failed {
                sha: Some("sha".into()),
                summary: None,
            },
        );
        partial.failed_ids.insert("a".into());
        observe(&partial);
        let ws = workspace.lock();
        let pr = &ws.data().missions[0].pull_requests[0];
        assert_eq!(
            pr.info.state,
            git::PrState::Merged,
            "a CI failure must not discard a freshly observed merge"
        );
        assert!(!pr.source_project_ids.contains(&"a".into()));
        drop(ws);
        let mut new_identity = mission_pr_result("b", git::PrState::Open, None);
        new_identity.ci.clear();
        let info = new_identity
            .pr_infos
            .get_mut("b")
            .unwrap()
            .as_mut()
            .unwrap();
        info.number = 8;
        info.url = "https://github.com/owner/repo/pull/8".into();
        observe(&new_identity);
        let ws = workspace.lock();
        let pr = ws.data().missions[0]
            .pull_requests
            .iter()
            .find(|pr| pr.identity.number == 8)
            .unwrap();
        assert!(pr.available);
        assert_eq!(
            pr.ci, None,
            "initializing a new PR must not import the old identity's checks"
        );
        assert_eq!(
            ws.data().missions[0]
                .pull_requests
                .iter()
                .find(|pr| pr.identity.number == 7)
                .unwrap()
                .info
                .state,
            git::PrState::Merged
        );
    }

    #[test]
    fn mission_retry_fetches_after_failure_and_rate_limit() {
        // Isolate the process-wide HTTP mock and token environment from parallel tests.
        const CHILD: &str = "OKENA_MISSION_POLL_TEST_CHILD";
        const VERIFIED: &str = "mission poll recovery verified";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "git_poll::tests::mission_retry_fetches_after_failure_and_rate_limit",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env("GH_TOKEN", "test-token")
                .env("GH_HOST", "github.com")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains(VERIFIED));
            return;
        }
        use okena_transport::http::{HttpResponse, testing};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let failure = Arc::new(AtomicUsize::new(0));
        let ci_calls = Arc::new(AtomicUsize::new(0));
        let pr_number = Arc::new(AtomicUsize::new(7));
        let mock_failure = failure.clone();
        let mock_calls = ci_calls.clone();
        let mock_number = pr_number.clone();
        let _mock = testing::mock(move |request| {
            assert_eq!(request.url(), "https://api.github.com/graphql");
            let query = request.json_body().unwrap()["query"].as_str().unwrap();
            let (status, body) = if query.contains("PullRequestList") {
                if mock_failure.load(Ordering::SeqCst) == 1 {
                    (429, serde_json::json!({"message":"rate limited"}))
                } else {
                    let number = mock_number.load(Ordering::SeqCst);
                    (
                        200,
                        serde_json::json!({"data":{"repository":{"pullRequests":{"nodes":[{
                            "number":number,"url":format!("https://github.com/owner/repo/pull/{number}"),"state":"OPEN","isDraft":false
                        }]}}}}),
                    )
                }
            } else {
                assert!(query.contains("PullRequestStatusChecks"));
                mock_calls.fetch_add(1, Ordering::SeqCst);
                if mock_failure.load(Ordering::SeqCst) == 2 {
                    (500, serde_json::json!({"message":"unavailable"}))
                } else {
                    (
                        200,
                        serde_json::json!({"data":{"repository":{"pullRequest":{"commits":{"nodes":[{"commit":{"statusCheckRollup":null}}]}}}}}),
                    )
                }
            };
            Ok(HttpResponse::new(
                status,
                Vec::new(),
                serde_json::to_vec(&body).unwrap(),
            ))
        });
        let path =
            std::env::temp_dir().join(format!("okena-mission-retry-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "fixture",
            ],
            vec![
                "remote",
                "add",
                "origin",
                "https://github.com/owner/repo.git",
            ],
            vec!["config", "branch.main.remote", "origin"],
            vec!["config", "branch.main.merge", "refs/heads/main"],
            vec!["update-ref", "refs/remotes/origin/main", "HEAD"],
        ] {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(&path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            for mode in [1, 2] {
                let (workspace, tick) = mission_workspace(&[("p1", "t1")]);
                let mut apply = ApplyHarness::new();
                let mut failed_ci = HashSet::new();
                let mut cached_repository = None;
                let projects = vec![("p1".to_string(), path.to_string_lossy().into_owned())];
                let visible = HashSet::from(["p1".to_string()]);
                let failure_cycle = if mode == 1 { 13 } else { 121 };
                let retry_cycle = failure_cycle + 12;
                for (cycle, failing, number) in [(1, false, 7), (failure_cycle, true, 7), (retry_cycle, false, 7), (retry_cycle + 12, false, 8)] {
                    failure.store(if failing { mode } else { 0 }, Ordering::SeqCst);
                    pr_number.store(number, Ordering::SeqCst);
                    let mut polls = select_github_polls(&projects, &visible, &apply.schedule, &apply.pr_infos, cycle, true, &HashSet::new(), false);
                    assert_eq!(polls.len(), 1);
                    if cycle == retry_cycle {
                        assert!(!apply.schedule.is_rate_limited(cycle));
                        assert_eq!(polls[0].ci_skip_sha, None, "failure must require a real retry even though the upstream commit is unchanged");
                    }
                    polls[0].cached_repository = cached_repository.clone();
                    if polls[0].want_pr { apply.schedule.pr_dispatched("p1", cycle); }
                    if polls[0].want_ci { apply.schedule.ci_dispatched("p1", cycle); }
                    let before_calls = ci_calls.load(Ordering::SeqCst);
                    let (tx, mut rx) = mpsc::unbounded_channel();
                    poll_github(polls, HashMap::new(), HashMap::from([("p1".into(), Some("main".into()))]), tx).await;
                    let GithubPassMessage::Project(result) = rx.recv().await.unwrap() else { panic!("missing project result") };
                    cached_repository = result.repositories.get("p1").cloned();
                    record_mission_observations(&result, &HashMap::new(), &apply.last, &mut failed_ci, &workspace, &tick);
                    apply.apply(*result, cycle, &HashMap::new());
                    assert_eq!(workspace.lock().data().missions[0].pull_requests.iter().find(|pr| pr.identity.number == u32::try_from(number).unwrap()).unwrap().available, !failing);
                    if cycle == 1 { assert!(apply.schedule.ci_skip_sha("p1", 2).is_some()); }
                    if failing && mode == 1 { assert!(apply.schedule.is_rate_limited(cycle + 1)); }
                    if failing {
                        assert!(!apply.schedule.has_urgent());
                        assert!(!apply.schedule.ci_due("p1", cycle + 1, true), "failure must preserve the dispatched retry cadence");
                    }
                    if cycle == retry_cycle {
                        assert!(ci_calls.load(Ordering::SeqCst) > before_calls, "recovery must actually reach the HTTP mock, not use Unchanged");
                    }
                    if number == 8 {
                        assert!(ci_calls.load(Ordering::SeqCst) > before_calls, "a new PR identity must not reuse settled checks solely because its SHA matches");
                    }
                }
            }
        });
        std::fs::remove_dir_all(path).unwrap();
        println!("{VERIFIED}");
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

        let polls = select_github_polls(
            &projects(),
            &visible,
            &schedule,
            &HashMap::new(),
            1,
            true,
            &HashSet::new(),
            false,
        );

        assert_eq!(polls.len(), 1);
        assert_eq!(polls[0].id, "visible");
    }

    #[test]
    fn an_explicit_request_reaches_a_hidden_project() {
        let visible = HashSet::new();
        let mut schedule = GithubPollSchedule::default();
        schedule.force("hidden");

        // Off-cadence too: an explicit action shouldn't wait for the next tick.
        let polls = select_github_polls(
            &projects(),
            &visible,
            &schedule,
            &HashMap::new(),
            4,
            false,
            &HashSet::new(),
            false,
        );

        assert_eq!(polls.len(), 1);
        assert_eq!(polls[0].id, "hidden");
    }

    #[test]
    fn a_project_a_running_pass_holds_is_not_polled_twice() {
        let visible = HashSet::from(["visible".to_string()]);
        let schedule = GithubPollSchedule::default();
        let in_flight = HashSet::from(["visible".to_string()]);

        let polls = select_github_polls(
            &projects(),
            &visible,
            &schedule,
            &HashMap::new(),
            1,
            true,
            &in_flight,
            false,
        );

        assert!(polls.is_empty());
    }

    #[test]
    fn a_forced_project_starts_its_own_pass_while_another_runs() {
        // "visible" is mid-pass, so this cycle is urgent-only: the ordinary due
        // project waits, the branch-switched one goes out now.
        let visible = HashSet::from(["visible".to_string(), "hidden".to_string()]);
        let mut schedule = GithubPollSchedule::default();
        schedule.force("hidden");
        let in_flight = HashSet::from(["visible".to_string()]);

        let polls = select_github_polls(
            &projects(),
            &visible,
            &schedule,
            &HashMap::new(),
            1,
            true,
            &in_flight,
            true,
        );

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
            select_github_polls(
                &projects(),
                &visible,
                &schedule,
                &HashMap::new(),
                5,
                true,
                &HashSet::new(),
                false,
            )
            .is_empty()
        );

        // …and when it is, the cached commit rides along.
        let polls = select_github_polls(
            &projects(),
            &visible,
            &schedule,
            &HashMap::new(),
            13,
            true,
            &HashSet::new(),
            false,
        );
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

        let polls = select_github_polls(
            &projects(),
            &visible,
            &schedule,
            &HashMap::new(),
            4,
            true,
            &HashSet::new(),
            false,
        );

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
