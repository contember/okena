pub use okena_workspace::{hook_monitor, hooks, persistence, settings, state, toast};

// request_broker / requests / worktree_sync are gpui-gated in okena-workspace,
// so re-export them only when the gpui feature is enabled.
#[cfg(feature = "gpui")]
pub use okena_workspace::{request_broker, requests, worktree_sync};

// focus is re-exported implicitly (no types used directly from main app)
// sessions is re-exported implicitly (accessed through persistence re-exports)
#[allow(unused_imports)]
pub use okena_workspace::{focus, sessions};

pub mod actions;
