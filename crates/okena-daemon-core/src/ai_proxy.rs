//! Daemon side of the AI proxy (`okena-ai`): environment for new shells,
//! starting the proxy process, forwarding `ActionRequest::AiProxy`, and
//! publishing the pool's accounts to clients.

use okena_core::ai_proxy::{
    AiAccount, AiProxyCommand, AiProxyState, ProxyEndpoint, ensure_endpoint, ensure_management_key,
    load_endpoint,
};
use okena_terminal::backend::{LaunchEnvironment, TerminalBackend};
use okena_workspace::persistence::AppSettings;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, watch};

/// How often the published accounts follow the proxy's usage and cooldowns.
const ACCOUNTS_POLL_INTERVAL: Duration = Duration::from_secs(15);
/// Longest wait between attempts to start a proxy that keeps failing to come up.
const MAX_RESTART_DELAY: Duration = Duration::from_secs(10 * 60);

fn profile_root() -> Option<PathBuf> {
    okena_core::profiles::try_current().map(|profile| profile.root.clone())
}

/// Environment for newly spawned PTYs: the Claude config dir and, with the
/// proxy enabled, `ANTHROPIC_BASE_URL` naming the terminal's project, so the
/// proxy applies that project's routing policy. Shells that already run keep
/// the environment they started with.
pub(crate) fn launch_environment(settings: &AppSettings) -> LaunchEnvironment {
    let claude_env = okena_workspace::claude_env::claude_pty_env_for_settings(settings);
    let endpoint = if settings.ai_proxy_enabled
        && let Some(root) = profile_root()
    {
        ensure_endpoint(&root)
            .inspect_err(|error| log::error!("[ai-proxy] no endpoint for new shells: {error:#}"))
            .ok()
    } else {
        None
    };
    Arc::new(move |project_id| {
        let mut env = claude_env.clone();
        if let Some(endpoint) = &endpoint {
            let url = project_id.map_or_else(
                || endpoint.base_url(),
                |project_id| endpoint.project_url(project_id),
            );
            env.push(("ANTHROPIC_BASE_URL".into(), Some(url)));
        }
        env
    })
}

async fn connect() -> anyhow::Result<okena_ai::client::ProxyClient> {
    let root = profile_root().ok_or_else(|| anyhow::anyhow!("No active profile"))?;
    let exe = okena_remote_server::local::remember_current_executable()?;
    okena_ai::client::ensure_running(&root, &exe).await
}

/// Start the proxy when it is enabled, then refresh the shell environment in
/// case the proxy had to move to another port.
pub(crate) fn start_if_enabled(
    settings: &Arc<Mutex<AppSettings>>,
    backend: &Arc<dyn TerminalBackend>,
    runtime: &tokio::runtime::Handle,
) {
    if !settings.lock().ai_proxy_enabled {
        return;
    }
    let settings = settings.clone();
    let backend = backend.clone();
    runtime.spawn(async move {
        match connect().await {
            Ok(_) => backend.set_launch_environment(launch_environment(&settings.lock())),
            Err(error) => log::error!("[ai-proxy] failed to start: {error:#}"),
        }
    });
}

/// Run one management command against the proxy, starting it if needed.
/// Management works whether or not new shells are routed through the proxy.
pub(crate) async fn execute(command: AiProxyCommand) -> AiProxyState {
    let client = match connect().await {
        Ok(client) => client,
        Err(error) => {
            return AiProxyState {
                status: None,
                failure: Some(format!("{error:#}")),
            };
        }
    };
    match client.command(&command).await {
        Ok(status) => AiProxyState {
            status: Some(status),
            failure: None,
        },
        // A failed command (e.g. a mistyped login code) still shows the pool.
        Err(error) => AiProxyState {
            status: client.command(&AiProxyCommand::Status).await.ok(),
            failure: Some(format!("{error:#}")),
        },
    }
}

/// The pool's accounts as clients see them in `StateResponse::ai_accounts`.
#[derive(Clone)]
pub struct AiProxyAccounts {
    accounts: Arc<Mutex<Vec<AiAccount>>>,
    state_version: watch::Sender<u64>,
    refresh: Arc<Notify>,
}

