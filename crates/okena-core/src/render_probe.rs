//! Opt-in aggregate render-performance instrumentation.
//!
//! Build with `render-perf-probe`, then set `OKENA_RENDER_PERF_PROBE=1` before
//! starting Okena. The probe emits one bounded, aggregate-only summary after
//! roughly every 10 seconds of observed render activity. It never accepts or
//! logs terminal content, paths, commands, or per-terminal/window identifiers.
//!
//! Summaries are event-driven: a quiet process does not keep a reporting timer
//! alive, and an elapsed window is emitted when the next measured event arrives.

/// Aggregate statistics produced by one terminal paint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalPaintStats {
    /// Number of live pane viewers registered for this terminal at paint time.
    pub live_viewers: usize,
    /// Whether grid layout was reused instead of scanning the terminal model.
    pub grid_cache_hit: bool,
    pub cells_scanned: usize,
    /// Model cells reported as changed when a safe damage source is available.
    /// `None` means the current renderer cannot measure this without consuming
    /// shared multi-view state.
    pub cells_changed: Option<usize>,
    pub text_runs: usize,
    pub background_rects: usize,
}

#[cfg(feature = "render-perf-probe")]
mod imp {
    use super::TerminalPaintStats;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    const REPORT_INTERVAL: Duration = Duration::from_secs(10);
    const MAX_TIMING_SAMPLES: usize = 8_192;

    #[derive(Debug, Default)]
    struct TimingSamples {
        /// Rolling sample used for percentiles. Counts and max still cover every event.
        values_us: Vec<u64>,
        count: u64,
        next_replace: usize,
        overwritten: u64,
        max_us: u64,
    }

    impl TimingSamples {
        fn record(&mut self, duration: Duration) {
            let value = duration_us(duration);
            self.count = self.count.saturating_add(1);
            self.max_us = self.max_us.max(value);
            if self.values_us.len() < MAX_TIMING_SAMPLES {
                self.values_us.push(value);
            } else {
                self.values_us[self.next_replace] = value;
                self.next_replace = (self.next_replace + 1) % MAX_TIMING_SAMPLES;
                self.overwritten = self.overwritten.saturating_add(1);
            }
        }

        fn summarize(mut self) -> TimingSummary {
            self.values_us.sort_unstable();
            TimingSummary {
                count: self.count,
                retained: as_u64(self.values_us.len()),
                p50_us: nearest_rank(&self.values_us, 50),
                p95_us: nearest_rank(&self.values_us, 95),
                max_us: self.max_us,
                overwritten: self.overwritten,
            }
        }
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    struct TimingSummary {
        count: u64,
        retained: u64,
        p50_us: u64,
        p95_us: u64,
        max_us: u64,
        overwritten: u64,
    }

    #[derive(Debug)]
    struct MetricsWindow {
        started: Instant,
        activity_frames: u64,
        repaint_terminal_total: u64,
        repaint_terminal_max: u64,
        registered_terminal_total: u64,
        registered_terminal_max: u64,
        repaint_pane_total: u64,
        repaint_pane_max: u64,
        terminal_paints: TimingSamples,
        terminal_live_viewers_total: u64,
        terminal_live_viewers_max: u64,
        terminal_grid_cache_hits: u64,
        terminal_grid_cache_misses: u64,
        cells_scanned: u64,
        changed_cell_samples: u64,
        cells_changed: u64,
        text_runs: u64,
        background_rects: u64,
        sidebar_activity_invalidations: u64,
        sidebar_renders: TimingSamples,
    }

    impl MetricsWindow {
        fn new(started: Instant) -> Self {
            Self {
                started,
                activity_frames: 0,
                repaint_terminal_total: 0,
                repaint_terminal_max: 0,
                registered_terminal_total: 0,
                registered_terminal_max: 0,
                repaint_pane_total: 0,
                repaint_pane_max: 0,
                terminal_paints: TimingSamples::default(),
                terminal_live_viewers_total: 0,
                terminal_live_viewers_max: 0,
                terminal_grid_cache_hits: 0,
                terminal_grid_cache_misses: 0,
                cells_scanned: 0,
                changed_cell_samples: 0,
                cells_changed: 0,
                text_runs: 0,
                background_rects: 0,
                sidebar_activity_invalidations: 0,
                sidebar_renders: TimingSamples::default(),
            }
        }

