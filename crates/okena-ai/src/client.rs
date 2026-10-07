//! Daemon-side access to the proxy: start it when needed and send management commands.

use crate::server::Health;
use okena_core::ai_proxy::{
    AiProxyCommand, AiProxyStatus, ProxyEndpoint, ensure_endpoint, load_endpoint,
};
use std::path::Path;
use std::time::Duration;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

pub struct ProxyClient {
    endpoint: ProxyEndpoint,
    http: reqwest::Client,
}

impl ProxyClient {
    pub fn new(endpoint: ProxyEndpoint) -> Self {
        Self {
            endpoint,
            http: reqwest::Client::builder()
                // Loopback must never go through a configured HTTP proxy.
                .no_proxy()
                .build()
                .unwrap_or_default(),
        }
    }

    pub fn endpoint(&self) -> &ProxyEndpoint {
        &self.endpoint
    }

    fn url(&self, path: &str) -> String {
        format!("{}/_okena/{path}", self.endpoint.base_url())
    }

    pub async fn health(&self) -> anyhow::Result<Health> {
        Ok(self
            .http
            .get(self.url("health"))
            .timeout(Duration::from_secs(2))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    pub async fn command(&self, command: &AiProxyCommand) -> anyhow::Result<AiProxyStatus> {
        let response = self
            .http
            .post(self.url("command"))
            .json(command)
            // Finishing a login waits for the provider's token endpoint.
            .timeout(Duration::from_secs(60))
            .send()
            .await?;
        if !response.status().is_success() {
            let body: serde_json::Value = response.json().await.unwrap_or_default();
            let message = body["error"]["message"]
                .as_str()
                .unwrap_or("AI proxy command failed");
            anyhow::bail!("{message}");
        }
        Ok(response.json().await?)
    }

    /// Ask an idle proxy to exit. `false` when requests are in flight.
    pub async fn shutdown(&self) -> anyhow::Result<bool> {
        let response = self
            .http
            .post(self.url("shutdown"))
            .timeout(Duration::from_secs(2))
            .send()
            .await?;
        Ok(response.status().is_success())
    }
}

/// Make sure a proxy of this build serves the profile, starting `exe --ai-proxy`
/// when none answers. A proxy of another version is replaced only while idle, so
/// an update never cuts a running response; otherwise the old one keeps serving.
pub async fn ensure_running(profile_root: &Path, exe: &Path) -> anyhow::Result<ProxyClient> {
    let client = ProxyClient::new(ensure_endpoint(profile_root)?);
    match client.health().await {
        Ok(health) if health.version == crate::VERSION => return Ok(client),
        Ok(health) => {
            if health.inflight > 0 || !client.shutdown().await.unwrap_or(false) {
                log::info!(
                    "[ai-proxy] keeping busy proxy {} until it is idle",
                    health.version
                );
                return Ok(client);
            }
            log::info!(
                "[ai-proxy] replacing proxy {} with {}",
                health.version,
                crate::VERSION
            );
            wait_until(|| async { client.health().await.is_err() }).await;
        }
        Err(_) => {}
    }
    spawn_detached(exe, profile_root)?;
    let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        // The proxy may have moved to another port if the old one was taken.
        if let Ok(Some(endpoint)) = load_endpoint(profile_root) {
            let started = ProxyClient::new(endpoint);
            if started.health().await.is_ok() {
                return Ok(started);
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("AI proxy did not start; see ai-proxy/proxy.log")
}

async fn wait_until<F, Fut>(mut ready: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if ready().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Start the proxy outside the daemon's session/process group, so stopping the
/// daemon (or its terminal) does not stop the proxy that live shells point at.
fn spawn_detached(exe: &Path, profile_root: &Path) -> anyhow::Result<()> {
    let mut command = std::process::Command::new(exe);
    command
        .arg(crate::PROXY_FLAG)
        .arg(profile_root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and touches no parent state.
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    let mut child = command.spawn()?;
    // Reap the child if it exits while the daemon still runs.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
