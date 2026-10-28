//! Wire types for agent usage (Claude Code / Codex / Grok rate limits, and the
//! Claude account pool of the AI proxy).
//!
//! The daemon polls each enabled provider and publishes one [`ApiUsage`]
//! snapshot. Clients get it from `GET /v1/usage`, from the
//! `POST /v1/usage/refresh` response, and as the `usage_changed` WebSocket
//! message (sent once right after auth and again on every change).
//!
//! All timestamps are Unix epoch **seconds** (`f64`, UTC) on the daemon's
//! clock. Percentages are `0.0..=100.0` (a provider can report slightly more
//! than 100 when over the cap; clamp when drawing).
//!
//! ## Pace on the client
//!
//! A window's linear "time elapsed" fraction is derived from `reset_at` and
//! `window_seconds`:
//!
//! ```text
//! start        = reset_at - window_seconds
//! time_pct     = clamp((now - start) / window_seconds * 100, 0, 100)
//! ```
//!
//! A window with `window_seconds == 0` or no `reset_at` has no pace.
//!
//! Desktop severity uses `max(abs, pace)`: abs is Critical above 80 %,
//! Warning above 60 %; pace is Critical when `used_percent > time_pct + 15`,
//! Warning when `> time_pct + 5`. For ~weekly windows (`window_seconds` within
//! 2..=8 days) the desktop reshapes the bar to one block per working day in
//! [`ApiUsage::working_days`] and advances `time_pct` across working days only;
//! clients that want identical numbers implement the same reshaping (see
//! `okena-agent-usage::pace`), others can use the linear value.

use serde::{Deserialize, Serialize};

/// Snapshot of every usage provider the daemon knows about.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ApiUsage {
    /// One entry per provider, always present and in a stable order
    /// (`claude`, `codex`, then `grok`). A provider the user has not enabled has
    /// `status: "disabled"` and no windows.
    #[serde(default)]
    pub providers: Vec<ApiProviderUsage>,
    /// Days of the week the user works, `0 = Monday .. 6 = Sunday`, ascending.
    /// Never empty; all seven days means "no tailoring". This is the same
    /// preference the desktop uses for its weekly pace marker.
    #[serde(default = "all_working_days")]
    pub working_days: Vec<u8>,
    /// When the daemon published this snapshot's latest change (epoch
    /// seconds); never earlier than a snapshot it replaces. Compare with the
    /// client clock to detect skew before computing pace.
    #[serde(default)]
    pub generated_at: f64,
}

fn all_working_days() -> Vec<u8> {
    (0..7).collect()
}

impl ApiUsage {
    /// The entry for one provider, if the daemon reported it.
    pub fn provider(&self, provider: ApiUsageProvider) -> Option<&ApiProviderUsage> {
        self.providers
            .iter()
            .find(|entry| entry.provider == provider)
    }
}

/// Which agent a usage entry belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiUsageProvider {
    Claude,
    Codex,
    Grok,
    /// A provider added by a newer daemon.
    #[serde(other)]
    Unknown,
}

impl ApiUsageProvider {
    /// Wire name (`"claude"` / `"codex"` / `"grok"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Grok => "grok",
            Self::Unknown => "unknown",
        }
    }
}

/// Fetch state of a provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiUsageStatus {
    /// The provider's extension is turned off in settings; nothing is fetched.
    Disabled,
    /// No usable credentials on the daemon's machine (not signed in, token
    /// expired, or the Keychain refused). `error` explains which.
    NoCredentials,
    /// Enabled, but the first fetch has not finished yet.
    Pending,
    /// The last fetch succeeded.
    Ok,
    /// The last fetch failed. The windows (if any) are from the last success,
    /// see `fetched_at`; `error` says what went wrong.
    Error,
    /// A status added by a newer daemon.
    #[serde(other)]
    Unknown,
}

