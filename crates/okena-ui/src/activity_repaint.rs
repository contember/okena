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

/// Coalesces activity-driven repaint requests while a window is inactive.
///
/// Call [`request`](Self::request) when model activity may have changed what a
/// view displays. A `true` return means the caller should notify the view now;
/// `false` means either the window is inactive or a repaint is already pending.
/// Call [`repainted`](Self::repainted) from the view's render path, and call
/// [`set_active`](Self::set_active) when its OS window activation changes.
#[derive(Debug, Clone, Copy)]
pub struct ActivityRepaintGate {
    active: bool,
    repaint_pending: bool,
}

impl ActivityRepaintGate {
    pub fn new(active: bool) -> Self {
        Self {
            active,
            repaint_pending: false,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Records activity and reports whether a repaint should be scheduled now.
    pub fn request(&mut self) -> bool {
        if self.repaint_pending {
            return false;
        }
        self.repaint_pending = true;
        self.active
    }

    /// Updates activation and reports whether pending activity should be flushed.
    pub fn set_active(&mut self, active: bool) -> bool {
        self.active = active;
        active && self.repaint_pending
    }

    /// Marks the pending model state as represented by the rendered view.
    pub fn repainted(&mut self) {
        self.repaint_pending = false;
    }
}

#[cfg(test)]
mod tests {
    use super::{ActivityRepaintBatch, ActivityRepaintGate};
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

    #[test]
    fn active_requests_coalesce_until_rendered() {
        let mut gate = ActivityRepaintGate::new(true);

        assert!(gate.request());
        assert!(!gate.request());

        gate.repainted();
        assert!(gate.request());
    }

    #[test]
    fn inactive_requests_flush_once_on_activation() {
        let mut gate = ActivityRepaintGate::new(false);

        assert!(!gate.request());
        assert!(!gate.request());
        assert!(gate.set_active(true));

        gate.repainted();
        assert!(!gate.set_active(false));
        assert!(!gate.set_active(true));
    }

    #[test]
    fn deactivation_preserves_an_already_pending_repaint() {
        let mut gate = ActivityRepaintGate::new(true);

        assert!(gate.request());
        assert!(!gate.set_active(false));
        assert!(gate.set_active(true));
    }
}
