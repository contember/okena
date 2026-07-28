use std::collections::HashSet;
use std::hash::Hash;

/// Collects exact repaint keys behind one scheduled frame callback.
///
/// [`queue`](Self::queue) returns `true` only for the first key(s) in a batch,
/// telling the caller to schedule one timer. Further activity is deduplicated
/// until [`take`](Self::take) drains the keys and rearms the batch.
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

    pub fn take(&mut self) -> HashSet<T> {
        self.scheduled = false;
        std::mem::take(&mut self.pending)
    }
}

#[cfg(test)]
mod tests {
    use super::ActivityRepaintBatch;
    use std::collections::HashSet;

    #[test]
    fn repaint_batch_deduplicates_keys_behind_one_schedule() {
        let mut batch = ActivityRepaintBatch::default();

        assert!(batch.queue(["a", "b"]));
        assert!(!batch.queue(["b", "c"]));
        assert_eq!(batch.take(), HashSet::from(["a", "b", "c"]));
        assert!(batch.queue(["a"]), "taking the batch rearms scheduling");
    }

    #[test]
    fn empty_repaint_batch_does_not_schedule() {
        let mut batch = ActivityRepaintBatch::<&str>::default();
        assert!(!batch.queue([]));
    }
}
