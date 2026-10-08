//! Request relay: `/v1/messages*` is served by a pool account; everything else
//! (OAuth, profile, telemetry) passes through with the caller's own credentials.

use crate::accounts::{Accounts, Selection};
use crate::oauth::OAUTH_BETA;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;
/// Upstream attempts per request (account failover, one refresh retry).
const MAX_ATTEMPTS: usize = 4;

pub struct Relay {
    pub accounts: Arc<Accounts>,
    pub http: reqwest::Client,
    pub api: String,
    pub inflight: Arc<AtomicUsize>,
}

/// Counts a request as in flight until its response body is finished or dropped.
struct InflightGuard(Arc<AtomicUsize>);

impl InflightGuard {
    fn new(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(counter.clone())
    }
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Relay {
    /// `path_and_query` is the request target with the endpoint token and the
    /// project segment stripped; `project` selects the routing policy.
    pub async fn handle(
        &self,
        method: Method,
        project: Option<&str>,
        path_and_query: &str,
        headers: HeaderMap,
        body: Body,
    ) -> Response {
        let guard = InflightGuard::new(&self.inflight);
        let body = match axum::body::to_bytes(body, MAX_BODY_BYTES).await {
            Ok(body) => body,
            Err(_) => return error(StatusCode::PAYLOAD_TOO_LARGE, "Request body is too large"),
        };
        let url = format!("{}{}", self.api, path_and_query);
        let path = path_and_query.split('?').next().unwrap_or_default();
        if method != Method::POST || !path.starts_with("/v1/messages") {
            return self.passthrough(method, &url, &headers, body, guard).await;
        }

        let mut payload: Option<serde_json::Value> = serde_json::from_slice(&body).ok();
        let session = session_id(&headers, payload.as_ref());
        let mut excluded: Vec<String> = Vec::new();
        let mut refreshed: Vec<String> = Vec::new();
        let mut rate_limited: Option<(StatusCode, HeaderMap, Bytes)> = None;
        let mut selected = None;
        for _ in 0..MAX_ATTEMPTS {
            let account = match selected.take() {
                Some(account) => account,
                None => match self
                    .accounts
                    .select(project, session.as_deref(), &excluded)
                    .await
                {
                    Selection::Selected(account) => account,
                    Selection::NoAccounts => {
                        return self.passthrough(method, &url, &headers, body, guard).await;
                    }
                    Selection::NoPermittedAccount => {
                        // 400 rather than 403: `claude` shows the message as is,
                        // where a 401/403 can send the user to `/login`.
                        return error(
                            StatusCode::BAD_REQUEST,
                            "No account this terminal's policy permits is enabled and \
                             logged in. Check the policy in Settings > AI Proxy.",
                        );
                    }
                    Selection::Exhausted { retry_at_ms } => {
                        return exhausted(rate_limited, retry_at_ms);
                    }
                },
            };
            let outgoing = match (payload.as_mut(), account.account_uuid.as_deref()) {
                (Some(payload), Some(uuid)) => {
                    if rewrite_account_uuid(payload, uuid) {
                        serde_json::to_vec(payload).map_or_else(|_| body.clone(), Bytes::from)
                    } else {
                        body.clone()
                    }
                }
                _ => body.clone(),
            };
            let mut request_headers = forward_headers(&headers);
            request_headers.remove("x-api-key");
            if let Ok(value) = HeaderValue::from_str(&format!("Bearer {}", account.access_token)) {
                request_headers.insert("authorization", value);
            }
            ensure_oauth_beta(&mut request_headers);
            let response = match self
                .http
                .post(&url)
                .headers(request_headers)
                .body(outgoing)
                .send()
                .await
            {
                Ok(response) => response,
                Err(err) => {
                    log::warn!("[ai-proxy] upstream request failed: {err}");
                    return error(StatusCode::BAD_GATEWAY, "Upstream connection failed");
                }
            };
            let status = response.status();
            log::debug!(
                "[ai-proxy] {path} for project {} via account {}: {status}",
                project.unwrap_or("-"),
                account.id
            );
            self.accounts.observe(
                &account.id,
                session.as_deref(),
                status.as_u16(),
                response.headers(),
            );
            match status {
                StatusCode::UNAUTHORIZED if !refreshed.contains(&account.id) => {
                    refreshed.push(account.id.clone());
                    match self
                        .accounts
                        .fresh_token(&account.id, Some(&account.access_token))
                        .await
                    {
                        Ok(retry) => selected = Some(retry),
                        Err(_) => excluded.push(account.id),
                    }
                }
                StatusCode::UNAUTHORIZED => {
                    self.accounts.mark_needs_login(&account.id);
                    excluded.push(account.id);
                }
                StatusCode::TOO_MANY_REQUESTS => {
                    log::info!(
                        "[ai-proxy] account {} rate limited, failing over",
                        account.id
                    );
                    let headers = response.headers().clone();
                    let body = response.bytes().await.unwrap_or_default();
                    rate_limited = Some((status, headers, body));
                    excluded.push(account.id);
                }
                _ => return stream(response, guard),
            }
        }
        exhausted(rate_limited, None)
    }

    async fn passthrough(
        &self,
        method: Method,
        url: &str,
        headers: &HeaderMap,
        body: Bytes,
        guard: InflightGuard,
    ) -> Response {
        match self
            .http
            .request(method, url)
            .headers(forward_headers(headers))
            .body(body)
            .send()
            .await
        {
            Ok(response) => stream(response, guard),
            Err(err) => {
                log::warn!("[ai-proxy] passthrough request failed: {err}");
                error(StatusCode::BAD_GATEWAY, "Upstream connection failed")
            }
        }
    }
}

/// Relay status, headers and the body as it arrives; dropping the downstream
/// connection drops the upstream one.
fn stream(response: reqwest::Response, guard: InflightGuard) -> Response {
    let status = response.status();
    let headers = strip_hop_by_hop(response.headers());
    let body = response.bytes_stream().map(move |chunk| {
        let _ = &guard;
        chunk
    });
    let mut relayed = Response::new(Body::from_stream(body));
    *relayed.status_mut() = status;
    *relayed.headers_mut() = headers;
    relayed
}

fn exhausted(
    rate_limited: Option<(StatusCode, HeaderMap, Bytes)>,
    retry_at_ms: Option<i64>,
) -> Response {
    if let Some((status, headers, body)) = rate_limited {
        let mut response = Response::new(Body::from(body));
        *response.status_mut() = status;
        *response.headers_mut() = strip_hop_by_hop(&headers);
        return response;
    }
    let mut response = error(
        StatusCode::TOO_MANY_REQUESTS,
        "All AI proxy accounts are rate limited",
    );
    let seconds = retry_at_ms
        .map(|at| ((at - crate::now_ms()) / 1000).max(1))
        .unwrap_or(60);
    if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
        response.headers_mut().insert("retry-after", value);
    }
    response
}

