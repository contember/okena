//! Codex usage: `~/.codex/auth.json` credentials with OAuth token refresh
//! (written back to the file), the `chatgpt.com/backend-api/codex/usage`
//! fetch, reset credits, and response parsing.

use std::cmp::Ordering;
use std::path::PathBuf;
use std::time::Duration;

use okena_core::api::{
    ApiResetCredit, ApiResetCredits, ApiUsageCredits, ApiUsageWindow, ApiUsageWindowKind,
};

use crate::claude::{parse_iso8601_to_epoch, parse_retry_after};
use crate::provider::{FetchOutcome, ProviderData};

/// Codex OAuth client ID (public, embedded in the Codex CLI binary).
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const USAGE_URL: &str = "https://chatgpt.com/backend-api/codex/usage";
const RESET_CREDITS_URL: &str = "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const USER_AGENT: &str = concat!("okena/", env!("CARGO_PKG_VERSION"));

const NO_CREDENTIALS: &str = "Codex credentials missing or invalid; sign in with the Codex CLI";

pub struct CodexAuth {
    access_token: String,
    refresh_token: String,
    pub account_id: String,
    auth_path: PathBuf,
}

impl std::fmt::Debug for CodexAuth {
    // Never print tokens.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexAuth")
            .field("account_id", &self.account_id)
            .field("auth_path", &self.auth_path)
            .finish_non_exhaustive()
    }
}

/// Path of the Codex CLI credentials file.
pub fn auth_path() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".codex/auth.json"))
}

/// Read Codex OAuth credentials from `~/.codex/auth.json`.
pub fn read_auth() -> Option<CodexAuth> {
    read_auth_from(auth_path()?)
}