        fn record_activity_frame(
            &mut self,
            repaint_terminals: usize,
            registered_terminals: usize,
            repaint_panes: usize,
        ) {
            let repaint_terminals = as_u64(repaint_terminals);
            let registered_terminals = as_u64(registered_terminals);
            let repaint_panes = as_u64(repaint_panes);
            self.activity_frames = self.activity_frames.saturating_add(1);
            self.repaint_terminal_total = self
                .repaint_terminal_total
                .saturating_add(repaint_terminals);
            self.repaint_terminal_max = self.repaint_terminal_max.max(repaint_terminals);
            self.registered_terminal_total = self
                .registered_terminal_total
                .saturating_add(registered_terminals);
            self.registered_terminal_max = self.registered_terminal_max.max(registered_terminals);
            self.repaint_pane_total = self.repaint_pane_total.saturating_add(repaint_panes);
            self.repaint_pane_max = self.repaint_pane_max.max(repaint_panes);
        }

        fn record_terminal_paint(&mut self, duration: Duration, stats: TerminalPaintStats) {
            self.terminal_paints.record(duration);
            let live_viewers = as_u64(stats.live_viewers);
            self.terminal_live_viewers_total = self
                .terminal_live_viewers_total
                .saturating_add(live_viewers);
            self.terminal_live_viewers_max = self.terminal_live_viewers_max.max(live_viewers);
            if stats.grid_cache_hit {
                self.terminal_grid_cache_hits = self.terminal_grid_cache_hits.saturating_add(1);
            } else {
                self.terminal_grid_cache_misses = self.terminal_grid_cache_misses.saturating_add(1);
            }
            self.cells_scanned = self
                .cells_scanned
                .saturating_add(as_u64(stats.cells_scanned));
            if let Some(changed) = stats.cells_changed {
                self.changed_cell_samples = self.changed_cell_samples.saturating_add(1);
                self.cells_changed = self.cells_changed.saturating_add(as_u64(changed));
            }
            self.text_runs = self.text_runs.saturating_add(as_u64(stats.text_runs));
            self.background_rects = self
                .background_rects
                .saturating_add(as_u64(stats.background_rects));
        }

        fn record_sidebar_activity_invalidation(&mut self) {
            self.sidebar_activity_invalidations =
                self.sidebar_activity_invalidations.saturating_add(1);
        }

        fn record_sidebar_render(&mut self, duration: Duration) {
            self.sidebar_renders.record(duration);
        }

        fn update(
            &mut self,
            now: Instant,
            update: impl FnOnce(&mut MetricsWindow),
        ) -> Option<Summary> {
            let summary = self.take_summary_if_due(now);
            update(self);
            summary
        }

        fn take_summary_if_due(&mut self, now: Instant) -> Option<Summary> {
            let elapsed = now.saturating_duration_since(self.started);
            if elapsed < REPORT_INTERVAL {
                return None;
            }

            let completed = std::mem::replace(self, Self::new(now));
            Some(completed.into_summary(elapsed))
        }

        fn into_summary(self, elapsed: Duration) -> Summary {
            let terminal_paints = self.terminal_paints.summarize();
            let sidebar_renders = self.sidebar_renders.summarize();
            Summary {
                window_ms: duration_ms(elapsed),
                activity_frames: self.activity_frames,
                repaint_terminal_total: self.repaint_terminal_total,
                repaint_terminal_max: self.repaint_terminal_max,
                registered_terminal_total: self.registered_terminal_total,
                registered_terminal_max: self.registered_terminal_max,
                repaint_pane_total: self.repaint_pane_total,
                repaint_pane_max: self.repaint_pane_max,
                terminal_paints,
                terminal_live_viewers_total: self.terminal_live_viewers_total,
                terminal_live_viewers_max: self.terminal_live_viewers_max,
                terminal_grid_cache_hits: self.terminal_grid_cache_hits,
                terminal_grid_cache_misses: self.terminal_grid_cache_misses,
                cells_scanned: self.cells_scanned,
                changed_cell_samples: self.changed_cell_samples,
                cells_changed: self.cells_changed,
                text_runs: self.text_runs,
                background_rects: self.background_rects,
                sidebar_activity_invalidations: self.sidebar_activity_invalidations,
                sidebar_renders,
            }
        }
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    struct Summary {
        window_ms: u64,
        activity_frames: u64,
        repaint_terminal_total: u64,
        repaint_terminal_max: u64,
        registered_terminal_total: u64,
        registered_terminal_max: u64,
        repaint_pane_total: u64,
        repaint_pane_max: u64,
        terminal_paints: TimingSummary,
        terminal_live_viewers_total: u64,
        terminal_live_viewers_max: u64,
        terminal_grid_cache_hits: u64,
        terminal_grid_cache_misses: u64,
        cells_scanned: u64,
        changed_cell_samples: u64,
        cells_changed: u64,
        text_runs: u64,
        background_rects: u64,
        sidebar_activity_invalidations: u64,
        sidebar_renders: TimingSummary,
    }

