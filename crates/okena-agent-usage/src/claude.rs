//! Claude Code usage: OAuth credentials (credentials file or macOS Keychain),
//! the `api.anthropic.com/api/oauth/usage` fetch, and response parsing.
//!
//! Okena never refreshes Claude tokens — the `claude` CLI does that. An
//! expired token is reported as missing credentials.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};

use okena_core::api::{ApiExtraUsage, ApiUsageWindow, ApiUsageWindowKind};

use crate::provider::{FetchOutcome, ProviderData};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

/// Period durations for each tier.
/// Upper bound for one macOS Keychain lookup (`security find-generic-password`).
#[cfg(target_os = "macos")]
const KEYCHAIN_TIMEOUT: Duration = Duration::from_secs(5);
const FIVE_HOUR_SECS: f64 = 5.0 * 3600.0;
const SEVEN_DAY_SECS: f64 = 7.0 * 86400.0;

/// The OAuth token plus the plan label from the same credentials blob.
pub struct ClaudeCredentials {
    access_token: String,
    pub plan: Option<String>,
}

impl std::fmt::Debug for ClaudeCredentials {
    // Never print the token.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeCredentials")
            .field("access_token", &"<redacted>")
            .field("plan", &self.plan)
            .finish()
    }
}

/// Compute the macOS Keychain service name for a given Claude config directory.
/// The Claude CLI uses "Claude Code-credentials" for the default ~/.claude, and
/// "Claude Code-credentials-<sha256(path)[..8 hex]>" for any custom config dir.
#[cfg(target_os = "macos")]
pub fn keychain_service_name(claude_dir: &Path) -> String {
    const BASE: &str = "Claude Code-credentials";
    // Canonicalize both sides (like `okena_workspace::claude_env`), so a
    // symlinked home or `~/.claude` still maps to the default service.
    let default_dir = dirs::home_dir()
        .map(|h| h.join(".claude"))
        .map(|dir| dir.canonicalize().unwrap_or(dir));
    let canonical = claude_dir
        .canonicalize()
        .unwrap_or_else(|_| claude_dir.to_path_buf());
    if Some(&canonical) == default_dir.as_ref() {
        BASE.to_string()
    } else {
        suffixed_keychain_service_name(claude_dir)
    }
}

#[cfg(target_os = "macos")]
fn suffixed_keychain_service_name(claude_dir: &Path) -> String {
    const BASE: &str = "Claude Code-credentials";
    let canonical = claude_dir
        .canonicalize()
        .unwrap_or_else(|_| claude_dir.to_path_buf());
    let mut h = Sha256::new();
    h.update(canonical.to_string_lossy().as_bytes());
    let d = h.finalize();
    format!("{BASE}-{:02x}{:02x}{:02x}{:02x}", d[0], d[1], d[2], d[3])
}

