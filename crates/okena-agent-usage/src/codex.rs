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

/// Result of an OAuth refresh-token exchange.
#[derive(Debug, PartialEq)]
enum RefreshError {
    /// The token endpoint rejected the refresh token (HTTP 400/401, e.g.
    /// `invalid_grant`): the user has to sign in again.
    Rejected,
    /// Network/transport failure, 5xx, or an unexpected response. Transient:
    /// retried with the normal backoff while the last data stays visible.
    Transient(String),
}

/// Form-urlencode one value (`application/x-www-form-urlencoded`).
fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn refresh_request_body(refresh_token: &str) -> String {
    format!(
        "grant_type=refresh_token&client_id={}&refresh_token={}",
        form_encode(CODEX_CLIENT_ID),
        form_encode(refresh_token)
    )
}

/// New tokens from a successful refresh response.
#[derive(Debug)]
struct RefreshedTokens {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
}

/// Classify the token endpoint's answer. Only an explicit 400/401 means the
/// refresh token is dead; everything else is transient.
fn classify_refresh_response(
    response: Result<okena_transport::http::HttpResponse, okena_transport::http::HttpError>,
) -> Result<RefreshedTokens, RefreshError> {
    let response = match response {
        Ok(response) => response,
        Err(_) => {
            return Err(RefreshError::Transient(
                "Codex token refresh failed (network error or timeout)".into(),
            ));
        }
    };
    match response.status() {
        400 | 401 => return Err(RefreshError::Rejected),
        status if !response.is_success() => {
            return Err(RefreshError::Transient(format!(
                "Codex token refresh failed: HTTP {status}"
            )));
        }
        _ => {}
    }
    let body: serde_json::Value = response
        .json()
        .map_err(|_| RefreshError::Transient("Codex token refresh returned invalid JSON".into()))?;
    let token = |key: &str| {
        body[key]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    };
    let access_token = token("access_token").ok_or_else(|| {
        RefreshError::Transient("Codex token refresh response had no access token".into())
    })?;
    Ok(RefreshedTokens {
        access_token,
        refresh_token: token("refresh_token"),
        id_token: token("id_token"),
    })
}

/// Refresh the OAuth access token using the refresh token and persist the new
/// tokens back to `auth.json` (as the Codex CLI would).
fn refresh_access_token(auth: &CodexAuth) -> Result<String, RefreshError> {
    refresh_access_token_with(auth, |refresh_token| {
        classify_refresh_response(okena_transport::http::send(
            okena_transport::http::HttpRequest::post(TOKEN_URL)
                .body(
                    "application/x-www-form-urlencoded",
                    refresh_request_body(refresh_token),
                )
                .timeout(Duration::from_secs(10))
                .user_agent(USER_AGENT)
                .label("codex.token-refresh"),
        ))
    })
}

/// [`refresh_access_token`] with the token endpoint call injected (tests).
///
/// The provider rotates the refresh token, and every Okena daemon on the
/// machine shares `auth.json`, so the exchange runs under an exclusive lock
/// file. Whoever waited re-reads the file and uses the tokens another process
/// stored meanwhile, including one the Codex CLI wrote, instead of spending a
/// refresh token that is already dead.
fn refresh_access_token_with(
    auth: &CodexAuth,
    exchange: impl FnOnce(&str) -> Result<RefreshedTokens, RefreshError>,
) -> Result<String, RefreshError> {
    let _lock = lock_refresh(&auth.auth_path)
        .inspect_err(|error| {
            log::warn!("[codex-usage] refreshing without the auth.json lock: {error}");
        })
        .ok();
    if let Some(current) = read_auth_from(auth.auth_path.clone())
        && (current.access_token != auth.access_token
            || current.refresh_token != auth.refresh_token)
    {
        log::debug!("[codex-usage] auth.json changed meanwhile; using its tokens");
        return Ok(current.access_token);
    }
    let tokens = exchange(&auth.refresh_token)?;
    if let Err(error) = persist_refreshed_tokens(&auth.auth_path, &tokens) {
        // The new token still works for this fetch; the CLI keeps its own
        // copy. Log without any token material.
        log::warn!("[codex-usage] could not write refreshed tokens to auth.json: {error}");
    }
    Ok(tokens.access_token)
}

