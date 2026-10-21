//! uniffi mirrors of the agent usage snapshot (`okena_core::api::ApiUsage`),
//! including the Claude account pool of the AI proxy.
//!
//! The daemon pushes the snapshot as the `usage_changed` WS message; the
//! connection manager stores the latest one per connection and bumps a
//! revision counter so the RN state poll can skip the (history-heavy) record
//! conversion when nothing changed.
//!
//! Wire enums are flattened to their snake_case wire strings (`provider`,
//! `status`, window `kind`, pool nonstop `kind`) so the RN side can switch on the same tokens the
//! HTTP API documents, and unknown values from a newer daemon stay readable.

use okena_core::api::{
    ApiExtraUsage, ApiPool, ApiPoolAccount, ApiPoolAmount, ApiPoolNonstop, ApiPoolPace,
    ApiPoolToday, ApiProviderUsage, ApiResetCredit, ApiResetCredits, ApiUsage, ApiUsageCredits,
    ApiUsagePoint, ApiUsageProvider, ApiUsageStatus, ApiUsageWindow, ApiUsageWindowKind,
};

/// Full usage snapshot of one server.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct UsageInfo {
    /// One entry per provider (`claude`, `codex`, then `grok`).
    pub providers: Vec<ProviderUsageInfo>,
    /// Working days, `0 = Monday .. 6 = Sunday`; never empty. `u32` because
    /// ubrn maps `Vec<u8>` to an `ArrayBuffer`.
    pub working_days: Vec<u32>,
    /// When the daemon produced the snapshot (epoch seconds, daemon clock).
    pub generated_at: f64,
}

/// Usage of one provider.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ProviderUsageInfo {
    /// `claude` | `codex` | `grok` | `unknown`.
    pub provider: String,
    /// `disabled` | `no_credentials` | `pending` | `ok` | `error` | `unknown`.
    pub status: String,
    pub plan: Option<String>,
    pub fetched_at: Option<f64>,
    pub last_attempt_at: Option<f64>,
    pub next_attempt_at: Option<f64>,
    pub fetching: bool,
    pub error: Option<String>,
    pub consecutive_failures: u32,
    pub rate_limited_until: Option<f64>,
    pub windows: Vec<UsageWindowInfo>,
    pub extra_usage: Option<ExtraUsageInfo>,
    pub credits: Option<UsageCreditsInfo>,
    pub reset_credits: Option<ResetCreditsInfo>,
    /// Grok prepaid credit balance, in the provider's credit units.
    pub prepaid_balance: Option<f64>,
    /// Claude only: the AI proxy's account pool, present while the host routes
    /// new shells through it; show it instead of `windows` then.
    pub pool: Option<PoolInfo>,
}

/// One rate-limit window.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct UsageWindowInfo {
    pub id: String,
    pub label: String,
    /// `five_hour` | `weekly` | `weekly_scoped` | `code_review` | `other`.
    pub kind: String,
    /// Model name of a `weekly_scoped` window.
    pub scoped_model: Option<String>,
    pub used_percent: f64,
    pub window_seconds: f64,
    pub reset_at: Option<f64>,
    pub active: bool,
    pub history: Vec<UsagePointInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct UsagePointInfo {
    pub recorded_at: f64,
    pub used_percent: f64,
}

/// Claude paid usage beyond the plan.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ExtraUsageInfo {
    pub is_enabled: bool,
    pub monthly_limit_cents: f64,
    pub used_credits_cents: f64,
    pub utilization: f64,
}

/// Codex credit balance.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct UsageCreditsInfo {
    pub has_credits: bool,
    pub unlimited: bool,
    pub balance: f64,
}

/// Codex rate-limit reset credits.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ResetCreditsInfo {
    /// `u32` instead of the wire `u64` so JS gets a number, not a bigint.
    pub available_count: u32,
    pub credits: Vec<ResetCreditInfo>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ResetCreditInfo {
    pub title: String,
    pub expires_at: Option<f64>,
}

