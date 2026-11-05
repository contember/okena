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
/// `interval`. A pane of a paced window that is not due is deferred as a
/// `(terminal, window)` pair. The flush started at the returned deadline
/// presents exactly those pairs, so a burst's last frame lands and no other
/// pane repaints for it.
#[derive(Debug)]
pub struct UnfocusedWindowPacer<W, T> {
    interval: Duration,
    last_presented: HashMap<W, Instant>,
    deferred: HashSet<(T, W)>,
    flush_deadline: Option<Instant>,
}

/// The decisions of one activity frame, applied by
/// [`UnfocusedWindowPacer::finish_frame`].
#[derive(Debug)]
pub struct PacedFrame<W, T> {
    now: Instant,
    /// A flush presents only these pairs; a regular frame any pane.
    only: Option<HashSet<(T, W)>>,
    presented_windows: HashSet<W>,
    presented: Vec<(T, W)>,
    deferred: Vec<(T, W)>,
}

impl<W: Eq + Hash + Copy, T: Eq + Hash + Clone> UnfocusedWindowPacer<W, T> {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_presented: HashMap::new(),
            deferred: HashSet::new(),
            flush_deadline: None,
        }
    }

    /// A regular activity frame.
    pub fn frame(&self, now: Instant) -> PacedFrame<W, T> {
        PacedFrame {
            now,
            only: None,
            presented_windows: HashSet::new(),
            presented: Vec::new(),
            deferred: Vec::new(),
        }
    }

    /// A frame for the deferred pairs, and the terminals to present in it.
    pub fn start_flush(&mut self, now: Instant) -> (PacedFrame<W, T>, Vec<T>) {
        self.flush_deadline = None;
        let only = std::mem::take(&mut self.deferred);
        let terminals: HashSet<T> = only.iter().map(|(terminal, _)| terminal.clone()).collect();
        let mut frame = self.frame(now);
        frame.only = Some(only);
        (frame, terminals.into_iter().collect())
    }

    /// Whether a pane of `terminal` shown in `window` repaints in `frame`.
    /// `paced`: the window has no OS focus and the cap is on.
    pub fn admit(
        &self,
        frame: &mut PacedFrame<W, T>,
        terminal: &T,
        window: Option<W>,
        paced: bool,
    ) -> bool {
        if let Some(only) = &frame.only {
            match window {
                Some(window) if only.contains(&(terminal.clone(), window)) => {}
                _ => return false,
            }
        }
        let Some(window) = window else {
            return true;
        };
        if paced && !frame.presented_windows.contains(&window) && !self.is_due(window, frame.now) {
            frame.deferred.push((terminal.clone(), window));
            return false;
        }
        if paced {
            frame.presented_windows.insert(window);
        }
        if !self.deferred.is_empty() {
            frame.presented.push((terminal.clone(), window));
        }
        true
    }

    /// Apply `frame`. Returns the deadline of a flush timer to arm when the
    /// earliest deferred window is due before any timer already armed.
    pub fn finish_frame(&mut self, frame: PacedFrame<W, T>) -> Option<Instant> {
        for window in frame.presented_windows {
            self.last_presented.insert(window, frame.now);
        }
        for pair in &frame.presented {
            self.deferred.remove(pair);
        }
        self.deferred.extend(frame.deferred);
        let due = self
            .deferred
            .iter()
            .map(|(_, window)| {
                self.last_presented
                    .get(window)
                    .map_or(frame.now, |last| *last + self.interval)
            })
            .min()?;
        if self.flush_deadline.is_some_and(|deadline| deadline <= due) {
            return None;
        }
        self.flush_deadline = Some(due);
        Some(due)
    }

    /// Whether the flush timer armed for `deadline` is still the current one.
    pub fn is_flush_current(&self, deadline: Instant) -> bool {
        self.flush_deadline == Some(deadline)
    }

    pub fn forget(&mut self, window: W) {
        self.last_presented.remove(&window);
        self.deferred.retain(|(_, deferred)| *deferred != window);
    }

    fn is_due(&self, window: W, now: Instant) -> bool {
        self.last_presented
            .get(&window)
            .is_none_or(|last| now.saturating_duration_since(*last) >= self.interval)
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

    const INTERVAL: Duration = Duration::from_millis(100);

    fn ms(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    /// Run one regular frame over `panes` (terminal, window, paced); returns
    /// the panes that repainted and the flush deadline to arm.
    fn frame(
        pacer: &mut UnfocusedWindowPacer<&'static str, &'static str>,
        now: Instant,
        panes: &[(&'static str, &'static str, bool)],
    ) -> (Vec<(&'static str, &'static str)>, Option<Instant>) {
        let mut frame = pacer.frame(now);
        let shown = panes
            .iter()
            .filter(|(terminal, window, paced)| {
                pacer.admit(&mut frame, terminal, Some(*window), *paced)
            })
            .map(|(terminal, window, _)| (*terminal, *window))
            .collect();
        (shown, pacer.finish_frame(frame))
    }

    /// Run the flush over every pane of the deferred terminals.
    fn flush(
        pacer: &mut UnfocusedWindowPacer<&'static str, &'static str>,
        now: Instant,
        panes: &[(&'static str, &'static str, bool)],
    ) -> (Vec<(&'static str, &'static str)>, Option<Instant>) {
        let (mut frame, terminals) = pacer.start_flush(now);
        let shown = panes
            .iter()
            .filter(|(terminal, _, _)| terminals.contains(terminal))
            .filter(|(terminal, window, paced)| {
                pacer.admit(&mut frame, terminal, Some(*window), *paced)
            })
            .map(|(terminal, window, _)| (*terminal, *window))
            .collect();
        (shown, pacer.finish_frame(frame))
    }

    #[test]
    fn a_paced_window_presents_once_per_interval_and_its_last_frame_lands() {
        let mut pacer = UnfocusedWindowPacer::new(INTERVAL);
        let start = Instant::now();
        let pane = [("t", "a", true)];

        assert_eq!(frame(&mut pacer, start, &pane), (vec![("t", "a")], None));
        assert_eq!(
            frame(&mut pacer, ms(start, 20), &pane),
            (vec![], Some(ms(start, 100)))
        );
        assert_eq!(
            frame(&mut pacer, ms(start, 40), &pane),
            (vec![], None),
            "one timer"
        );

        assert!(pacer.is_flush_current(ms(start, 100)));
        assert_eq!(
            flush(&mut pacer, ms(start, 100), &pane),
            (vec![("t", "a")], None)
        );
        assert_eq!(
            flush(&mut pacer, ms(start, 200), &pane),
            (vec![], None),
            "nothing left"
        );
    }

    #[test]
    fn unpaced_panes_always_present_and_windows_pace_independently() {
        let mut pacer = UnfocusedWindowPacer::new(INTERVAL);
        let start = Instant::now();
        frame(&mut pacer, start, &[("t", "a", true)]);

        let (shown, _) = frame(
            &mut pacer,
            ms(start, 20),
            &[("t", "a", true), ("t", "b", true), ("t", "focused", false)],
        );

        assert_eq!(shown, vec![("t", "b"), ("t", "focused")]);
    }

    #[test]
    fn a_terminal_in_two_paced_windows_stops_repainting_once_its_frames_land() {
        let mut pacer = UnfocusedWindowPacer::new(INTERVAL);
        let start = Instant::now();
        let panes = [("t", "a", true), ("t", "b", true), ("t", "focused", false)];
        frame(&mut pacer, start, &[("other", "a", true)]);

        // One burst of output: window b presents, a is not due yet.
        let (shown, deadline) = frame(&mut pacer, ms(start, 50), &panes);
        assert_eq!(shown, vec![("t", "b"), ("t", "focused")]);
        assert_eq!(deadline, Some(ms(start, 100)));

        // The flush repaints only the deferred pane, then the terminal is quiet.
        assert_eq!(
            flush(&mut pacer, ms(start, 100), &panes),
            (vec![("t", "a")], None)
        );
        assert_eq!(flush(&mut pacer, ms(start, 200), &panes), (vec![], None));
    }

    #[test]
    fn a_regular_frame_that_presents_a_deferred_pane_cancels_its_flush() {
        let mut pacer = UnfocusedWindowPacer::new(INTERVAL);
        let start = Instant::now();
        let pane = [("t", "a", true)];
        frame(&mut pacer, start, &pane);
        frame(&mut pacer, ms(start, 20), &pane);

        assert_eq!(
            frame(&mut pacer, ms(start, 100), &pane),
            (vec![("t", "a")], None)
        );

        assert_eq!(flush(&mut pacer, ms(start, 100), &pane), (vec![], None));
    }

    #[test]
    fn a_deferred_pane_presents_at_once_when_it_is_no_longer_paced() {
        let mut pacer = UnfocusedWindowPacer::new(INTERVAL);
        let start = Instant::now();
        frame(&mut pacer, start, &[("t", "a", true)]);
        frame(&mut pacer, ms(start, 20), &[("t", "a", true)]);

        // The window gained focus, or the cap was turned off, before it was due.
        let unpaced = [("t", "a", false)];
        assert_eq!(
            flush(&mut pacer, ms(start, 30), &unpaced),
            (vec![("t", "a")], None)
        );
    }

    #[test]
    fn an_earlier_deadline_supersedes_the_armed_flush() {
        let mut pacer = UnfocusedWindowPacer::new(INTERVAL);
        let start = Instant::now();
        frame(&mut pacer, ms(start, 50), &[("t", "late", true)]);
        frame(&mut pacer, start, &[("u", "early", true)]);

        assert_eq!(
            frame(&mut pacer, ms(start, 60), &[("t", "late", true)]).1,
            Some(ms(start, 150))
        );
        assert_eq!(
            frame(&mut pacer, ms(start, 60), &[("u", "early", true)]).1,
            Some(ms(start, 100))
        );

        assert!(!pacer.is_flush_current(ms(start, 150)));
        assert!(pacer.is_flush_current(ms(start, 100)));
    }

    #[test]
    fn a_forgotten_window_drops_its_deferred_panes() {
        let mut pacer = UnfocusedWindowPacer::new(INTERVAL);
        let start = Instant::now();
        frame(&mut pacer, start, &[("t", "a", true)]);
        frame(&mut pacer, ms(start, 20), &[("t", "a", true)]);

        pacer.forget("a");

        assert_eq!(
            flush(&mut pacer, ms(start, 100), &[("t", "a", true)]),
            (vec![], None)
        );
    }
}
