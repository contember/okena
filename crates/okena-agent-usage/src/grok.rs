//! Grok usage: the Grok CLI session from `$GROK_HOME/auth.json` (or
//! `~/.grok/auth.json`), the `cli-chat-proxy.grok.com` billing fetch, and
//! parsing its credit fields into the included / on-demand windows.

use std::path::{Path, PathBuf};
use std::time::Duration;

use okena_core::api::{ApiUsageWindow, ApiUsageWindowKind};
use okena_transport::http::{HttpError, HttpRequest, HttpResponse};

use crate::claude::parse_retry_after;
use crate::provider::{FetchOutcome, ProviderData};

/// The Grok CLI requests this with its grok.com session. `grok.com` itself
/// returns 404 for the same path.
const BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
const USER_AGENT: &str = concat!("okena/", env!("CARGO_PKG_VERSION"));

const NO_CREDENTIALS: &str = "Grok credentials missing; sign in with `grok login`";
const SESSION_EXPIRED: &str = "Grok session expired — run `grok` to refresh";

/// Window id (and history series name) of the included monthly credits.
const INCLUDED_ID: &str = "included";
/// Window id (and history series name) of the on-demand spending cap.
const ON_DEMAND_ID: &str = "on_demand";

/// Okena only reads the Grok CLI session. Refreshing would rotate the refresh
/// token the CLI also holds, so an expired session is left for the CLI to renew.
struct GrokAuth {
    access_token: String,
    user_id: String,
    expires_at: Option<String>,
}

/// `GROK_HOME` when it exists, otherwise `~/.grok`.
fn resolve_grok_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("GROK_HOME") {
        let path = PathBuf::from(dir);
        if path.is_dir() {
            return path;
        }
        log::warn!("[grok-usage] GROK_HOME does not exist, falling back to ~/.grok");
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".grok")
}

