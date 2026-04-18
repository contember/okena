use std::sync::atomic::{AtomicU64, Ordering};

/// Process-global resize authority. "Last to interact wins" across all terminals
/// in this process: whichever side most recently typed or clicked gets to drive
/// resize for every terminal. No time-based reclaim — the origin side takes over
/// by actually interacting, not by waiting.
///
/// Implemented with a monotonically-increasing sequence counter to avoid
/// timestamp collisions. Each claim bumps the counter and records the new value
/// on the claiming side. Higher value wins. Both zero (initial) resolves to
/// Local, so terminals behave normally before any interaction happens.
static RESIZE_AUTH_SEQ: AtomicU64 = AtomicU64::new(0);
static LAST_LOCAL_SEQ: AtomicU64 = AtomicU64::new(0);
static LAST_REMOTE_SEQ: AtomicU64 = AtomicU64::new(0);

pub fn claim_resize_authority_local() {
    let seq = RESIZE_AUTH_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    LAST_LOCAL_SEQ.store(seq, Ordering::Relaxed);
}

pub fn claim_resize_authority_remote() {
    let seq = RESIZE_AUTH_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    LAST_REMOTE_SEQ.store(seq, Ordering::Relaxed);
}

pub fn is_resize_authority_local() -> bool {
    LAST_LOCAL_SEQ.load(Ordering::Relaxed) >= LAST_REMOTE_SEQ.load(Ordering::Relaxed)
}

#[cfg(test)]
pub(super) fn reset_resize_authority() {
    RESIZE_AUTH_SEQ.store(0, Ordering::Relaxed);
    LAST_LOCAL_SEQ.store(0, Ordering::Relaxed);
    LAST_REMOTE_SEQ.store(0, Ordering::Relaxed);
}
