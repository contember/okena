//! Claude subscription OAuth, mirroring the manual-code flow of `claude auth login`.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use okena_core::ai_proxy::AiUsage;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub const OAUTH_BETA: &str = "oauth-2025-04-20";
const AUTHORIZE_URL: &str = "https://claude.com/cai/oauth/authorize";
const REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
const SCOPES: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

/// Upstream hosts. Tests point both at a loopback fake.
#[derive(Clone, Debug)]
pub struct Upstream {
    pub api: String,
    pub token_url: String,
}

impl Default for Upstream {
    fn default() -> Self {
        Self {
            api: "https://api.anthropic.com".into(),
            token_url: "https://platform.claude.com/v1/oauth/token".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at_ms: i64,
}

impl Tokens {
    /// Same five-minute margin the CLI uses before refreshing.
    pub fn expires_soon(&self, now_ms: i64) -> bool {
        now_ms + 5 * 60 * 1000 >= self.expires_at_ms
    }
}

pub struct Pkce {
    pub verifier: String,
    pub state: String,
}

impl Pkce {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            verifier: random_url_safe()?,
            state: random_url_safe()?,
        })
    }

    pub fn authorize_url(&self) -> anyhow::Result<String> {
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(self.verifier.as_bytes()));
        let mut url = reqwest::Url::parse(AUTHORIZE_URL)?;
        url.query_pairs_mut()
            .append_pair("code", "true")
            .append_pair("client_id", CLIENT_ID)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", REDIRECT_URI)
            .append_pair("scope", SCOPES)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &self.state);
        Ok(url.into())
    }
}

fn random_url_safe() -> anyhow::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|error| anyhow::anyhow!("getrandom: {error}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

#[derive(Deserialize)]
struct TokenReply {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
    account: Option<TokenAccount>,
}

#[derive(Deserialize)]
struct TokenAccount {
    uuid: Option<String>,
    email_address: Option<String>,
}

pub struct LoginResult {
    pub tokens: Tokens,
    pub account_uuid: Option<String>,
    pub email: Option<String>,
}

/// Exchange the code the provider displays after authorization. The page shows
/// `code#state`; a bare code is accepted too.
pub async fn exchange_code(
    http: &reqwest::Client,
    upstream: &Upstream,
    pkce: &Pkce,
    pasted: &str,
) -> anyhow::Result<LoginResult> {
    let pasted = pasted.trim();
    let (code, state) = pasted.split_once('#').unwrap_or((pasted, &pkce.state));
    anyhow::ensure!(!code.is_empty(), "Empty authorization code");
    anyhow::ensure!(state == pkce.state, "The code belongs to a different login");
    let response = http
        .post(&upstream.token_url)
        .json(&serde_json::json!({
            "grant_type": "authorization_code",
            "code": code,
            "state": state,
            "redirect_uri": REDIRECT_URI,
            "client_id": CLIENT_ID,
            "code_verifier": pkce.verifier,
        }))
        .send()
        .await?;
    let status = response.status();
    anyhow::ensure!(
        status.is_success(),
        "Token exchange failed ({status}); the code may be expired or already used"
    );
    let reply: TokenReply = response.json().await?;
    let account = reply.account.as_ref();
    Ok(LoginResult {
        account_uuid: account.and_then(|account| account.uuid.clone()),
        email: account.and_then(|account| account.email_address.clone()),
        tokens: tokens_from(
            reply.access_token,
            reply.refresh_token,
            reply.expires_in,
            None,
        )?,
    })
}

pub enum RefreshError {
    /// The provider rejected the refresh token; only a new login helps.
    Rejected,
    Transient(anyhow::Error),
}

pub async fn refresh(
    http: &reqwest::Client,
    upstream: &Upstream,
    tokens: &Tokens,
) -> Result<Tokens, RefreshError> {
    let response = http
        .post(&upstream.token_url)
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": tokens.refresh_token,
            "client_id": CLIENT_ID,
        }))
        .send()
        .await
        .map_err(|error| RefreshError::Transient(error.into()))?;
    let status = response.status();
    if status == reqwest::StatusCode::BAD_REQUEST || status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(RefreshError::Rejected);
    }
    if !status.is_success() {
        return Err(RefreshError::Transient(anyhow::anyhow!(
            "token refresh returned {status}"
        )));
    }
    let reply: TokenReply = response
        .json()
        .await
        .map_err(|error| RefreshError::Transient(error.into()))?;
    tokens_from(
        reply.access_token,
        reply.refresh_token,
        reply.expires_in,
        Some(&tokens.refresh_token),
    )
    .map_err(RefreshError::Transient)
}

