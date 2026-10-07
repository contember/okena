//! Per-profile Claude account proxy. Runs as `okena --ai-proxy <profile-root>`
//! (or `okena-daemon --ai-proxy …`), outliving the workspace daemon so that
//! persistent `claude` sessions keep their `ANTHROPIC_BASE_URL` working.
//! See `crates/okena-ai/CLAUDE.md`.

#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

mod accounts;
pub mod client;
mod oauth;
mod proxy;
mod server;

pub use server::run;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Flag that selects the proxy in the `okena` and `okena-daemon` binaries.
pub const PROXY_FLAG: &str = "--ai-proxy";

/// `Some(profile_root)` when the process was started as the proxy.
pub fn proxy_args(args: &[String]) -> Option<std::path::PathBuf> {
    match args {
        [_, flag, root] if flag == PROXY_FLAG => Some(root.into()),
        _ => None,
    }
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}
