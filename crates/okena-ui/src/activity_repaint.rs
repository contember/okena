use std::collections::HashSet;
use std::hash::Hash;

/// Collects exact repaint keys behind a leading-edge presentation batch.
///
/// [`queue`](Self::queue) returns `true` when activity transitions from idle.
/// The caller drains those keys with [`take_immediate`](Self::take_immediate),
/// presents them immediately, and starts its frame timer. Activity during that
/// interval is deduplicated for [`take_scheduled`](Self::take_scheduled). The
/// timer keeps running while frames contain activity and rearms the leading edge
/// after one empty frame.
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
    pub fn queue(&mut self, keys: impl IntoIterator<Item = T>) -> bool {
        self.pending.extend(keys);
        if self.scheduled || self.pending.is_empty() {
            return false;
        }
        self.scheduled = true;
        true
    }

    /// Drain the leading-edge keys without ending the scheduled frame window.
    pub fn take_immediate(&mut self) -> HashSet<T> {
        debug_assert!(self.scheduled);
        std::mem::take(&mut self.pending)
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
    fn first_activity_is_immediate_then_frames_are_deduplicated() {
        let mut batch = ActivityRepaintBatch::default();

        assert!(batch.queue(["a", "b"]));
        assert_eq!(batch.take_immediate(), HashSet::from(["a", "b"]));

        assert!(!batch.queue(["b", "c"]));
        assert!(!batch.queue(["c"]));
        assert_eq!(batch.take_scheduled(), Some(HashSet::from(["b", "c"])));

        assert_eq!(batch.take_scheduled(), None, "an empty frame ends batching");
        assert!(batch.queue(["a"]), "activity after idle is immediate again");
    }

    #[test]
    fn empty_repaint_batch_does_not_schedule() {
        let mut batch = ActivityRepaintBatch::<&str>::default();
        assert!(!batch.queue([]));
    }
}
