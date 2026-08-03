use std::collections::HashSet;
use std::hash::Hash;

/// Immediate presentation work produced by one activity event.
#[derive(Debug)]
pub struct ActivityRepaintDecision<T> {
    /// Keys to present synchronously for either the leading edge or input priority.
    pub immediate: HashSet<T>,
    /// Whether this event transitioned from idle and must start the sole timer loop.
    pub start_timer: bool,
}

/// Collects exact repaint keys behind a leading-edge presentation batch.
///
/// [`queue_activity`](Self::queue_activity) presents the idle-to-active edge
/// immediately and tells the caller to start one frame timer. While that timer
/// is active, ordinary activity is deduplicated for
/// [`take_scheduled`](Self::take_scheduled), but selected input-response keys can
/// be promoted to immediate presentation without starting another timer.
#[derive(Debug)]
pub struct ActivityRepaintBatch<T> {
    pending: HashSet<T>,
    scheduled: bool,
}

impl<T> Default for ActivityRepaintBatch<T> {
    fn default() -> Self {
        Self {
            pending: HashSet::new(),
            scheduled: false,
        }
    }
}

impl<T: Eq + Hash> ActivityRepaintBatch<T> {
    pub fn queue_activity(
        &mut self,
        keys: impl IntoIterator<Item = T>,
        promoted: impl IntoIterator<Item = T>,
    ) -> ActivityRepaintDecision<T> {
        self.pending.extend(keys);
        if !self.scheduled && !self.pending.is_empty() {
            self.scheduled = true;
            return ActivityRepaintDecision {
                immediate: std::mem::take(&mut self.pending),
                start_timer: true,
            };
        }

        let mut immediate = HashSet::new();
        for key in promoted {
            if self.pending.remove(&key) {
                immediate.insert(key);
            }
        }
        ActivityRepaintDecision {
            immediate,
            start_timer: false,
        }
    }

    /// Drain one scheduled frame, or end the frame window when activity is idle.
    pub fn take_scheduled(&mut self) -> Option<HashSet<T>> {
        if self.pending.is_empty() {
            self.scheduled = false;
            None
        } else {
            Some(std::mem::take(&mut self.pending))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ActivityRepaintBatch;
    use std::collections::HashSet;

    #[test]
    fn leading_edge_is_immediate_and_starts_one_timer() {
        let mut batch = ActivityRepaintBatch::default();

        let decision = batch.queue_activity(["a", "b"], []);

        assert!(decision.start_timer);
        assert_eq!(decision.immediate, HashSet::from(["a", "b"]));
    }

    #[test]
    fn input_response_is_promoted_while_unrelated_activity_stays_batched() {
        let mut batch = ActivityRepaintBatch::default();
        let leading = batch.queue_activity(["initial"], []);
        assert!(leading.start_timer);

        let busy = batch.queue_activity(["input", "background"], ["input"]);

        assert!(!busy.start_timer, "promotion must not start a second timer");
        assert_eq!(busy.immediate, HashSet::from(["input"]));
        assert_eq!(
            batch.take_scheduled(),
            Some(HashSet::from(["background"])),
            "promoted ids must not repaint again on the scheduled frame"
        );
        assert_eq!(batch.take_scheduled(), None, "an empty frame rearms idle");

        let next = batch.queue_activity(["next"], []);
        assert!(next.start_timer);
        assert_eq!(next.immediate, HashSet::from(["next"]));
    }

    #[test]
    fn empty_activity_does_not_schedule() {
        let mut batch = ActivityRepaintBatch::<&str>::default();
        let decision = batch.queue_activity([], []);
        assert!(!decision.start_timer);
        assert!(decision.immediate.is_empty());
    }
}