/// Take the exclusive lock that serializes Okena's refreshes of `auth_path`.
/// A separate file, because `auth.json` itself is replaced by rename.
fn lock_refresh(auth_path: &std::path::Path) -> std::io::Result<std::fs::File> {
    let lock_path = auth_path.with_file_name(".auth.json.okena.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)?;
    file.lock()?;
    Ok(file)
}

/// `last_refresh` in the Codex CLI's format: RFC 3339 UTC, microseconds.
fn last_refresh_now() -> String {
    format!("{:.6}", jiff::Timestamp::now())
}

/// Write the refreshed tokens into `auth.json`, keeping every other field.
/// Atomic: a 0600 temp file in the same directory, fsync, rename.
fn persist_refreshed_tokens(
    path: &std::path::Path,
    tokens: &RefreshedTokens,
) -> std::io::Result<()> {
    let content = std::fs::read_to_string(path)?;
    let mut file_json: serde_json::Value = serde_json::from_str(&content)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let Some(root) = file_json.as_object_mut() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "auth.json is not an object",
        ));
    };
    let Some(file_tokens) = root.get_mut("tokens").and_then(|t| t.as_object_mut()) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "auth.json has no tokens object",
        ));
    };
    let mut set = |key: &str, value: &str| {
        file_tokens.insert(
            key.to_string(),
            serde_json::Value::String(value.to_string()),
        );
    };
    set("access_token", &tokens.access_token);
    if let Some(refresh) = &tokens.refresh_token {
        set("refresh_token", refresh);
    }
    if let Some(id_token) = &tokens.id_token {
        set("id_token", id_token);
    }
    root.insert(
        "last_refresh".to_string(),
        serde_json::Value::String(last_refresh_now()),
    );
    let updated = serde_json::to_string_pretty(&file_json)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    write_atomic_private(path, updated.as_bytes())
}