impl AiProxyAccounts {
    pub(crate) fn new(state_version: watch::Sender<u64>) -> Self {
        Self {
            accounts: Arc::new(Mutex::new(Vec::new())),
            state_version,
            refresh: Arc::new(Notify::new()),
        }
    }

    pub(crate) fn snapshot(&self) -> Vec<AiAccount> {
        self.accounts.lock().clone()
    }

    /// Re-read the accounts now instead of at the next poll.
    pub(crate) fn refresh(&self) {
        self.refresh.notify_one();
    }

    fn publish(&self, accounts: Vec<AiAccount>) {
        let mut published = self.accounts.lock();
        if *published != accounts {
            *published = accounts;
            self.state_version
                .send_modify(|version| *version = version.wrapping_add(1));
        }
    }
}

/// Keep [`AiProxyAccounts`] in step with the proxy while new shells are routed
/// through it. New shells get its URL whether or not it listens, so a proxy
/// that died is started again, and a port it moved to reaches new shells.
/// With routing off, or the proxy unreachable, nothing is published: the
/// `claude` login's own usage is then the relevant one.
pub(crate) async fn run_accounts_poll(
    accounts: AiProxyAccounts,
    settings: Arc<Mutex<AppSettings>>,
    backend: Arc<dyn TerminalBackend>,
) {
    let mut routed_endpoint: Option<ProxyEndpoint> = None;
    let mut restart = RestartBackoff::new();
    loop {
        let enabled = settings.lock().ai_proxy_enabled;
        let current = if enabled {
            match read_or_restart(&mut restart).await {
                Ok((endpoint, current)) => {
                    if routed_endpoint.as_ref() != Some(&endpoint) {
                        backend.set_launch_environment(launch_environment(&settings.lock()));
                        routed_endpoint = Some(endpoint);
                    }
                    current
                }
                Err(error) => {
                    log::warn!("[ai-proxy] proxy unreachable: {error:#}");
                    Vec::new()
                }
            }
        } else {
            routed_endpoint = None;
            Vec::new()
        };
        accounts.publish(current);
        tokio::select! {
            _ = tokio::time::sleep(ACCOUNTS_POLL_INTERVAL) => {}
            _ = accounts.refresh.notified() => {}
        }
    }
}

/// Read the running proxy's accounts; when it does not answer, start it, at
/// most as often as `restart` allows.
async fn read_or_restart(
    restart: &mut RestartBackoff,
) -> anyhow::Result<(ProxyEndpoint, Vec<AiAccount>)> {
    match read_accounts().await {
        Ok(read) => {
            restart.reset();
            return Ok(read);
        }
        Err(error) if !restart.due() => return Err(error),
        Err(error) => log::warn!("[ai-proxy] proxy unreachable, starting it: {error:#}"),
    }
    restart.back_off();
    connect().await?;
    read_accounts().await
}

/// Ask a running proxy for its accounts without starting one.
async fn read_accounts() -> anyhow::Result<(ProxyEndpoint, Vec<AiAccount>)> {
    let root = profile_root().ok_or_else(|| anyhow::anyhow!("No active profile"))?;
    let endpoint = load_endpoint(&root)?.ok_or_else(|| anyhow::anyhow!("No proxy endpoint"))?;
    let client =
        okena_ai::client::ProxyClient::new(endpoint.clone(), ensure_management_key(&root)?);
    let accounts = client.command(&AiProxyCommand::Status).await?.accounts;
    Ok((endpoint, accounts))
}

/// Doubling delay between start attempts, so a proxy that exits at once (e.g.
/// on a corrupt `accounts.json`) is not respawned on every poll.
struct RestartBackoff {
    next_attempt: tokio::time::Instant,
    delay: Duration,
}

impl RestartBackoff {
    fn new() -> Self {
        Self {
            next_attempt: tokio::time::Instant::now(),
            delay: ACCOUNTS_POLL_INTERVAL,
        }
    }

    fn due(&self) -> bool {
        tokio::time::Instant::now() >= self.next_attempt
    }

    fn back_off(&mut self) {
        self.next_attempt = tokio::time::Instant::now() + self.delay;
        self.delay = (self.delay * 2).min(MAX_RESTART_DELAY);
    }

    fn reset(&mut self) {
        *self = Self::new();
    }
}