    static ENABLED: OnceLock<bool> = OnceLock::new();
    static METRICS: OnceLock<Mutex<MetricsWindow>> = OnceLock::new();

    pub fn enabled() -> bool {
        *ENABLED.get_or_init(|| {
            std::env::var("OKENA_RENDER_PERF_PROBE")
                .ok()
                .is_some_and(|value| !matches!(value.as_str(), "" | "0" | "false" | "off"))
        })
    }

    pub fn terminal_activity_frame(
        repaint_terminals: usize,
        registered_terminals: usize,
        repaint_panes: usize,
    ) {
        record(|metrics| {
            metrics.record_activity_frame(repaint_terminals, registered_terminals, repaint_panes);
        });
    }

    pub fn sidebar_activity_invalidation() {
        record(MetricsWindow::record_sidebar_activity_invalidation);
    }

    pub struct TerminalPaintGuard {
        started: Option<Instant>,
    }

    impl TerminalPaintGuard {
        pub fn finish(mut self, stats: TerminalPaintStats) {
            let Some(started) = self.started.take() else {
                return;
            };
            record(|metrics| metrics.record_terminal_paint(started.elapsed(), stats));
        }
    }

    pub fn terminal_paint() -> TerminalPaintGuard {
        TerminalPaintGuard {
            started: enabled().then(Instant::now),
        }
    }

    pub struct SidebarRenderGuard {
        started: Option<Instant>,
    }

    impl Drop for SidebarRenderGuard {
        fn drop(&mut self) {
            let Some(started) = self.started.take() else {
                return;
            };
            record(|metrics| metrics.record_sidebar_render(started.elapsed()));
        }
    }

    pub fn sidebar_render() -> SidebarRenderGuard {
        SidebarRenderGuard {
            started: enabled().then(Instant::now),
        }
    }

    fn record(update: impl FnOnce(&mut MetricsWindow)) {
        if !enabled() {
            return;
        }

        let now = Instant::now();
        let summary = METRICS
            .get_or_init(|| Mutex::new(MetricsWindow::new(now)))
            .lock()
            .ok()
            .and_then(|mut metrics| metrics.update(now, update));

        if let Some(summary) = summary {
            log::info!(target: "okena::render_perf", "{}", format_summary(summary));
        }
    }

    fn format_summary(summary: Summary) -> String {
        format!(
            "render_perf window_ms={} activity_frames={} repaint_terminal_total={} repaint_terminal_max={} registered_terminal_total={} registered_terminal_max={} repaint_pane_total={} repaint_pane_max={} terminal_paints={} terminal_timing_retained={} terminal_paint_us_p50={} terminal_paint_us_p95={} terminal_paint_us_max={} terminal_live_viewers_total={} terminal_live_viewers_max={} terminal_grid_cache_hits={} terminal_grid_cache_misses={} cells_scanned={} changed_cell_samples={} cells_changed={} text_runs={} background_rects={} sidebar_activity_invalidations={} sidebar_renders={} sidebar_timing_retained={} sidebar_render_us_p50={} sidebar_render_us_p95={} sidebar_render_us_max={} terminal_timing_overwritten={} sidebar_timing_overwritten={}",
            summary.window_ms,
            summary.activity_frames,
            summary.repaint_terminal_total,
            summary.repaint_terminal_max,
            summary.registered_terminal_total,
            summary.registered_terminal_max,
            summary.repaint_pane_total,
            summary.repaint_pane_max,
            summary.terminal_paints.count,
            summary.terminal_paints.retained,
            summary.terminal_paints.p50_us,
            summary.terminal_paints.p95_us,
            summary.terminal_paints.max_us,
            summary.terminal_live_viewers_total,
            summary.terminal_live_viewers_max,
            summary.terminal_grid_cache_hits,
            summary.terminal_grid_cache_misses,
            summary.cells_scanned,
            summary.changed_cell_samples,
            summary.cells_changed,
            summary.text_runs,
            summary.background_rects,
            summary.sidebar_activity_invalidations,
            summary.sidebar_renders.count,
            summary.sidebar_renders.retained,
            summary.sidebar_renders.p50_us,
            summary.sidebar_renders.p95_us,
            summary.sidebar_renders.max_us,
            summary.terminal_paints.overwritten,
            summary.sidebar_renders.overwritten,
        )
    }

