//! Opt-in render/refresh-rate diagnostics.
//!
//! Set `OKENA_RENDER_STATS=1` to log, once per second, how many times each
//! labelled hot path ran. Used to diagnose idle CPU: e.g. is the terminal grid
//! repainting because of actual PTY output (`pty_output` high) or because some
//! background poller keeps invalidating the window (`window_render` high while
//! `pty_output` ≈ 0)?
//!
//! Zero cost when disabled: a single cached bool check per call.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static ENABLED: OnceLock<bool> = OnceLock::new();
static STATS: Mutex<Option<HashMap<&'static str, (u64, Instant)>>> = Mutex::new(None);

fn enabled() -> bool {
    *ENABLED.get_or_init(|| std::env::var_os("OKENA_RENDER_STATS").is_some())
}

/// Count one occurrence of `label`. Once per second per label, log the count
/// accumulated in that window as `<label>: N/s`. No-op unless
/// `OKENA_RENDER_STATS` is set.
pub fn tick(label: &'static str) {
    if !enabled() {
        return;
    }
    let now = Instant::now();
    let Ok(mut guard) = STATS.lock() else { return };
    let map = guard.get_or_insert_with(HashMap::new);
    let entry = map.entry(label).or_insert((0, now));
    entry.0 += 1;
    if now.duration_since(entry.1).as_secs() >= 1 {
        log::info!("[render-stats] {}: {}/s", label, entry.0);
        entry.0 = 0;
        entry.1 = now;
    }
}