/// The AI proxy's account pool (`ApiPool`). Quantities are already work time
/// and percent of an account's week; the severity rules are on `ApiPool`.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PoolInfo {
    pub today: PoolTodayInfo,
    pub nonstop: PoolNonstopInfo,
    pub pace: PoolPaceInfo,
    /// Every pool account, in pool order; those that do not serve requests
    /// have `blocked` set.
    pub accounts: Vec<PoolAccountInfo>,
}

/// Weighted weekly quota, as work and as a share of a week.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct PoolAmountInfo {
    pub work_seconds: f64,
    pub account_week_percent: f64,
}

/// The soft limit: what today may use.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PoolTodayInfo {
    /// Negative when today already used more than `safe_per_day`.
    pub left: PoolAmountInfo,
    pub used: PoolAmountInfo,
    pub safe_per_day: PoolAmountInfo,
    pub off_day: PoolAmountInfo,
    pub runs_short_at: Option<f64>,
    /// Label of the account whose weekly reset is at `runs_short_at`.
    pub runs_short_before: Option<String>,
    pub runs_out_at_recent_pace: Option<f64>,
}

/// The hard limit: nonstop work until every account is blocked.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PoolNonstopInfo {
    /// `lasts` | `beyond_horizon` | `blocked` | `unknown`.
    pub kind: String,
    /// `lasts`: work until every account is blocked.
    pub work_seconds: Option<f64>,
    /// `beyond_horizon`: the simulated horizon.
    pub horizon_seconds: Option<f64>,
    /// `blocked`: when the first account is back, if known.
    pub until: Option<f64>,
}

/// The pace that converts quota into work time.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct PoolPaceInfo {
    pub session_fill_seconds: f64,
    pub session_week_percent: f64,
    pub from_history: bool,
}

/// One pool account.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PoolAccountInfo {
    pub id: String,
    pub label: String,
    /// `Max 20x` / `Max 5x`; `None` for the base plan.
    pub tier: Option<String>,
    /// Why the proxy skips the account now (`Disabled`, `7d limit`,
    /// `429 cooldown`).
    pub blocked: Option<String>,
    pub used_today_percent: f64,
    /// The `five_hour` and `seven_day` windows.
    pub windows: Vec<UsageWindowInfo>,
}

fn status_str(status: ApiUsageStatus) -> &'static str {
    match status {
        ApiUsageStatus::Disabled => "disabled",
        ApiUsageStatus::NoCredentials => "no_credentials",
        ApiUsageStatus::Pending => "pending",
        ApiUsageStatus::Ok => "ok",
        ApiUsageStatus::Error => "error",
        ApiUsageStatus::Unknown => "unknown",
    }
}

/// Parse the provider name the RN side passes to `request_usage_refresh`.
pub fn parse_provider(name: &str) -> Option<ApiUsageProvider> {
    match name {
        "claude" => Some(ApiUsageProvider::Claude),
        "codex" => Some(ApiUsageProvider::Codex),
        "grok" => Some(ApiUsageProvider::Grok),
        _ => None,
    }
}

impl From<&ApiUsage> for UsageInfo {
    fn from(usage: &ApiUsage) -> Self {
        Self {
            providers: usage.providers.iter().map(Into::into).collect(),
            working_days: usage.working_days.iter().map(|&day| u32::from(day)).collect(),
            generated_at: usage.generated_at,
        }
    }
}

impl From<&ApiProviderUsage> for ProviderUsageInfo {
    fn from(p: &ApiProviderUsage) -> Self {
        Self {
            provider: p.provider.as_str().to_string(),
            status: status_str(p.status).to_string(),
            plan: p.plan.clone(),
            fetched_at: p.fetched_at,
            last_attempt_at: p.last_attempt_at,
            next_attempt_at: p.next_attempt_at,
            fetching: p.fetching,
            error: p.error.clone(),
            consecutive_failures: p.consecutive_failures,
            rate_limited_until: p.rate_limited_until,
            windows: p.windows.iter().map(Into::into).collect(),
            extra_usage: p.extra_usage.as_ref().map(Into::into),
            credits: p.credits.as_ref().map(Into::into),
            reset_credits: p.reset_credits.as_ref().map(Into::into),
            prepaid_balance: p.prepaid_balance,
            pool: p.pool.as_ref().map(Into::into),
        }
    }
}