/// Usage of one provider on the daemon's machine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiProviderUsage {
    pub provider: ApiUsageProvider,
    pub status: ApiUsageStatus,
    /// Subscription plan label as the provider reports it (e.g. `"max"`,
    /// `"pro"`, `"plus"`), unformatted.
    #[serde(default)]
    pub plan: Option<String>,
    /// Last successful fetch (epoch seconds). The windows below are from it.
    #[serde(default)]
    pub fetched_at: Option<f64>,
    /// Start of the most recent fetch attempt, successful or not.
    #[serde(default)]
    pub last_attempt_at: Option<f64>,
    /// When the daemon plans the next attempt.
    #[serde(default)]
    pub next_attempt_at: Option<f64>,
    /// A fetch is in flight right now.
    #[serde(default)]
    pub fetching: bool,
    /// Human-readable reason for `error` / `no_credentials`. Never contains a
    /// token.
    #[serde(default)]
    pub error: Option<String>,
    /// Failed attempts since the last success.
    #[serde(default)]
    pub consecutive_failures: u32,
    /// The provider answered 429; no fetch (and no refresh) before this time.
    #[serde(default)]
    pub rate_limited_until: Option<f64>,
    /// Rate-limit windows, in display order.
    #[serde(default)]
    pub windows: Vec<ApiUsageWindow>,
    /// Claude only: paid usage beyond the plan.
    #[serde(default)]
    pub extra_usage: Option<ApiExtraUsage>,
    /// Codex only: credit balance.
    #[serde(default)]
    pub credits: Option<ApiUsageCredits>,
    /// Codex only: rate-limit reset credits the user can redeem.
    #[serde(default)]
    pub reset_credits: Option<ApiResetCredits>,
    /// Grok only: prepaid credit balance, unformatted, in the provider's
    /// credit units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepaid_balance: Option<f64>,
    /// Claude only: the AI proxy's account pool, present while the host routes
    /// new shells through the proxy, the proxy answers and has accounts, and
    /// Claude usage is not disabled. Those shells then use the pool, not the
    /// `claude` login, so clients show the pool instead of `windows`. Daemons
    /// older than the pool never send it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<ApiPool>,
}

impl ApiProviderUsage {
    /// An entry with no data yet.
    pub fn empty(provider: ApiUsageProvider, status: ApiUsageStatus) -> Self {
        Self {
            provider,
            status,
            plan: None,
            fetched_at: None,
            last_attempt_at: None,
            next_attempt_at: None,
            fetching: false,
            error: None,
            consecutive_failures: 0,
            rate_limited_until: None,
            windows: Vec::new(),
            extra_usage: None,
            credits: None,
            reset_credits: None,
            prepaid_balance: None,
            pool: None,
        }
    }

    /// The window with this id, if present.
    pub fn window(&self, id: &str) -> Option<&ApiUsageWindow> {
        self.windows.iter().find(|window| window.id == id)
    }
}

/// One rate-limit window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiUsageWindow {
    /// Stable key, also the history series name: Claude `five_hour`,
    /// `seven_day`, `seven_day_<model lowercase>`; Codex `primary`,
    /// `secondary`, `code_review`; Grok `included`, `on_demand`.
    pub id: String,
    /// Display label the desktop uses (`Session`, `Weekly`, `Fable`,
    /// `Rate Limit`, `Secondary`, `Code Review`, `Credits`, `On-demand`).
    pub label: String,
    pub kind: ApiUsageWindowKind,
    /// Utilization, 0–100.
    pub used_percent: f64,
    /// Length of the window in seconds (18000 for 5 h, 604800 for 7 d);
    /// `0` when the provider did not say (Grok without a billing period).
    pub window_seconds: f64,
    /// When the window resets (epoch seconds). `None` when the provider did
    /// not say; pace cannot be computed then.
    #[serde(default)]
    pub reset_at: Option<f64>,
    /// Claude model-scoped limits: the provider marks this limit as the one
    /// currently binding. The desktop shows a scoped row when `active` or
    /// `used_percent >= 0.5`, and in the status bar when `active` or `>= 80`.
    /// Always `true` for unscoped windows.
    #[serde(default = "default_true")]
    pub active: bool,
    /// Recorded samples inside the current window (from `reset_at -
    /// window_seconds` until now), oldest first, at most
    /// [`USAGE_HISTORY_MAX_POINTS`]. Plateaus are collapsed to their first and
    /// last sample, so a straight line between neighbours is exact.
    #[serde(default)]
    pub history: Vec<ApiUsagePoint>,
}

fn default_true() -> bool {
    true
}

