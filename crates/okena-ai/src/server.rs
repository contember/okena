//! The `okena --ai-proxy <profile-root>` process.

use crate::accounts::Accounts;
use crate::activity::Activity;
use crate::oauth::Upstream;
use crate::proxy::{Relay, error};
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use fs2::FileExt;
use okena_core::ai_proxy::{
    AiProxyCommand, PROJECT_SEGMENT, ProxyEndpoint, ensure_management_key, is_routable_project_id,
    proxy_dir,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Notify;

const USAGE_POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Serialize, Deserialize)]
pub struct Health {
    pub version: String,
    pub inflight: usize,
    pub pid: u32,
}

struct Service {
    token: String,
    management_key: String,
    relay: Relay,
    shutdown: Notify,
}

/// Entry point of the proxy process. Returns once another instance holds the
/// profile's lock, or after a management shutdown.
pub fn run(profile_root: PathBuf) -> anyhow::Result<()> {
    let dir = proxy_dir(&profile_root);
    std::fs::create_dir_all(&dir)?;
    init_logging(&dir);
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("proxy.lock"))?;
    if lock.try_lock_exclusive().is_err() {
        log::info!("[ai-proxy] another instance owns {}", dir.display());
        return Ok(());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("okena-ai-proxy")
        .build()?;
    let result = runtime.block_on(serve(profile_root, Upstream::default(), async {
        let _ = tokio::signal::ctrl_c().await;
    }));
    if let Err(error) = &result {
        log::error!("[ai-proxy] stopped: {error:#}");
    }
    drop(lock);
    result
}

fn init_logging(dir: &Path) {
    let path = dir.join("proxy.log");
    // Keep one previous log; the proxy is long-lived and logs little.
    if std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() > 4 * 1024 * 1024) {
        let _ = std::fs::rename(&path, dir.join("proxy.log.1"));
    }
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        builder.target(env_logger::Target::Pipe(Box::new(file)));
    }
    let _ = builder.try_init();
}

/// Bind the profile's endpoint and serve until `stop` resolves or a management
/// shutdown arrives. The caller holds the instance lock.
pub async fn serve(
    profile_root: PathBuf,
    upstream: Upstream,
    stop: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let mut endpoint = okena_core::ai_proxy::ensure_endpoint(&profile_root)?;
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", endpoint.port)).await {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            // Another program took the port. Shells started with the old URL
            // cannot be served either way; new shells get the new one.
            log::warn!("[ai-proxy] port {} is taken, moving", endpoint.port);
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
            endpoint = ProxyEndpoint {
                port: listener.local_addr()?.port(),
                token: endpoint.token,
            };
            okena_core::ai_proxy::save_endpoint(&profile_root, &endpoint)?;
            listener
        }
        Err(error) => return Err(error.into()),
    };
    let management_key = ensure_management_key(&profile_root)?;
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        // Bounds header and per-chunk waits, so a stalled upstream cannot hold an
        // in-flight request (and with it a version replacement) forever.
        .read_timeout(upstream.silence_timeout)
        .build()?;
    let accounts = Arc::new(Accounts::open(
        &proxy_dir(&profile_root),
        http.clone(),
        upstream.clone(),
    )?);
    let service = Arc::new(Service {
        token: endpoint.token,
        management_key,
        relay: Relay {
            accounts: accounts.clone(),
            http,
            api: upstream.api,
            inflight: Arc::new(AtomicUsize::new(0)),
            activity: Activity::new(),
        },
        shutdown: Notify::new(),
    });
    let poller = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        loop {
            accounts.poll_usage().await;
            tokio::time::sleep(USAGE_POLL_INTERVAL).await;
        }
    });
    log::info!(
        "[ai-proxy] {} listening on 127.0.0.1:{}",
        crate::VERSION,
        endpoint.port
    );
    let app = axum::Router::new()
        .fallback(dispatch)
        .with_state(service.clone());
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = stop => {}
                _ = service.shutdown.notified() => {}
            }
        })
        .await;
    poller.abort();
    log::info!("[ai-proxy] shut down");
    Ok(result?)
}

async fn dispatch(State(service): State<Arc<Service>>, request: Request) -> Response {
    let target = request
        .uri()
        .path_and_query()
        .map(|target| target.as_str().to_owned())
        .unwrap_or_default();
    if let Some(management) = target.strip_prefix("/_okena/") {
        if !has_management_key(request.headers(), &service.management_key) {
            return error(StatusCode::UNAUTHORIZED, "Management key required");
        }
        return manage(&service, management, request.into_body()).await;
    }
    let Some((project, rest)) = strip_token(&target, &service.token).and_then(split_project) else {
        return error(StatusCode::NOT_FOUND, "Unknown endpoint");
    };
    let (project, rest) = (project.map(str::to_owned), rest.to_owned());
    let (parts, body) = request.into_parts();
    service
        .relay
        .handle(parts.method, project.as_deref(), &rest, parts.headers, body)
        .await
}

/// `/<token>/rest` → `/rest`.
fn strip_token<'a>(target: &'a str, token: &str) -> Option<&'a str> {
    let after_slash = target.strip_prefix('/')?;
    let end = after_slash.find(['/', '?']).unwrap_or(after_slash.len());
    let (candidate, rest) = after_slash.split_at(end);
    same_secret(candidate, token).then_some(rest)
}

