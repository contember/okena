use std::collections::{HashMap, HashSet};
use std::hash::Hash;

/// Title invalidations survive until each window's shared repaint pacer admits
/// them. Keep IDs rather than title strings so presentation reads the latest value.
pub(super) struct PendingTitleRepaints<W> {
    windows: HashMap<W, HashSet<String>>,
}

impl<W> Default for PendingTitleRepaints<W> {
    fn default() -> Self {
        Self {
            windows: HashMap::new(),
        }
    }
}

impl<W: Eq + Hash> PendingTitleRepaints<W> {
    pub fn queue(&mut self, windows: impl IntoIterator<Item = W>, ids: &[String]) {
        for window in windows {
            self.windows
                .entry(window)
                .or_default()
                .extend(ids.iter().cloned());
        }
    }

    pub fn take_admitted(
        &mut self,
        window: W,
        ids: &[String],
        mut admit: impl FnMut(&String) -> bool,
    ) -> Vec<String> {
        let Some(pending) = self.windows.get_mut(&window) else {
            return Vec::new();
        };
        let mut admitted = Vec::new();
        for id in ids {
            if pending.contains(id) && admit(id) {
                pending.remove(id);
                admitted.push(id.clone());
            }
        }
        admitted
    }

    pub fn forget(&mut self, window: W) {
        self.windows.remove(&window);
    }
}

#[cfg(test)]
mod tests {
    use super::PendingTitleRepaints;
    use okena_ui::activity_repaint::{ActivityRepaintBatch, UnfocusedWindowPacer};
    use std::time::{Duration, Instant};

    #[test]
    fn title_only_burst_is_coalesced_and_background_trailing_update_lands() {
        let mut pending = PendingTitleRepaints::default();
        let mut batch = ActivityRepaintBatch::default();
        let mut pacer = UnfocusedWindowPacer::new(Duration::from_millis(100));
        let start = Instant::now();
        let ids = vec!["spinner".to_owned()];
        pending.queue([1, 2], &ids);
        let leading = batch.queue_activity(ids.clone(), []);
        assert!(leading.start_timer);
        let mut frame = pacer.frame(start);
        for window in [1, 2] {
            assert_eq!(
                pending.take_admitted(window, &ids, |id| {
                    pacer.admit(&mut frame, id, Some(window), window == 2)
                }),
                ids
            );
        }
        assert!(pacer.finish_frame(frame).is_none());

        // Many title events before the next presentation produce one batch.
        for _ in 0..100 {
            pending.queue([1, 2], &ids);
            let decision = batch.queue_activity(ids.clone(), []);
            assert!(decision.immediate.is_empty());
            assert!(!decision.start_timer);
        }
        let scheduled: Vec<_> = batch.take_scheduled().unwrap().into_iter().collect();
        assert_eq!(scheduled, ids);
        let mut frame = pacer.frame(start + Duration::from_millis(20));
        assert_eq!(
            pending.take_admitted(1, &scheduled, |id| {
                pacer.admit(&mut frame, id, Some(1), false)
            }),
            ids
        );
        assert!(
            pending
                .take_admitted(2, &scheduled, |id| {
                    pacer.admit(&mut frame, id, Some(2), true)
                })
                .is_empty()
        );
        let deadline = pacer.finish_frame(frame).unwrap();
        assert_eq!(deadline, start + Duration::from_millis(100));
        assert!(batch.take_scheduled().is_none());

        // No more terminal output and no mounted content pane are needed.
        let (mut frame, trailing) = pacer.start_flush(deadline);
        assert!(
            pending
                .take_admitted(1, &trailing, |_| panic!("already presented"))
                .is_empty()
        );
        assert_eq!(
            pending.take_admitted(2, &trailing, |id| {
                pacer.admit(&mut frame, id, Some(2), true)
            }),
            ids
        );
        assert!(pacer.finish_frame(frame).is_none());
    }

    #[test]
    fn deferred_titles_survive_other_frames_and_are_dropped_when_window_closes() {
        let mut pending = PendingTitleRepaints::default();
        let ids = vec!["spinner".to_owned()];
        pending.queue([1, 2], &ids);
        assert!(
            pending
                .take_admitted(1, &["other".to_owned()], |_| true)
                .is_empty()
        );
        assert!(pending.take_admitted(1, &ids, |_| false).is_empty());
        pending.forget(2);
        assert!(
            pending
                .take_admitted(2, &ids, |_| panic!("closed window"))
                .is_empty()
        );
        assert_eq!(pending.take_admitted(1, &ids, |_| true), ids);
    }

    #[test]
    fn shared_content_frame_and_unpaced_flush_admit_pending_titles() {
        let mut pending = PendingTitleRepaints::default();
        let mut pacer = UnfocusedWindowPacer::new(Duration::from_millis(100));
        let start = Instant::now();
        let ids = vec!["spinner".to_owned()];
        let mut frame = pacer.frame(start);
        // Content already consumed this window's frame, but its title is allowed
        // in that same frame instead of being delayed by another interval.
        assert!(pacer.admit(&mut frame, &ids[0], Some(1), true));
        pending.queue([1], &ids);
        assert_eq!(
            pending.take_admitted(1, &ids, |id| { pacer.admit(&mut frame, id, Some(1), true) }),
            ids
        );
        assert!(pacer.finish_frame(frame).is_none());
        pending.queue([1], &ids);
        let mut frame = pacer.frame(start + Duration::from_millis(20));
        assert!(
            pending
                .take_admitted(1, &ids, |id| { pacer.admit(&mut frame, id, Some(1), true) })
                .is_empty()
        );
        assert!(pacer.finish_frame(frame).is_some());
        let (mut frame, deferred) = pacer.start_flush(start + Duration::from_millis(30));
        // Both activation and disabling the cap make this window unpaced.
        assert_eq!(
            pending.take_admitted(1, &deferred, |id| {
                pacer.admit(&mut frame, id, Some(1), false)
            }),
            ids
        );
        assert!(pacer.finish_frame(frame).is_none());
    }
}