/// Keychain services to try, in order. Claude Code can create both the
/// default service and the suffixed service when `CLAUDE_CONFIG_DIR`
/// explicitly points at `~/.claude`, so the default dir tries both.
#[cfg(target_os = "macos")]
pub fn keychain_service_names(claude_dir: &Path) -> Vec<String> {
    let primary = keychain_service_name(claude_dir);
    let suffixed = suffixed_keychain_service_name(claude_dir);
    if primary == suffixed {
        vec![primary]
    } else {
        vec![primary, suffixed]
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Why a credentials blob was rejected.
#[derive(Debug, PartialEq, Eq)]
enum TokenProblem {
    Missing,
    Expired,
}

fn extract_credentials(json_str: &str, now_ms: u64) -> Result<ClaudeCredentials, TokenProblem> {
    let v: serde_json::Value = serde_json::from_str(json_str).map_err(|_| TokenProblem::Missing)?;
    let oauth = &v["claudeAiOauth"];
    let token = oauth["accessToken"]
        .as_str()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or(TokenProblem::Missing)?;
    if let Some(expires_at) = oauth["expiresAt"].as_u64()
        && expires_at <= now_ms
    {
        return Err(TokenProblem::Expired);
    }
    let plan = oauth["subscriptionType"]
        .as_str()
        .map(str::trim)
        .filter(|plan| !plan.is_empty())
        .map(ToOwned::to_owned);
    Ok(ClaudeCredentials {
        access_token: token.to_string(),
        plan,
    })
}

/// Read the OAuth token and plan for `claude_dir`: the `.credentials.json`
/// file first, then (macOS) the per-config-dir Keychain services. The error
/// is a user-facing reason and never contains secret material.
pub fn read_credentials(claude_dir: &Path) -> Result<ClaudeCredentials, String> {
    let now = now_ms();
    let mut expired = false;

    if let Ok(content) = std::fs::read_to_string(claude_dir.join(".credentials.json")) {
        match extract_credentials(&content, now) {
            Ok(credentials) => return Ok(credentials),
            Err(TokenProblem::Expired) => expired = true,
            Err(TokenProblem::Missing) => {}
        }
    }

    #[cfg(target_os = "macos")]
    let mut keychain_failure: Option<String> = None;
    #[cfg(target_os = "macos")]
    {
        // `-a $USER` matches how Claude Code stores the item. Without a user
        // name, search by service alone.
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .ok()
            .filter(|user| !user.is_empty());
        for service in keychain_service_names(claude_dir) {
            let mut command = okena_core::process::command("security");
            command.args(["find-generic-password", "-s", &service]);
            if let Some(user) = &user {
                command.args(["-a", user]);
            }
            command.arg("-w");
            // Bounded: a locked keychain waiting on an access prompt that no
            // one can answer must not pin the fetch (and the daemon's
            // shutdown) on the blocking pool.
            let output = match okena_core::process::safe_output_with_timeout(
                &mut command,
                KEYCHAIN_TIMEOUT,
            ) {
                Ok(output) => output,
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {
                    keychain_failure = Some(
                        "macOS Keychain lookup timed out (locked keychain or pending access prompt)"
                            .to_string(),
                    );
                    break;
                }
                Err(error) => {
                    keychain_failure = Some(format!("could not run `security`: {error}"));
                    break;
                }
            };
            if output.status.success() {
                let content = String::from_utf8_lossy(&output.stdout).trim().to_string();
                match extract_credentials(&content, now) {
                    Ok(credentials) => return Ok(credentials),
                    Err(TokenProblem::Expired) => expired = true,
                    Err(TokenProblem::Missing) => {}
                }
            } else {
                // 44 = errSecItemNotFound (just not stored there). Anything else
                // (e.g. 36 / 51 / 128: locked keychain, no GUI session to show
                // the access prompt, prompt denied) is worth surfacing.
                let code = output.status.code();
                if code != Some(44) {
                    keychain_failure = Some(match code {
                        Some(code) => {
                            format!("macOS Keychain refused access (security exit {code})")
                        }
                        None => "macOS Keychain lookup was interrupted".to_string(),
                    });
                }
            }
        }
    }

    if expired {
        return Err("Claude token expired; run `claude` to sign in again".into());
    }
    #[cfg(target_os = "macos")]
    if let Some(failure) = keychain_failure {
        return Err(format!(
            "No Claude credentials in {}; {failure}",
            claude_dir.display()
        ));
    }
    Err(format!(
        "No Claude credentials found for {}; sign in with `claude`",
        claude_dir.display()
    ))
}

/// Fetch and parse usage for the Claude account configured in `claude_dir`.
/// Does not record history — the caller does (see [`crate::fetch_claude`]).
pub fn fetch(claude_dir: &Path) -> FetchOutcome {
    let credentials = match read_credentials(claude_dir) {
        Ok(credentials) => credentials,
        Err(reason) => return FetchOutcome::NoCredentials(reason),
    };

    let response = okena_transport::http::send(
        okena_transport::http::HttpRequest::get(USAGE_URL)
            .bearer(&credentials.access_token)
            .header("anthropic-beta", "oauth-2025-04-20")
            .timeout(Duration::from_secs(10))
            .label("claude.usage")
            // Safety floor: real cadence is 300s (≥30s on retry, ≥60s on a
            // client refresh); a 5s floor only ever catches a runaway loop.
            .min_interval(Duration::from_secs(5)),
    );

    let resp = match response {
        Ok(resp) => resp,
        Err(error) => {
            log::warn!("[claude-usage] request failed: {error}");
            return FetchOutcome::Failed(
                "Could not connect to the Claude usage API (network error or timeout)".into(),
            );
        }
    };
    let status = resp.status();
    log::debug!("[claude-usage] HTTP {status}");
    if status == 429 {
        return FetchOutcome::RateLimited {
            retry_after: parse_retry_after(resp.header("retry-after")),
        };
    }
    if status == 401 || status == 403 {
        return FetchOutcome::Failed(format!(
            "Claude rejected the token (HTTP {status}); run `claude` to sign in again"
        ));
    }
    if !resp.is_success() {
        return FetchOutcome::Failed(format!("Usage request failed: HTTP {status}"));
    }
    let parsed: serde_json::Value = match resp.json() {
        Ok(value) => value,
        Err(_) => return FetchOutcome::Failed("Invalid JSON in usage response".into()),
    };
    let mut data = parse_usage(&parsed);
    data.plan = credentials.plan;
    FetchOutcome::Ok(data)
}

/// Parse a `retry-after` header given in seconds.
pub fn parse_retry_after(value: Option<&str>) -> Option<Duration> {
    value
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

/// Usage info for a single rate-limit tier.
#[derive(Clone, Debug, PartialEq)]
struct TierUsage {
    utilization: f64,
    reset_epoch: Option<f64>,
    period_secs: f64,
}

/// A model-scoped weekly limit, e.g. a dedicated Fable bucket.
#[derive(Clone, Debug, PartialEq)]
struct ScopedTierUsage {
    label: String,
    tier: TierUsage,
    active: bool,
}

/// Parse the `/api/oauth/usage` response. `plan` is left `None`; it comes from
/// the credentials.
pub fn parse_usage(resp: &serde_json::Value) -> ProviderData {
    let mut five_hour = parse_tier(resp, "five_hour", FIVE_HOUR_SECS);
    let mut seven_day = parse_tier(resp, "seven_day", SEVEN_DAY_SECS);
    let mut weekly_scoped = Vec::new();

    if let Some(tier) = parse_tier(resp, "seven_day_sonnet", SEVEN_DAY_SECS) {
        upsert_weekly_scoped(&mut weekly_scoped, "Sonnet".to_string(), tier, false);
    }
    if let Some(tier) = parse_tier(resp, "seven_day_opus", SEVEN_DAY_SECS) {
        upsert_weekly_scoped(&mut weekly_scoped, "Opus".to_string(), tier, false);
    }

    apply_limits(resp, &mut five_hour, &mut seven_day, &mut weekly_scoped);

    let extra_usage = resp.get("extra_usage").map(|eu| ApiExtraUsage {
        is_enabled: eu["is_enabled"].as_bool().unwrap_or(false),
        monthly_limit_cents: eu["monthly_limit"].as_f64().unwrap_or(0.0),
        used_credits_cents: eu["used_credits"].as_f64().unwrap_or(0.0),
        utilization: eu["utilization"].as_f64().unwrap_or(0.0),
    });

    let mut windows = Vec::new();
    if let Some(tier) = five_hour {
        windows.push(window(
            "five_hour".into(),
            "Session".into(),
            ApiUsageWindowKind::FiveHour,
            &tier,
            true,
        ));
    }
    if let Some(tier) = seven_day {
        windows.push(window(
            "seven_day".into(),
            "Weekly".into(),
            ApiUsageWindowKind::Weekly,
            &tier,
            true,
        ));
    }
    for scoped in weekly_scoped {
        windows.push(window(
            format!("seven_day_{}", scoped.label.to_lowercase()),
            scoped.label.clone(),
            ApiUsageWindowKind::WeeklyScoped {
                model: scoped.label,
            },
            &scoped.tier,
            scoped.active,
        ));
    }

    ProviderData {
        plan: None,
        windows,
        extra_usage,
        credits: None,
        reset_credits: None,
        prepaid_balance: None,
    }
}

fn window(
    id: String,
    label: String,
    kind: ApiUsageWindowKind,
    tier: &TierUsage,
    active: bool,
) -> ApiUsageWindow {
    ApiUsageWindow {
        id,
        label,
        kind,
        used_percent: tier.utilization,
        window_seconds: tier.period_secs,
        reset_at: tier.reset_epoch,
        active,
        history: Vec::new(),
    }
}

fn parse_tier(resp: &serde_json::Value, key: &str, period_secs: f64) -> Option<TierUsage> {
    let tier = resp.get(key)?.as_object()?;
    let reset_epoch = tier
        .get("resets_at")
        .and_then(|v| v.as_str())
        .and_then(parse_iso8601_to_epoch);
    Some(TierUsage {
        utilization: tier
            .get("utilization")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
        reset_epoch,
        period_secs,
    })
}

fn apply_limits(
    resp: &serde_json::Value,
    five_hour: &mut Option<TierUsage>,
    seven_day: &mut Option<TierUsage>,
    weekly_scoped: &mut Vec<ScopedTierUsage>,
) {
    let Some(limits) = resp.get("limits").and_then(|v| v.as_array()) else {
        return;
    };

    for limit in limits {
        let kind = limit.get("kind").and_then(|v| v.as_str());
        let group = limit.get("group").and_then(|v| v.as_str());
        let scoped_label = scoped_limit_label(limit);

        if kind == Some("session") || group == Some("session") {
            let fallback_reset = five_hour.as_ref().and_then(|tier| tier.reset_epoch);
            if let Some(tier) = parse_limit_tier(limit, FIVE_HOUR_SECS, fallback_reset) {
                *five_hour = Some(tier);
            }
            continue;
        }

        let scope_is_empty = limit.get("scope").is_none_or(|scope| scope.is_null());
        if kind == Some("weekly_all") || (group == Some("weekly") && scope_is_empty) {
            let fallback_reset = seven_day.as_ref().and_then(|tier| tier.reset_epoch);
            if let Some(tier) = parse_limit_tier(limit, SEVEN_DAY_SECS, fallback_reset) {
                *seven_day = Some(tier);
            }
            continue;
        }

        if kind == Some("weekly_scoped") || (group == Some("weekly") && scoped_label.is_some()) {
            let Some(label) = scoped_label else {
                continue;
            };
            let fallback_reset = scoped_reset_epoch(weekly_scoped, &label)
                .or_else(|| seven_day.as_ref().and_then(|tier| tier.reset_epoch));
            if let Some(tier) = parse_limit_tier(limit, SEVEN_DAY_SECS, fallback_reset) {
                let active = limit
                    .get("is_active")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                upsert_weekly_scoped(weekly_scoped, label, tier, active);
            }
        }
    }
}

fn parse_limit_tier(
    limit: &serde_json::Value,
    period_secs: f64,
    fallback_reset_epoch: Option<f64>,
) -> Option<TierUsage> {
    let utilization = limit
        .get("percent")
        .and_then(|v| v.as_f64())
        .or_else(|| limit.get("utilization").and_then(|v| v.as_f64()))?;
    let reset_epoch = limit
        .get("resets_at")
        .and_then(|v| v.as_str())
        .and_then(parse_iso8601_to_epoch)
        .or(fallback_reset_epoch);

    Some(TierUsage {
        utilization,
        reset_epoch,
        period_secs,
    })
}

fn scoped_limit_label(limit: &serde_json::Value) -> Option<String> {
    let scope = limit.get("scope")?;

    if let Some(model) = scope.get("model")
        && let Some(label) = scope_label(model)
    {
        return Some(label);
    }

    if let Some(surface) = scope.get("surface")
        && let Some(label) = scope_label(surface)
    {
        return Some(label);
    }

    None
}

fn scope_label(scope: &serde_json::Value) -> Option<String> {
    scope
        .get("display_name")
        .and_then(|v| v.as_str())
        .or_else(|| scope.get("name").and_then(|v| v.as_str()))
        .or_else(|| scope.get("id").and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(ToOwned::to_owned)
}

fn scoped_reset_epoch(weekly_scoped: &[ScopedTierUsage], label: &str) -> Option<f64> {
    weekly_scoped
        .iter()
        .find(|entry| entry.label.eq_ignore_ascii_case(label))
        .and_then(|entry| entry.tier.reset_epoch)
}

fn upsert_weekly_scoped(
    weekly_scoped: &mut Vec<ScopedTierUsage>,
    label: String,
    tier: TierUsage,
    active: bool,
) {
    if let Some(existing) = weekly_scoped
        .iter_mut()
        .find(|entry| entry.label.eq_ignore_ascii_case(&label))
    {
        existing.tier = tier;
        existing.active = active;
    } else {
        weekly_scoped.push(ScopedTierUsage {
            label,
            tier,
            active,
        });
    }
}

/// Parse an ISO 8601 timestamp (via `jiff`) to Unix epoch seconds.
pub fn parse_iso8601_to_epoch(ts: &str) -> Option<f64> {
    let timestamp: jiff::Timestamp = ts.parse().ok()?;
    Some(timestamp.as_millisecond() as f64 / 1_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> serde_json::Value {
        serde_json::from_str(include_str!("../tests/fixtures/claude_usage.json")).unwrap()
    }

    fn scoped<'a>(data: &'a ProviderData, label: &str) -> &'a ApiUsageWindow {
        data.windows
            .iter()
            .find(|w| w.label == label)
            .unwrap_or_else(|| panic!("no {label} window"))
    }

    #[test]
    fn parses_fixture_into_wire_windows() {
        let data = parse_usage(&fixture());
        let ids: Vec<&str> = data.windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "five_hour",
                "seven_day",
                "seven_day_sonnet",
                "seven_day_fable"
            ]
        );

        let session = &data.windows[0];
        assert_eq!(session.label, "Session");
        assert_eq!(session.kind, ApiUsageWindowKind::FiveHour);
        assert_eq!(session.used_percent, 46.0);
        assert_eq!(session.window_seconds, 18000.0);
        assert_eq!(
            session.reset_at,
            parse_iso8601_to_epoch("2026-07-07T14:00:00.000Z")
        );

        let weekly = &data.windows[1];
        assert_eq!(weekly.kind, ApiUsageWindowKind::Weekly);
        assert_eq!(weekly.used_percent, 79.0);
        assert_eq!(weekly.window_seconds, 604800.0);

        let fable = scoped(&data, "Fable");
        assert_eq!(
            fable.kind,
            ApiUsageWindowKind::WeeklyScoped {
                model: "Fable".into()
            }
        );
        assert!(fable.active);
        assert_eq!(fable.used_percent, 100.0);
        // No own reset → falls back to the weekly reset.
        assert_eq!(fable.reset_at, weekly.reset_at);

        let sonnet = scoped(&data, "Sonnet");
        assert!(!sonnet.active);
        assert_eq!(sonnet.used_percent, 3.0);

        let extra = data.extra_usage.expect("extra usage");
        assert!(extra.is_enabled);
        assert_eq!(extra.monthly_limit_cents, 5000.0);
        assert_eq!(extra.used_credits_cents, 1234.0);
        assert_eq!(extra.utilization, 24.68);
        assert!(data.plan.is_none(), "plan comes from credentials");
    }

    #[test]
    fn test_parse_usage_applies_limits_with_top_level_reset_fallback() {
        let resp = serde_json::json!({
            "five_hour": { "utilization": 10.0, "resets_at": "2026-07-07T14:00:00.000Z" },
            "seven_day": { "utilization": 20.0, "resets_at": "2026-07-08T21:00:00.000Z" },
            "limits": [
                { "kind": "session", "group": "session", "percent": 46, "resets_at": null, "scope": null },
                { "kind": "weekly_all", "group": "weekly", "percent": 79, "resets_at": null, "scope": null },
                {
                    "kind": "weekly_scoped", "group": "weekly", "percent": 100, "resets_at": null,
                    "scope": { "model": { "id": null, "display_name": "Fable" }, "surface": null },
                    "is_active": true
                }
            ],
            "extra_usage": { "is_enabled": false }
        });

        let usage = parse_usage(&resp);
        let five_hour = &usage.windows[0];
        let seven_day = &usage.windows[1];
        let fable = scoped(&usage, "Fable");

        assert_eq!(five_hour.used_percent, 46.0);
        assert_eq!(
            five_hour.reset_at,
            parse_iso8601_to_epoch("2026-07-07T14:00:00.000Z")
        );
        assert_eq!(seven_day.used_percent, 79.0);
        assert_eq!(
            seven_day.reset_at,
            parse_iso8601_to_epoch("2026-07-08T21:00:00.000Z")
        );
        assert_eq!(fable.used_percent, 100.0);
        assert_eq!(
            fable.reset_at,
            parse_iso8601_to_epoch("2026-07-08T21:00:00.000Z")
        );
        assert!(fable.active);
        assert!(!usage.extra_usage.unwrap().is_enabled);
    }

    #[test]
    fn test_parse_usage_scoped_limit_uses_own_reset_when_present() {
        let resp = serde_json::json!({
            "seven_day": { "utilization": 20.0, "resets_at": "2026-07-08T21:00:00.000Z" },
            "limits": [{
                "kind": "weekly_scoped", "group": "weekly", "percent": 50,
                "resets_at": "2026-07-09T09:30:00.000Z",
                "scope": { "model": { "display_name": "Fable" } },
                "is_active": false
            }]
        });

        let usage = parse_usage(&resp);
        assert_eq!(
            scoped(&usage, "Fable").reset_at,
            parse_iso8601_to_epoch("2026-07-09T09:30:00.000Z")
        );
    }

    #[test]
    fn test_parse_usage_limits_override_legacy_scoped_bucket() {
        let resp = serde_json::json!({
            "seven_day": { "utilization": 20.0, "resets_at": "2026-07-08T21:00:00.000Z" },
            "seven_day_opus": { "utilization": 12.0, "resets_at": "2026-07-08T20:00:00.000Z" },
            "limits": [{
                "kind": "weekly_scoped", "group": "weekly", "percent": 88, "resets_at": null,
                "scope": { "model": { "display_name": "Opus" } },
                "is_active": true
            }]
        });

        let usage = parse_usage(&resp);
        let opus: Vec<_> = usage.windows.iter().filter(|w| w.label == "Opus").collect();
        assert_eq!(opus.len(), 1);
        assert_eq!(opus[0].used_percent, 88.0);
        assert_eq!(
            opus[0].reset_at,
            parse_iso8601_to_epoch("2026-07-08T20:00:00.000Z")
        );
        assert!(opus[0].active);
        assert_eq!(opus[0].id, "seven_day_opus");
    }

    #[test]
    fn empty_response_has_no_windows() {
        let data = parse_usage(&serde_json::json!({}));
        assert!(data.windows.is_empty());
        assert!(data.extra_usage.is_none());
    }

    #[test]
    fn credentials_from_file_with_plan() {
        let dir = tempfile::tempdir().unwrap();
        let creds = serde_json::json!({
            "claudeAiOauth": { "accessToken": "test-token-abc", "subscriptionType": "max" }
        });
        std::fs::write(dir.path().join(".credentials.json"), creds.to_string()).unwrap();
        // The file-based path wins over Keychain when a valid file is present.
        let credentials = read_credentials(dir.path()).unwrap();
        assert_eq!(credentials.access_token, "test-token-abc");
        assert_eq!(credentials.plan.as_deref(), Some("max"));
        assert!(!format!("{credentials:?}").contains("test-token-abc"));
    }

    #[test]
    fn extract_rejects_empty_and_expired_tokens() {
        let empty = serde_json::json!({ "claudeAiOauth": { "accessToken": "" } });
        assert_eq!(
            extract_credentials(&empty.to_string(), 1_000).unwrap_err(),
            TokenProblem::Missing
        );
        let expired = serde_json::json!({
            "claudeAiOauth": { "accessToken": "expired-token", "expiresAt": 999 }
        });
        assert_eq!(
            extract_credentials(&expired.to_string(), 1_000).unwrap_err(),
            TokenProblem::Expired
        );
        let fresh = serde_json::json!({
            "claudeAiOauth": { "accessToken": "fresh-token", "expiresAt": 1_001 }
        });
        assert_eq!(
            extract_credentials(&fresh.to_string(), 1_000)
                .unwrap()
                .access_token,
            "fresh-token"
        );
        assert_eq!(
            extract_credentials("not json", 1_000).unwrap_err(),
            TokenProblem::Missing
        );
    }

    #[test]
    fn retry_after_parses_seconds_only() {
        assert_eq!(
            parse_retry_after(Some("120")),
            Some(Duration::from_secs(120))
        );
        assert_eq!(parse_retry_after(Some(" 0 ")), Some(Duration::from_secs(0)));
        assert_eq!(parse_retry_after(Some("Wed, 21 Oct 2015")), None);
        assert_eq!(parse_retry_after(None), None);
    }

    #[test]
    fn test_parse_iso8601_to_epoch() {
        // 2025-01-01T00:00:00Z = 1735689600
        let epoch = parse_iso8601_to_epoch("2025-01-01T00:00:00.000Z").unwrap();
        assert!((epoch - 1735689600.0).abs() < 1.0);
        assert!(parse_iso8601_to_epoch("not-a-date").is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_keychain_service_default() {
        let default_dir = dirs::home_dir().unwrap().join(".claude");
        // Only meaningful where ~/.claude exists (developer machines).
        if default_dir.exists() {
            assert_eq!(
                keychain_service_name(&default_dir),
                "Claude Code-credentials"
            );
            let names = keychain_service_names(&default_dir);
            assert_eq!(names.len(), 2);
            assert_eq!(names[0], "Claude Code-credentials");
            assert!(names[1].starts_with("Claude Code-credentials-"));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_keychain_service_custom() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap();
        let service = keychain_service_name(&path);

        let mut h = Sha256::new();
        h.update(path.to_string_lossy().as_bytes());
        let d = h.finalize();
        let expected = format!(
            "Claude Code-credentials-{:02x}{:02x}{:02x}{:02x}",
            d[0], d[1], d[2], d[3]
        );
        assert_eq!(service, expected);
        assert_eq!(keychain_service_names(dir.path()), vec![expected]);
    }
}
