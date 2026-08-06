use std::collections::{HashMap, HashSet};

use crate::api::ActionRequest;

/// One poll cycle is one tick of the 5s git-status cadence, in both the daemon
/// poller and the GUI watcher. Every constant below is expressed in cycles.
pub const POLL_CYCLE_SECS: u64 = 5;
/// `gh pr list` cadence per project (~60s).
pub const PR_POLL_EVERY_N_CYCLES: u64 = 12;
/// CI cadence for a project whose own checks are still running (~15s).
pub const CI_PENDING_POLL_EVERY_N_CYCLES: u64 = 3;
/// CI cadence for a project whose checks have settled (~60s).
pub const CI_SETTLED_POLL_EVERY_N_CYCLES: u64 = 12;
/// A settled result is revalidated on the wire this often (~10min) even when
/// the commit hasn't moved, so a re-run of an old commit's checks is noticed.
pub const CI_REVALIDATE_EVERY_N_CYCLES: u64 = 120;
/// First backoff after GitHub reports the API rate limit as exhausted (~60s).
pub const RATE_LIMIT_BACKOFF_MIN_CYCLES: u64 = 12;
/// Ceiling for the doubling backoff (~30min — GitHub's window is an hour).
pub const RATE_LIMIT_BACKOFF_MAX_CYCLES: u64 = 360;

/// Per-project scheduling for the `gh` PR/CI fan-out, shared by the daemon
/// poller and the GUI watcher.
///
/// It answers three questions the poll loops used to answer with global state:
/// *is this project due?* (per project, so one repo with running CI cannot pull
/// every other repo onto the fast cadence), *can the fetch be skipped
/// outright?* (the cached result still describes the current upstream commit),
/// and *should we be talking to GitHub at all?* (a rate-limit gate).
#[derive(Debug, Default)]
pub struct GithubPollSchedule {
    projects: HashMap<String, ProjectSchedule>,
    /// Cycle at which `gh` may run again after a rate-limit refusal.
    gate_until_cycle: Option<u64>,
    /// Current backoff length; doubles per consecutive refusal.
    backoff_cycles: u64,
}

#[derive(Debug, Clone)]
struct ProjectSchedule {
    next_pr_cycle: u64,
    next_ci_cycle: u64,
    /// Upstream commit the cached CI summary describes. Only set while that
    /// summary is settled — a pending one has to be re-fetched to make progress.
    settled_ci_sha: Option<String>,
    /// Cycle at which a settled result is re-fetched even if the commit is
    /// unchanged.
    ci_revalidate_cycle: u64,
    /// Fetch on the next pass without waiting for the cadence tick.
    urgent: bool,
}

impl Default for ProjectSchedule {
    fn default() -> Self {
        // Cycle 0 is startup — the app is already busy spawning terminals and
        // the gix status renders without `gh`. First fetch lands on cycle 1.
        Self {
            next_pr_cycle: 1,
            next_ci_cycle: 1,
            settled_ci_sha: None,
            ci_revalidate_cycle: 0,
            urgent: false,
        }
    }
}

impl GithubPollSchedule {
    /// Drop bookkeeping for projects that no longer exist.
    pub fn retain(&mut self, active: &HashSet<String>) {
        self.projects.retain(|id, _| active.contains(id));
    }

    /// Cached PR/CI is invalid (branch changed): fetch as soon as possible.
    pub fn force(&mut self, id: &str) {
        let entry = self.entry(id);
        entry.urgent = true;
        entry.next_pr_cycle = 0;
        entry.next_ci_cycle = 0;
        entry.settled_ci_sha = None;
        entry.ci_revalidate_cycle = 0;
    }

    /// A project became relevant. Only worth an off-cadence fetch when the
    /// caller holds no PR/CI result for it — otherwise the cached badge is
    /// still good and the cadence will refresh it.
    ///
    /// `has_cached_result` is the caller's PR/CI cache, deliberately not this
    /// schedule's own bookkeeping: a project can carry a schedule entry with no
    /// result behind it (its fetch was discarded, or it dropped out of the poll
    /// scope before one landed). Keying off the entry made this a permanent
    /// no-op for such a project, so nothing could ever unstick its badge.
    pub fn force_if_unfetched(&mut self, id: &str, has_cached_result: bool) {
        if !has_cached_result {
            self.force(id);
        }
    }

    /// Whether any project wants an off-cadence fetch right now.
    pub fn has_urgent(&self) -> bool {
        self.projects.values().any(|entry| entry.urgent)
    }

