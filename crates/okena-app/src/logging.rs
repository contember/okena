//! In-app log capture + the runtime-reloadable capture filter behind the log
//! console (`ShowLogConsole`).
//!
//! Why a custom logger instead of plain `env_logger`: `env_logger`'s level
//! filter is baked at `init()` and cannot change at runtime, and it has no
//! in-memory sink a UI can read. So we wrap it:
//!
//! - The original `env_logger::Logger` stays as the **file/stderr sink** with
//!   its startup `RUST_LOG` filter — existing behavior is unchanged.
//! - A second **capture sink** pushes records into an in-memory ring buffer,
//!   gated by an `ArcSwap`-held [`env_filter::Filter`] that the log console can
//!   replace at runtime (lock-free on the logging path).
//!
//! The global max level is forced to `Trace` so capture can be escalated to
//! `debug`/`trace` for any target without a restart; each sink then applies its
//! own filter. (`log` is built without `release_max_level_*`, so trace/debug
//! are not compiled out in release.)

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::sync::Arc;

use arc_swap::ArcSwap;
use env_filter::Filter;

/// Default capture filter: everything at `info`, plus our own `okena*` crates
/// (including the `okena::cmd` command-bus target) at `debug`. Keeps the ring
/// free of `gpui`/`wgpu`/`winit` trace noise while still showing app debug;
/// the console can escalate specific targets (e.g. `okena::cmd=trace`).
pub const DEFAULT_CAPTURE: &str = "info,okena=debug";

/// Ring buffer capacity. At ~200 bytes/line this caps the console at a few MB.
const RING_CAPACITY: usize = 10_000;

/// One captured log record, cheap to clone for snapshots.
#[derive(Clone)]
pub struct LogLine {
    /// Monotonic sequence number; lets the console fetch only what's new.
    pub seq: u64,
    pub timestamp: time::OffsetDateTime,
    pub level: log::Level,
    pub target: String,
    pub message: String,
}

struct Ring {
    buf: VecDeque<LogLine>,
    next_seq: u64,
}

/// Process-global capture state, shared by the logger (writer) and the log
/// console view (reader).
pub struct LogHub {
    filter: ArcSwap<Filter>,
    /// The directive string currently in effect, for display/edit in the UI.
    directives: Mutex<String>,
    ring: Mutex<Ring>,
}

impl LogHub {
    fn new(directives: &str) -> Self {
        Self {
            filter: ArcSwap::from_pointee(build_filter(directives)),
            directives: Mutex::new(directives.to_string()),
            ring: Mutex::new(Ring {
                buf: VecDeque::with_capacity(1024),
                next_seq: 0,
            }),
        }
    }

    /// Cheap pre-check used by `Log::enabled` so callers can short-circuit.
    fn capture_enabled(&self, metadata: &log::Metadata) -> bool {
        self.filter.load().enabled(metadata)
    }

    /// Append a record to the ring if it passes the capture filter.
    fn capture(&self, record: &log::Record) {
        if !self.filter.load().matches(record) {
            return;
        }
        let timestamp =
            time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
        let Ok(mut ring) = self.ring.lock() else {
            return;
        };
        let seq = ring.next_seq;
        ring.next_seq += 1;
        ring.buf.push_back(LogLine {
            seq,
            timestamp,
            level: record.level(),
            target: record.target().to_string(),
            message: format!("{}", record.args()),
        });
        if ring.buf.len() > RING_CAPACITY {
            ring.buf.pop_front();
        }
    }

    /// Replace the capture filter at runtime (RUST_LOG syntax, e.g.
    /// `info,okena::cmd=trace`). Lock-free for concurrent loggers.
    pub fn set_capture_filter(&self, directives: &str) {
        self.filter.store(Arc::new(build_filter(directives)));
        if let Ok(mut d) = self.directives.lock() {
            *d = directives.to_string();
        }
        // Surface the change in the log itself so the console shows it landed.
        log::info!(target: "okena::log", "capture filter set to `{directives}`");
    }

    /// The directive string currently in effect.
    pub fn directives(&self) -> String {
        self.directives.lock().map(|d| d.clone()).unwrap_or_default()
    }

    /// Clone every buffered line with `seq >= since` (use `0` for the full
    /// backlog). The console keeps the last seen seq and polls for the tail.
    pub fn snapshot_since(&self, since: u64) -> Vec<LogLine> {
        let Ok(ring) = self.ring.lock() else {
            return Vec::new();
        };
        ring.buf.iter().filter(|l| l.seq >= since).cloned().collect()
    }

    /// Seq the next captured line will get — i.e. one past the newest line.
    pub fn next_seq(&self) -> u64 {
        self.ring.lock().map(|r| r.next_seq).unwrap_or(0)
    }

