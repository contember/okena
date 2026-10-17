//! uniffi mirrors of the agent usage snapshot (`okena_core::api::ApiUsage`).
//!
//! The daemon pushes the snapshot as the `usage_changed` WS message; the
//! connection manager stores the latest one per connection and bumps a
//! revision counter so the RN state poll can skip the (history-heavy) record
//! conversion when nothing changed.
//!
//! Wire enums are flattened to their snake_case wire strings (`provider`,
//! `status`, window `kind`) so the RN side can switch on the same tokens the
//! HTTP API documents, and unknown values from a newer daemon stay readable.

use okena_core::api::{
    ApiExtraUsage, ApiProviderUsage, ApiResetCredit, ApiResetCredits, ApiUsage, ApiUsageCredits,
    ApiUsagePoint, ApiUsageProvider, ApiUsageStatus, ApiUsageWindow, ApiUsageWindowKind,
};

/// Full usage snapshot of one server.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct UsageInfo {
    /// One entry per provider (`claude`, then `codex`).
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
    /// `claude` | `codex` | `unknown`.
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
                                    "used_credits_cents": 250.0, "utilization": 2.5}
                },
                {
                    "provider": "codex", "status": "no_credentials", "error": "not signed in",
                    "credits": {"has_credits": true, "unlimited": false, "balance": 4.5},
                    "reset_credits": {"available_count": 7000000000u64,
                                      "credits": [{"title": "Reset", "expires_at": null}]}
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

        assert_eq!(info.providers[2].provider, "unknown");
        assert_eq!(info.providers[2].status, "unknown");
    }

    #[test]
    fn parses_refresh_provider_names() {
        assert_eq!(parse_provider("claude"), Some(ApiUsageProvider::Claude));
        assert_eq!(parse_provider("codex"), Some(ApiUsageProvider::Codex));
        assert_eq!(parse_provider("gemini"), None);
    }
}
