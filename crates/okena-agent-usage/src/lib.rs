#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

//! GPUI-free agent usage core.
//!
//! Everything provider-related that used to live in the Claude and Codex
//! desktop extensions: credential reading (credentials file / macOS Keychain /
//! `~/.codex/auth.json`), Codex token refresh, the blocking HTTP fetches,
//! parsing into the [`okena_core::api::ApiUsage`] wire model, the retry and
//! backoff policy ([`provider`]), and history recording ([`history`]).
//! [`pace`] holds the pure pace/severity/working-day math the desktop widgets
//! render with.
//!
//! The daemon (`okena-daemon-core::usage_poll`) drives this on a timer and
//! serves the result over the remote API; nothing here spawns threads or
//! talks to gpui. All fetch functions block — call them from a blocking pool.

pub mod claude;
pub mod codex;
pub mod history;
pub mod pace;
pub mod pool;
pub mod pool_summary;
pub mod provider;

use std::path::Path;

use okena_core::api::{ApiUsage, ApiUsageProvider};

pub use provider::{FetchOutcome, ProviderData, ProviderTracker};

/// Extension id whose enablement turns Claude usage polling on.
pub const CLAUDE_EXTENSION_ID: &str = "claude-code";
/// Extension id whose enablement turns Codex usage polling on.
pub const CODEX_EXTENSION_ID: &str = "codex";

/// Providers in wire order.
pub const PROVIDERS: [ApiUsageProvider; 2] = [ApiUsageProvider::Claude, ApiUsageProvider::Codex];

/// Fetch Claude usage for `claude_dir` and record/attach history.
pub fn fetch_claude(claude_dir: &Path) -> FetchOutcome {
    let mut outcome = claude::fetch(claude_dir);
    if let FetchOutcome::Ok(data) = &mut outcome {
        history::record_windows(
            history::Provider::Claude,
            claude_dir.to_string_lossy().into_owned(),
            data.plan.clone(),
            &mut data.windows,
        );
    }
    outcome
}

/// Fetch Codex usage and record/attach history.
pub fn fetch_codex() -> FetchOutcome {
    let (mut outcome, account_id) = codex::fetch();
    if let (FetchOutcome::Ok(data), Some(account_id)) = (&mut outcome, account_id) {
        history::record_windows(
            history::Provider::Codex,
            account_id,
            data.plan.clone(),
            &mut data.windows,
        );
    }
    outcome
}

/// Assemble the wire snapshot from the trackers (in [`PROVIDERS`] order).
pub fn snapshot<'a>(
    trackers: impl IntoIterator<Item = &'a ProviderTracker>,
    working_days: pace::WorkingDays,
    now_epoch: f64,
) -> ApiUsage {
    ApiUsage {
        providers: trackers
            .into_iter()
            .map(|tracker| tracker.entry().clone())
            .collect(),
        working_days: working_days.to_indices(),
        generated_at: now_epoch,
    }
}

/// Current Unix time in seconds.
pub fn now_epoch() -> f64 {
    pace::now_secs().unwrap_or(0.0)
}
