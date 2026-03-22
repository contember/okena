//! Generic selection handling utilities.
//!
//! Provides reusable selection state and traits for normalizing selections
//! across different position types (2D coordinates, 1D offsets, etc.).

use gpui::{ClipboardItem, Context};

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

/// Copy text to clipboard if it's not empty.
///
/// This is a convenience function to avoid duplicating clipboard logic.
pub fn copy_to_clipboard<V: 'static>(cx: &mut Context<V>, text: Option<String>) {
    if let Some(text) = text {
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }
}

/// Extension trait for SelectionState with 2D positions to check line selection.
pub trait Selection2DExtension {
    /// Check if the given line is fully or partially selected.
    fn line_has_selection(&self, line: usize) -> bool;
}

impl Selection2DExtension for SelectionState<(usize, usize)> {
    fn line_has_selection(&self, line: usize) -> bool {
        if let Some(((start_line, _), (end_line, _))) = self.normalized() {
            line >= start_line && line <= end_line
        } else {
            false
        }
    }
}

/// Extension trait for SelectionState with 1D offsets to check non-empty selection.
pub trait Selection1DExtension {
    /// Get normalized selection only if start != end.
    fn normalized_non_empty(&self) -> Option<(usize, usize)>;
}

impl Selection1DExtension for SelectionState<usize> {
    fn normalized_non_empty(&self) -> Option<(usize, usize)> {
        match (self.start, self.end) {
            (Some(start), Some(end)) if start != end => {
                if start <= end {
                    Some((start, end))
                } else {
                    Some((end, start))
                }
            }
            _ => None,
        }
    }
}

/// Extension trait for SelectionState with 2D positions to check non-empty selection.
pub trait Selection2DNonEmpty {
    /// Get normalized selection only if start != end (zero-width clicks return None).
    fn normalized_non_empty(&self) -> Option<((usize, usize), (usize, usize))>;
}

impl Selection2DNonEmpty for SelectionState<(usize, usize)> {
    fn normalized_non_empty(&self) -> Option<((usize, usize), (usize, usize))> {
        match (&self.start, &self.end) {
            (Some(s), Some(e)) if s != e => {
                Some(<(usize, usize)>::normalized_pair(*s, *e))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_2d_non_empty_zero_width_returns_none() {
        let mut sel = SelectionState::<(usize, usize)>::default();
        sel.start = Some((5, 3));
        sel.end = Some((5, 3));
        assert!(sel.normalized_non_empty().is_none());
    }

    #[test]
    fn test_2d_non_empty_with_selection() {
        let mut sel = SelectionState::<(usize, usize)>::default();
        sel.start = Some((5, 3));
        sel.end = Some((5, 10));
        assert_eq!(sel.normalized_non_empty(), Some(((5, 3), (5, 10))));
    }

    #[test]
    fn test_2d_non_empty_reversed() {
        let mut sel = SelectionState::<(usize, usize)>::default();
        sel.start = Some((10, 0));
        sel.end = Some((5, 3));
        assert_eq!(sel.normalized_non_empty(), Some(((5, 3), (10, 0))));
    }

    #[test]
    fn test_2d_non_empty_no_start() {
        let sel = SelectionState::<(usize, usize)>::default();
        assert!(sel.normalized_non_empty().is_none());
    }
}
