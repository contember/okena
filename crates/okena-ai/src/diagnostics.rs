//! Bounded provider error details; request bodies and authentication headers stay out of logs.

use axum::http::{HeaderMap, Method, StatusCode};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
struct InferenceDetails {
    model: Option<String>,
    max_tokens: Option<u64>,
    stream: Option<bool>,
    body_bytes: usize,
    session_hash: Option<String>,
    client_retry: Option<u32>,
}

#[derive(Clone, Debug)]
pub(crate) struct RequestContext {
    id: u64,
    started: Instant,
    attempt: usize,
    attempt_started: Instant,
    inference: Option<InferenceDetails>,
    inflight: Option<Arc<AtomicUsize>>,
    method: Method,
    path: String,
    project: Option<String>,
    account: Option<String>,
}

impl RequestContext {
    pub fn new(method: &Method, path: &str, project: Option<&str>) -> Self {
        Self {
            id: NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed),
            started: Instant::now(),
            attempt: 0,
            attempt_started: Instant::now(),
            inference: None,
            inflight: None,
            method: method.clone(),
            path: bounded(path, 256),
            project: project.map(|value| bounded(value, 128)),
            account: None,
        }
    }

    pub fn inference(
        mut self,
        payload: Option<&Value>,
        body_bytes: usize,
        session: Option<&str>,
        headers: &HeaderMap,
        inflight: &Arc<AtomicUsize>,
    ) -> Self {
        self.inference = Some(InferenceDetails {
            model: payload
                .and_then(|value| value.get("model"))
                .and_then(Value::as_str)
                .map(|value| bounded(value, 128)),
            max_tokens: payload
                .and_then(|value| value.get("max_tokens"))
                .and_then(Value::as_u64),
            stream: payload
                .and_then(|value| value.get("stream"))
                .and_then(Value::as_bool),
            body_bytes,
            session_hash: session
                .map(|value| format!("{:x}", Sha256::digest(value.as_bytes()))[..16].to_owned()),
            client_retry: headers
                .get("x-stainless-retry-count")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse().ok()),
        });
        self.inflight = Some(inflight.clone());
        self
    }

    pub fn for_account(&self, account: &str, attempt: usize) -> Self {
        Self {
            account: Some(account.to_owned()),
            attempt,
            attempt_started: Instant::now(),
            ..self.clone()
        }
    }

    pub fn project(&self) -> Option<&str> {
        self.project.as_deref()
    }

    pub fn account(&self) -> Option<&str> {
        self.account.as_deref()
    }

    pub fn response_error(
        &self,
        status: StatusCode,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Option<String> {
        let details = response_details(headers, body);
        log::warn!(
            "[ai-proxy] upstream_error {self} status={} details={}",
            status.as_u16(),
            details
        );
        details.message
    }
}

impl std::fmt::Display for RequestContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "relay_id={}-{} method={} path={:?} project={:?} account={:?} attempt={} elapsed_ms={} attempt_ms={}",
            std::process::id(),
            self.id,
            self.method,
            self.path,
            self.project,
            self.account,
            self.attempt,
            self.started.elapsed().as_millis(),
            self.attempt_started.elapsed().as_millis()
        )?;
        if let Some(details) = &self.inference {
            write!(
                f,
                " model={:?} max_tokens={:?} stream={:?} body_bytes={} session_hash={:?} client_retry={:?}",
                details.model,
                details.max_tokens,
                details.stream,
                details.body_bytes,
                details.session_hash,
                details.client_retry
            )?;
        }
        if let Some(inflight) = &self.inflight {
            write!(f, " proxy_inflight={}", inflight.load(Ordering::SeqCst))?;
        }
        Ok(())
    }
}

struct ErrorDetails {
    kind: Option<String>,
    message: Option<String>,
    request_id: Option<String>,
    headers: BTreeMap<String, String>,
    body_bytes: usize,
    json_fields: Vec<String>,
    parse_error: Option<String>,
}

impl std::fmt::Display for ErrorDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "type={:?} message={:?} request_id={:?} headers={:?} body_bytes={} json_fields={:?} parse_error={:?}",
            self.kind,
            self.message,
            self.request_id,
            self.headers,
            self.body_bytes,
            self.json_fields,
            self.parse_error
        )
    }
}

