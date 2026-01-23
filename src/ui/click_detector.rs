//! Reusable double-click detection utility.
//!
//! This module provides a generic double-click detector that can be used
//! throughout the application for consistent double-click behavior.

use std::time::{Duration, Instant};

/// A generic double-click detector.
///
/// Tracks clicks on items identified by a key type `K` and detects
/// when two clicks happen within a configurable threshold.
///
/// # Example
///
/// ```ignore
/// let mut detector = ClickDetector::<String>::new();
///
/// // First click
/// assert!(!detector.check("item1".to_string()));
///
/// // Second click within threshold
/// assert!(detector.check("item1".to_string())); // Returns true (double-click)
/// ```
pub struct ClickDetector<K: Eq + Clone> {
    last_click: Option<(K, Instant)>,
    threshold: Duration,
}

impl<K: Eq + Clone> Default for ClickDetector<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Eq + Clone> ClickDetector<K> {
    /// Create a new ClickDetector with the default threshold (400ms).
    pub fn new() -> Self {
        Self::with_threshold(Duration::from_millis(400))
    }

    /// Create a new ClickDetector with a custom threshold.
    pub fn with_threshold(threshold: Duration) -> Self {
        Self {
            last_click: None,
            threshold,
        }
    }

    /// Check if this click constitutes a double-click.
    ///
    /// Returns `true` if this is a double-click (same key, within threshold).
    /// After a double-click is detected, the state is reset.
    ///
    /// Returns `false` and records this click for future comparison otherwise.
    pub fn check(&mut self, key: K) -> bool {
        let now = Instant::now();

        let is_double = if let Some((last_key, last_time)) = &self.last_click {
            *last_key == key && now.duration_since(*last_time) < self.threshold
        } else {
            false
        };

        if is_double {
            self.last_click = None;
            true
        } else {
            self.last_click = Some((key, now));
            false
        }
    }

    /// Reset the click state, forgetting any previous click.
    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.last_click = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_click() {
        let mut detector = ClickDetector::<String>::new();
        assert!(!detector.check("item1".to_string()));
    }

    #[test]
    fn test_different_keys() {
        let mut detector = ClickDetector::<String>::new();
        assert!(!detector.check("item1".to_string()));
        assert!(!detector.check("item2".to_string())); // Different key, not double-click
    }
}