fn write_atomic_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let dir = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "auth.json has no parent")
    })?;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "auth.json".into());
    let tmp = dir.join(format!(".{file_name}.okena-{}.tmp", std::process::id()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        #[cfg(unix)]
        {
            // `mode` only applies on creation; a leftover temp file keeps its
            // old permissions otherwise.
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)?;
        #[cfg(unix)]
        if let Ok(dir) = std::fs::File::open(dir) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
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
            access_token = match refresh_access_token(&auth) {
                Ok(new_token) => new_token,
                Err(RefreshError::Rejected) => {
                    return (
                        FetchOutcome::NoCredentials(
                            "Codex token refresh was rejected; sign in again with the Codex CLI"
                                .into(),
                        ),
                        account_id,
                    );
                }
                Err(RefreshError::Transient(error)) => {
                    return (FetchOutcome::Failed(error), account_id);
                }
            };
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
        prepaid_balance: None,
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

    fn write_auth(path: &std::path::Path, access: &str, refresh: &str) {
        let auth = serde_json::json!({
            "tokens": { "access_token": access, "refresh_token": refresh, "account_id": "acct-1" }
        });
        std::fs::write(path, auth.to_string()).unwrap();
    }

    #[test]
    fn a_refresh_uses_tokens_another_process_stored_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        write_auth(&path, "old-access", "old-refresh");
        let auth = read_auth_from(path.clone()).unwrap();
        write_auth(&path, "their-access", "their-refresh");

        let token = refresh_access_token_with(&auth, |_| panic!("refreshed a rotated token"));
        assert_eq!(token, Ok("their-access".to_string()));
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["tokens"]["refresh_token"], "their-refresh");
    }

    #[test]
    fn a_refresh_of_unchanged_tokens_exchanges_and_stores_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        write_auth(&path, "old-access", "old-refresh");
        let auth = read_auth_from(path.clone()).unwrap();

        let token = refresh_access_token_with(&auth, |refresh_token| {
            assert_eq!(refresh_token, "old-refresh");
            Ok(RefreshedTokens {
                access_token: "new-access".into(),
                refresh_token: Some("new-refresh".into()),
                id_token: None,
            })
        });
        assert_eq!(token, Ok("new-access".to_string()));
        let reread = read_auth_from(path).unwrap();
        assert_eq!(reread.access_token, "new-access");
        assert_eq!(reread.refresh_token, "new-refresh");
        // The lock is released with the refresh: the next one can take it.
        let lock = std::fs::File::open(dir.path().join(".auth.json.okena.lock")).unwrap();
        assert!(lock.try_lock().is_ok());
    }

    #[test]
    fn auth_file_parsing_and_token_write_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(
            &path,
            serde_json::json!({
                "auth_mode": "chatgpt",
                "OPENAI_API_KEY": null,
                "last_refresh": "2020-01-01T00:00:00.000000Z",
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

        persist_refreshed_tokens(
            &path,
            &RefreshedTokens {
                access_token: "new-access".into(),
                refresh_token: Some("new-refresh".into()),
                id_token: None,
            },
        )
        .unwrap();
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["tokens"]["access_token"], "new-access");
        assert_eq!(written["tokens"]["refresh_token"], "new-refresh");
        assert_eq!(written["tokens"]["id_token"], "keep-me");
        assert_eq!(written["auth_mode"], "chatgpt");
        assert!(written["OPENAI_API_KEY"].is_null());
        let last_refresh = written["last_refresh"].as_str().unwrap();
        assert!(last_refresh.ends_with('Z'), "{last_refresh}");
        assert!(last_refresh.parse::<jiff::Timestamp>().is_ok());
        assert_ne!(last_refresh, "2020-01-01T00:00:00.000000Z");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name() != "auth.json")
            .collect();
        assert!(leftovers.is_empty(), "temp file left behind");

        std::fs::write(&path, r#"{"tokens": {"access_token": "x"}}"#).unwrap();
        assert!(read_auth_from(path).is_none());
    }

    fn response(status: u16, body: &str) -> okena_transport::http::HttpResponse {
        okena_transport::http::HttpResponse::new(status, Vec::new(), body.as_bytes().to_vec())
    }

    #[test]
    fn refresh_errors_only_sign_out_on_400_and_401() {
        assert_eq!(
            classify_refresh_response(Ok(response(400, r#"{"error":"invalid_grant"}"#)))
                .unwrap_err(),
            RefreshError::Rejected
        );
        assert_eq!(
            classify_refresh_response(Ok(response(401, ""))).unwrap_err(),
            RefreshError::Rejected
        );
        for status in [403, 429, 500, 503] {
            assert!(matches!(
                classify_refresh_response(Ok(response(status, ""))).unwrap_err(),
                RefreshError::Transient(_)
            ));
        }
        assert!(matches!(
            classify_refresh_response(Err(okena_transport::http::HttpError::Transport(
                "timed out".into()
            )))
            .unwrap_err(),
            RefreshError::Transient(_)
        ));
        assert!(matches!(
            classify_refresh_response(Ok(response(200, "not json"))).unwrap_err(),
            RefreshError::Transient(_)
        ));
        let tokens = classify_refresh_response(Ok(response(
            200,
            r#"{"access_token":"a","refresh_token":"r","id_token":"i"}"#,
        )))
        .unwrap();
        assert_eq!(tokens.access_token, "a");
        assert_eq!(tokens.refresh_token.as_deref(), Some("r"));
        assert_eq!(tokens.id_token.as_deref(), Some("i"));
    }

    #[test]
    fn refresh_body_is_form_encoded() {
        assert_eq!(
            refresh_request_body("a+b/c=d&e f"),
            format!(
                "grant_type=refresh_token&client_id={CODEX_CLIENT_ID}&refresh_token=a%2Bb%2Fc%3Dd%26e%20f"
            )
        );
    }
}
