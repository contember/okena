//! Client-side soft-close + restart-daemon toast-action ids.
//!
//! The grace-period "soft close" orchestration now lives entirely on the daemon
//! (it owns the PTYs and the authoritative layout): the daemon ejects a busy
//! terminal, keeps its PTY alive for the grace period, builds the Undo /
//! Close-now toast, and finalizes on expiry. See `okena_daemon_core::soft_close`
//! and the daemon command loop. The client just routes a clicked toast button
//! back to the daemon (`WindowView::handle_toast_action`).
//!
//! What remains here is purely the client-side id vocabulary:
//!   * the soft-close toast-action prefixes + decoder, re-exported from
//!     `okena-core` so the daemon (which builds the toast) and this client
//!     (which decodes a clicked button) share one source of truth, and
//!   * the restart-daemon confirm-toast action ids (a local, non-soft-close
//!     toast), kept here so all toast-action ids live in one place.

// Re-exported under the historical local names so existing callers
// (`crate::soft_close::{UNDO_PREFIX, KILL_PREFIX, decode_action}`) keep working.
pub use okena_core::soft_close::{
    decode_action, SOFT_CLOSE_KILL_PREFIX as KILL_PREFIX, SOFT_CLOSE_UNDO_PREFIX as UNDO_PREFIX,
};

/// Stable id for the "Restart the daemon?" confirmation toast (so it can be
/// dismissed by id once the user picks an action).
pub const RESTART_DAEMON_TOAST_ID: &str = "restart-daemon-confirm";
/// Action id for the "Restart" button on the restart-daemon confirm toast.
pub const RESTART_DAEMON_CONFIRM_PREFIX: &str = "restart_daemon_confirm";
/// Action id for the "Cancel" button on the restart-daemon confirm toast.
pub const RESTART_DAEMON_CANCEL_PREFIX: &str = "restart_daemon_cancel";
