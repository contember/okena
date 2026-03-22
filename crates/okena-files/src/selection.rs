//! Selection handling utilities.
//!
//! Re-exports core selection types from okena-core and provides
//! gpui-specific selection helpers.

// Re-export core selection types
pub use okena_core::selection::{Selectable, SelectionState};

use gpui::{ClipboardItem, Context};

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