    fn nearest_rank(sorted: &[u64], percentile: usize) -> u64 {
        if sorted.is_empty() {
            return 0;
        }
        let rank = percentile.saturating_mul(sorted.len()).saturating_add(99) / 100;
        sorted[rank.max(1).saturating_sub(1).min(sorted.len() - 1)]
    }

    fn as_u64(value: usize) -> u64 {
        u64::try_from(value).unwrap_or(u64::MAX)
    }

    fn duration_us(duration: Duration) -> u64 {
        u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
    }

    fn duration_ms(duration: Duration) -> u64 {
        u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn stats(cells_changed: Option<usize>, grid_cache_hit: bool) -> TerminalPaintStats {
            TerminalPaintStats {
                live_viewers: 2,
                grid_cache_hit,
                cells_scanned: 2_000,
                cells_changed,
                text_runs: 40,
                background_rects: 8,
            }
        }

        #[test]
        fn percentile_uses_nearest_rank_for_empty_singleton_odd_and_even_samples() {
            assert_eq!(nearest_rank(&[], 50), 0);
            assert_eq!(nearest_rank(&[7], 95), 7);
            assert_eq!(nearest_rank(&[10, 20, 30], 50), 20);
            assert_eq!(nearest_rank(&[10, 20, 30, 40], 50), 20);
            assert_eq!(nearest_rank(&[10, 20, 30, 40], 95), 40);
        }

        #[test]
        fn summary_accumulates_aggregate_counts_and_timings() {
            let start = Instant::now();
            let mut metrics = MetricsWindow::new(start);
            metrics.record_activity_frame(3, 2, 4);
            metrics.record_activity_frame(5, 4, 7);
            metrics.record_terminal_paint(Duration::from_micros(100), stats(Some(12), false));
            metrics.record_terminal_paint(Duration::from_micros(300), stats(None, true));
            metrics.record_sidebar_activity_invalidation();
            metrics.record_sidebar_render(Duration::from_micros(50));

            let summary = metrics
                .take_summary_if_due(start + REPORT_INTERVAL)
                .expect("summary should be due");

            assert_eq!(summary.activity_frames, 2);
            assert_eq!(summary.repaint_terminal_total, 8);
            assert_eq!(summary.repaint_terminal_max, 5);
            assert_eq!(summary.registered_terminal_total, 6);
            assert_eq!(summary.registered_terminal_max, 4);
            assert_eq!(summary.repaint_pane_total, 11);
            assert_eq!(summary.repaint_pane_max, 7);
            assert_eq!(summary.terminal_paints.count, 2);
            assert_eq!(summary.terminal_live_viewers_total, 4);
            assert_eq!(summary.terminal_live_viewers_max, 2);
            assert_eq!(summary.terminal_grid_cache_hits, 1);
            assert_eq!(summary.terminal_grid_cache_misses, 1);
            assert_eq!(summary.terminal_paints.retained, 2);
            assert_eq!(summary.terminal_paints.p50_us, 100);
            assert_eq!(summary.terminal_paints.p95_us, 300);
            assert_eq!(summary.cells_scanned, 4_000);
            assert_eq!(summary.changed_cell_samples, 1);
            assert_eq!(summary.cells_changed, 12);
            assert_eq!(summary.text_runs, 80);
            assert_eq!(summary.background_rects, 16);
            assert_eq!(summary.sidebar_activity_invalidations, 1);
            assert_eq!(summary.sidebar_renders.count, 1);
            assert_eq!(summary.sidebar_renders.max_us, 50);
        }

        #[test]
        fn timing_storage_is_bounded_and_rolls_forward_to_late_samples() {
            let mut timings = TimingSamples::default();
            for _ in 0..MAX_TIMING_SAMPLES {
                timings.record(Duration::from_micros(10));
            }
            for value in [1_000, 2_000, 3_000] {
                timings.record(Duration::from_micros(value));
            }

            assert!(timings.values_us.contains(&3_000));
            let summary = timings.summarize();
            assert_eq!(summary.count, as_u64(MAX_TIMING_SAMPLES + 3));
            assert_eq!(summary.retained, as_u64(MAX_TIMING_SAMPLES));
            assert_eq!(summary.max_us, 3_000);
            assert_eq!(summary.overwritten, 3);
        }

        #[test]
        fn completed_window_resets_every_counter() {
            let start = Instant::now();
            let mut metrics = MetricsWindow::new(start);
            metrics.record_activity_frame(4, 3, 6);
            let first = metrics
                .take_summary_if_due(start + REPORT_INTERVAL)
                .expect("first summary");
            assert_eq!(first.activity_frames, 1);

            metrics.record_sidebar_activity_invalidation();
            let second = metrics
                .take_summary_if_due(start + REPORT_INTERVAL + REPORT_INTERVAL)
                .expect("second summary");
            assert_eq!(second.activity_frames, 0);
            assert_eq!(second.repaint_terminal_total, 0);
            assert_eq!(second.sidebar_activity_invalidations, 1);
            assert_eq!(second.terminal_paints, TimingSummary::default());
        }

        #[test]
        fn elapsed_window_closes_before_the_next_event_is_recorded() {
            let start = Instant::now();
            let mut metrics = MetricsWindow::new(start);
            metrics.record_activity_frame(2, 1, 2);

            let first = metrics
                .update(start + REPORT_INTERVAL, |next| {
                    next.record_activity_frame(7, 5, 9);
                })
                .expect("elapsed window");
            assert_eq!(first.activity_frames, 1);
            assert_eq!(first.repaint_terminal_total, 2);

            let second = metrics
                .take_summary_if_due(start + REPORT_INTERVAL + REPORT_INTERVAL)
                .expect("new window");
            assert_eq!(second.activity_frames, 1);
            assert_eq!(second.repaint_terminal_total, 7);
        }

        #[test]
        fn formatted_summary_has_only_the_fixed_aggregate_schema() {
            let line = format_summary(Summary::default());
            for prohibited in [
                " terminal=",
                " viewer=",
                " window=",
                " path=",
                " command=",
                " content=",
                " id=",
            ] {
                assert!(!line.contains(prohibited), "found {prohibited} in {line}");
            }
            let fields: Vec<_> = line
                .split_whitespace()
                .skip(1)
                .map(|field| field.split('=').next().unwrap_or_default())
                .collect();
            assert_eq!(
                fields,
                [
                    "window_ms",
                    "activity_frames",
                    "repaint_terminal_total",
                    "repaint_terminal_max",
                    "registered_terminal_total",
                    "registered_terminal_max",
                    "repaint_pane_total",
                    "repaint_pane_max",
                    "terminal_paints",
                    "terminal_timing_retained",
                    "terminal_paint_us_p50",
                    "terminal_paint_us_p95",
                    "terminal_paint_us_max",
                    "terminal_live_viewers_total",
                    "terminal_live_viewers_max",
                    "terminal_grid_cache_hits",
                    "terminal_grid_cache_misses",
                    "cells_scanned",
                    "changed_cell_samples",
                    "cells_changed",
                    "text_runs",
                    "background_rects",
                    "sidebar_activity_invalidations",
                    "sidebar_renders",
                    "sidebar_timing_retained",
                    "sidebar_render_us_p50",
                    "sidebar_render_us_p95",
                    "sidebar_render_us_max",
                    "terminal_timing_overwritten",
                    "sidebar_timing_overwritten",
                ]
            );
        }
    }
}

#[cfg(not(feature = "render-perf-probe"))]
mod imp {
    use super::TerminalPaintStats;

    #[inline(always)]
    pub fn enabled() -> bool {
        false
    }

    #[inline(always)]
    pub fn terminal_activity_frame(
        _repaint_terminals: usize,
        _registered_terminals: usize,
        _repaint_panes: usize,
    ) {
    }

    #[inline(always)]
    pub fn sidebar_activity_invalidation() {}

    pub struct TerminalPaintGuard;

    impl TerminalPaintGuard {
        #[inline(always)]
        pub fn finish(self, _stats: TerminalPaintStats) {}
    }

    #[inline(always)]
    pub fn terminal_paint() -> TerminalPaintGuard {
        TerminalPaintGuard
    }

    pub struct SidebarRenderGuard;

    #[inline(always)]
    pub fn sidebar_render() -> SidebarRenderGuard {
        SidebarRenderGuard
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn feature_off_api_is_a_noop() {
            assert!(!enabled());
            terminal_activity_frame(3, 2, 4);
            sidebar_activity_invalidation();
            terminal_paint().finish(TerminalPaintStats::default());
            drop(sidebar_render());
        }
    }
}

pub use imp::*;