impl From<&ApiUsageWindow> for UsageWindowInfo {
    fn from(w: &ApiUsageWindow) -> Self {
        let (kind, scoped_model) = match &w.kind {
            ApiUsageWindowKind::FiveHour => ("five_hour", None),
            ApiUsageWindowKind::Weekly => ("weekly", None),
            ApiUsageWindowKind::WeeklyScoped { model } => ("weekly_scoped", Some(model.clone())),
            ApiUsageWindowKind::CodeReview => ("code_review", None),
            ApiUsageWindowKind::Other => ("other", None),
        };
        Self {
            id: w.id.clone(),
            label: w.label.clone(),
            kind: kind.to_string(),
            scoped_model,
            used_percent: w.used_percent,
            window_seconds: w.window_seconds,
            reset_at: w.reset_at,
            active: w.active,
            history: w.history.iter().map(Into::into).collect(),
        }
    }
}

impl From<&ApiUsagePoint> for UsagePointInfo {
    fn from(p: &ApiUsagePoint) -> Self {
        Self {
            recorded_at: p.recorded_at,
            used_percent: p.used_percent,
        }
    }
}

impl From<&ApiExtraUsage> for ExtraUsageInfo {
    fn from(e: &ApiExtraUsage) -> Self {
        Self {
            is_enabled: e.is_enabled,
            monthly_limit_cents: e.monthly_limit_cents,
            used_credits_cents: e.used_credits_cents,
            utilization: e.utilization,
        }
    }
}

impl From<&ApiUsageCredits> for UsageCreditsInfo {
    fn from(c: &ApiUsageCredits) -> Self {
        Self {
            has_credits: c.has_credits,
            unlimited: c.unlimited,
            balance: c.balance,
        }
    }
}

impl From<&ApiResetCredits> for ResetCreditsInfo {
    fn from(r: &ApiResetCredits) -> Self {
        Self {
            available_count: u32::try_from(r.available_count).unwrap_or(u32::MAX),
            credits: r.credits.iter().map(Into::into).collect(),
        }
    }
}

impl From<&ApiResetCredit> for ResetCreditInfo {
    fn from(c: &ApiResetCredit) -> Self {
        Self {
            title: c.title.clone(),
            expires_at: c.expires_at,
        }
    }
}

impl From<&ApiPool> for PoolInfo {
    fn from(pool: &ApiPool) -> Self {
        Self {
            today: (&pool.today).into(),
            nonstop: (&pool.nonstop).into(),
            pace: pool.pace.into(),
            accounts: pool.accounts.iter().map(Into::into).collect(),
        }
    }
}

impl From<ApiPoolAmount> for PoolAmountInfo {
    fn from(amount: ApiPoolAmount) -> Self {
        Self {
            work_seconds: amount.work_seconds,
            account_week_percent: amount.account_week_percent,
        }
    }
}

impl From<&ApiPoolToday> for PoolTodayInfo {
    fn from(today: &ApiPoolToday) -> Self {
        Self {
            left: today.left.into(),
            used: today.used.into(),
            safe_per_day: today.safe_per_day.into(),
            off_day: today.off_day.into(),
            runs_short_at: today.runs_short_at,
            runs_short_before: today.runs_short_before.clone(),
            runs_out_at_recent_pace: today.runs_out_at_recent_pace,
        }
    }
}

impl From<&ApiPoolNonstop> for PoolNonstopInfo {
    fn from(nonstop: &ApiPoolNonstop) -> Self {
        let (kind, work_seconds, horizon_seconds, until) = match *nonstop {
            ApiPoolNonstop::Lasts { work_seconds } => ("lasts", Some(work_seconds), None, None),
            ApiPoolNonstop::BeyondHorizon { horizon_seconds } => {
                ("beyond_horizon", None, Some(horizon_seconds), None)
            }
            ApiPoolNonstop::Blocked { until } => ("blocked", None, None, until),
            ApiPoolNonstop::Unknown => ("unknown", None, None, None),
        };
        Self {
            kind: kind.to_string(),
            work_seconds,
            horizon_seconds,
            until,
        }
    }
}

impl From<ApiPoolPace> for PoolPaceInfo {
    fn from(pace: ApiPoolPace) -> Self {
        Self {
            session_fill_seconds: pace.session_fill_seconds,
            session_week_percent: pace.session_week_percent,
            from_history: pace.from_history,
        }
    }
}

