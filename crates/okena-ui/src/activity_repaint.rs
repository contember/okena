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

/// Decision produced when activity enters a throttled repaint stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActivityRepaintThrottleDecision {
    /// Present immediately on the idle-to-active edge.
    pub repaint_now: bool,
    /// Start the stream's sole cadence timer.
    pub start_timer: bool,
}

/// Leading-edge repaint plus coalesced sustained/trailing updates.
///
/// The caller presents [`ActivityRepaintThrottleDecision::repaint_now`]
/// immediately and starts one timer when requested. Each timer tick calls
/// [`timer_tick`](Self::timer_tick): `true` presents one coalesced repaint and
/// keeps the timer alive; `false` returns the stream to idle.
#[derive(Debug, Default)]
pub struct ActivityRepaintThrottle {
    pending: bool,
    scheduled: bool,
}

impl ActivityRepaintThrottle {
    pub fn on_activity(&mut self) -> ActivityRepaintThrottleDecision {
        self.pending = true;
        if self.scheduled {
            return ActivityRepaintThrottleDecision::default();
        }

        self.pending = false;
        self.scheduled = true;
        ActivityRepaintThrottleDecision {
            repaint_now: true,
            start_timer: true,
        }
    }

    /// Advance one cadence tick. A pending event produces the trailing/sustained
    /// repaint; an empty tick stops the timer and rearms the leading edge.
    pub fn timer_tick(&mut self) -> bool {
        if self.pending {
            self.pending = false;
            true
        } else {
            self.scheduled = false;
            false
        }
    }
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
    use super::{ActivityRepaintBatch, ActivityRepaintThrottle};
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
    fn throttled_activity_has_one_leading_edge_trailing_ticks_and_idle_rearming() {
        let mut throttle = ActivityRepaintThrottle::default();

        let leading = throttle.on_activity();
        assert!(leading.repaint_now);
        assert!(leading.start_timer);

        let repeated = throttle.on_activity();
        assert!(!repeated.repaint_now);
        assert!(!repeated.start_timer);
        assert!(
            throttle.timer_tick(),
            "pending activity gets one trailing repaint"
        );

        let sustained = throttle.on_activity();
        assert!(!sustained.repaint_now);
        assert!(!sustained.start_timer);
        assert!(
            throttle.timer_tick(),
            "the existing timer handles sustained activity"
        );
        assert!(
            !throttle.timer_tick(),
            "an empty tick returns the stream to idle"
        );

        let rearmed = throttle.on_activity();
        assert!(rearmed.repaint_now);
        assert!(rearmed.start_timer);
    }

    #[test]
    fn empty_activity_does_not_schedule() {
        let mut batch = ActivityRepaintBatch::<&str>::default();
        let decision = batch.queue_activity([], []);
        assert!(!decision.start_timer);
        assert!(decision.immediate.is_empty());
    }
}