fn response_details(headers: &HeaderMap, body: &[u8]) -> ErrorDetails {
    let (value, parse_error) = match serde_json::from_slice::<Value>(body) {
        Ok(value) => (value, None),
        Err(error) => (
            Value::Null,
            Some(format!(
                "{:?} at line {} column {}",
                error.classify(),
                error.line(),
                error.column()
            )),
        ),
    };
    let error = &value["error"];
    let kind = string_field(error, &["type", "code"])
        .or_else(|| string_field(&value, &["type", "code"]))
        .map(|value| bounded(value, 128));
    let message = string_field(error, &["message", "error_description", "detail"])
        .or_else(|| string_field(&value, &["message", "error_description", "detail"]))
        .or_else(|| error.as_str())
        .map(|value| bounded(value, 2048));
    let request_id = string_field(&value, &["request_id"])
        .or_else(|| {
            headers
                .get("request-id")
                .and_then(|value| value.to_str().ok())
        })
        .or_else(|| {
            headers
                .get("x-request-id")
                .and_then(|value| value.to_str().ok())
        })
        .map(|value| bounded(value, 256));
    let mut json_fields = field_names(&value, "");
    json_fields.extend(field_names(error, "error."));
    ErrorDetails {
        kind,
        message,
        request_id,
        headers: diagnostic_headers(headers),
        body_bytes: body.len(),
        json_fields,
        parse_error,
    }
}

fn string_field<'a>(value: &'a Value, names: &[&str]) -> Option<&'a str> {
    names.iter().find_map(|name| value[*name].as_str())
}

fn field_names(value: &Value, prefix: &str) -> Vec<String> {
    value
        .as_object()
        .into_iter()
        .flat_map(|object| object.keys())
        .take(16)
        .map(|name| format!("{prefix}{}", bounded(name, 64)))
        .collect()
}

pub(crate) fn diagnostic_headers(headers: &HeaderMap) -> BTreeMap<String, String> {
    headers
        .iter()
        .filter(|(name, _)| {
            matches!(
                name.as_str(),
                "request-id"
                    | "x-request-id"
                    | "retry-after"
                    | "retry-after-ms"
                    | "x-should-retry"
                    | "x-envoy-upstream-service-time"
                    | "content-type"
                    | "content-encoding"
                    | "content-length"
                    | "server"
                    | "cf-ray"
            ) || name.as_str().starts_with("anthropic-ratelimit-")
                || name.as_str().starts_with("x-ratelimit-")
        })
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.to_string(), bounded(value, 256)))
        })
        .take(32)
        .collect()
}

