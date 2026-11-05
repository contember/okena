use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::time::{Duration, Instant};

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

/// Paces activity repaints of windows without OS focus to one frame per
/// `interval`. A window is due once `interval` has passed since it last
/// presented. The caller defers a pane whose window is not due and presents it
/// again at [`next_due`](Self::next_due), so a burst's last frame still lands.
#[derive(Debug)]
pub struct UnfocusedWindowPacer<W> {
    interval: Duration,
    last_presented: HashMap<W, Instant>,
}

impl<W: Eq + Hash + Copy> UnfocusedWindowPacer<W> {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_presented: HashMap::new(),
        }
    }

    pub fn is_due(&self, window: W, now: Instant) -> bool {
        self.last_presented
            .get(&window)
            .is_none_or(|last| now.saturating_duration_since(*last) >= self.interval)
    }

    pub fn mark_presented(&mut self, window: W, now: Instant) {
        self.last_presented.insert(window, now);
    }

    /// The earliest instant any of `windows` is due again.
    pub fn next_due(&self, windows: impl IntoIterator<Item = W>) -> Option<Instant> {
        windows
            .into_iter()
            .filter_map(|window| self.last_presented.get(&window))
            .map(|last| *last + self.interval)
            .min()
    }

    pub fn forget(&mut self, window: W) {
        self.last_presented.remove(&window);
    }
}

#[cfg(test)]
mod tests {
    use super::{ActivityRepaintBatch, ActivityRepaintThrottle, UnfocusedWindowPacer};
    use std::collections::HashSet;
    use std::time::{Duration, Instant};

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
    #[test]
    fn unfocused_window_presents_at_most_once_per_interval() {
        let interval = Duration::from_millis(100);
        let mut pacer = UnfocusedWindowPacer::new(interval);
        let start = Instant::now();

        assert!(
            pacer.is_due("a", start),
            "a window that never presented is due"
        );
        pacer.mark_presented("a", start);

        let soon = start + Duration::from_millis(20);
        assert!(!pacer.is_due("a", soon));
        assert!(pacer.is_due("b", soon), "windows are paced independently");
        assert_eq!(pacer.next_due(["a"]), Some(start + interval));

        assert!(pacer.is_due("a", start + interval));
    }

    #[test]
    fn next_due_is_the_earliest_deferred_window() {
        let interval = Duration::from_millis(100);
        let mut pacer = UnfocusedWindowPacer::new(interval);
        let start = Instant::now();
        pacer.mark_presented("late", start + Duration::from_millis(50));
        pacer.mark_presented("early", start);

        assert_eq!(pacer.next_due(["late", "early"]), Some(start + interval));
        assert_eq!(pacer.next_due(["never"]), None);
    }

    #[test]
    fn a_forgotten_window_is_due_at_once() {
        let mut pacer = UnfocusedWindowPacer::new(Duration::from_millis(100));
        let start = Instant::now();
        pacer.mark_presented("a", start);

        pacer.forget("a");

        assert!(pacer.is_due("a", start));
    }
}