/// Upper bound on [`ApiUsageWindow::history`] length.
pub const USAGE_HISTORY_MAX_POINTS: usize = 400;

/// What a window measures.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApiUsageWindowKind {
    /// The rolling ~5 hour session limit.
    FiveHour,
    /// The weekly limit across all models.
    Weekly,
    /// A weekly limit scoped to one model (or surface), e.g. `{"type":
    /// "weekly_scoped", "model": "Fable"}`.
    WeeklyScoped { model: String },
    /// Codex code-review limit.
    CodeReview,
    /// Any other window length, or a kind added by a newer daemon.
    #[serde(other)]
    Other,
}

/// One history sample.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiUsagePoint {
    /// Epoch seconds.
    pub recorded_at: f64,
    pub used_percent: f64,
}

/// Claude paid usage beyond the plan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiExtraUsage {
    pub is_enabled: bool,
    /// Monthly spend limit in cents (USD minor units).
    pub monthly_limit_cents: f64,
    /// Spent this month, in cents.
    pub used_credits_cents: f64,
    /// `used / limit`, 0–100.
    pub utilization: f64,
}

/// Codex credit balance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiUsageCredits {
    pub has_credits: bool,
    pub unlimited: bool,
    /// Balance in USD.
    pub balance: f64,
}

/// Codex rate-limit reset credits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiResetCredits {
    /// How many resets are available. Can exceed `credits.len()` when the
    /// provider lists fewer details than it counts.
    pub available_count: u64,
    /// Available credits, soonest expiry first, then those without expiry.
    #[serde(default)]
    pub credits: Vec<ApiResetCredit>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiResetCredit {
    pub title: String,
    /// Epoch seconds; `None` = no expiration.
    #[serde(default)]
    pub expires_at: Option<f64>,
}

/// The AI proxy's account pool on one timeline (see
/// `okena-agent-usage::pool`). Every account has its own 5 h and 7 d windows,
/// so the pool is summarized as work time instead of percentages.
///
/// Quantities are weekly quota weighted by plan size (Pro 1, Max 5x 5,
/// Max 20x 20) and given two ways, see [`ApiPoolAmount`]. Days follow the
/// daemon's time zone and [`ApiUsage::working_days`].
///
/// Desktop severity: Today is Critical when `today.left` is negative and
/// Warning below one hour of work; Nonstop is Critical when blocked or below
/// one hour, Warning below three hours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiPool {
    pub today: ApiPoolToday,
    pub nonstop: ApiPoolNonstop,
    pub pace: ApiPoolPace,
    /// Every pool account, in pool order. Only accounts that serve requests
    /// (enabled and logged in) count in the numbers above; the others have
    /// `blocked` set. Daemons before that listed only the serving ones.
    #[serde(default)]
    pub accounts: Vec<ApiPoolAccount>,
}

/// Weighted weekly quota, as work and as a share of a week.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiPoolAmount {
    /// Work time at the pool's pace ([`ApiPoolPace`]), in seconds.
    pub work_seconds: f64,
    /// Percent of one average pool account's week.
    pub account_week_percent: f64,
}

/// The soft limit: what today may use so every working day until the resets
/// still has quota.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiPoolToday {
    /// `safe_per_day - used`; negative when today already used more.
    pub left: ApiPoolAmount,
    /// Used since the daemon's local midnight.
    pub used: ApiPoolAmount,
    /// The most each working day may use without running short.
    pub safe_per_day: ApiPoolAmount,
    /// What each non-working day uses, as last week. Clients show it only
    /// when [`ApiUsage::working_days`] is not every day and it is above zero.
    pub off_day: ApiPoolAmount,
    /// Using more than `safe_per_day` makes the pool run short at this time
    /// (epoch seconds). `None`: more only takes from what the pool refills
    /// for later weeks.
    #[serde(default)]
    pub runs_short_at: Option<f64>,
    /// Label of the account whose weekly reset next follows `runs_short_at`.
    #[serde(default)]
    pub runs_short_before: Option<String>,
    /// At the last 24 hours' pace the pool runs out at this time (epoch
    /// seconds). `None`: it lasts the week.
    #[serde(default)]
    pub runs_out_at_recent_pace: Option<f64>,
}

