//! Request relay: `/v1/messages*` is served by a pool account; everything else
//! (OAuth, profile, telemetry) passes through with the caller's own credentials.

use crate::accounts::{Accounts, Selection};
use crate::activity::Activity;
use crate::diagnostics::{RequestContext, diagnostic_headers};
use crate::oauth::OAUTH_BETA;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use okena_core::ai_proxy::AiActivityKind;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;
/// Upstream attempts per request (account failover, one refresh retry).
const MAX_ATTEMPTS: usize = 4;
const QUOTA_PROBE_MODEL: &str = "claude-haiku-4-5-20251001";

pub struct Relay {
    pub accounts: Arc<Accounts>,
    pub http: reqwest::Client,
    pub api: String,
    pub inflight: Arc<AtomicUsize>,
    pub activity: Activity,
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
        let Ok(target) = reqwest::Url::parse(&format!("{}{}", self.api, path_and_query)) else {
            return error(StatusCode::BAD_REQUEST, "Invalid request path");
        };
        // Judge the path the request will reach: parsing resolves `..` segments,
        // percent-encoded ones too, and a pool account must not leave inference.
        let path = target.path();
        let url = target.as_str();
        let context = RequestContext::new(&method, path, project);
        if method != Method::POST || !is_inference_path(path) {
            return match self.passthrough(method, url, &headers, body).await {
                Ok(response)
                    if response.status().is_client_error()
                        || response.status().is_server_error() =>
                {
                    read_error_response(response, &context)
                        .await
                        .into_response()
                }
                Ok(response) => stream(response, guard, context),
                Err(err) => {
                    log::warn!(
                        "[ai-proxy] connection_error {context} error={}",
                        err.without_url()
                    );
                    error(StatusCode::BAD_GATEWAY, "Upstream connection failed")
                }
            };
        }

