//! Generic selection handling types.
//!
//! Provides reusable selection state and traits for normalizing selections
//! across different position types (2D coordinates, 1D offsets, etc.).

/// Generic selection state over position type P.
#[derive(Clone, Default)]
pub struct SelectionState<P: Clone + Default> {
    /// Start position of selection
    pub start: Option<P>,
    /// End position of selection
    pub end: Option<P>,
    /// Whether we're currently dragging/selecting
    pub is_selecting: bool,
}

/// Trait for normalizing selection positions (ensuring start <= end).
pub trait Selectable: Clone {
    /// Given two positions, return them in normalized order (smaller first).
    fn normalized_pair(a: Self, b: Self) -> (Self, Self);
}

#[allow(dead_code)]
impl<P: Selectable + Default> SelectionState<P> {
    /// Get normalized selection range (start <= end).
    pub fn normalized(&self) -> Option<(P, P)> {
        match (&self.start, &self.end) {
            (Some(s), Some(e)) => Some(P::normalized_pair(s.clone(), e.clone())),
            _ => None,
        }
    }

    /// Start a new selection at the given position.
    pub fn start_at(&mut self, pos: P) {
        self.start = Some(pos.clone());
        self.end = Some(pos);
        self.is_selecting = true;
    }

    /// Update the end position during drag.
    pub fn update_end(&mut self, pos: P) {
        if self.is_selecting {
            self.end = Some(pos);
        }
    }

    /// Finish the selection (stop dragging).
    pub fn finish(&mut self) {
        self.is_selecting = false;
    }

    /// Clear the selection entirely.
    pub fn clear(&mut self) {
        self.start = None;
        self.end = None;
        self.is_selecting = false;
    }

    /// Check if there is an active selection (start and end are set).
    pub fn has_selection(&self) -> bool {
        self.start.is_some() && self.end.is_some()
    }
}

// Implementation for 2D positions (line, col) or (col, row)
// Compares first by first coordinate, then by second coordinate
impl Selectable for (usize, usize) {
    fn normalized_pair(a: Self, b: Self) -> (Self, Self) {
        if a.0 < b.0 || (a.0 == b.0 && a.1 <= b.1) {
            (a, b)
        } else {
            (b, a)
        }
    }
}

// Implementation for 1D offset (e.g., character offset in markdown)
impl Selectable for usize {
    fn normalized_pair(a: Self, b: Self) -> (Self, Self) {
        if a <= b {
            (a, b)
        } else {
            (b, a)
        }
    }
}
