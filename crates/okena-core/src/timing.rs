//! Slow-path instrumentation for diagnosing UI freezes.
//!
//! Drop a `SlowGuard::new("label")` at the top of any block that runs on a
//! latency-sensitive thread (GPUI main, render, action handlers). On drop the
//! guard logs a `warn!` with the elapsed time if it crossed `THRESHOLD_MS`.
//!
//! Threshold is chosen so we hear about real freezes (>500 ms) without flooding
//! the log under normal load.

use std::time::{Duration, Instant};

pub const SLOW_THRESHOLD_MS: u64 = 500;

pub struct SlowGuard {
    label: &'static str,
    detail: Option<String>,
    start: Instant,
    threshold: Duration,
}

impl SlowGuard {
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            detail: None,
            start: Instant::now(),
            threshold: Duration::from_millis(SLOW_THRESHOLD_MS),
        }
    }

    pub fn with_detail(label: &'static str, detail: impl Into<String>) -> Self {
        Self {
            label,
            detail: Some(detail.into()),
            start: Instant::now(),
            threshold: Duration::from_millis(SLOW_THRESHOLD_MS),
        }
    }

    /// Update the detail string mid-flight (e.g., after we know payload size).
    pub fn set_detail(&mut self, detail: impl Into<String>) {
        self.detail = Some(detail.into());
    }
}

impl Drop for SlowGuard {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed();
        if elapsed >= self.threshold {
            match &self.detail {
                Some(d) => log::warn!("[slow] {} took {:?} ({})", self.label, elapsed, d),
                None => log::warn!("[slow] {} took {:?}", self.label, elapsed),
            }
        }
    }
}
