//! Which projects get a full git status walk in a poll cycle.
//!
//! A walk stats the whole worktree, so its cost grows with the number of
//! projects, not with what changed. The schedule spends it where a status can
//! actually have moved:
//!
//! - **Visible and active** (a change signal within [`ACTIVE_WINDOW_CYCLES`]):
//!   every cycle, as before.
//! - **Visible and idle**: every [`IDLE_WALK_EVERY_CYCLES`], which still picks
//!   up edits made outside Okena's terminals.
//! - **Hidden**: only once a change signal arrived since its last walk, no more
//!   often than [`IDLE_WALK_EVERY_CYCLES`]; otherwise every
//!   [`HIDDEN_FALLBACK_CYCLES`].
//!
//! Change signals are terminal output in the project, a rewritten index and a
//! moved HEAD. Forced projects (explicit actions, a HEAD change, a project
//! entering a viewport) bypass the schedule entirely.

use std::collections::{HashMap, HashSet};

/// A visible project stays on the per-cycle cadence this long after its last
/// change signal (12 × 5 s = 60 s).
pub(crate) const ACTIVE_WINDOW_CYCLES: u64 = 12;
/// Idle visible projects, and hidden ones with a pending change signal, are
/// walked at most this often (6 × 5 s = 30 s).
pub(crate) const IDLE_WALK_EVERY_CYCLES: u64 = 6;
/// A hidden project with no change signal is still walked this often, for
/// edits nothing reported (60 × 5 s = 5 min).
pub(crate) const HIDDEN_FALLBACK_CYCLES: u64 = 60;

#[derive(Debug, Default)]
struct ProjectWalkState {
    last_walk_cycle: Option<u64>,
    last_signal_cycle: Option<u64>,
    /// A change signal arrived after the last walk started.
    signal_pending: bool,
}

#[derive(Debug, Default)]
pub(crate) struct LocalWalkSchedule {
    projects: HashMap<String, ProjectWalkState>,
}

impl LocalWalkSchedule {
    /// Record a change signal for `id`. Call it before [`Self::select`] of the
    /// same cycle so the walk it causes clears it.
    pub(crate) fn note_signal(&mut self, id: &str, cycle: u64) {
        let state = self.projects.entry(id.to_string()).or_default();
        state.last_signal_cycle = Some(cycle);
        state.signal_pending = true;
    }

    /// Record that `id` was walked (successfully or not) in `cycle`.
    pub(crate) fn note_walked(&mut self, id: &str, cycle: u64) {
        let state = self.projects.entry(id.to_string()).or_default();
        state.last_walk_cycle = Some(cycle);
        state.signal_pending = false;
    }

    pub(crate) fn retain(&mut self, active_ids: &HashSet<String>) {
        self.projects.retain(|id, _| active_ids.contains(id));
    }

    /// The projects to walk now. `forced_ids` are walked whatever their
    /// schedule; the cadence rules apply only when `cadence_due`.
    pub(crate) fn select(
        &self,
        active_ids: &HashSet<String>,
        visible_ids: &HashSet<String>,
        forced_ids: &HashSet<String>,
        cycle: u64,
        cadence_due: bool,
    ) -> HashSet<String> {
        active_ids
            .iter()
            .filter(|id| {
                forced_ids.contains(*id)
                    || (cadence_due && self.cadence_due(id, visible_ids.contains(*id), cycle))
            })
            .cloned()
            .collect()
    }

    fn cadence_due(&self, id: &str, visible: bool, cycle: u64) -> bool {
        let Some(state) = self.projects.get(id) else {
            return true;
        };
        let Some(last_walk) = state.last_walk_cycle else {
            return true;
        };
        let since_walk = cycle.saturating_sub(last_walk);
        if visible {
            let active = state
                .last_signal_cycle
                .is_some_and(|signal| cycle.saturating_sub(signal) < ACTIVE_WINDOW_CYCLES);
            active || since_walk >= IDLE_WALK_EVERY_CYCLES
        } else {
            (state.signal_pending && since_walk >= IDLE_WALK_EVERY_CYCLES)
                || since_walk >= HIDDEN_FALLBACK_CYCLES
        }
    }
}

/// Per-project fingerprint of terminal output, compared cycle to cycle.
///
/// The fingerprint is the sum of the project's terminals' content generations,
/// so any output (or a terminal opening or closing) changes it.
#[derive(Debug, Default)]
pub(crate) struct OutputFingerprints {
    previous: HashMap<String, u64>,
}