    pub fn pr_due(&self, id: &str, cycle: u64, cadence_due: bool) -> bool {
        match self.projects.get(id) {
            Some(entry) => entry.urgent || (cadence_due && cycle >= entry.next_pr_cycle),
            None => cadence_due && cycle >= ProjectSchedule::default().next_pr_cycle,
        }
    }

    pub fn ci_due(&self, id: &str, cycle: u64, cadence_due: bool) -> bool {
        match self.projects.get(id) {
            Some(entry) => entry.urgent || (cadence_due && cycle >= entry.next_ci_cycle),
            None => cadence_due && cycle >= ProjectSchedule::default().next_ci_cycle,
        }
    }

    /// The commit whose CI result the caller already holds, so the fetch can be
    /// skipped when the upstream commit still matches. `None` forces a real
    /// request — nothing cached, the last result was pending, or the
    /// revalidation deadline has passed.
    pub fn ci_skip_sha(&self, id: &str, cycle: u64) -> Option<String> {
        let entry = self.projects.get(id)?;
        if cycle >= entry.ci_revalidate_cycle {
            return None;
        }
        entry.settled_ci_sha.clone()
    }

    /// A PR fetch is on its way out. Pushes the next due cycle forward so an
    /// in-flight pass is never dispatched twice; the result refines it.
    pub fn pr_dispatched(&mut self, id: &str, cycle: u64) {
        let entry = self.entry(id);
        entry.next_pr_cycle = cycle + PR_POLL_EVERY_N_CYCLES;
        entry.urgent = false;
    }

    pub fn ci_dispatched(&mut self, id: &str, cycle: u64) {
        let entry = self.entry(id);
        entry.next_ci_cycle = cycle + CI_SETTLED_POLL_EVERY_N_CYCLES;
        entry.urgent = false;
    }

    pub fn record_pr(&mut self, id: &str, cycle: u64) {
        let entry = self.entry(id);
        entry.next_pr_cycle = cycle + PR_POLL_EVERY_N_CYCLES;
        entry.urgent = false;
    }

    /// A CI fetch came back. `pending` drives this project's own cadence, and a
    /// settled result caches its commit so the next poll can skip the wire.
    pub fn record_ci(&mut self, id: &str, cycle: u64, pending: bool, sha: Option<String>) {
        let entry = self.entry(id);
        entry.next_ci_cycle = cycle
            + if pending {
                CI_PENDING_POLL_EVERY_N_CYCLES
            } else {
                CI_SETTLED_POLL_EVERY_N_CYCLES
            };
        entry.urgent = false;
        entry.settled_ci_sha = if pending { None } else { sha };
        entry.ci_revalidate_cycle = cycle + CI_REVALIDATE_EVERY_N_CYCLES;
    }

    /// The upstream commit was unchanged, so no request went out. Keeps the
    /// cached commit and revalidation deadline, just re-arms the cadence.
    pub fn record_ci_unchanged(&mut self, id: &str, cycle: u64) {
        let entry = self.entry(id);
        entry.next_ci_cycle = cycle + CI_SETTLED_POLL_EVERY_N_CYCLES;
        entry.urgent = false;
    }

    /// GitHub refused because the API rate limit is exhausted. Parks every
    /// project's `gh` traffic, doubling the wait per consecutive refusal.
    pub fn note_rate_limited(&mut self, cycle: u64) {
        self.backoff_cycles = if self.backoff_cycles == 0 {
            RATE_LIMIT_BACKOFF_MIN_CYCLES
        } else {
            (self.backoff_cycles * 2).min(RATE_LIMIT_BACKOFF_MAX_CYCLES)
        };
        self.gate_until_cycle = Some(cycle + self.backoff_cycles);
    }

    /// A request went through — clear the gate and the accumulated backoff.
    pub fn note_request_succeeded(&mut self) {
        self.backoff_cycles = 0;
        self.gate_until_cycle = None;
    }

    /// Whether `gh` is currently parked by the rate-limit gate.
    pub fn is_rate_limited(&self, cycle: u64) -> bool {
        self.gate_until_cycle.is_some_and(|until| cycle < until)
    }

    /// Length of the current rate-limit backoff, in cycles. For logging.
    pub fn rate_limit_backoff_cycles(&self) -> u64 {
        self.backoff_cycles
    }

    /// Whether this project asked for an off-cadence fetch that hasn't been
    /// dispatched yet.
    pub fn is_urgent(&self, id: &str) -> bool {
        self.projects.get(id).is_some_and(|entry| entry.urgent)
    }

    fn entry(&mut self, id: &str) -> &mut ProjectSchedule {
        self.projects.entry(id.to_string()).or_default()
    }
}

/// External wake-up request for git / GitHub status polling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitPollTrigger {
    pub project_id: Option<String>,
    pub poll_github: bool,
    pub invalidate_github: bool,
}