        let mut payload: Option<serde_json::Value> = serde_json::from_slice(&body).ok();
        let session = session_id(&headers, payload.as_ref());
        let mut context = context.inference(
            payload.as_ref(),
            body.len(),
            session.as_deref(),
            &headers,
            &self.inflight,
        );
        let mut body = body;
        if path == "/v1/messages"
            && let Some(payload) = payload.as_mut()
            && rewrite_quota_probe_model(payload)
        {
            body = match serde_json::to_vec(payload) {
                Ok(body) => Bytes::from(body),
                Err(_) => {
                    return error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Could not encode quota check",
                    );
                }
            };
            log::info!(
                "[ai-proxy] quota_probe_model_rewrite {context} upstream_model={QUOTA_PROBE_MODEL:?}"
            );
            context = context.inference(
                Some(payload),
                body.len(),
                session.as_deref(),
                &headers,
                &self.inflight,
            );
        }
        let mut excluded: Vec<String> = Vec::new();
        let mut refreshed: Vec<String> = Vec::new();
        let mut rate_limited: Option<ErrorResponse> = None;
        let mut selected = None;
        for attempt in 1..=MAX_ATTEMPTS {
            let account = match selected.take() {
                Some(account) => account,
                None => match self
                    .accounts
                    .select(project, session.as_deref(), &excluded)
                    .await
                {
                    Selection::Selected(account) => account,
                    Selection::NoAccounts => {
                        return match self.passthrough(method, url, &headers, body).await {
                            Ok(response) => self.answer(context, response, guard).await,
                            Err(err) => self.unanswered(&context, err),
                        };
                    }
                    Selection::NoPermittedAccount => {
                        self.accounts.log_availability(&context, &excluded);
                        log::warn!(
                            "[ai-proxy] proxy_rejection {context} status=400 reason=no_permitted_account"
                        );
                        self.activity
                            .event(project, AiActivityKind::NoPermittedAccount);
                        // 400 rather than 403: `claude` shows the message as is,
                        // where a 401/403 can send the user to `/login`.
                        return error(
                            StatusCode::BAD_REQUEST,
                            "No account this terminal's policy permits is enabled and \
                             logged in. Check the policy in Settings > AI Proxy.",
                        );
                    }
                    Selection::Exhausted { retry_at_ms } => {
                        self.accounts.log_availability(&context, &excluded);
                        log::warn!(
                            "[ai-proxy] proxy_rejection {context} status=429 reason=accounts_unavailable retry_at_ms={retry_at_ms:?}"
                        );
                        self.activity.event(project, AiActivityKind::AllRateLimited);
                        return exhausted(rate_limited, retry_at_ms);
                    }
                },
            };
            let account_context = context.for_account(&account.id, attempt);
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
                .post(url)
                .headers(request_headers)
                .body(outgoing)
                .send()
                .await
            {
                Ok(response) => response,
                Err(err) => return self.unanswered(&account_context, err),
            };
            let status = response.status();
            log::debug!(
                "[ai-proxy] upstream_response {account_context} status={status} headers={:?}",
                diagnostic_headers(response.headers())
            );
            self.accounts.observe(
                &account.id,
                session.as_deref(),
                status.as_u16(),
                response.headers(),
            );
            match status {
                StatusCode::UNAUTHORIZED => {
                    read_error_response(response, &account_context).await;
                    self.activity.log_traffic(&account_context);
                    self.activity.event(
                        project,
                        AiActivityKind::Unauthorized {
                            account_id: account.id.clone(),
                        },
                    );
                    if !refreshed.contains(&account.id) {
                        refreshed.push(account.id.clone());
                        match self
                            .accounts
                            .fresh_token(&account.id, Some(&account.access_token))
                            .await
                        {
                            Ok(retry) => selected = Some(retry),
                            Err(error) => {
                                log::warn!(
                                    "[ai-proxy] refresh_error {account_context} error={error:#}"
                                );
                                excluded.push(account.id);
                            }
                        }
                    } else {
                        self.accounts.mark_needs_login(&account.id);
                        excluded.push(account.id);
                    }
                }
                StatusCode::TOO_MANY_REQUESTS => {
                    self.activity.rate_limited(project, &account.id);
                    rate_limited = Some(read_error_response(response, &account_context).await);
                    self.activity.log_traffic(&account_context);
                    self.accounts.log_availability(&account_context, &excluded);
                    excluded.push(account.id);
                }
                _ => {
                    if attempt > 1 && status.is_success() {
                        log::info!(
                            "[ai-proxy] failover_recovered {account_context} status={status}"
                        );
                    }
                    return self.answer(account_context, response, guard).await;
                }
            }
        }
        log::warn!(
            "[ai-proxy] proxy_rejection {context} status=429 reason=attempts_exhausted attempts={MAX_ATTEMPTS}"
        );
        self.activity.event(project, AiActivityKind::AllRateLimited);
        exhausted(rate_limited, None)
    }

    /// Relay the response that answers a `/v1/messages` request and record it.
    /// An error response is read whole, so its message can be recorded.
    async fn answer(
        &self,
        context: RequestContext,
        response: reqwest::Response,
        guard: InflightGuard,
    ) -> Response {
        let status = response.status();
        if !status.is_client_error() && !status.is_server_error() {
            self.activity
                .answered(context.project(), context.account(), status.as_u16(), None);
            return stream(response, guard, context);
        }
        let failure = read_error_response(response, &context).await;
        self.activity.answered(
            context.project(),
            context.account(),
            status.as_u16(),
            failure.message.clone(),
        );
        self.activity.log_traffic(&context);
        failure.into_response()
    }

    fn unanswered(&self, context: &RequestContext, err: reqwest::Error) -> Response {
        let err = err.without_url();
        log::warn!("[ai-proxy] connection_error {context} error={err}");
        self.activity
            .unanswered(context.project(), context.account(), upstream_failure(&err));
        self.activity.log_traffic(context);
        error(StatusCode::BAD_GATEWAY, "Upstream connection failed")
    }

    /// Forward the request with the caller's own credentials.
    async fn passthrough(
        &self,
        method: Method,
        url: &str,
        headers: &HeaderMap,
        body: Bytes,
    ) -> reqwest::Result<reqwest::Response> {
        self.http
            .request(method, url)
            .headers(forward_headers(headers))
            .body(body)
            .send()
            .await
    }
}

/// Relay status, headers and the body as it arrives; dropping the downstream
/// connection drops the upstream one.
fn stream(response: reqwest::Response, guard: InflightGuard, context: RequestContext) -> Response {
    let status = response.status();
    let headers = strip_hop_by_hop(response.headers());
    let diagnostics = diagnostic_headers(response.headers());
    let body = response.bytes_stream().map(move |chunk| {
        let _ = &guard;
        chunk.map_err(|error| {
            let error = error.without_url();
            log::warn!(
                "[ai-proxy] stream_error {context} status={} headers={diagnostics:?} error={error}",
                status.as_u16()
            );
            error
        })
    });
    let mut relayed = Response::new(Body::from_stream(body));
    *relayed.status_mut() = status;
    *relayed.headers_mut() = headers;
    relayed
}

struct ErrorResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
    message: Option<String>,
}

impl IntoResponse for ErrorResponse {
    fn into_response(self) -> Response {
        buffered(self.status, &self.headers, self.body)
    }
}

async fn read_error_response(
    response: reqwest::Response,
    context: &RequestContext,
) -> ErrorResponse {
    let status = response.status();
    let headers = response.headers().clone();
    let body = match response.bytes().await {
        Ok(body) => body,
        Err(error) => {
            log::warn!(
                "[ai-proxy] error_body_read_failed {context} status={} headers={:?} error={}",
                status.as_u16(),
                diagnostic_headers(&headers),
                error.without_url()
            );
            Bytes::new()
        }
    };
    let message = context.response_error(status, &headers, &body);
    ErrorResponse {
        status,
        headers,
        body,
        message,
    }
}