    /// Drop all buffered lines (console "clear"). Does not reset `seq`.
    pub fn clear(&self) {
        if let Ok(mut ring) = self.ring.lock() {
            ring.buf.clear();
        }
    }
}

fn build_filter(directives: &str) -> Filter {
    env_filter::Builder::new().parse(directives).build()
}

/// The wrapping logger: file/stderr via the inner `env_logger`, plus the
/// in-memory capture sink.
struct OkenaLogger {
    inner: env_logger::Logger,
    hub: &'static LogHub,
}

impl log::Log for OkenaLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.inner.enabled(metadata) || self.hub.capture_enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        // Inner self-filters to its startup level (file/stderr unchanged).
        self.inner.log(record);
        // Capture self-filters to the runtime capture filter.
        self.hub.capture(record);
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

static LOG_HUB: OnceLock<LogHub> = OnceLock::new();

/// The global log hub, if logging has been initialized. The log console reads
/// from this; `None` before `init` (e.g. in unrelated unit tests).
pub fn hub() -> Option<&'static LogHub> {
    LOG_HUB.get()
}

/// Install the wrapping logger. `inner` is the fully-configured
/// `env_logger::Logger` (target + RUST_LOG filter) that `main` would otherwise
/// have `.init()`-ed. Sets the global max level to `Trace` so the capture
/// filter can be escalated at runtime.
pub fn init(inner: env_logger::Logger) {
    let hub = LOG_HUB.get_or_init(|| LogHub::new(DEFAULT_CAPTURE));
    log::set_max_level(log::LevelFilter::Trace);
    if log::set_boxed_logger(Box::new(OkenaLogger { inner, hub })).is_err() {
        // A logger was already installed (e.g. a test harness). Leave it.
        eprintln!("okena: a global logger was already set; in-app log capture disabled");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(hub: &LogHub, level: log::Level, target: &str, msg: &str) {
        hub.capture(
            &log::Record::builder()
                .level(level)
                .target(target)
                .args(format_args!("{msg}"))
                .build(),
        );
    }

    #[test]
    fn default_filter_captures_okena_debug_drops_extern_debug() {
        let hub = LogHub::new(DEFAULT_CAPTURE);
        rec(&hub, log::Level::Debug, "okena::cmd", "bus debug");
        rec(&hub, log::Level::Debug, "wgpu_core::device", "noise");
        rec(&hub, log::Level::Info, "wgpu_core::device", "kept at info");
        let lines = hub.snapshot_since(0);
        let msgs: Vec<&str> = lines.iter().map(|l| l.message.as_str()).collect();
        assert_eq!(msgs, vec!["bus debug", "kept at info"]);
    }

    #[test]
    fn runtime_filter_escalation_changes_what_is_captured() {
        let hub = LogHub::new("off");
        rec(&hub, log::Level::Trace, "okena::cmd", "before");
        assert!(hub.snapshot_since(0).is_empty());

        hub.set_capture_filter("okena::cmd=trace");
        rec(&hub, log::Level::Trace, "okena::cmd", "after");
        // The set_capture_filter info line is on target okena::log, which the
        // `okena::cmd=trace` directive does not admit, so only "after" lands.
        let msgs: Vec<String> = hub.snapshot_since(0).into_iter().map(|l| l.message).collect();
        assert!(msgs.contains(&"after".to_string()));
        assert!(!msgs.contains(&"before".to_string()));
    }

    #[test]
    fn snapshot_since_returns_only_new_lines() {
        let hub = LogHub::new("trace");
        rec(&hub, log::Level::Info, "okena", "a");
        rec(&hub, log::Level::Info, "okena", "b");
        let cursor = hub.next_seq();
        rec(&hub, log::Level::Info, "okena", "c");
        let new: Vec<String> = hub.snapshot_since(cursor).into_iter().map(|l| l.message).collect();
        assert_eq!(new, vec!["c"]);
    }

    #[test]
    fn ring_caps_capacity_and_preserves_seq() {
        let hub = LogHub::new("trace");
        for i in 0..(RING_CAPACITY + 50) {
            rec(&hub, log::Level::Info, "okena", &format!("line {i}"));
        }
        let lines = hub.snapshot_since(0);
        assert_eq!(lines.len(), RING_CAPACITY);
        // Oldest 50 were evicted; seq keeps climbing (no reset).
        assert_eq!(lines.first().unwrap().seq, 50);
        assert_eq!(lines.last().unwrap().seq, (RING_CAPACITY + 50 - 1) as u64);
    }

    #[test]
    fn clear_empties_buffer_without_resetting_seq() {
        let hub = LogHub::new("trace");
        rec(&hub, log::Level::Info, "okena", "x");
        let seq_before = hub.next_seq();
        hub.clear();
        assert!(hub.snapshot_since(0).is_empty());
        assert_eq!(hub.next_seq(), seq_before);
    }
}