/// Error body in the Anthropic API shape, so the CLI shows the message.
pub fn error(status: StatusCode, message: &str) -> Response {
    let kind = match status {
        StatusCode::TOO_MANY_REQUESTS => "rate_limit_error",
        StatusCode::PAYLOAD_TOO_LARGE => "request_too_large",
        StatusCode::BAD_REQUEST => "invalid_request_error",
        _ => "api_error",
    };
    (
        status,
        axum::Json(serde_json::json!({
            "type": "error",
            "error": {"type": kind, "message": format!("Okena AI proxy: {message}")},
        })),
    )
        .into_response()
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "host"
            | "content-length"
    )
}

fn strip_hop_by_hop(headers: &HeaderMap) -> HeaderMap {
    headers
        .iter()
        .filter(|(name, _)| !is_hop_by_hop(name.as_str()))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn forward_headers(headers: &HeaderMap) -> HeaderMap {
    strip_hop_by_hop(headers)
}

fn ensure_oauth_beta(headers: &mut HeaderMap) {
    let current = headers
        .get("anthropic-beta")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    if current.split(',').any(|beta| beta.trim() == OAUTH_BETA) {
        return;
    }
    let value = if current.trim().is_empty() {
        OAUTH_BETA.to_owned()
    } else {
        format!("{current},{OAUTH_BETA}")
    };
    if let Ok(value) = HeaderValue::from_str(&value) {
        headers.insert("anthropic-beta", value);
    }
}

/// The conversation id Claude Code sends, used for account affinity.
fn session_id(headers: &HeaderMap, payload: Option<&serde_json::Value>) -> Option<String> {
    if let Some(value) = headers
        .get("x-claude-code-session-id")
        .and_then(|value| value.to_str().ok())
    {
        return Some(value.to_owned());
    }
    let user_id = payload?.pointer("/metadata/user_id")?.as_str()?;
    if let Ok(serde_json::Value::Object(identity)) = serde_json::from_str(user_id) {
        return identity.get("session_id")?.as_str().map(str::to_owned);
    }
    user_id.split_once("_session_").map(|(_, id)| id.to_owned())
}

/// Point `metadata.user_id` at the serving account, as the CLI would have sent
/// it had that account been logged in. Returns whether the payload changed.
fn rewrite_account_uuid(payload: &mut serde_json::Value, account_uuid: &str) -> bool {
    let Some(user_id) = payload.pointer_mut("/metadata/user_id") else {
        return false;
    };
    let Some(raw) = user_id.as_str() else {
        return false;
    };
    let rewritten = if let Ok(serde_json::Value::Object(mut identity)) = serde_json::from_str(raw) {
        if !identity.contains_key("account_uuid") {
            return false;
        }
        identity.insert("account_uuid".into(), account_uuid.into());
        serde_json::Value::Object(identity).to_string()
    } else if let Some((prefix, rest)) = raw.split_once("_account_")
        && let Some((_, session)) = rest.split_once("_session_")
    {
        format!("{prefix}_account_{account_uuid}_session_{session}")
    } else {
        return false;
    };
    *user_id = rewritten.into();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_json_and_legacy_user_ids() {
        let mut json = serde_json::json!({"metadata": {"user_id":
            r#"{"device_id":"d","account_uuid":"old","session_id":"s1"}"#}});
        assert!(rewrite_account_uuid(&mut json, "new"));
        let identity: serde_json::Value =
            serde_json::from_str(json["metadata"]["user_id"].as_str().unwrap()).unwrap();
        assert_eq!(identity["account_uuid"], "new");
        assert_eq!(identity["session_id"], "s1");

        let mut legacy =
            serde_json::json!({"metadata": {"user_id": "user_abc_account_old_session_s2"}});
        assert!(rewrite_account_uuid(&mut legacy, "new"));
        assert_eq!(
            legacy["metadata"]["user_id"],
            "user_abc_account_new_session_s2"
        );

        let mut absent = serde_json::json!({"model": "x"});
        assert!(!rewrite_account_uuid(&mut absent, "new"));
    }

    #[test]
    fn session_prefers_header_then_metadata() {
        let mut headers = HeaderMap::new();
        let payload = serde_json::json!({"metadata": {"user_id": r#"{"session_id":"from-body"}"#}});
        assert_eq!(
            session_id(&headers, Some(&payload)).as_deref(),
            Some("from-body")
        );
        headers.insert(
            "x-claude-code-session-id",
            HeaderValue::from_static("from-header"),
        );
        assert_eq!(
            session_id(&headers, Some(&payload)).as_deref(),
            Some("from-header")
        );
    }

    #[test]
    fn oauth_beta_is_appended_once() {
        let mut headers = HeaderMap::new();
        headers.insert("anthropic-beta", HeaderValue::from_static("a,b"));
        ensure_oauth_beta(&mut headers);
        ensure_oauth_beta(&mut headers);
        assert_eq!(headers["anthropic-beta"], format!("a,b,{OAUTH_BETA}"));
    }
}
