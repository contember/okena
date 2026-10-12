//! Bounded provider error details; request bodies and authentication headers stay out of logs.

use axum::http::{HeaderMap, Method, StatusCode};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub(crate) struct RequestContext {
    method: Method,
    path: String,
    project: Option<String>,
    account: Option<String>,
}

impl RequestContext {
    pub fn new(method: &Method, path: &str, project: Option<&str>) -> Self {
        Self {
            method: method.clone(),
            path: bounded(path, 256),
            project: project.map(|value| bounded(value, 128)),
            account: None,
        }
    }

    pub fn for_account(&self, account: &str) -> Self {
        Self {
            account: Some(account.to_owned()),
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
            "method={} path={:?} project={:?} account={:?}",
            self.method, self.path, self.project, self.account
        )
    }
}

#[derive(Default, Deserialize)]
struct ErrorEnvelope {
    error: Option<ProviderError>,
    request_id: Option<String>,
}

#[derive(Deserialize)]
struct ProviderError {
    #[serde(rename = "type")]
    kind: Option<String>,
    message: Option<String>,
}

struct ErrorDetails {
    kind: Option<String>,
    message: Option<String>,
    request_id: Option<String>,
    headers: BTreeMap<String, String>,
    body_bytes: usize,
}

impl std::fmt::Display for ErrorDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "type={:?} message={:?} request_id={:?} headers={:?} body_bytes={}",
            self.kind, self.message, self.request_id, self.headers, self.body_bytes
        )
    }
}

fn response_details(headers: &HeaderMap, body: &[u8]) -> ErrorDetails {
    let error: ErrorEnvelope = serde_json::from_slice(body).unwrap_or_default();
    let kind = error
        .error
        .as_ref()
        .and_then(|error| error.kind.as_deref())
        .map(|value| bounded(value, 128));
    let message = error
        .error
        .as_ref()
        .and_then(|error| error.message.as_deref())
        .map(|value| bounded(value, 2048));
    let request_id = error.request_id.as_deref().map(|value| bounded(value, 256));
    ErrorDetails {
        kind,
        message,
        request_id,
        headers: diagnostic_headers(headers),
        body_bytes: body.len(),
    }
}

pub(crate) fn diagnostic_headers(headers: &HeaderMap) -> BTreeMap<String, String> {
    headers
        .iter()
        .filter(|(name, _)| {
            matches!(
                name.as_str(),
                "request-id" | "x-request-id" | "retry-after" | "content-type" | "cf-ray"
            ) || name.as_str().starts_with("anthropic-ratelimit-")
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
    fn error_details_keep_diagnostics_without_dumping_headers_or_body() {
        let mut headers = HeaderMap::new();
        for (name, value) in [
            ("request-id", "req-123"),
            ("retry-after", "30"),
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
}
