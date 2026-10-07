//! The `okena --ai-proxy <profile-root>` process.

use crate::accounts::Accounts;
use crate::oauth::Upstream;
use crate::proxy::{Relay, error};
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use fs2::FileExt;
use okena_core::ai_proxy::{AiProxyCommand, ProxyEndpoint, proxy_dir};
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
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .build()?;
    let accounts = Arc::new(Accounts::open(
        proxy_dir(&profile_root).join("accounts.json"),
        http.clone(),
        upstream.clone(),
    )?);
    let service = Arc::new(Service {
        token: endpoint.token,
        relay: Relay {
            accounts: accounts.clone(),
            http,
            api: upstream.api,
            inflight: Arc::new(AtomicUsize::new(0)),
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
    let Some(rest) = strip_token(&target, &service.token) else {
        return error(StatusCode::NOT_FOUND, "Unknown endpoint");
    };
    let rest = rest.to_owned();
    if let Some(management) = rest.strip_prefix("/_okena/") {
        return manage(&service, management, request.into_body()).await;
    }
    let (parts, body) = request.into_parts();
    service
        .relay
        .handle(parts.method, &rest, parts.headers, body)
        .await
}

/// `/<token>/rest` → `/rest`, comparing the token in constant time.
fn strip_token<'a>(target: &'a str, token: &str) -> Option<&'a str> {
    let after_slash = target.strip_prefix('/')?;
    let end = after_slash.find(['/', '?']).unwrap_or(after_slash.len());
    let (candidate, rest) = after_slash.split_at(end);
    let matches = candidate.len() == token.len()
        && candidate
            .bytes()
            .zip(token.bytes())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0;
    matches.then_some(rest)
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
            let outcome = match command {
                AiProxyCommand::Status => Ok(()),
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
            };
            match outcome {
                Ok(()) => axum::Json(accounts.status(inflight)).into_response(),
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
    use parking_lot::Mutex;
    use std::collections::HashMap;

    /// Fake provider: per access token, a queue of statuses for `/v1/messages`.
    #[derive(Default)]
    struct Fake {
        replies: Mutex<HashMap<String, Vec<u16>>>,
        seen: Mutex<Vec<(String, String)>>,
        refreshes: AtomicUsize,
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
            let mut response = (
                StatusCode::from_u16(status).unwrap(),
                format!("served by {token}"),
            )
                .into_response();
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
            .route("/v1/oauth/token", axum::routing::post(token))
            .route(
                "/api/oauth/usage",
                axum::routing::get(|| async { StatusCode::NOT_FOUND }),
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
        base_url: String,
        fake: Arc<Fake>,
        http: reqwest::Client,
    }

    async fn harness(accounts: Vec<serde_json::Value>) -> Harness {
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
        };
        let endpoint = okena_core::ai_proxy::ensure_endpoint(&root).unwrap();
        tokio::spawn(serve(root, upstream, std::future::pending()));
        let client = crate::client::ProxyClient::new(endpoint.clone());
        for _ in 0..100 {
            if client.health().await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Harness {
            _dir: dir,
            base_url: endpoint.base_url(),
            fake,
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
        }
    }

    impl Harness {
        async fn message(&self, session: &str) -> (u16, String) {
            let response = self
                .http
                .post(format!("{}/v1/messages", self.base_url))
                .header("authorization", "Bearer caller-own-token")
                .header("x-claude-code-session-id", session)
                .body(r#"{"model":"m","metadata":{"user_id":"{\"account_uuid\":\"caller\",\"session_id\":\"s\"}"}}"#)
                .send()
                .await
                .unwrap();
            (response.status().as_u16(), response.text().await.unwrap())
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
    }

    #[tokio::test]
    async fn empty_pool_passes_caller_credentials_through() {
        let h = harness(vec![]).await;
        let (status, body) = h.message("conv").await;
        assert_eq!(status, 200);
        assert_eq!(body, "served by caller-own-token");
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
