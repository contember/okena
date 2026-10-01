//! A list of shown items plus the selected one, remembered by a stable key so
//! replacing the items (a new filter, an async result) never moves the
//! selection onto a different item.

pub(super) struct ListSelection<T, K> {
    items: Vec<T>,
    selected: Option<K>,
    key: fn(&T) -> K,
}

impl<T, K: PartialEq> ListSelection<T, K> {
    pub(super) fn new(key: fn(&T) -> K) -> Self {
        Self {
            items: Vec::new(),
            selected: None,
            key,
        }
    }

    pub(super) fn items(&self) -> &[T] {
        &self.items
    }

    /// Replace the shown items. The selection survives if its key is still
    /// shown, and is cleared otherwise.
    pub(super) fn set_items(&mut self, items: Vec<T>) {
        self.items = items;
        if self.position().is_none() {
            self.selected = None;
        }
    }

    pub(super) fn selected(&self) -> Option<&T> {
        self.position().map(|index| &self.items[index])
    }

    pub(super) fn is_selected(&self, item: &T) -> bool {
        self.selected.as_ref() == Some(&(self.key)(item))
    }

    pub(super) fn has_selection(&self) -> bool {
        self.selected.is_some()
    }

    pub(super) fn select(&mut self, key: K) {
        self.selected = self
            .items
            .iter()
            .any(|item| (self.key)(item) == key)
            .then_some(key);
    }

    pub(super) fn select_first(&mut self) {
        self.selected = self.items.first().map(self.key);
    }

    pub(super) fn clear(&mut self) {
        self.selected = None;
    }

    /// Down with nothing selected selects the first item.
    pub(super) fn move_down(&mut self) {
        let next = match self.position() {
            Some(index) => (index + 1).min(self.items.len() - 1),
            None => 0,
        };
        self.selected = self.items.get(next).map(self.key);
    }

    /// Up with nothing selected does nothing; it stops at the first item.
    pub(super) fn move_up(&mut self) {
        if let Some(index) = self.position() {
            self.selected = Some((self.key)(&self.items[index.saturating_sub(1)]));
        }
    }

    fn position(&self) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        self.items
            .iter()
            .position(|item| &(self.key)(item) == selected)
    }
}

#[cfg(test)]
mod tests {
    use super::ListSelection;

    fn selection(items: &[u32]) -> ListSelection<u32, u32> {
        let mut selection = ListSelection::new(|item: &u32| *item);
        selection.set_items(items.to_vec());
        selection
    }

    #[test]
    fn selection_follows_its_key_across_reordered_items() {
        let mut selection = selection(&[1, 2, 3]);
        selection.select(2);
        selection.set_items(vec![3, 2, 9]);
        assert_eq!(selection.selected(), Some(&2));
    }

    #[test]
    fn selection_clears_when_its_item_disappears() {
        let mut selection = selection(&[1, 2, 3]);
        selection.select(2);
        selection.set_items(vec![1, 3]);
        assert_eq!(selection.selected(), None);
        assert!(!selection.has_selection());
    }

    #[test]
    fn selecting_an_absent_key_selects_nothing() {
        let mut selection = selection(&[1, 2]);
        selection.select(7);
        assert_eq!(selection.selected(), None);
    }

    #[test]
    fn down_starts_at_first_and_stops_at_last() {
        let mut selection = selection(&[1, 2]);
        selection.move_down();
        assert_eq!(selection.selected(), Some(&1));
        selection.move_down();
        selection.move_down();
        assert_eq!(selection.selected(), Some(&2));
    }

    #[test]
    fn up_needs_a_selection_and_stops_at_first() {
        let mut selection = selection(&[1, 2]);
        selection.move_up();
        assert_eq!(selection.selected(), None);
        selection.select(2);
        selection.move_up();
        selection.move_up();
        assert_eq!(selection.selected(), Some(&1));
    }

    #[test]
    fn moving_in_an_empty_list_selects_nothing() {
        let mut selection = selection(&[]);
        selection.move_down();
        selection.move_up();
        selection.select_first();
        assert_eq!(selection.selected(), None);
    }
}