fn bounded(value: &str, max_chars: usize) -> String {
    match value.char_indices().nth(max_chars) {
        Some((end, _)) => format!("{}…", &value[..end]),
        None => value.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inference_context_correlates_attempts_without_logging_private_fields() {
        let payload = serde_json::json!({
            "model": "claude-test\nmodel", "max_tokens": 4096, "stream": true,
            "messages": [{"role": "user", "content": "private-prompt"}],
            "metadata": {"user_id": "private-identity"},
        });
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer private-token".parse().unwrap());
        headers.insert("x-stainless-retry-count", "2".parse().unwrap());
        let inflight = Arc::new(AtomicUsize::new(3));
        let context = RequestContext::new(&Method::POST, "/v1/messages", Some("p")).inference(
            Some(&payload),
            1234,
            Some("private-session"),
            &headers,
            &inflight,
        );
        let first = context.for_account("a", 1);
        let second = context.for_account("b", 2);
        assert_eq!(first.id, second.id);
        assert_ne!(
            first.id,
            RequestContext::new(&Method::POST, "/v1/messages", None).id
        );
        let text = second.to_string();
        for expected in [
            "model=Some(\"claude-test\\nmodel\")",
            "max_tokens=Some(4096)",
            "stream=Some(true)",
            "body_bytes=1234",
            "client_retry=Some(2)",
            "attempt=2",
            "proxy_inflight=3",
            "session_hash=Some(",
        ] {
            assert!(text.contains(expected), "{text}");
        }
        assert!(!text.contains("private-"), "{text}");
        assert!(!text.contains('\n'));
        inflight.store(1, Ordering::SeqCst);
        assert!(second.to_string().contains("proxy_inflight=1"));
    }

    #[test]
    fn inference_metadata_is_bounded_and_wrong_types_are_omitted() {
        let payload = serde_json::json!({
            "model": "ž".repeat(200), "max_tokens": "private-value", "stream": {},
        });
        let mut headers = HeaderMap::new();
        headers.insert("x-stainless-retry-count", "private-retry".parse().unwrap());
        let context = RequestContext::new(&Method::POST, "/v1/messages", None).inference(
            Some(&payload),
            0,
            None,
            &headers,
            &Arc::default(),
        );
        let text = context.to_string();
        assert!(text.contains(&format!("{}…", "ž".repeat(128))));
        assert!(!text.contains(&"ž".repeat(129)));
        for expected in [
            "max_tokens=None",
            "stream=None",
            "client_retry=None",
            "session_hash=None",
        ] {
            assert!(text.contains(expected), "{text}");
        }
        assert!(!text.contains("private-"));
    }

    #[test]
    fn error_details_keep_diagnostics_without_dumping_headers_or_body() {
        let mut headers = HeaderMap::new();
        for (name, value) in [
            ("request-id", "req-123"),
            ("retry-after", "30"),
            ("retry-after-ms", "30000"),
            ("x-ratelimit-remaining-requests", "0"),
            ("x-should-retry", "true"),
            ("anthropic-ratelimit-unified-reset", "2000"),
            ("authorization", "Bearer secret-token"),
            ("set-cookie", "private-cookie"),
        ] {
            headers.insert(name, value.parse().unwrap());
        }
        let body = serde_json::json!({
            "error": {"type": "rate_limit_error", "message": "Too many requests\ntry later"},
            "request_id": "body-request-id", "access_token": "private-token",
            "messages": ["private-prompt"],
        });
        let details = response_details(&headers, &serde_json::to_vec(&body).unwrap()).to_string();
        for expected in [
            "req-123",
            "body-request-id",
            "retry-after",
            "retry-after-ms",
            "x-ratelimit-remaining-requests",
            "x-should-retry",
            "30",
            "rate_limit_error",
            "Too many requests\\ntry later",
            "2000",
        ] {
            assert!(details.contains(expected), "{details}");
        }
        for secret in [
            "secret-token",
            "private-cookie",
            "private-token",
            "private-prompt",
        ] {
            assert!(!details.contains(secret), "{details}");
        }
        assert!(!details.contains('\n'));
    }

    #[test]
    fn unstructured_errors_are_not_dumped_and_messages_are_bounded() {
        let details =
            response_details(&HeaderMap::new(), b"<html>private response</html>").to_string();
        assert!(details.contains("message=None"));
        assert!(!details.contains("private response"));
        let body = serde_json::json!({"error": {"message": "ž".repeat(3000)}});
        let details =
            response_details(&HeaderMap::new(), &serde_json::to_vec(&body).unwrap()).to_string();
        assert!(details.contains(&format!("{}…", "ž".repeat(2048))));
        assert!(!details.contains(&"ž".repeat(2049)));
    }

    #[test]
    fn alternate_error_formats_keep_messages_and_header_request_ids() {
        let mut headers = HeaderMap::new();
        headers.insert("request-id", "header-request".parse().unwrap());
        for body in [
            r#"{"error":"Too many requests"}"#,
            r#"{"message":"Too many requests","error":42}"#,
            r#"{"error":"rate_limit","error_description":"Too many requests"}"#,
            r#"{"detail":"Too many requests"}"#,
            r#"{"error":{"code":"rate_limit","message":"Too many requests"},"request_id":42}"#,
        ] {
            let details = response_details(&headers, body.as_bytes());
            assert_eq!(details.message.as_deref(), Some("Too many requests"));
            assert_eq!(details.request_id.as_deref(), Some("header-request"));
            assert_eq!(details.parse_error, None);
        }
    }

    #[test]
    fn unknown_json_reports_bounded_field_names_without_values() {
        let body = serde_json::json!({
            "error": {"reason": "private-reason", "token": "private-token"},
            "messages": ["private-prompt"],
        });
        let details = response_details(&HeaderMap::new(), &serde_json::to_vec(&body).unwrap());
        assert_eq!(details.message, None);
        assert_eq!(
            details.json_fields,
            ["error", "messages", "error.reason", "error.token"]
        );
        assert!(!details.to_string().contains("private-"));

        let fields = (0..40)
            .map(|index| (format!("{index:02}{}", "x".repeat(100)), Value::Null))
            .collect();
        let names = field_names(&Value::Object(fields), "");
        assert_eq!(names.len(), 16);
        assert!(names.iter().all(|name| name.chars().count() == 65));
    }

    #[test]
    fn malformed_json_and_encoded_bodies_report_parse_failures() {
        let mut headers = HeaderMap::new();
        headers.insert("content-encoding", "gzip".parse().unwrap());
        for body in [b"\x1f\x8b\x08".as_slice(), br#"{"message":"private-text"#] {
            let details = response_details(&headers, body);
            assert!(details.parse_error.is_some());
            assert_eq!(details.headers["content-encoding"], "gzip");
            assert!(!details.to_string().contains("private-text"));
        }
    }
}