impl OutputFingerprints {
    /// Store this cycle's fingerprints and return the projects whose output
    /// moved since the previous call. A project seen for the first time has no
    /// baseline and is not reported.
    pub(crate) fn changed(&mut self, current: HashMap<String, u64>) -> Vec<String> {
        let changed = current
            .iter()
            .filter(|(id, fingerprint)| {
                self.previous
                    .get(*id)
                    .is_some_and(|previous| previous != *fingerprint)
            })
            .map(|(id, _)| id.clone())
            .collect();
        self.previous = current;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(values: &[&str]) -> HashSet<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    /// Walk whatever is due in each cycle of `cycles`, returning the cycles in
    /// which `id` was walked.
    fn walks_of(
        schedule: &mut LocalWalkSchedule,
        id: &str,
        visible: bool,
        cycles: std::ops::Range<u64>,
        signals: &[u64],
    ) -> Vec<u64> {
        let active = ids(&[id]);
        let visible_ids = if visible { active.clone() } else { ids(&[]) };
        let mut walked = Vec::new();
        for cycle in cycles {
            if signals.contains(&cycle) {
                schedule.note_signal(id, cycle);
            }
            if schedule
                .select(&active, &visible_ids, &ids(&[]), cycle, true)
                .contains(id)
            {
                schedule.note_walked(id, cycle);
                walked.push(cycle);
            }
        }
        walked
    }

    #[test]
    fn a_new_project_is_walked_at_once() {
        let schedule = LocalWalkSchedule::default();
        let all = ids(&["visible", "hidden"]);
        assert_eq!(
            schedule.select(&all, &ids(&["visible"]), &ids(&[]), 7, true),
            all
        );
    }

    #[test]
    fn a_quiet_hidden_project_falls_back_to_five_minutes() {
        let mut schedule = LocalWalkSchedule::default();
        let walked = walks_of(&mut schedule, "p", false, 0..130, &[]);
        assert_eq!(walked, [0, 60, 120]);
    }

    #[test]
    fn a_hidden_project_with_a_signal_is_walked_on_the_idle_cadence() {
        let mut schedule = LocalWalkSchedule::default();
        // Signal right after a walk: due once the idle cadence allows it.
        let walked = walks_of(&mut schedule, "p", false, 0..20, &[1]);
        assert_eq!(walked, [0, 6]);
    }

    #[test]
    fn a_signal_long_after_the_last_walk_is_served_on_the_next_cycle() {
        let mut schedule = LocalWalkSchedule::default();
        let walked = walks_of(&mut schedule, "p", false, 0..40, &[30]);
        assert_eq!(walked, [0, 30]);
    }

    #[test]
    fn a_walk_consumes_the_signal() {
        let mut schedule = LocalWalkSchedule::default();
        let walked = walks_of(&mut schedule, "p", false, 0..61, &[0]);
        assert_eq!(
            walked,
            [0, 60],
            "the signal of cycle 0 is covered by that walk"
        );
    }

    #[test]
    fn an_active_visible_project_is_walked_every_cycle() {
        let mut schedule = LocalWalkSchedule::default();
        let walked = walks_of(&mut schedule, "p", true, 0..4, &[0, 1, 2, 3]);
        assert_eq!(walked, [0, 1, 2, 3]);
    }

    #[test]
    fn a_visible_project_backs_off_after_the_active_window() {
        let mut schedule = LocalWalkSchedule::default();
        let walked = walks_of(&mut schedule, "p", true, 0..30, &[0]);
        let active: Vec<u64> = (0..ACTIVE_WINDOW_CYCLES).collect();
        assert_eq!(walked[..active.len()], active[..]);
        assert_eq!(walked[active.len()..], [17, 23, 29]);
    }

    #[test]
    fn an_idle_visible_project_is_walked_every_thirty_seconds() {
        let mut schedule = LocalWalkSchedule::default();
        let walked = walks_of(&mut schedule, "p", true, 0..20, &[]);
        assert_eq!(walked, [0, 6, 12, 18]);
    }

    #[test]
    fn forced_projects_bypass_the_cadence() {
        let mut schedule = LocalWalkSchedule::default();
        let all = ids(&["p"]);
        schedule.note_walked("p", 0);
        assert!(
            schedule
                .select(&all, &ids(&[]), &ids(&[]), 1, true)
                .is_empty()
        );
        assert_eq!(schedule.select(&all, &ids(&[]), &all, 1, false), all);
    }

    #[test]
    fn nothing_but_forced_projects_between_cadence_ticks() {
        let schedule = LocalWalkSchedule::default();
        let all = ids(&["p", "q"]);
        assert_eq!(
            schedule.select(&all, &all, &ids(&["q"]), 0, false),
            ids(&["q"])
        );
    }

    #[test]
    fn deleted_projects_are_never_selected() {
        let schedule = LocalWalkSchedule::default();
        assert!(
            schedule
                .select(&ids(&["p"]), &ids(&[]), &ids(&["gone"]), 0, true)
                .contains("p")
        );
        assert!(
            !schedule
                .select(&ids(&["p"]), &ids(&[]), &ids(&["gone"]), 0, true)
                .contains("gone")
        );
    }

    #[test]
    fn output_fingerprints_report_only_moved_projects() {
        let mut fingerprints = OutputFingerprints::default();
        let first = HashMap::from([("a".to_string(), 1), ("b".to_string(), 5)]);
        assert!(fingerprints.changed(first).is_empty(), "no baseline yet");

        let second = HashMap::from([
            ("a".to_string(), 2),
            ("b".to_string(), 5),
            ("c".to_string(), 9),
        ]);
        assert_eq!(fingerprints.changed(second), ["a".to_string()]);
    }
}