impl GitPollTrigger {
    pub fn head_change(project_id: String) -> Self {
        Self {
            project_id: Some(project_id),
            poll_github: false,
            invalidate_github: false,
        }
    }

    pub fn branch_change(project_id: String) -> Self {
        Self {
            project_id: Some(project_id),
            poll_github: true,
            invalidate_github: true,
        }
    }

    pub fn project_visible(project_id: String) -> Self {
        Self {
            project_id: Some(project_id),
            poll_github: true,
            invalidate_github: false,
        }
    }

    pub fn visibility_changed() -> Self {
        Self {
            project_id: None,
            poll_github: false,
            invalidate_github: false,
        }
    }
}

/// Map an action to the git-poll wake-up it should trigger (if any).
///
/// Shared by both daemon entry points so the same actions always drive the same
/// immediate refresh. Both `okena-daemon` and `okena --headless` run
/// `DaemonCore` and call this after a successful action.
///
/// - Branch checkout invalidates the cached PR/CI (they belong to the old
///   branch) and re-polls `gh`.
/// - Requesting git status, or showing a project in the overview, marks the
///   project visible so `gh` is fetched when it has no cached PR/CI yet.
pub fn git_poll_trigger_for_action(action: &ActionRequest) -> Option<GitPollTrigger> {
    match action {
        ActionRequest::GitCheckoutLocalBranch { project_id, .. }
        | ActionRequest::GitCheckoutRemoteBranch { project_id, .. }
        | ActionRequest::GitCreateAndCheckoutBranch { project_id, .. } => {
            Some(GitPollTrigger::branch_change(project_id.clone()))
        }
        ActionRequest::GitStatus { project_id }
        | ActionRequest::SetProjectShowInOverview {
            project_id,
            show: true,
            ..
        } => Some(GitPollTrigger::project_visible(project_id.clone())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_change_only_wakes_local_git_poll() {
        let trigger = GitPollTrigger::head_change("p1".to_string());
        assert_eq!(trigger.project_id.as_deref(), Some("p1"));
        assert!(!trigger.poll_github);
        assert!(!trigger.invalidate_github);
    }

    #[test]
    fn branch_checkout_creates_invalidating_trigger() {
        let trigger = git_poll_trigger_for_action(&ActionRequest::GitCheckoutLocalBranch {
            project_id: "p1".to_string(),
            branch: "feature".to_string(),
        })
        .expect("branch checkout creates trigger");
        assert_eq!(trigger.project_id.as_deref(), Some("p1"));
        assert!(trigger.poll_github);
        assert!(trigger.invalidate_github);
    }

    #[test]
    fn visible_project_actions_create_non_invalidating_trigger() {
        let trigger = git_poll_trigger_for_action(&ActionRequest::SetProjectShowInOverview {
            project_id: "p1".to_string(),
            show: true,
            window: None,
        })
        .expect("showing a project creates trigger");
        assert_eq!(trigger.project_id.as_deref(), Some("p1"));
        assert!(trigger.poll_github);
        assert!(!trigger.invalidate_github);

        let trigger = git_poll_trigger_for_action(&ActionRequest::GitStatus {
            project_id: "p1".to_string(),
        })
        .expect("requesting git status creates trigger");
        assert_eq!(trigger.project_id.as_deref(), Some("p1"));
        assert!(trigger.poll_github);
        assert!(!trigger.invalidate_github);

        assert!(
            git_poll_trigger_for_action(&ActionRequest::SetProjectShowInOverview {
                project_id: "p1".to_string(),
                show: false,
                window: None,
            })
            .is_none()
        );
    }

    #[test]
    fn first_fetch_lands_on_cycle_one_not_at_startup() {
        let schedule = GithubPollSchedule::default();
        assert!(!schedule.pr_due("p1", 0, true));
        assert!(!schedule.ci_due("p1", 0, true));
        assert!(schedule.pr_due("p1", 1, true));
        assert!(schedule.ci_due("p1", 1, true));
    }

    #[test]
    fn nothing_is_due_between_cadence_ticks() {
        let schedule = GithubPollSchedule::default();
        assert!(!schedule.pr_due("p1", 5, false));
        assert!(!schedule.ci_due("p1", 5, false));
    }

    #[test]
    fn forced_project_is_due_off_cadence() {
        let mut schedule = GithubPollSchedule::default();
        schedule.force("p1");
        assert!(schedule.has_urgent());
        assert!(schedule.pr_due("p1", 3, false));
        assert!(schedule.ci_due("p1", 3, false));
        schedule.ci_dispatched("p1", 3);
        schedule.pr_dispatched("p1", 3);
        assert!(!schedule.has_urgent());
    }

    #[test]
    fn pending_ci_stays_local_to_its_own_project() {
        let mut schedule = GithubPollSchedule::default();
        schedule.record_ci("busy", 10, true, None);
        schedule.record_ci("idle", 10, false, Some("abc".into()));

        // Busy repo is back 3 cycles later, idle one only after 12.
        assert!(schedule.ci_due("busy", 13, true));
        assert!(!schedule.ci_due("idle", 13, true));
        assert!(schedule.ci_due("idle", 22, true));
    }

    #[test]
    fn settled_result_skips_the_wire_while_the_commit_holds() {
        let mut schedule = GithubPollSchedule::default();
        schedule.record_ci("p1", 10, false, Some("abc".into()));
        assert_eq!(schedule.ci_skip_sha("p1", 22).as_deref(), Some("abc"));

        // …but is revalidated once the staleness deadline passes.
        assert_eq!(
            schedule.ci_skip_sha("p1", 10 + CI_REVALIDATE_EVERY_N_CYCLES),
            None
        );
    }

    #[test]
    fn pending_result_is_never_cacheable() {
        let mut schedule = GithubPollSchedule::default();
        schedule.record_ci("p1", 10, true, Some("abc".into()));
        assert_eq!(schedule.ci_skip_sha("p1", 12), None);
    }

    #[test]
    fn unchanged_result_keeps_the_cached_commit() {
        let mut schedule = GithubPollSchedule::default();
        schedule.record_ci("p1", 10, false, Some("abc".into()));
        schedule.record_ci_unchanged("p1", 22);
        assert_eq!(schedule.ci_skip_sha("p1", 23).as_deref(), Some("abc"));
        assert!(schedule.ci_due("p1", 34, true));
    }

    #[test]
    fn branch_change_drops_the_cached_commit() {
        let mut schedule = GithubPollSchedule::default();
        schedule.record_ci("p1", 10, false, Some("abc".into()));
        schedule.force("p1");
        assert_eq!(schedule.ci_skip_sha("p1", 11), None);
    }

    #[test]
    fn relevance_only_forces_projects_with_no_cached_result() {
        let mut schedule = GithubPollSchedule::default();
        schedule.force_if_unfetched("fresh", false);
        assert!(schedule.ci_due("fresh", 0, false));

        schedule.record_ci("known", 10, false, Some("abc".into()));
        schedule.force_if_unfetched("known", true);
        assert!(!schedule.ci_due("known", 11, true));
    }

    #[test]
    fn a_schedule_entry_without_a_result_still_forces() {
        // A project can hold a schedule entry with nothing behind it — its
        // fetch was discarded, or it left the poll scope before one landed.
        // Judging "unfetched" by the entry made this a permanent no-op, which
        // is how a badge could stay stale for as long as the daemon ran.
        let mut schedule = GithubPollSchedule::default();
        schedule.pr_dispatched("stalled", 0);
        assert!(!schedule.pr_due("stalled", 5, true));

        schedule.force_if_unfetched("stalled", false);
        assert!(schedule.pr_due("stalled", 5, false));
    }

    #[test]
    fn rate_limit_gate_backs_off_and_clears() {
        let mut schedule = GithubPollSchedule::default();
        schedule.note_rate_limited(0);
        assert!(schedule.is_rate_limited(RATE_LIMIT_BACKOFF_MIN_CYCLES - 1));
        assert!(!schedule.is_rate_limited(RATE_LIMIT_BACKOFF_MIN_CYCLES));

        // Consecutive refusals double the wait, up to the ceiling.
        schedule.note_rate_limited(100);
        assert!(schedule.is_rate_limited(100 + RATE_LIMIT_BACKOFF_MIN_CYCLES));
        for cycle in 0..20 {
            schedule.note_rate_limited(cycle);
        }
        assert!(!schedule.is_rate_limited(19 + RATE_LIMIT_BACKOFF_MAX_CYCLES + 1));

        schedule.note_request_succeeded();
        assert!(!schedule.is_rate_limited(0));
    }

    #[test]
    fn retain_drops_removed_projects() {
        let mut schedule = GithubPollSchedule::default();
        schedule.record_ci("gone", 10, false, Some("abc".into()));
        schedule.record_ci("kept", 10, false, Some("def".into()));
        schedule.retain(&HashSet::from(["kept".to_string()]));
        assert_eq!(schedule.ci_skip_sha("gone", 11), None);
        assert_eq!(schedule.ci_skip_sha("kept", 11).as_deref(), Some("def"));
    }
}