fn buffered(status: StatusCode, headers: &HeaderMap, body: Bytes) -> Response {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    *response.headers_mut() = strip_hop_by_hop(headers);
    response
}

fn exhausted(rate_limited: Option<ErrorResponse>, retry_at_ms: Option<i64>) -> Response {
    if let Some(response) = rate_limited {
        return response.into_response();
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

fn upstream_failure(err: &reqwest::Error) -> String {
    if err.is_timeout() {
        "No response from the provider".to_owned()
    } else if err.is_connect() {
        "Could not connect to the provider".to_owned()
    } else {
        err.to_string()
    }
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

/// `/v1/messages` and its sub-resources (e.g. `count_tokens`).
fn is_inference_path(path: &str) -> bool {
    path == "/v1/messages" || path.starts_with("/v1/messages/")
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

fn rewrite_quota_probe_model(payload: &mut serde_json::Value) -> bool {
    let Some(request) = payload.as_object_mut() else {
        return false;
    };
    if request.keys().any(|key| {
        !matches!(
            key.as_str(),
            "model" | "max_tokens" | "messages" | "metadata" | "stream"
        )
    }) || request
        .get("max_tokens")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
        || request
            .get("stream")
            .is_some_and(|value| value.as_bool() != Some(false))
    {
        return false;
    }
    let Some(messages) = request
        .get("messages")
        .and_then(serde_json::Value::as_array)
    else {
        return false;
    };
    let [message] = messages.as_slice() else {
        return false;
    };
    let Some(message) = message.as_object() else {
        return false;
    };
    if message.len() != 2
        || message.get("role").and_then(serde_json::Value::as_str) != Some("user")
        || message.get("content").and_then(serde_json::Value::as_str) != Some("quota")
    {
        return false;
    }
    let Some(model) = request.get_mut("model") else {
        return false;
    };
    if !model
        .as_str()
        .is_some_and(|model| !model.is_empty() && model != QUOTA_PROBE_MODEL)
    {
        return false;
    }
    // Claude Code's startup probe can select Opus behind a custom base URL;
    // its minimal request gets a bare 429 where the same probe on Haiku succeeds.
    *model = serde_json::Value::String(QUOTA_PROBE_MODEL.to_owned());
    true
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
    fn only_the_minimal_quota_probe_changes_model() {
        let probe = serde_json::json!({
            "model": "claude-opus-5-5", "max_tokens": 1,
            "messages": [{"role": "user", "content": "quota"}],
            "metadata": {"user_id": "preserve-identity"},
        });
        for stream in [None, Some(false)] {
            let mut payload = probe.clone();
            if let Some(stream) = stream {
                payload["stream"] = stream.into();
            }
            let mut expected = payload.clone();
            expected["model"] = QUOTA_PROBE_MODEL.into();
            assert!(rewrite_quota_probe_model(&mut payload));
            assert_eq!(payload, expected);
            assert!(!rewrite_quota_probe_model(&mut payload));
        }

        for (key, value) in [
            ("stream", serde_json::json!(true)),
            ("stream", serde_json::Value::Null),
            ("stream", serde_json::json!("false")),
            ("max_tokens", serde_json::json!(2)),
            ("max_tokens", serde_json::json!("1")),
            ("system", serde_json::json!("")),
            ("tools", serde_json::json!([])),
            ("thinking", serde_json::json!({"type": "adaptive"})),
            ("temperature", serde_json::json!(0)),
            ("model", serde_json::Value::Null),
            ("model", serde_json::json!("")),
            ("messages", serde_json::json!([])),
            (
                "messages",
                serde_json::json!([{"role": "user", "content": "hello"}]),
            ),
            (
                "messages",
                serde_json::json!([{"role": "assistant", "content": "quota"}]),
            ),
            (
                "messages",
                serde_json::json!([{"role": "user", "content": "quota", "extra": true}]),
            ),
            (
                "messages",
                serde_json::json!([{"role": "user", "content": [{"type": "text", "text": "quota"}]}]),
            ),
            (
                "messages",
                serde_json::json!([{"role": "user", "content": "quota"}, {"role": "user", "content": "quota"}]),
            ),
        ] {
            let mut payload = probe.clone();
            payload[key] = value;
            let original = payload.clone();
            assert!(!rewrite_quota_probe_model(&mut payload), "{original}");
            assert_eq!(payload, original);
        }
        for key in ["model", "max_tokens", "messages"] {
            let mut payload = probe.clone();
            payload.as_object_mut().unwrap().remove(key);
            let original = payload.clone();
            assert!(!rewrite_quota_probe_model(&mut payload));
            assert_eq!(payload, original);
        }
        for mut payload in [serde_json::Value::Null, serde_json::json!([])] {
            assert!(!rewrite_quota_probe_model(&mut payload));
        }
    }

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
