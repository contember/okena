//! Daemon-side access to the proxy: start it when needed and send management commands.

use crate::management::Address;
use crate::server::{Health, LOCK_WAIT};
use okena_core::ai_proxy::{
    AiProxyCommand, AiProxyStatus, ProxyEndpoint, ensure_endpoint, ensure_management_key,
    load_endpoint,
};
use std::path::Path;
use std::process::Child;
use std::time::Duration;

/// How long a proxy asked to exit may take to stop answering.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
/// A new proxy may first wait for the lock of the one it replaces.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(LOCK_WAIT.as_secs() + 10);

pub struct ProxyClient {
    endpoint: ProxyEndpoint,
    management_key: String,
    http: reqwest::Client,
}

impl ProxyClient {
    /// Client for the proxy of `profile_root`, from its endpoint and management
    /// key. Fails when there is no endpoint yet.
    pub fn open(profile_root: &Path) -> anyhow::Result<Self> {
        let endpoint =
            load_endpoint(profile_root)?.ok_or_else(|| anyhow::anyhow!("No proxy endpoint"))?;
        let management_key = ensure_management_key(profile_root)?;
        let http = Address::new(profile_root, &management_key).client()?;
        Ok(Self {
            endpoint,
            management_key,
            http,
        })
    }

    pub fn endpoint(&self) -> &ProxyEndpoint {
        &self.endpoint
    }

    fn management(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        // The host is not resolved: every request goes to the management address.
        self.http
            .request(method, format!("http://okena-ai-proxy/_okena/{path}"))
            .bearer_auth(&self.management_key)
    }

    pub async fn health(&self) -> anyhow::Result<Health> {
        Ok(self
            .management(reqwest::Method::GET, "health")
            .timeout(Duration::from_secs(2))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    pub async fn command(&self, command: &AiProxyCommand) -> anyhow::Result<AiProxyStatus> {
        let response = self
            .management(reqwest::Method::POST, "command")
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
            .management(reqwest::Method::POST, "shutdown")
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
    ensure_endpoint(profile_root)?;
    let client = ProxyClient::open(profile_root)?;
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
            wait_until_stopped(&client).await;
        }
        Err(_) => {}
    }
    let mut launched = spawn_detached(exe, profile_root)?;
    let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;
    loop {
        if client.health().await.is_ok() {
            launched.reap();
            // The proxy may have moved to another port if the old one was taken.
            return ProxyClient::open(profile_root);
        }
        if let Some(status) = launched.child.try_wait()? {
            if launched.own_scope && !status.success() {
                log::warn!("[ai-proxy] systemd-run failed ({status}); starting the proxy directly");
                launched = spawn_detached_here(exe, profile_root)?;
                continue;
            }
            anyhow::bail!("AI proxy exited ({status}); see ai-proxy/proxy.log");
        }
        if tokio::time::Instant::now() >= deadline {
            launched.reap();
            anyhow::bail!("AI proxy did not start; see ai-proxy/proxy.log");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_until_stopped(client: &ProxyClient) {
    let deadline = tokio::time::Instant::now() + SHUTDOWN_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if client.health().await.is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

struct Launched {
    child: Child,
    /// Started through `systemd-run --scope`, whose own failure looks like an exit.
    own_scope: bool,
}

impl Launched {
    /// Reap the child if it exits while the daemon still runs.
    fn reap(self) {
        let mut child = self.child;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// Start the proxy so that stopping the daemon, its terminal or its cgroup
/// does not stop the proxy that live shells point at. On Linux it gets its own
/// systemd scope, so an OOM kill of the app's cgroup spares it.
fn spawn_detached(exe: &Path, profile_root: &Path) -> anyhow::Result<Launched> {
    #[cfg(target_os = "linux")]
    {
        let mut command = std::process::Command::new("systemd-run");
        command
            .args(["--user", "--scope", "--quiet", "--collect", "--"])
            .arg(exe);
        match spawn(command, profile_root) {
            Ok(child) => {
                return Ok(Launched {
                    child,
                    own_scope: true,
                });
            }
            Err(error) => {
                log::info!("[ai-proxy] systemd-run unavailable ({error}); starting the proxy directly");
            }
        }
    }
    spawn_detached_here(exe, profile_root)
}

/// Start the proxy outside the daemon's session/process group (Unix) or
/// console and job (Windows).
fn spawn_detached_here(exe: &Path, profile_root: &Path) -> anyhow::Result<Launched> {
    let child = spawn(std::process::Command::new(exe), profile_root)?;
    Ok(Launched {
        child,
        own_scope: false,
    })
}

fn spawn(mut command: std::process::Command, profile_root: &Path) -> std::io::Result<Child> {
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
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const ERROR_ACCESS_DENIED: i32 = 5;
        let detached = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW;
        // Outside the app's job, so closing the job does not end the proxy.
        command.creation_flags(detached | CREATE_BREAKAWAY_FROM_JOB);
        match command.spawn() {
            // The job does not allow breakaway; stay in it rather than not start.
            Err(error) if error.raw_os_error() == Some(ERROR_ACCESS_DENIED) => {
                command.creation_flags(detached);
            }
            result => return result,
        }
    }
    command.spawn()
}