fn read_grok_auth(dir: &Path) -> Option<GrokAuth> {
    let path = dir.join("auth.json");
    let content = std::fs::read_to_string(&path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    let entries = value.as_object()?;
    let now = jiff::Timestamp::now();
    let entry = entries
        .values()
        .filter(|entry| {
            entry["key"]
                .as_str()
                .is_some_and(|token| !token.trim().is_empty())
        })
        .min_by_key(|entry| access_token_expired(entry["expires_at"].as_str(), now) as u8)?;
    let access_token = entry["key"].as_str()?.trim().to_string();
    let user_id = entry["user_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .unwrap_or("default")
        .to_string();
    Some(GrokAuth {
        access_token,
        user_id,
        expires_at: non_empty(entry["expires_at"].as_str()),
    })
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn access_token_expired(expires_at: Option<&str>, now: jiff::Timestamp) -> bool {
    let Some(expires_at) = expires_at.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    let Ok(expires) = expires_at.parse::<jiff::Timestamp>() else {
        return true;
    };
    expires <= now
}

fn client_version(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("version.json"))
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        .and_then(|value| non_empty(value["version"].as_str()))
        .unwrap_or_else(|| "okena".to_string())
}

fn billing_request(access_token: &str, version: &str) -> HttpRequest {
    HttpRequest::get(BILLING_URL)
        .bearer(access_token)
        .header("Accept", "application/json")
        .header("x-grok-client-version", version)
        .header("x-grok-client-identifier", "xai-grok-cli")
        .header("X-XAI-Token-Auth", "xai-grok-cli")
        .timeout(Duration::from_secs(10))
        .user_agent(USER_AGENT)
        .label("grok.usage")
}

/// Fetch billing usage. Returns the outcome and the Grok user id (the history
/// source) when a session was readable.
pub fn fetch() -> (FetchOutcome, Option<String>) {
    fetch_in(&resolve_grok_dir(), okena_transport::http::send)
}

fn fetch_in(
    dir: &Path,
    send: impl FnOnce(HttpRequest) -> Result<HttpResponse, HttpError>,
) -> (FetchOutcome, Option<String>) {
    let Some(auth) = read_grok_auth(dir) else {
        return (FetchOutcome::NoCredentials(NO_CREDENTIALS.into()), None);
    };
    let user_id = Some(auth.user_id);
    if access_token_expired(auth.expires_at.as_deref(), jiff::Timestamp::now()) {
        return (FetchOutcome::NoCredentials(SESSION_EXPIRED.into()), user_id);
    }
    let version = client_version(dir);
    let response = match send(billing_request(&auth.access_token, &version)) {
        Ok(response) => response,
        Err(_) => {
            return (
                FetchOutcome::Failed("Could not connect to Grok usage API".into()),
                user_id,
            );
        }
    };
    match response.status() {
        401 => return (FetchOutcome::NoCredentials(SESSION_EXPIRED.into()), user_id),
        429 => {
            let retry_after = parse_retry_after(response.header("retry-after"));
            return (FetchOutcome::RateLimited { retry_after }, user_id);
        }
        status if !response.is_success() => {
            return (
                FetchOutcome::Failed(format!("Usage request failed: HTTP {status}")),
                user_id,
            );
        }
        _ => {}
    }
    let body: serde_json::Value = match response.json() {
        Ok(body) => body,
        Err(_) => {
            return (
                FetchOutcome::Failed("Invalid JSON in usage response".into()),
                user_id,
            );
        }
    };
    match parse_usage(&body) {
        Ok(data) => (FetchOutcome::Ok(data), user_id),
        Err(error) => (FetchOutcome::Failed(error), user_id),
    }
}

/// Parse the billing response. Field names are searched at any depth (the CLI
/// proxy nests them under `config`) in camelCase and snake_case. Errors when
/// it carries neither included credits nor an on-demand cap.
pub fn parse_usage(body: &serde_json::Value) -> Result<ProviderData, String> {
    let plan = find_string(
        body,
        &[
            "subscriptionTier",
            "subscription_tier",
            "subscription_tier_display",
        ],
    );
    let included_used = find_number(body, &["includedUsed", "included_used"])
        .or_else(|| find_number(body, &["totalUsed", "total_used"]));
    let monthly_limit = find_number(body, &["monthlyLimit", "monthly_limit"]);
    let included_percent = usage_percent(
        find_number(body, &["creditUsagePercent", "credit_usage_percent"]),
        included_used,
        monthly_limit,
    );
    let period = billing_window(
        find_string(body, &["billingPeriodStart", "billing_period_start"]).as_deref(),
        find_string(body, &["billingPeriodEnd", "billing_period_end"]).as_deref(),
    );
    let included = included_percent
        .map(|used_percent| credit_window(INCLUDED_ID, "Credits", used_percent, &period));

    let on_demand_cap = find_number(body, &["onDemandCap", "on_demand_cap"]);
    let on_demand_used = find_number(body, &["onDemandUsed", "on_demand_used"]).unwrap_or(0.0);
    let on_demand = on_demand_cap.filter(|cap| *cap > 0.0).map(|cap| {
        let used_percent = (on_demand_used / cap * 100.0).clamp(0.0, 100.0);
        credit_window(ON_DEMAND_ID, "On-demand", used_percent, &period)
    });
    if included.is_none() && on_demand.is_none() {
        log::warn!(
            "[grok-usage] usage response had no recognized credit fields; keys: {}",
            value_keys(body)
        );
        return Err("Usage response contains no credit limit".to_string());
    }
    Ok(ProviderData {
        plan,
        windows: [included, on_demand].into_iter().flatten().collect(),
        extra_usage: None,
        credits: None,
        reset_credits: None,
        prepaid_balance: find_number(body, &["prepaidBalance", "prepaid_balance"]),
    })
}

/// The billing period both credit windows share.
struct BillingWindow {
    /// `0` unless both ends are known and the end is after the start.
    window_seconds: f64,
    reset_at: Option<f64>,
}

fn billing_window(start: Option<&str>, end: Option<&str>) -> BillingWindow {
    let start = start.and_then(parse_epoch);
    let end = end.and_then(parse_epoch);
    match (start, end) {
        (Some(start), Some(end)) if end > start => BillingWindow {
            window_seconds: end - start,
            reset_at: Some(end),
        },
        _ => BillingWindow {
            window_seconds: 0.0,
            reset_at: end,
        },
    }
}

fn credit_window(
    id: &str,
    label: &str,
    used_percent: f64,
    period: &BillingWindow,
) -> ApiUsageWindow {
    ApiUsageWindow {
        id: id.into(),
        label: label.into(),
        kind: ApiUsageWindowKind::Other,
        used_percent,
        window_seconds: period.window_seconds,
        reset_at: period.reset_at,
        active: true,
        history: Vec::new(),
    }
}

fn usage_percent(explicit: Option<f64>, used: Option<f64>, limit: Option<f64>) -> Option<f64> {
    let ratio = match (used, limit) {
        (Some(used), Some(limit)) if limit > 0.0 => Some((used / limit * 100.0).clamp(0.0, 100.0)),
        _ => None,
    };
    match explicit.filter(|value| value.is_finite() && *value >= 0.0) {
        Some(value) if value <= 1.0 => {
            if let Some(ratio) = ratio
                && (ratio - value * 100.0).abs() <= 1.5
            {
                return Some(ratio);
            }
            Some(value.clamp(0.0, 100.0))
        }
        Some(value) => Some(value.clamp(0.0, 100.0)),
        None => ratio,
    }
}

fn parse_epoch(value: &str) -> Option<f64> {
    let normalized = value.trim().replace("+00:00", "Z");
    let timestamp: jiff::Timestamp = normalized.parse().ok()?;
    Some(timestamp.as_millisecond() as f64 / 1_000.0)
}

fn find_number(value: &serde_json::Value, names: &[&str]) -> Option<f64> {
    find_value(value, names).and_then(number)
}

fn find_string(value: &serde_json::Value, names: &[&str]) -> Option<String> {
    find_value(value, names).and_then(|value| non_empty(value.as_str()))
}

fn find_value<'a>(value: &'a serde_json::Value, names: &[&str]) -> Option<&'a serde_json::Value> {
    let mut queue = vec![value];
    while let Some(current) = queue.pop() {
        if let Some(object) = current.as_object() {
            for name in names {
                if let Some(found) = object.get(*name) {
                    return Some(found);
                }
            }
            queue.extend(object.values());
        } else if let Some(items) = current.as_array() {
            queue.extend(items);
        }
    }
    None
}