/// The hard limit: how long nonstop work goes on before every account is
/// blocked by its 5 h or weekly limit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApiPoolNonstop {
    /// Every account is blocked after this much work, in seconds.
    Lasts { work_seconds: f64 },
    /// No account is blocked within the simulated horizon (seconds).
    BeyondHorizon { horizon_seconds: f64 },
    /// Every account is blocked now; the first is back at `until` (epoch
    /// seconds), if known.
    Blocked {
        #[serde(default)]
        until: Option<f64>,
    },
    /// A state added by a newer daemon.
    #[serde(other)]
    Unknown,
}

/// The pace that converts quota into work time.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiPoolPace {
    /// Work time that fills one 5 h window of an average account, in seconds.
    pub session_fill_seconds: f64,
    /// Percent of a week one full 5 h window costs.
    pub session_week_percent: f64,
    /// Measured from the last 7 days of history; `false` means defaults.
    pub from_history: bool,
}

/// One pool account.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiPoolAccount {
    /// The proxy's account id (`AiAccount::id`).
    pub id: String,
    pub label: String,
    /// Plan size label (`"Max 20x"`, `"Max 5x"`); `None` for the base plan.
    #[serde(default)]
    pub tier: Option<String>,
    /// Why the proxy skips the account now (`"Needs login"`, `"Disabled"`,
    /// `"7d limit"`, `"5h limit"`, `"429 cooldown"`); `None` while it serves
    /// requests.
    #[serde(default)]
    pub blocked: Option<String>,
    /// When `blocked` ends (epoch seconds): the exhausted window's reset or
    /// the end of the 429 cooldown. `None` when unknown or not timed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_until: Option<f64>,
    /// Weekly percent of this account used since the daemon's local midnight.
    pub used_today_percent: f64,
    /// The `five_hour` and `seven_day` windows, shaped like provider windows
    /// but without `history`: the pool is republished whenever an account's
    /// utilization moves, and clients draw only the bars. The history is
    /// served on request, see [`ApiPoolAccountHistory`].
    #[serde(default)]
    pub windows: Vec<ApiUsageWindow>,
}

/// `GET /v1/usage/pool/{account_id}`: one pool account's windows with their
/// `history`, from the usage history the daemon loads for the pool (at most
/// a minute old). An account that does not serve requests has no history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiPoolAccountHistory {
    pub account_id: String,
    /// The account's `five_hour` and `seven_day` windows, as in
    /// [`ApiPoolAccount::windows`] but with `history` filled.
    #[serde(default)]
    pub windows: Vec<ApiUsageWindow>,
}

/// `POST /v1/usage/refresh` body. An empty body (or `{}`) refreshes every
/// enabled provider.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageRefreshRequest {
    #[serde(default)]
    pub provider: Option<ApiUsageProvider>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_kind_wire_shape() {
        let scoped = ApiUsageWindowKind::WeeklyScoped {
            model: "Fable".into(),
        };
        assert_eq!(
            serde_json::to_value(&scoped).unwrap(),
            serde_json::json!({"type": "weekly_scoped", "model": "Fable"})
        );
        assert_eq!(
            serde_json::to_value(ApiUsageWindowKind::FiveHour).unwrap(),
            serde_json::json!({"type": "five_hour"})
        );
        let future: ApiUsageWindowKind =
            serde_json::from_value(serde_json::json!({"type": "monthly"})).unwrap();
        assert_eq!(future, ApiUsageWindowKind::Other);
    }

    #[test]
    fn unknown_enums_and_missing_fields_deserialize() {
        let usage: ApiUsage = serde_json::from_value(serde_json::json!({
            "providers": [{
                "provider": "gemini",
                "status": "throttled"
            }]
        }))
        .unwrap();
        assert_eq!(usage.providers[0].provider, ApiUsageProvider::Unknown);
        assert_eq!(usage.providers[0].status, ApiUsageStatus::Unknown);
        assert_eq!(usage.working_days, vec![0, 1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn refresh_request_rejects_unknown_fields() {
        assert!(serde_json::from_str::<UsageRefreshRequest>(r#"{"providr":"claude"}"#).is_err());
        let request: UsageRefreshRequest = serde_json::from_str(r#"{"provider":"codex"}"#).unwrap();
        assert_eq!(request.provider, Some(ApiUsageProvider::Codex));
    }
}