fn read_auth_from(path: PathBuf) -> Option<CodexAuth> {
    let content = std::fs::read_to_string(&path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    let tokens = &v["tokens"];
    Some(CodexAuth {
        access_token: tokens["access_token"].as_str()?.to_string(),
        refresh_token: tokens["refresh_token"].as_str()?.to_string(),
        account_id: tokens["account_id"].as_str()?.to_string(),
        auth_path: path,
    })
}

/// Refresh the OAuth access token using the refresh token and persist the new
/// tokens back to `auth.json` (as the Codex CLI would).
fn refresh_access_token(auth: &CodexAuth) -> Option<String> {
    let resp: serde_json::Value = okena_transport::http::send(
        okena_transport::http::HttpRequest::post(TOKEN_URL)
            .body(
                "application/x-www-form-urlencoded",
                format!(
                    "grant_type=refresh_token&client_id={}&refresh_token={}",
                    CODEX_CLIENT_ID, auth.refresh_token
                ),
            )
            .timeout(Duration::from_secs(10))
            .user_agent(USER_AGENT)
            .label("codex.token-refresh"),
    )
    .ok()?
    .json()
    .ok()?;

    let new_access = resp["access_token"].as_str()?;
    let new_refresh = resp["refresh_token"].as_str();
    persist_refreshed_tokens(&auth.auth_path, new_access, new_refresh);
    Some(new_access.to_string())
}

fn persist_refreshed_tokens(path: &std::path::Path, access: &str, refresh: Option<&str>) {
    if let Ok(content) = std::fs::read_to_string(path)
        && let Ok(mut file_json) = serde_json::from_str::<serde_json::Value>(&content)
    {
        if let Some(tokens) = file_json.get_mut("tokens").and_then(|t| t.as_object_mut()) {
            tokens.insert(
                "access_token".to_string(),
                serde_json::Value::String(access.to_string()),
            );
            if let Some(rt) = refresh {
                tokens.insert(
                    "refresh_token".to_string(),
                    serde_json::Value::String(rt.to_string()),
                );
            }
        }
        if let Ok(updated) = serde_json::to_string_pretty(&file_json) {
            let _ = std::fs::write(path, updated);
        }
    }
}

enum RequestError {
    Network,
    Status(u16, Option<Duration>),
}

fn get_with_token(
    url: &str,
    label: &'static str,
    access_token: &str,
    account_id: &str,
) -> Result<okena_transport::http::HttpResponse, RequestError> {
    // No min_interval floor: one tick can legitimately issue two requests
    // with this label (cached token → 401 → refresh → retry).
    let resp = okena_transport::http::send(
        okena_transport::http::HttpRequest::get(url)
            .bearer(access_token)
            .header("chatgpt-account-id", account_id)
            .timeout(Duration::from_secs(10))
            .user_agent(USER_AGENT)
            .header("Accept", "application/json")
            .label(label),
    )
    .map_err(|_| RequestError::Network)?;
    if resp.is_success() {
        Ok(resp)
    } else {
        let retry_after = parse_retry_after(resp.header("retry-after"));
        Err(RequestError::Status(resp.status(), retry_after))
    }
}

fn fetch_reset_credits(access_token: &str, account_id: &str) -> Option<ApiResetCredits> {
    let resp = match get_with_token(
        RESET_CREDITS_URL,
        "codex.reset-credits",
        access_token,
        account_id,
    ) {
        Ok(resp) => resp,
        Err(RequestError::Status(status, _)) => {
            log::warn!("[codex-usage] reset credits API returned HTTP {status}");
            return None;
        }
        Err(RequestError::Network) => {
            log::warn!("[codex-usage] reset credits request failed");
            return None;
        }
    };
    let body: serde_json::Value = match resp.json() {
        Ok(body) => body,
        Err(error) => {
            log::warn!("[codex-usage] failed to decode reset credits: {error}");
            return None;
        }
    };
    let parsed = parse_reset_credits(&body);
    if parsed.is_none() {
        log::warn!("[codex-usage] reset credits response had an unexpected shape");
    }
    parsed
}

fn request_error_outcome(error: RequestError) -> FetchOutcome {
    match error {
        RequestError::Status(429, retry_after) => FetchOutcome::RateLimited { retry_after },
        RequestError::Status(code, _) => {
            FetchOutcome::Failed(format!("Usage request failed: HTTP {code}"))
        }
        RequestError::Network => {
            FetchOutcome::Failed("Could not connect to usage API (network error or timeout)".into())
        }
    }
}

/// Fetch usage + reset credits. Returns the outcome and the account id (the
/// history source) when credentials were readable.
pub fn fetch() -> (FetchOutcome, Option<String>) {
    let Some(auth) = read_auth() else {
        return (FetchOutcome::NoCredentials(NO_CREDENTIALS.into()), None);
    };
    let account_id = Some(auth.account_id.clone());
    let mut access_token = auth.access_token.clone();

    // Try the cached access token first, refresh on 401.
    let resp = match get_with_token(USAGE_URL, "codex.usage", &access_token, &auth.account_id) {
        Ok(resp) => resp,
        Err(RequestError::Status(401, _)) => {
            let Some(new_token) = refresh_access_token(&auth) else {
                return (
                    FetchOutcome::NoCredentials(
                        "Codex token refresh failed; sign in again with the Codex CLI".into(),
                    ),
                    account_id,
                );
            };
            access_token = new_token;
            match get_with_token(USAGE_URL, "codex.usage", &access_token, &auth.account_id) {
                Ok(resp) => resp,
                Err(error) => {
                    log::warn!("[codex-usage] usage request failed after token refresh");
                    return (request_error_outcome(error), account_id);
                }
            }
        }
        Err(error) => return (request_error_outcome(error), account_id),
    };

    let body: serde_json::Value = match resp.json() {
        Ok(body) => body,
        Err(_) => {
            return (
                FetchOutcome::Failed("Invalid JSON in usage response".into()),
                account_id,
            );
        }
    };
    let mut data = match parse_usage(&body) {
        Ok(data) => data,
        Err(error) => return (FetchOutcome::Failed(error), account_id),
    };
    data.reset_credits = fetch_reset_credits(&access_token, &auth.account_id);
    (FetchOutcome::Ok(data), account_id)
}

/// Classify a rate-limit window by its length.
fn kind_for_window(window_seconds: f64) -> ApiUsageWindowKind {
    if (window_seconds - 5.0 * 3600.0).abs() <= 3600.0 {
        ApiUsageWindowKind::FiveHour
    } else if (window_seconds - 7.0 * 86400.0).abs() <= 86400.0 {
        ApiUsageWindowKind::Weekly
    } else {
        ApiUsageWindowKind::Other
    }
}

fn parse_window(
    v: &serde_json::Value,
    id: &str,
    label: &str,
    kind: Option<ApiUsageWindowKind>,
) -> Option<ApiUsageWindow> {
    v.as_object()?;
    let used = v["used_percent"].as_f64()?;
    let window_seconds = v["limit_window_seconds"].as_u64().unwrap_or(0) as f64;
    let reset_at = v["reset_at"].as_u64().filter(|&r| r > 0).map(|r| r as f64);
    Some(ApiUsageWindow {
        id: id.into(),
        label: label.into(),
        kind: kind.unwrap_or_else(|| kind_for_window(window_seconds)),
        used_percent: used,
        window_seconds,
        reset_at,
        active: true,
        history: Vec::new(),
    })
}

/// Parse the `/codex/usage` response. Errors when it carries no rate-limit
/// window at all (a zero-usage window still counts).
pub fn parse_usage(body: &serde_json::Value) -> Result<ProviderData, String> {
    let plan = body["plan_type"].as_str().unwrap_or("unknown").to_string();

    let primary = parse_window(
        &body["rate_limit"]["primary_window"],
        "primary",
        "Rate Limit",
        None,
    );
    let secondary = parse_window(
        &body["rate_limit"]["secondary_window"],
        "secondary",
        "Secondary",
        None,
    );
    if primary.is_none() && secondary.is_none() {
        return Err("Usage response contains no rate-limit windows".into());
    }
    let review = parse_window(
        &body["code_review_rate_limit"]["primary_window"],
        "code_review",
        "Code Review",
        Some(ApiUsageWindowKind::CodeReview),
    );

    let credits = body["credits"].as_object().map(|c| ApiUsageCredits {
        has_credits: c
            .get("has_credits")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        unlimited: c
            .get("unlimited")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        balance: c.get("balance").and_then(|v| v.as_f64()).unwrap_or(0.0),
    });

    Ok(ProviderData {
        plan: Some(plan),
        windows: [primary, secondary, review].into_iter().flatten().collect(),
        extra_usage: None,
        credits,
        reset_credits: None,
    })
}

/// Parse the reset-credits response: available credits only, soonest expiry
/// first, credits without expiry last.
pub fn parse_reset_credits(body: &serde_json::Value) -> Option<ApiResetCredits> {
    let raw_credits = body["credits"].as_array()?;
    let mut credits: Vec<ApiResetCredit> = raw_credits
        .iter()
        .filter(|credit| credit["status"].as_str() == Some("available"))
        .map(|credit| ApiResetCredit {
            title: credit["title"]
                .as_str()
                .filter(|title| !title.trim().is_empty())
                .unwrap_or("Full reset")
                .to_string(),
            expires_at: credit["expires_at"]
                .as_str()
                .and_then(parse_iso8601_to_epoch),
        })
        .collect();

    credits.sort_by(|left, right| match (left.expires_at, right.expires_at) {
        (Some(left), Some(right)) => left.total_cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });

    let available_count = body["available_count"]
        .as_u64()
        .unwrap_or(credits.len() as u64);

    Some(ApiResetCredits {
        available_count,
        credits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> serde_json::Value {
        let text = match name {
            "usage" => include_str!("../tests/fixtures/codex_usage.json"),
            _ => include_str!("../tests/fixtures/codex_reset_credits.json"),
        };
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn parses_usage_fixture() {
        let data = parse_usage(&fixture("usage")).unwrap();
        assert_eq!(data.plan.as_deref(), Some("plus"));
        let ids: Vec<&str> = data.windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, vec!["primary", "secondary", "code_review"]);

        let primary = &data.windows[0];
        assert_eq!(primary.label, "Rate Limit");
        assert_eq!(primary.kind, ApiUsageWindowKind::FiveHour);
        assert_eq!(primary.used_percent, 23.0);
        assert_eq!(primary.window_seconds, 18000.0);
        assert_eq!(primary.reset_at, Some(1782371508.0));

        let secondary = &data.windows[1];
        assert_eq!(secondary.kind, ApiUsageWindowKind::Weekly);
        assert_eq!(secondary.used_percent, 41.6);

        let review = &data.windows[2];
        assert_eq!(review.kind, ApiUsageWindowKind::CodeReview);
        assert_eq!(review.used_percent, 0.0);

        let credits = data.credits.unwrap();
        assert!(credits.has_credits && !credits.unlimited);
        assert_eq!(credits.balance, 12.5);
    }

    #[test]
    fn usage_response_requires_usage_but_accepts_zero() {
        assert!(parse_usage(&serde_json::json!({})).is_err());
        assert!(
            parse_usage(&serde_json::json!({
                "rate_limit": {"primary_window": {"reset_at": 1782371508u64}}
            }))
            .is_err()
        );
        let data = parse_usage(&serde_json::json!({
            "rate_limit": {"primary_window": {
                "used_percent": 0,
                "limit_window_seconds": 604800,
                "reset_at": 1782371508u64
            }}
        }))
        .unwrap();
        assert_eq!(data.windows[0].used_percent, 0.0);
        assert_eq!(data.windows[0].kind, ApiUsageWindowKind::Weekly);
        assert_eq!(data.plan.as_deref(), Some("unknown"));
    }

    #[test]
    fn missing_reset_is_none_and_odd_lengths_are_other() {
        let data = parse_usage(&serde_json::json!({
            "rate_limit": {"primary_window": {
                "used_percent": 5,
                "limit_window_seconds": 2592000,
                "reset_at": 0
            }}
        }))
        .unwrap();
        assert_eq!(data.windows[0].reset_at, None);
        assert_eq!(data.windows[0].kind, ApiUsageWindowKind::Other);
    }

    #[test]
    fn parse_reset_credits_filters_available_and_sorts_by_expiry() {
        let parsed = parse_reset_credits(&fixture("resets")).expect("reset credits should parse");
        assert_eq!(parsed.available_count, 3);
        assert_eq!(parsed.credits.len(), 3);
        assert_eq!(parsed.credits[0].title, "First reset");
        assert_eq!(parsed.credits[1].title, "Later reset");
        assert_eq!(parsed.credits[2].title, "Full reset");
        assert!(parsed.credits[0].expires_at.is_some());
        assert!(parsed.credits[2].expires_at.is_none());
    }

    #[test]
    fn parse_reset_credits_derives_missing_count() {
        let body = serde_json::json!({
            "credits": [
                { "status": "available", "title": "Full reset", "expires_at": "2026-07-18T00:32:13Z" },
                { "status": "redeeming", "title": "In progress", "expires_at": "2026-07-19T00:32:13Z" }
            ]
        });
        let parsed = parse_reset_credits(&body).expect("reset credits should parse");
        assert_eq!(parsed.available_count, 1);
        assert_eq!(parsed.credits.len(), 1);
    }

    #[test]
    fn auth_file_parsing_and_token_write_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(
            &path,
            serde_json::json!({
                "OPENAI_API_KEY": null,
                "tokens": {
                    "access_token": "old-access",
                    "refresh_token": "old-refresh",
                    "account_id": "acct-1",
                    "id_token": "keep-me"
                }
            })
            .to_string(),
        )
        .unwrap();
        let auth = read_auth_from(path.clone()).expect("auth parses");
        assert_eq!(auth.account_id, "acct-1");
        assert!(!format!("{auth:?}").contains("old-access"));

        persist_refreshed_tokens(&path, "new-access", Some("new-refresh"));
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["tokens"]["access_token"], "new-access");
        assert_eq!(written["tokens"]["refresh_token"], "new-refresh");
        assert_eq!(written["tokens"]["id_token"], "keep-me");

        std::fs::write(&path, r#"{"tokens": {"access_token": "x"}}"#).unwrap();
        assert!(read_auth_from(path).is_none());
    }
}
