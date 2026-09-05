#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

// GPUI-only: this is shared UI state (an Entity behind a global). The daemon
// builds this crate without gpui and has no use for it.
pub mod access_history;
pub mod actions;
pub mod claude_env;
pub mod context;
pub mod focus;
#[cfg(feature = "gpui")]
pub mod harness_state;
pub mod hook_monitor;
pub mod hooks;
pub mod lifecycle;
pub mod persistence;
pub mod remote_apply;
pub mod remote_sync;
#[cfg(feature = "gpui")]
pub mod request_broker;
#[cfg(feature = "gpui")]
pub mod requests;
pub mod sessions;
pub mod settings;
pub mod sidebar_controller;
pub mod state;
pub mod toast;
pub mod visibility;
#[cfg(feature = "gpui")]
pub mod worktree_sync;