/// `/p/<project>/rest` → `(Some(project), "/rest")`; a target without the
/// project segment has no project. `None` for a malformed project id.
fn split_project(rest: &str) -> Option<(Option<&str>, &str)> {
    let Some(after) = rest
        .strip_prefix('/')
        .and_then(|rest| rest.strip_prefix(PROJECT_SEGMENT))
        .and_then(|rest| rest.strip_prefix('/'))
    else {
        return Some((None, rest));
    };
    let end = after.find(['/', '?']).unwrap_or(after.len());
    let (project, rest) = after.split_at(end);
    is_routable_project_id(project).then_some((Some(project), rest))
}

fn has_management_key(headers: &axum::http::HeaderMap, key: &str) -> bool {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|candidate| same_secret(candidate, key))
}

/// Constant-time comparison, so response timing does not leak the secret.
fn same_secret(candidate: &str, secret: &str) -> bool {
    candidate.len() == secret.len()
        && candidate
            .bytes()
            .zip(secret.bytes())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

async fn manage(service: &Service, path: &str, body: Body) -> Response {
    let inflight = service.relay.inflight.load(Ordering::SeqCst);
    match path {
        "health" => axum::Json(Health {
            version: crate::VERSION.to_owned(),
            inflight,
            pid: std::process::id(),
        })
        .into_response(),
        "shutdown" => {
            if inflight > 0 {
                return error(StatusCode::CONFLICT, "Requests are in flight");
            }
            service.shutdown.notify_one();
            StatusCode::NO_CONTENT.into_response()
        }
        "command" => {
            let command: AiProxyCommand = match axum::body::to_bytes(body, 64 * 1024)
                .await
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            {
                Some(command) => command,
                None => return error(StatusCode::BAD_REQUEST, "Invalid command"),
            };
            let accounts = &service.relay.accounts;
            let mut usage_history = None;
            let outcome = match command {
                AiProxyCommand::Status => Ok(()),
                AiProxyCommand::UsageHistory { account_id } => accounts
                    .usage_history(&account_id)
                    .await
                    .map(|history| usage_history = Some(history)),

                AiProxyCommand::StartLogin { label } => accounts.start_login(label),
                AiProxyCommand::FinishLogin { login_id, code } => {
                    accounts.finish_login(&login_id, &code).await
                }
                AiProxyCommand::CancelLogin { login_id } => {
                    accounts.cancel_login(&login_id);
                    Ok(())
                }
                AiProxyCommand::RemoveAccount { account_id } => accounts.remove(&account_id),
                AiProxyCommand::SetAccountEnabled {
                    account_id,
                    enabled,
                } => accounts.set_enabled(&account_id, enabled),
                AiProxyCommand::SetDefaultPolicy { policy } => accounts.set_default_policy(policy),
                AiProxyCommand::SetProjectPolicy { project_id, policy } => {
                    accounts.set_project_policy(&project_id, policy)
                }
            };
            match outcome {
                Ok(()) => {
                    let mut status = accounts.status(inflight, service.relay.activity.snapshot());
                    status.usage_history = usage_history;
                    axum::Json(status).into_response()
                }
                Err(err) => error(StatusCode::BAD_REQUEST, &err.to_string()),
            }
        }
        _ => error(StatusCode::NOT_FOUND, "Unknown management endpoint"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;
    use futures::StreamExt;
    use okena_core::ai_proxy::AiActivityKind;
    use parking_lot::Mutex;
    use std::collections::HashMap;

    static LOG_MESSAGES: Mutex<Vec<String>> = Mutex::new(Vec::new());
    static TEST_LOGGER: TestLogger = TestLogger;

    fn init_test_logger() {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            log::set_logger(&TEST_LOGGER).unwrap();
            log::set_max_level(log::LevelFilter::Warn);
        });
    }

    struct TestLogger;

    impl log::Log for TestLogger {
        fn enabled(&self, _: &log::Metadata<'_>) -> bool {
            true
        }

        fn log(&self, record: &log::Record<'_>) {
            if record.target().starts_with("okena_ai") {
                LOG_MESSAGES.lock().push(record.args().to_string());
            }
        }

        fn flush(&self) {}
    }

    /// Fake provider: per access token, a queue of statuses for `/v1/messages`.
    #[derive(Default)]
    struct Fake {
        replies: Mutex<HashMap<String, Vec<u16>>>,
        seen: Mutex<Vec<(String, String)>>,
        refreshes: AtomicUsize,
        usage: Mutex<HashMap<String, serde_json::Value>>,
        error_bodies: Mutex<HashMap<String, axum::body::Bytes>>,
        error_headers: Mutex<HeaderMap>,
    }

    async fn fake_upstream(fake: Arc<Fake>) -> String {
        async fn messages(
            State(fake): State<Arc<Fake>>,
            headers: HeaderMap,
            body: String,
        ) -> Response {
            let auth = headers["authorization"].to_str().unwrap().to_owned();
            fake.seen.lock().push((auth.clone(), body));
            let token = auth.trim_start_matches("Bearer ").to_owned();
            let status = fake
                .replies
                .lock()
                .get_mut(&token)
                .and_then(|queue| (!queue.is_empty()).then(|| queue.remove(0)))
                .unwrap_or(200);
            let body = if status >= 500 {
                r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#
                    .to_owned()
            } else {
                format!("served by {token}")
            };
            let mut response = (StatusCode::from_u16(status).unwrap(), body).into_response();
            if status >= 400 {
                if let Some(body) = fake.error_bodies.lock().get(&token) {
                    *response.body_mut() = Body::from(body.clone());
                }
                response
                    .headers_mut()
                    .extend(fake.error_headers.lock().clone());
            }
            response.headers_mut().insert(
                "anthropic-ratelimit-unified-5h-utilization",
                "0.25".parse().unwrap(),
            );
            response
        }
        async fn token(
            State(fake): State<Arc<Fake>>,
            axum::Json(body): axum::Json<serde_json::Value>,
        ) -> Response {
            assert_eq!(body["grant_type"], "refresh_token");
            fake.refreshes.fetch_add(1, Ordering::SeqCst);
            axum::Json(serde_json::json!({
                "access_token": format!("{}-refreshed", body["refresh_token"].as_str().unwrap()),
                "expires_in": 3600,
            }))
            .into_response()
        }
        let app = axum::Router::new()
            .route("/v1/messages", axum::routing::post(messages))
            .route("/v1/messages/count_tokens", axum::routing::post(messages))
            .route("/api/oauth/profile", axum::routing::post(messages))
            .route(
                "/v1/stall-headers",
                axum::routing::post(|| async {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    StatusCode::OK
                }),
            )
            .route(
                "/v1/stall-body",
                axum::routing::post(|| async {
                    let chunks =
                        futures::stream::once(async { Ok::<_, std::convert::Infallible>("first") })
                            .chain(futures::stream::pending());
                    Body::from_stream(chunks)
                }),
            )
            .route(
                "/v1/slow-body",
                axum::routing::post(|| async {
                    let chunks = futures::stream::iter(0..5).then(|index| async move {
                        tokio::time::sleep(Duration::from_millis(150)).await;
                        Ok::<_, std::convert::Infallible>(format!("chunk{index};"))
                    });
                    Body::from_stream(chunks)
                }),
            )
            .route("/v1/oauth/token", axum::routing::post(token))
            .route(
                "/api/oauth/usage",
                axum::routing::get(
                    |State(fake): State<Arc<Fake>>, headers: HeaderMap| async move {
                        let token = headers
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default();
                        match fake.usage.lock().get(token).cloned() {
                            Some(usage) => axum::Json(usage).into_response(),
                            None => StatusCode::NOT_FOUND.into_response(),
                        }
                    },
                ),
            )
            .with_state(fake);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{address}")
    }

    fn account(id: &str, access: &str, uuid: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id, "label": id, "email": null, "account_uuid": uuid, "plan": null,
            "enabled": true, "needs_login": false,
            "tokens": {"access_token": access, "refresh_token": format!("{id}-rt"), "expires_at_ms": i64::MAX / 2},
        })
    }

    struct Harness {
        _dir: tempfile::TempDir,
        endpoint: ProxyEndpoint,
        /// What a shell gets as `ANTHROPIC_BASE_URL`.
        base_url: String,
        origin: String,
        fake: Arc<Fake>,
        http: reqwest::Client,
        client: crate::client::ProxyClient,
    }

    async fn harness(accounts: Vec<serde_json::Value>) -> Harness {
        harness_with_silence(accounts, Upstream::default().silence_timeout).await
    }

    async fn harness_with_silence(
        accounts: Vec<serde_json::Value>,
        silence_timeout: Duration,
    ) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        okena_core::ai_proxy::write_private(
            &proxy_dir(&root).join("accounts.json"),
            &serde_json::to_vec(&serde_json::json!({"accounts": accounts})).unwrap(),
        )
        .unwrap();
        let fake = Arc::new(Fake::default());
        let api = fake_upstream(fake.clone()).await;
        let upstream = Upstream {
            token_url: format!("{api}/v1/oauth/token"),
            api,
            silence_timeout,
        };
        let endpoint = okena_core::ai_proxy::ensure_endpoint(&root).unwrap();
        let management_key = ensure_management_key(&root).unwrap();
        tokio::spawn(serve(root, upstream, std::future::pending()));
        let client = crate::client::ProxyClient::new(endpoint.clone(), management_key);
        for _ in 0..100 {
            if client.health().await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Harness {
            _dir: dir,
            base_url: endpoint.base_url(),
            origin: format!("http://127.0.0.1:{}", endpoint.port),
            endpoint,
            fake,
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            client,
        }
    }

    impl Harness {
        async fn message(&self, session: &str) -> (u16, String) {
            self.message_in(None, session).await
        }

        /// A request from a terminal of `project`.
        async fn message_in(&self, project: Option<&str>, session: &str) -> (u16, String) {
            let base_url = project.map_or_else(
                || self.base_url.clone(),
                |project| self.endpoint.project_url(project),
            );
            let response = self
                .http
                .post(format!("{base_url}/v1/messages"))
                .header("authorization", "Bearer caller-own-token")
                .header("x-claude-code-session-id", session)
                .body(r#"{"model":"m","metadata":{"user_id":"{\"account_uuid\":\"caller\",\"session_id\":\"s\"}"}}"#)
                .send()
                .await
                .unwrap();
            (response.status().as_u16(), response.text().await.unwrap())
        }

        async fn set_policy(&self, project: &str, policy: serde_json::Value) {
            let command = AiProxyCommand::SetProjectPolicy {
                project_id: project.into(),
                policy: Some(serde_json::from_value(policy).unwrap()),
            };
            self.client.command(&command).await.unwrap();
        }

        fn served_tokens(&self) -> Vec<String> {
            self.fake
                .seen
                .lock()
                .iter()
                .map(|(auth, _)| auth.trim_start_matches("Bearer ").to_owned())
                .collect()
        }

        async fn post(&self, path: &str) -> reqwest::Response {
            self.http
                .post(format!("{}{path}", self.base_url))
                .send()
                .await
                .unwrap()
        }

        async fn settles_idle(&self) -> bool {
            for _ in 0..100 {
                if self.client.health().await.unwrap().inflight == 0 {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            false
        }
    }

    #[tokio::test]
    async fn quota_probe_rewrite_reaches_upstream_with_or_without_account_identity() {
        let mut without_identity = account("a", "token-a", "uuid-a");
        without_identity["account_uuid"] = serde_json::Value::Null;
        for (accounts, expected_uuid) in [
            (vec![], "caller"),
            (vec![without_identity], "caller"),
            (vec![account("a", "token-a", "uuid-a")], "uuid-a"),
        ] {
            let h = harness(accounts).await;
            let probe = serde_json::json!({
                "model": "claude-opus-5-5", "max_tokens": 1,
                "messages": [{"role": "user", "content": "quota"}],
                "metadata": {"user_id": "{\"account_uuid\":\"caller\",\"session_id\":\"s\"}"},
            });
            for path in [
                "/v1/messages?beta=true",
                "/api/oauth/profile",
                "/v1/messages/count_tokens",
            ] {
                let response = h
                    .http
                    .post(format!("{}{path}", h.base_url))
                    .header("authorization", "Bearer caller-own-token")
                    .json(&probe)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                response.text().await.unwrap();
            }
            let seen = h.fake.seen.lock();
            let rewritten: serde_json::Value = serde_json::from_str(&seen[0].1).unwrap();
            let passthrough: serde_json::Value = serde_json::from_str(&seen[1].1).unwrap();
            assert_eq!(rewritten["model"], "claude-haiku-4-5-20251001");
            assert_eq!(rewritten["messages"], probe["messages"]);
            let identity: serde_json::Value =
                serde_json::from_str(rewritten["metadata"]["user_id"].as_str().unwrap()).unwrap();
            assert_eq!(identity["session_id"], "s");
            assert_eq!(identity["account_uuid"], expected_uuid);
            assert_eq!(passthrough, probe);
            let count_tokens: serde_json::Value = serde_json::from_str(&seen[2].1).unwrap();
            assert_eq!(count_tokens["model"], "claude-opus-5-5");
        }
    }

    #[tokio::test]
    async fn quota_probe_stays_on_haiku_across_account_failover() {
        let h = harness(vec![
            account("a", "token-a", "uuid-a"),
            account("b", "token-b", "uuid-b"),
        ])
        .await;
        h.fake.replies.lock().insert("token-a".into(), vec![429]);
        let response = h
            .http
            .post(format!("{}/v1/messages", h.base_url))
            .json(&serde_json::json!({
                "model": "claude-opus-5-5", "max_tokens": 1,
                "messages": [{"role": "user", "content": "quota"}],
                "metadata": {"user_id": "{\"account_uuid\":\"caller\"}"},
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.text().await.unwrap(), "served by token-b");
        let seen = h.fake.seen.lock();
        assert_eq!(seen.len(), 2);
        for ((_, body), uuid) in seen.iter().zip(["uuid-a", "uuid-b"]) {
            let payload: serde_json::Value = serde_json::from_str(body).unwrap();
            assert_eq!(payload["model"], "claude-haiku-4-5-20251001");
            let identity: serde_json::Value =
                serde_json::from_str(payload["metadata"]["user_id"].as_str().unwrap()).unwrap();
            assert_eq!(identity["account_uuid"], uuid);
        }
    }

    #[tokio::test]
    async fn rate_limited_account_fails_over_and_conversation_sticks() {
        let h = harness(vec![
            account("a", "token-a", "uuid-a"),
            account("b", "token-b", "uuid-b"),
        ])
        .await;
        h.fake.replies.lock().insert("token-a".into(), vec![429]);
        h.fake.replies.lock().insert("token-b".into(), vec![]);
        // Fresh accounts tie and the first one (`a`) is tried; its 429 moves to `b`.
        let (status, body) = h.message("conv-1").await;
        assert_eq!(status, 200);
        assert_eq!(body, "served by token-b");
        // `a` is cooling down and the conversation sticks to `b`.
        assert_eq!(h.message("conv-1").await.1, "served by token-b");
        let seen = h.fake.seen.lock().clone();
        assert!(
            seen.iter()
                .all(|(auth, _)| auth != "Bearer caller-own-token")
        );
        let served: serde_json::Value = serde_json::from_str(&seen.last().unwrap().1).unwrap();
        let identity: serde_json::Value =
            serde_json::from_str(served["metadata"]["user_id"].as_str().unwrap()).unwrap();
        assert_eq!(identity["account_uuid"], "uuid-b");
    }

    #[tokio::test]
    async fn rejected_access_token_is_refreshed_once() {
        let h = harness(vec![account("a", "token-a", "uuid-a")]).await;
        h.fake.replies.lock().insert("token-a".into(), vec![401]);
        let (status, body) = h.message("conv").await;
        assert_eq!(status, 200);
        assert_eq!(body, "served by a-rt-refreshed");
        assert_eq!(h.fake.refreshes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn all_accounts_limited_returns_the_provider_429() {
        let h = harness(vec![account("a", "token-a", "uuid-a")]).await;
        h.fake.replies.lock().insert("token-a".into(), vec![429]);
        let (status, body) = h.message("conv").await;
        assert_eq!(status, 429);
        assert_eq!(body, "served by token-a");
        let (status, body) = h.message("conv").await;
        assert_eq!(status, 429);
        assert!(body.contains("rate limited"), "{body}");
        let response = h.post("/v1/messages").await;
        assert_eq!(response.status(), 429);
        assert!(response.headers().contains_key("retry-after"));
        assert!(
            !response
                .headers()
                .contains_key("anthropic-ratelimit-unified-status")
        );
        assert!(
            !response
                .headers()
                .contains_key("anthropic-ratelimit-unified-representative-claim")
        );
    }

    #[tokio::test]
    async fn local_quota_rejection_reports_the_first_permitted_accounts_blocking_window() {
        let h = harness(vec![
            account("a", "token-a", "uuid-a"),
            account("b", "token-b", "uuid-b"),
        ])
        .await;
        let session_reset = "2099-01-01T00:00:00Z";
        let weekly_reset = "2099-01-02T00:00:00Z";
        for (token, weekly_utilization) in [("token-a", 25.0), ("token-b", 100.0)] {
            h.fake.usage.lock().insert(
                format!("Bearer {token}"),
                serde_json::json!({
                    "five_hour": {"utilization": 100.0, "resets_at": session_reset},
                    "seven_day": {"utilization": weekly_utilization, "resets_at": weekly_reset},
                }),
            );
        }
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let status = h.client.command(&AiProxyCommand::Status).await.unwrap();
                if status.accounts.iter().all(|account| {
                    account
                        .five_hour
                        .as_ref()
                        .is_some_and(|usage| usage.utilization == 100.0)
                }) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap();

        for (policy, claim, reset) in [
            (
                serde_json::json!({"others": "allow"}),
                "five_hour",
                "4070908800",
            ),
            (
                serde_json::json!({"others": "forbid", "accounts": {"b": "allow"}}),
                "seven_day",
                "4070995200",
            ),
        ] {
            h.set_policy("quota-project", policy).await;
            for _ in 0..2 {
                let response = h
                    .http
                    .post(format!(
                        "{}/v1/messages",
                        h.endpoint.project_url("quota-project")
                    ))
                    .json(&serde_json::json!({"model": "test", "max_tokens": 8, "messages": []}))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), 429);
                assert_eq!(
                    response.headers()["anthropic-ratelimit-unified-status"],
                    "rejected"
                );
                assert_eq!(
                    response.headers()["anthropic-ratelimit-unified-representative-claim"],
                    claim
                );
                assert_eq!(
                    response.headers()["anthropic-ratelimit-unified-reset"],
                    reset,
                );
                assert!(response.headers().contains_key("retry-after"));
            }
        }
        assert!(h.fake.seen.lock().is_empty());
    }

    #[tokio::test]
    async fn logs_provider_failures_and_local_rejections_without_changing_the_response() {
        init_test_logger();
        let h = harness(vec![
            account("a", "token-a", "uuid-a"),
            account("b", "token-b", "uuid-b"),
        ])
        .await;
        let rate_limited = r#"{"error":{"type":"rate_limit_error","message":"Too many concurrent requests"},"private":"private-response-data"}"#;
        let overloaded = r#"{"error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        h.fake
            .error_bodies
            .lock()
            .insert("token-a".into(), rate_limited.into());
        h.fake
            .error_bodies
            .lock()
            .insert("token-b".into(), overloaded.into());
        for (name, value) in [
            ("request-id", "diagnostics-request"),
            ("retry-after", "60"),
            ("set-cookie", "private-cookie"),
        ] {
            h.fake
                .error_headers
                .lock()
                .insert(name, value.parse().unwrap());
        }
        h.fake.replies.lock().insert("token-a".into(), vec![429]);
        h.fake
            .replies
            .lock()
            .insert("token-b".into(), vec![529, 429]);
        assert_eq!(
            h.message_in(Some("diagnostics-test"), "conv").await,
            (529, overloaded.into())
        );
        h.fake
            .error_bodies
            .lock()
            .insert("token-b".into(), rate_limited.into());
        let response = h
            .http
            .post(format!(
                "{}/v1/messages",
                h.endpoint.project_url("diagnostics-test")
            ))
            .body(r#"{"model":"m"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()["request-id"], "diagnostics-request");
        assert_eq!(response.headers()["retry-after"], "60");
        assert_eq!(response.text().await.unwrap(), rate_limited);
        assert_eq!(h.message_in(Some("diagnostics-test"), "conv").await.0, 429);

        let logs = LOG_MESSAGES
            .lock()
            .iter()
            .filter(|line| line.contains("diagnostics-test"))
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        for expected in [
            "upstream_error",
            "status=429",
            "status=529",
            "Too many concurrent requests",
            "Overloaded",
            "diagnostics-request",
            "retry-after",
            "proxy_rejection",
            "reason=accounts_unavailable",
            "retry_at_ms=Some",
            "account=Some(\"a\")",
            "account=Some(\"b\")",
            "relay_id=",
            "model=Some(\"m\")",
            "attempt=2",
            "attempt_ms=",
            "proxy_inflight=",
            "account_availability",
            "cooldown_until_ms=Some",
            "blocked_until_ms=Some",
            "recent_traffic",
            "rate_limited_1h=1",
        ] {
            assert!(logs.contains(expected), "{logs}");
        }
        for secret in [
            "token-a",
            "token-b",
            "private-cookie",
            "private-response-data",
            "caller-own-token",
        ] {
            assert!(!logs.contains(secret), "{logs}");
        }
        let errors: Vec<_> = logs
            .lines()
            .filter(|line| line.contains("upstream_error"))
            .collect();
        let relay_id = |line: &str| {
            line.split_whitespace()
                .find(|field| field.starts_with("relay_id="))
                .unwrap()
                .to_owned()
        };
        assert_eq!(errors.len(), 3, "{logs}");
        assert_eq!(relay_id(errors[0]), relay_id(errors[1]), "{logs}");
        assert_ne!(relay_id(errors[1]), relay_id(errors[2]), "{logs}");
    }

    #[tokio::test]
    async fn compressed_provider_errors_are_logged_and_relayed_decoded() {
        use std::io::Write;

        init_test_logger();
        let rate_limited = r#"{"error":{"type":"rate_limit_error","message":"Too many concurrent requests"},"private":"private-response-data"}"#;
        for encoding in ["gzip", "deflate"] {
            let h = harness(vec![account("a", "token-a", "uuid-a")]).await;
            let encoded = if encoding == "gzip" {
                let mut encoder =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(rate_limited.as_bytes()).unwrap();
                encoder.finish().unwrap()
            } else {
                let mut encoder =
                    flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(rate_limited.as_bytes()).unwrap();
                encoder.finish().unwrap()
            };
            let encoded_len = encoded.len();
            h.fake
                .error_bodies
                .lock()
                .insert("token-a".into(), encoded.into());
            h.fake.replies.lock().insert("token-a".into(), vec![429]);
            for (name, value) in [
                ("content-encoding", encoding.to_owned()),
                ("content-type", "application/json".to_owned()),
                ("content-length", encoded_len.to_string()),
            ] {
                h.fake
                    .error_headers
                    .lock()
                    .insert(name, value.parse().unwrap());
            }
            let project = format!("compressed-diagnostics-{encoding}");
            let response = reqwest::Client::builder()
                .no_proxy()
                .no_gzip()
                .no_brotli()
                .no_deflate()
                .build()
                .unwrap()
                .post(format!("{}/v1/messages", h.endpoint.project_url(&project)))
                .header("accept-encoding", encoding)
                .body(r#"{"model":"m"}"#)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
            assert!(!response.headers().contains_key("content-encoding"));
            assert_eq!(response.text().await.unwrap(), rate_limited);
            let logs = LOG_MESSAGES
                .lock()
                .iter()
                .filter(|line| line.contains(&project))
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");
            assert!(logs.contains("Too many concurrent requests"), "{logs}");
            assert!(logs.contains("parse_error=None"), "{logs}");
            assert!(!logs.contains("private-response-data"), "{logs}");
        }
    }

    #[tokio::test]
    async fn empty_pool_passes_caller_credentials_through() {
        let h = harness(vec![]).await;
        let (status, body) = h.message("conv").await;
        assert_eq!(status, 200);
        assert_eq!(body, "served by caller-own-token");
    }

    #[tokio::test]
    async fn dot_segments_cannot_carry_an_account_out_of_inference() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let h = harness(vec![account("a", "token-a", "uuid-a")]).await;
        // reqwest would resolve the dot segments itself, so the request is raw.
        for escape in ["../..", "%2e%2e/%2E%2E"] {
            let mut socket = tokio::net::TcpStream::connect(h.origin.trim_start_matches("http://"))
                .await
                .unwrap();
            let request = format!(
                "POST /{}/v1/messages/{escape}/api/oauth/profile HTTP/1.1\r\n\
                 Host: 127.0.0.1\r\nAuthorization: Bearer caller-own-token\r\n\
                 Content-Length: 0\r\nConnection: close\r\n\r\n",
                h.endpoint.token
            );
            socket.write_all(request.as_bytes()).await.unwrap();
            let mut response = String::new();
            socket.read_to_string(&mut response).await.unwrap();
            assert!(
                response.contains("served by caller-own-token"),
                "{response}"
            );
        }
        assert_eq!(h.served_tokens(), ["caller-own-token", "caller-own-token"]);
    }

    #[tokio::test]
    async fn wrong_token_is_rejected() {
        let h = harness(vec![]).await;
        let url = h.base_url.rsplit_once('/').unwrap().0;
        let response = h
            .http
            .post(format!("{url}/nope/v1/messages"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404);
        assert!(h.fake.seen.lock().is_empty());
    }

    #[tokio::test]
    async fn usage_history_is_persisted_per_account_and_loaded_on_demand() {
        let h = harness(vec![
            account("a", "token-a", "uuid-a"),
            account("b", "token-b", "uuid-b"),
        ])
        .await;
        let query = |id: &str| AiProxyCommand::UsageHistory {
            account_id: id.into(),
        };
        assert!(h.client.command(&query("missing")).await.is_err());
        assert!(
            h.client
                .command(&query("a"))
                .await
                .unwrap()
                .usage_history
                .unwrap()
                .samples
                .is_empty()
        );
        let reset = "2099-01-01T00:00:00Z";
        for (token, utilization) in [("token-a", 23.0), ("token-b", 71.0)] {
            h.fake.usage.lock().insert(
                format!("Bearer {token}"),
                serde_json::json!({
                    "five_hour": { "utilization": utilization, "resets_at": reset },
                    "seven_day": { "utilization": utilization / 2.0, "resets_at": reset },
                }),
            );
        }
        let history = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let status = h.client.command(&query("b")).await.unwrap();
                let history = status.usage_history.unwrap();
                if !history.samples.is_empty() {
                    break history;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(history.account_id, "b");
        assert_eq!(history.samples.len(), 1);
        assert_eq!(history.samples[0].source, "b");
        assert_eq!(history.samples[0].limits[0].name, "5h");
        assert_eq!(history.samples[0].limits[0].used_percent, 71.0);
        assert_eq!(history.samples[0].limits[1].name, "7d");
        assert_eq!(history.samples[0].limits[1].used_percent, 35.5);
        assert_eq!(history.samples[0].limits[0].window_seconds, 18000.0);
        assert!(history.samples[0].limits[0].reset_at.is_some());
        let ordinary = h.client.command(&AiProxyCommand::Status).await.unwrap();
        assert!(ordinary.usage_history.is_none());
        assert!(
            serde_json::to_value(ordinary)
                .unwrap()
                .get("usage_history")
                .is_none()
        );
        let first = h
            .client
            .command(&query("a"))
            .await
            .unwrap()
            .usage_history
            .unwrap();
        assert_eq!(first.samples.len(), 1);
        assert_eq!(first.samples[0].source, "a");
        assert_eq!(first.samples[0].limits[0].used_percent, 23.0);
        assert!(h.client.shutdown().await.unwrap());
        let reopened = Accounts::open(
            &proxy_dir(h._dir.path()),
            h.http.clone(),
            Upstream::default(),
        )
        .unwrap();
        assert_eq!(reopened.usage_history("a").await.unwrap(), first);
        assert_eq!(reopened.usage_history("b").await.unwrap(), history);
    }

    #[tokio::test]
    async fn management_needs_the_management_key() {
        let h = harness(vec![account("a", "token-a", "uuid-a")]).await;
        let remove = AiProxyCommand::RemoveAccount {
            account_id: "a".into(),
        };
        let response = h
            .http
            .post(format!("{}/_okena/command", h.base_url))
            .json(&remove)
            .send()
            .await
            .unwrap();
        assert!(!response.status().is_success());
        let shell_token = h.base_url.rsplit_once('/').unwrap().1;
        for path in ["health", "command", "shutdown"] {
            let url = format!("{}/_okena/{path}", h.origin);
            let anonymous = h.http.post(&url).json(&remove).send().await.unwrap();
            assert_eq!(anonymous.status(), 401, "{path}");
            let with_shell_token = h
                .http
                .post(&url)
                .bearer_auth(shell_token)
                .json(&remove)
                .send()
                .await
                .unwrap();
            assert_eq!(with_shell_token.status(), 401, "{path}");
        }
        let status = h.client.command(&AiProxyCommand::Status).await.unwrap();
        assert_eq!(status.accounts.len(), 1);
    }

    #[tokio::test]
    async fn silent_upstream_is_cut_and_frees_the_request() {
        let h = harness_with_silence(vec![], Duration::from_millis(300)).await;
        assert_eq!(h.post("/v1/stall-headers").await.status(), 502);

        let response = h.post("/v1/stall-body").await;
        assert_eq!(response.status(), 200);
        let body = tokio::time::timeout(Duration::from_secs(10), response.bytes())
            .await
            .expect("the proxy must cut a silent stream");
        assert!(body.is_err());
        assert!(h.settles_idle().await);
    }

    #[tokio::test]
    async fn slow_but_steady_stream_is_not_cut() {
        let h = harness_with_silence(vec![], Duration::from_millis(500)).await;
        let response = h.post("/v1/slow-body").await;
        let body = tokio::time::timeout(Duration::from_secs(10), response.text())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(body, "chunk0;chunk1;chunk2;chunk3;chunk4;");
    }

    #[tokio::test]
    async fn required_account_is_waited_for_instead_of_spilling_over() {
        let h = harness(vec![
            account("a", "token-a", "uuid-a"),
            account("b", "token-b", "uuid-b"),
        ])
        .await;
        h.set_policy(
            "x",
            serde_json::json!({"others": "forbid", "accounts": {"a": "allow"}}),
        )
        .await;
        h.fake.replies.lock().insert("token-a".into(), vec![429]);

        let (status, body) = h.message_in(Some("x"), "conv-x").await;
        assert_eq!((status, body.as_str()), (429, "served by token-a"));
        let (status, body) = h.message_in(Some("x"), "conv-x").await;
        assert_eq!(status, 429);
        assert!(body.contains("rate limited"), "{body}");
        assert_eq!(h.served_tokens(), ["token-a"]);

        // A project without a policy, and a terminal without a project, may use `b`.
        assert_eq!(
            h.message_in(Some("y"), "conv-y").await.1,
            "served by token-b"
        );
        assert_eq!(h.message("conv-plain").await.1, "served by token-b");
    }

    #[tokio::test]
    async fn preferred_account_comes_first_and_fails_over() {
        let h = harness(vec![
            account("a", "token-a", "uuid-a"),
            account("b", "token-b", "uuid-b"),
        ])
        .await;
        h.set_policy(
            "y",
            serde_json::json!({"others": "allow", "accounts": {"b": "prefer"}}),
        )
        .await;
        // Fresh accounts tie; without the preference `a` would serve.
        assert_eq!(
            h.message_in(Some("y"), "conv-1").await.1,
            "served by token-b"
        );

        h.fake.replies.lock().insert("token-b".into(), vec![429]);
        assert_eq!(
            h.message_in(Some("y"), "conv-2").await.1,
            "served by token-a"
        );
    }

    #[tokio::test]
    async fn forbidding_an_account_moves_its_conversations() {
        let h = harness(vec![
            account("a", "token-a", "uuid-a"),
            account("b", "token-b", "uuid-b"),
        ])
        .await;
        assert_eq!(h.message_in(Some("z"), "conv").await.1, "served by token-a");
        h.set_policy(
            "z",
            serde_json::json!({"others": "allow", "accounts": {"a": "forbid"}}),
        )
        .await;
        assert_eq!(h.message_in(Some("z"), "conv").await.1, "served by token-b");
    }

    #[tokio::test]
    async fn forbidding_policy_never_falls_back_to_caller_credentials() {
        let h = harness(vec![account("a", "token-a", "uuid-a")]).await;
        h.set_policy(
            "z",
            serde_json::json!({"others": "allow", "accounts": {"a": "forbid"}}),
        )
        .await;
        let (status, body) = h.message_in(Some("z"), "conv").await;
        assert_eq!(status, 400);
        assert!(
            body.contains("No account this terminal's policy permits"),
            "{body}"
        );
        assert!(h.fake.seen.lock().is_empty());
    }

    #[tokio::test]
    async fn removing_an_account_drops_its_rules() {
        let h = harness(vec![
            account("a", "token-a", "uuid-a"),
            account("b", "token-b", "uuid-b"),
        ])
        .await;
        h.set_policy(
            "x",
            serde_json::json!({"others": "forbid", "accounts": {"a": "allow", "gone": "allow"}}),
        )
        .await;
        let status = h
            .client
            .command(&AiProxyCommand::RemoveAccount {
                account_id: "a".into(),
            })
            .await
            .unwrap();
        let policy = &status.routing.projects["x"];
        assert!(policy.accounts.is_empty(), "{policy:?}");
        // "Only `a`" with `a` gone permits nothing, rather than every other account.
        assert_eq!(h.message_in(Some("x"), "conv").await.0, 400);
    }

    #[tokio::test]
    async fn activity_shows_failovers_errors_and_the_own_login() {
        let h = harness(vec![
            account("a", "token-a", "uuid-a"),
            account("b", "token-b", "uuid-b"),
        ])
        .await;
        h.fake.replies.lock().insert("token-a".into(), vec![429]);
        h.fake.replies.lock().insert("token-b".into(), vec![529]);
        let (status, body) = h.message_in(Some("x"), "conv").await;
        assert_eq!(status, 529);
        assert!(body.contains("Overloaded"), "{body}");

        let activity = h
            .client
            .command(&AiProxyCommand::Status)
            .await
            .unwrap()
            .activity;
        let route = |id: &str| {
            activity
                .routes
                .iter()
                .find(|route| route.account_id.as_deref() == Some(id))
                .unwrap()
        };
        assert_eq!(route("a").rate_limited, 1);
        assert_eq!(route("a").per_minute.iter().sum::<u32>(), 0);
        assert_eq!(route("b").errors, 1);
        assert_eq!(route("b").per_minute.last(), Some(&1));
        let kinds: Vec<_> = activity.events.iter().map(|event| &event.kind).collect();
        assert_eq!(
            kinds,
            [
                &AiActivityKind::Failed {
                    account_id: Some("b".into()),
                    status: Some(529),
                    message: Some("Overloaded".into()),
                },
                &AiActivityKind::RateLimited {
                    account_id: "a".into()
                },
            ]
        );
        assert!(
            activity
                .events
                .iter()
                .all(|event| event.project_id.as_deref() == Some("x"))
        );

        let own = harness(vec![]).await;
        own.message("conv").await;
        let activity = own
            .client
            .command(&AiProxyCommand::Status)
            .await
            .unwrap()
            .activity;
        assert_eq!(activity.routes.len(), 1);
        assert_eq!(activity.routes[0].account_id, None);
        assert_eq!(activity.routes[0].per_minute.last(), Some(&1));
        assert!(activity.events.is_empty());
    }

    #[test]
    fn project_segment_is_split_off() {
        assert_eq!(
            split_project("/p/abc-1/v1/messages?beta=true"),
            Some((Some("abc-1"), "/v1/messages?beta=true"))
        );
        assert_eq!(split_project("/v1/messages"), Some((None, "/v1/messages")));
        assert_eq!(split_project("/px/v1"), Some((None, "/px/v1")));
        assert_eq!(split_project("/p/"), None);
        assert_eq!(split_project("/p/a%2Fb/v1/messages"), None);
    }

    #[test]
    fn token_must_be_the_whole_first_segment() {
        assert_eq!(strip_token("/abc/v1/messages", "abc"), Some("/v1/messages"));
        assert_eq!(strip_token("/abc?x=1", "abc"), Some("?x=1"));
        assert_eq!(strip_token("/abc", "abc"), Some(""));
        assert_eq!(strip_token("/abcd/v1", "abc"), None);
        assert_eq!(strip_token("/ab/v1", "abc"), None);
        assert_eq!(strip_token("/v1/messages", "abc"), None);
    }
}