fn number(value: &serde_json::Value) -> Option<f64> {
    if let Some(inner) = value.get("val") {
        return number(inner);
    }
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|number| number as f64))
        .or_else(|| value.as_u64().map(|number| number as f64))
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        .filter(|number| number.is_finite())
}

fn value_keys(value: &serde_json::Value) -> String {
    value
        .as_object()
        .map(|object| object.keys().cloned().collect::<Vec<_>>().join(", "))
        .unwrap_or_else(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pace::linear_time_pct;

    fn epoch(value: &str) -> f64 {
        parse_epoch(value).unwrap()
    }

    fn window<'a>(data: &'a ProviderData, id: &str) -> Option<&'a ApiUsageWindow> {
        data.windows.iter().find(|window| window.id == id)
    }

    #[test]
    fn credit_fields_from_the_grok_cli_become_usage_windows() {
        let now = epoch("2026-10-13T00:00:00Z");
        let data = parse_usage(&serde_json::json!({
            "creditUsagePercent": 37,
            "monthlyLimit": 1000,
            "includedUsed": 370,
            "onDemandCap": 200,
            "onDemandUsed": 20,
            "prepaidBalance": 15.5,
            "billingPeriodStart": "2026-10-01T00:00:00Z",
            "billingPeriodEnd": "2026-11-01T00:00:00Z",
            "subscriptionTier": "SuperGrok"
        }))
        .unwrap();
        let included = window(&data, "included").unwrap();
        assert_eq!(included.label, "Credits");
        assert_eq!(included.kind, ApiUsageWindowKind::Other);
        assert!(included.active);
        assert_eq!(included.used_percent, 37.0);
        assert_eq!(included.reset_at, Some(epoch("2026-11-01T00:00:00Z")));
        assert!((included.window_seconds - 31.0 * 86_400.0).abs() < 1.0);
        let elapsed =
            linear_time_pct(included.reset_at.unwrap(), included.window_seconds, now).unwrap();
        assert!(elapsed > 30.0);
        let on_demand = window(&data, "on_demand").unwrap();
        assert_eq!(on_demand.label, "On-demand");
        assert_eq!(on_demand.used_percent, 10.0);
        assert_eq!(data.prepaid_balance, Some(15.5));
        assert_eq!(data.plan.as_deref(), Some("SuperGrok"));
    }

    #[test]
    fn time_elapsed_derived_from_reset_matches_the_billing_period() {
        let start = epoch("2026-10-01T00:00:00Z");
        let end = epoch("2026-11-01T00:00:00Z");
        let period = billing_window(Some("2026-10-01T00:00:00Z"), Some("2026-11-01T00:00:00Z"));
        for now in [
            start - 10.0,
            start,
            epoch("2026-10-13T07:30:00Z"),
            end,
            end + 10.0,
        ] {
            let expected = ((now - start) / (end - start) * 100.0).clamp(0.0, 100.0);
            let derived =
                linear_time_pct(period.reset_at.unwrap(), period.window_seconds, now).unwrap();
            assert!((derived - expected).abs() < 1e-9, "{derived} != {expected}");
        }
    }

    #[test]
    fn the_cli_proxy_response_is_a_weekly_credit_window() {
        let data = parse_usage(&serde_json::json!({
            "config": {
                "currentPeriod": {
                    "type": "USAGE_PERIOD_TYPE_WEEKLY",
                    "start": "2026-10-08T00:00:00+00:00",
                    "end": "2026-10-15T00:00:00+00:00"
                },
                "creditUsagePercent": 3.0,
                "onDemandCap": {"val": 0},
                "onDemandUsed": {"val": 0},
                "prepaidBalance": {"val": 0},
                "billingPeriodStart": "2026-10-08T00:00:00+00:00",
                "billingPeriodEnd": "2026-10-15T00:00:00+00:00"
            }
        }))
        .unwrap();
        let included = window(&data, "included").unwrap();
        assert_eq!(included.used_percent, 3.0);
        assert_eq!(included.window_seconds, 7.0 * 86_400.0);
        assert_eq!(included.reset_at, Some(epoch("2026-10-15T00:00:00Z")));
        assert!(window(&data, "on_demand").is_none());
        assert_eq!(data.prepaid_balance, Some(0.0));
        assert_eq!(data.plan, None);
    }

    #[test]
    fn a_fractional_credit_percent_is_scaled_when_usage_confirms_it() {
        let data = parse_usage(&serde_json::json!({
            "creditUsagePercent": 0.37,
            "includedUsed": 370,
            "monthlyLimit": 1000
        }))
        .unwrap();
        assert_eq!(window(&data, "included").unwrap().used_percent, 37.0);
    }

    #[test]
    fn used_and_limit_are_enough_when_the_percent_is_absent() {
        let data =
            parse_usage(&serde_json::json!({"included_used": 25, "monthly_limit": 100})).unwrap();
        assert_eq!(window(&data, "included").unwrap().used_percent, 25.0);
    }

    #[test]
    fn zero_usage_is_a_real_reading() {
        let data = parse_usage(&serde_json::json!({
            "subscriptionTier": "Free",
            "config": {"creditUsagePercent": 0, "monthlyLimit": 100, "includedUsed": 0}
        }))
        .unwrap();
        assert_eq!(window(&data, "included").unwrap().used_percent, 0.0);
        assert_eq!(data.plan.as_deref(), Some("Free"));
    }

    #[test]
    fn a_response_without_a_credit_limit_is_rejected() {
        assert!(parse_usage(&serde_json::json!({"subscriptionTier": "Free"})).is_err());
    }

    #[test]
    fn auth_file_selects_the_usable_session_and_detects_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let auth = serde_json::json!({
            "https://auth.x.ai::client": {
                "key": "access-token",
                "auth_mode": "oidc",
                "refresh_token": "refresh-token",
                "expires_at": "2026-07-10T17:58:56.997660Z",
                "oidc_issuer": "https://auth.x.ai",
                "oidc_client_id": "b1a00492-073a-47ea-816f-4c329264a828",
                "user_id": "user-1"
            }
        });
        std::fs::write(dir.path().join("auth.json"), auth.to_string()).unwrap();
        let session = read_grok_auth(dir.path()).unwrap();
        assert_eq!(session.user_id, "user-1");
        assert_eq!(session.access_token, "access-token");
        assert!(access_token_expired(
            session.expires_at.as_deref(),
            "2026-10-13T00:00:00Z".parse().unwrap()
        ));
        assert!(!access_token_expired(
            Some("2026-10-14T00:00:00Z"),
            "2026-10-13T00:00:00Z".parse().unwrap()
        ));
    }

    fn write_session(dir: &Path, expires_at: &str) {
        std::fs::write(
            dir.join("auth.json"),
            serde_json::json!({
                "https://auth.x.ai::client": {
                    "key": "access-token",
                    "expires_at": expires_at,
                    "user_id": "user-1"
                }
            })
            .to_string(),
        )
        .unwrap();
    }

    fn response(status: u16, headers: Vec<(String, String)>, body: &str) -> HttpResponse {
        HttpResponse::new(status, headers, body.as_bytes().to_vec())
    }

    #[test]
    fn an_expired_token_is_reported_without_a_network_call() {
        let dir = tempfile::tempdir().unwrap();
        write_session(dir.path(), "2000-01-01T00:00:00Z");
        let (outcome, user_id) =
            fetch_in(dir.path(), |_| panic!("an expired token must not be sent"));
        assert_eq!(
            outcome,
            FetchOutcome::NoCredentials(SESSION_EXPIRED.to_string())
        );
        assert_eq!(user_id.as_deref(), Some("user-1"));
    }

    #[test]
    fn a_missing_session_is_reported_without_a_network_call() {
        let dir = tempfile::tempdir().unwrap();
        let (outcome, user_id) = fetch_in(dir.path(), |_| panic!("no session, no request"));
        assert!(matches!(outcome, FetchOutcome::NoCredentials(_)));
        assert_eq!(user_id, None);
    }

    #[test]
    fn http_errors_map_to_outcomes() {
        let dir = tempfile::tempdir().unwrap();
        write_session(dir.path(), "2999-01-01T00:00:00Z");

        let (outcome, _) = fetch_in(dir.path(), |_| Ok(response(401, Vec::new(), "")));
        assert_eq!(
            outcome,
            FetchOutcome::NoCredentials(SESSION_EXPIRED.to_string())
        );

        let (outcome, _) = fetch_in(dir.path(), |_| {
            Ok(response(
                429,
                vec![("retry-after".into(), "120".into())],
                "",
            ))
        });
        assert_eq!(
            outcome,
            FetchOutcome::RateLimited {
                retry_after: Some(Duration::from_secs(120))
            }
        );

        let (outcome, _) = fetch_in(dir.path(), |_| Ok(response(500, Vec::new(), "")));
        assert_eq!(
            outcome,
            FetchOutcome::Failed("Usage request failed: HTTP 500".into())
        );

        let (outcome, _) = fetch_in(dir.path(), |_| {
            Err(HttpError::Transport("timed out".into()))
        });
        assert!(matches!(outcome, FetchOutcome::Failed(_)));
    }

    #[test]
    fn a_successful_fetch_parses_the_billing_response() {
        let dir = tempfile::tempdir().unwrap();
        write_session(dir.path(), "2999-01-01T00:00:00Z");
        let (outcome, user_id) = fetch_in(dir.path(), |request| {
            assert_eq!(request.url(), BILLING_URL);
            assert_eq!(
                request.header_value("x-grok-client-identifier"),
                Some("xai-grok-cli")
            );
            Ok(response(
                200,
                Vec::new(),
                r#"{"creditUsagePercent": 12, "subscriptionTier": "SuperGrok"}"#,
            ))
        });
        let FetchOutcome::Ok(data) = outcome else {
            panic!("expected usage, got {outcome:?}");
        };
        assert_eq!(window(&data, "included").unwrap().used_percent, 12.0);
        assert_eq!(user_id.as_deref(), Some("user-1"));
    }
}