impl From<&ApiPoolAccount> for PoolAccountInfo {
    fn from(account: &ApiPoolAccount) -> Self {
        Self {
            id: account.id.clone(),
            label: account.label.clone(),
            tier: account.tier.clone(),
            blocked: account.blocked.clone(),
            used_today_percent: account.used_today_percent,
            windows: account.windows.iter().map(Into::into).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ApiUsage {
        serde_json::from_value(serde_json::json!({
            "providers": [
                {
                    "provider": "claude", "status": "ok", "plan": "max",
                    "fetched_at": 100.0, "fetching": false,
                    "windows": [
                        {"id": "five_hour", "label": "Session", "kind": {"type": "five_hour"},
                         "used_percent": 20.0, "window_seconds": 18000.0, "reset_at": 200.0,
                         "history": [{"recorded_at": 90.0, "used_percent": 18.0}]},
                        {"id": "seven_day_fable", "label": "Fable",
                         "kind": {"type": "weekly_scoped", "model": "Fable"},
                         "used_percent": 0.2, "window_seconds": 604800.0, "active": false}
                    ],
                    "extra_usage": {"is_enabled": true, "monthly_limit_cents": 10000.0,
                                    "used_credits_cents": 250.0, "utilization": 2.5},
                    "pool": {
                        "today": {
                            "left": {"work_seconds": -2700.0, "account_week_percent": -2.0},
                            "used": {"work_seconds": 9000.0, "account_week_percent": 8.0},
                            "safe_per_day": {"work_seconds": 6300.0, "account_week_percent": 6.0},
                            "off_day": {"work_seconds": 1800.0, "account_week_percent": 1.5},
                            "runs_short_at": 500.0,
                            "runs_short_before": "Work"
                        },
                        "nonstop": {"type": "blocked", "until": 300.0},
                        "pace": {"session_fill_seconds": 7200.0, "session_week_percent": 9.0,
                                 "from_history": true},
                        "accounts": [{
                            "id": "a1", "label": "Work", "tier": "Max 5x", "blocked": "5h limit",
                            "used_today_percent": 3.5,
                            "windows": [
                                {"id": "five_hour", "label": "Session", "kind": {"type": "five_hour"},
                                 "used_percent": 100.0, "window_seconds": 18000.0, "reset_at": 300.0}
                            ]
                        }]
                    }
                },
                {
                    "provider": "codex", "status": "no_credentials", "error": "not signed in",
                    "credits": {"has_credits": true, "unlimited": false, "balance": 4.5},
                    "reset_credits": {"available_count": 7000000000u64,
                                      "credits": [{"title": "Reset", "expires_at": null}]}
                },
                {
                    "provider": "grok", "status": "ok", "prepaid_balance": 15.5,
                    "windows": [
                        {"id": "included", "label": "Credits", "kind": {"type": "other"},
                         "used_percent": 40.0, "window_seconds": 2592000.0}
                    ]
                },
                {"provider": "gemini", "status": "throttled"}
            ],
            "working_days": [0, 1, 2, 3, 4],
            "generated_at": 123.0
        }))
        .expect("sample usage parses")
    }

    #[test]
    fn converts_snapshot_to_flat_records() {
        let info = UsageInfo::from(&sample());
        assert_eq!(info.working_days, vec![0, 1, 2, 3, 4]);
        assert_eq!(info.generated_at, 123.0);

        let claude = &info.providers[0];
        assert_eq!(claude.provider, "claude");
        assert_eq!(claude.status, "ok");
        assert_eq!(claude.plan.as_deref(), Some("max"));
        assert_eq!(claude.windows[0].kind, "five_hour");
        assert_eq!(claude.windows[0].reset_at, Some(200.0));
        assert!(claude.windows[0].active, "unscoped windows default to active");
        assert_eq!(claude.windows[0].history.len(), 1);
        assert_eq!(claude.windows[1].kind, "weekly_scoped");
        assert_eq!(claude.windows[1].scoped_model.as_deref(), Some("Fable"));
        assert!(!claude.windows[1].active);
        assert_eq!(
            claude.extra_usage.as_ref().map(|e| e.used_credits_cents),
            Some(250.0)
        );

        let codex = &info.providers[1];
        assert_eq!(codex.status, "no_credentials");
        assert_eq!(codex.error.as_deref(), Some("not signed in"));
        assert_eq!(codex.credits.as_ref().map(|c| c.balance), Some(4.5));
        let resets = codex.reset_credits.as_ref().expect("reset credits");
        assert_eq!(resets.available_count, u32::MAX, "u64 count saturates");
        assert_eq!(resets.credits[0].expires_at, None);
        assert_eq!(codex.pool, None);

        let grok = &info.providers[2];
        assert_eq!(grok.provider, "grok");
        assert_eq!(grok.prepaid_balance, Some(15.5));
        assert_eq!(grok.windows[0].id, "included");
        assert_eq!(grok.windows[0].kind, "other");

        assert_eq!(info.providers[3].provider, "unknown");
        assert_eq!(info.providers[3].status, "unknown");
    }

    #[test]
    fn converts_the_account_pool() {
        let info = UsageInfo::from(&sample());
        let pool = info.providers[0].pool.as_ref().expect("claude pool");

        assert_eq!(pool.today.left.work_seconds, -2700.0);
        assert_eq!(pool.today.used.work_seconds, 9000.0);
        assert_eq!(pool.today.used.account_week_percent, 8.0);
        assert_eq!(pool.today.safe_per_day.account_week_percent, 6.0);
        assert_eq!(pool.today.off_day.work_seconds, 1800.0);
        assert_eq!(pool.today.off_day.account_week_percent, 1.5);
        assert_eq!(pool.today.runs_short_at, Some(500.0));
        assert_eq!(pool.today.runs_short_before.as_deref(), Some("Work"));
        assert_eq!(pool.today.runs_out_at_recent_pace, None);
        assert_eq!(pool.nonstop.kind, "blocked");
        assert_eq!(pool.nonstop.until, Some(300.0));
        assert_eq!(pool.pace.session_fill_seconds, 7200.0);
        assert_eq!(pool.pace.session_week_percent, 9.0);
        assert!(pool.pace.from_history);

        let account = &pool.accounts[0];
        assert_eq!(account.id, "a1");
        assert_eq!(account.label, "Work");
        assert_eq!(account.tier.as_deref(), Some("Max 5x"));
        assert_eq!(account.blocked.as_deref(), Some("5h limit"));
        assert_eq!(account.used_today_percent, 3.5);
        assert_eq!(account.windows[0].kind, "five_hour");
        assert_eq!(account.windows[0].reset_at, Some(300.0));
    }

    #[test]
    fn flattens_every_nonstop_state() {
        let flat = |json: serde_json::Value| {
            let nonstop: ApiPoolNonstop = serde_json::from_value(json).expect("nonstop parses");
            PoolNonstopInfo::from(&nonstop)
        };
        assert_eq!(
            flat(serde_json::json!({"type": "lasts", "work_seconds": 93600.0})),
            PoolNonstopInfo {
                kind: "lasts".into(),
                work_seconds: Some(93600.0),
                horizon_seconds: None,
                until: None,
            }
        );
        assert_eq!(
            flat(serde_json::json!({"type": "beyond_horizon", "horizon_seconds": 604800.0})),
            PoolNonstopInfo {
                kind: "beyond_horizon".into(),
                work_seconds: None,
                horizon_seconds: Some(604800.0),
                until: None,
            }
        );
        assert_eq!(
            flat(serde_json::json!({"type": "blocked"})),
            PoolNonstopInfo {
                kind: "blocked".into(),
                work_seconds: None,
                horizon_seconds: None,
                until: None,
            }
        );
        assert_eq!(flat(serde_json::json!({"type": "drained"})).kind, "unknown");
    }

    #[test]
    fn parses_refresh_provider_names() {
        assert_eq!(parse_provider("claude"), Some(ApiUsageProvider::Claude));
        assert_eq!(parse_provider("codex"), Some(ApiUsageProvider::Codex));
        assert_eq!(parse_provider("grok"), Some(ApiUsageProvider::Grok));
        assert_eq!(parse_provider("gemini"), None);
    }
}