fn tokens_from(
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
    previous_refresh: Option<&str>,
) -> anyhow::Result<Tokens> {
    let refresh_token = refresh_token
        .or_else(|| previous_refresh.map(str::to_owned))
        .ok_or_else(|| anyhow::anyhow!("Token reply has no refresh token"))?;
    anyhow::ensure!(
        !access_token.is_empty() && expires_in > 0,
        "Invalid token reply"
    );
    Ok(Tokens {
        access_token,
        refresh_token,
        expires_at_ms: crate::now_ms() + expires_in * 1000,
    })
}

/// Subscription type ("max", "pro", …) from the OAuth profile, as the CLI derives it.
pub async fn fetch_plan(
    http: &reqwest::Client,
    upstream: &Upstream,
    access_token: &str,
) -> Option<String> {
    let profile: serde_json::Value = http
        .get(format!("{}/api/oauth/profile", upstream.api))
        .bearer_auth(access_token)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    let kind = profile["organization"]["organization_type"].as_str()?;
    Some(kind.strip_prefix("claude_").unwrap_or(kind).to_owned())
}

#[derive(Debug, Default, PartialEq)]
pub struct UsageSnapshot {
    pub five_hour: Option<AiUsage>,
    pub seven_day: Option<AiUsage>,
}

pub async fn fetch_usage(
    http: &reqwest::Client,
    upstream: &Upstream,
    access_token: &str,
) -> anyhow::Result<UsageSnapshot> {
    let body: serde_json::Value = http
        .get(format!("{}/api/oauth/usage", upstream.api))
        .bearer_auth(access_token)
        .header("anthropic-beta", OAUTH_BETA)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(parse_usage(&body))
}

/// Accepts both the `five_hour`/`seven_day` objects and the newer `limits` list.
pub fn parse_usage(body: &serde_json::Value) -> UsageSnapshot {
    fn window(value: &serde_json::Value, percent_key: &str) -> Option<AiUsage> {
        let utilization = value.get(percent_key)?.as_f64()?;
        Some(AiUsage {
            utilization: utilization.clamp(0.0, 100.0),
            resets_at_ms: value.get("resets_at").and_then(parse_timestamp_ms),
        })
    }
    let mut usage = UsageSnapshot {
        five_hour: body
            .get("five_hour")
            .and_then(|value| window(value, "utilization")),
        seven_day: body
            .get("seven_day")
            .and_then(|value| window(value, "utilization")),
    };
    for limit in body
        .get("limits")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
    {
        let parsed = window(limit, "percent").or_else(|| window(limit, "utilization"));
        match limit.get("kind").and_then(|value| value.as_str()) {
            Some("session") => usage.five_hour = parsed.or(usage.five_hour.take()),
            Some("weekly_all") => usage.seven_day = parsed.or(usage.seven_day.take()),
            _ => {}
        }
    }
    usage
}

fn parse_timestamp_ms(value: &serde_json::Value) -> Option<i64> {
    if let Some(text) = value.as_str() {
        return text
            .parse::<jiff::Timestamp>()
            .ok()
            .map(|timestamp| timestamp.as_millisecond());
    }
    value.as_f64().map(|seconds| (seconds * 1000.0) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_carries_pkce_and_state() {
        let pkce = Pkce::new().unwrap();
        let url = reqwest::Url::parse(&pkce.authorize_url().unwrap()).unwrap();
        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query["client_id"], CLIENT_ID);
        assert_eq!(query["state"], pkce.state);
        assert_eq!(query["code_challenge_method"], "S256");
        assert_eq!(
            query["code_challenge"],
            URL_SAFE_NO_PAD.encode(Sha256::digest(pkce.verifier.as_bytes()))
        );
        assert!(query["scope"].contains("user:inference"));
    }

    #[test]
    fn usage_parses_both_shapes() {
        let legacy = parse_usage(&serde_json::json!({
            "five_hour": {"utilization": 42.0, "resets_at": "2026-10-07T12:00:00Z"},
            "seven_day": {"utilization": 7.5, "resets_at": null}
        }));
        assert_eq!(legacy.five_hour.as_ref().unwrap().utilization, 42.0);
        assert!(legacy.five_hour.unwrap().resets_at_ms.is_some());
        assert_eq!(legacy.seven_day.unwrap().resets_at_ms, None);

        let limits = parse_usage(&serde_json::json!({
            "limits": [
                {"kind": "session", "percent": 101},
                {"kind": "weekly_all", "percent": 3},
                {"kind": "weekly_scoped", "percent": 99}
            ]
        }));
        assert_eq!(limits.five_hour.unwrap().utilization, 100.0);
        assert_eq!(limits.seven_day.unwrap().utilization, 3.0);
    }
}
