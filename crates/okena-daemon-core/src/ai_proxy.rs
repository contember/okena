//! Daemon side of the AI proxy (`okena-ai`): environment for new shells,
//! starting the proxy process, and forwarding `ActionRequest::AiProxy`.

use crate::claude_remote::PaneEnv;
use okena_core::ai_proxy::{AiProxyCommand, AiProxyState, ensure_endpoint};
use okena_terminal::backend::TerminalBackend;
use okena_workspace::persistence::AppSettings;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;

fn profile_root() -> Option<PathBuf> {
    okena_core::profiles::try_current().map(|profile| profile.root.clone())
}

/// Extra environment for newly spawned PTYs: the Claude config dir and, with
/// the proxy enabled, `ANTHROPIC_BASE_URL`. Shells that already run keep the
/// environment they started with. [`PaneEnv::pty_env`] adds Claude remote on top.
pub(crate) fn pty_env(settings: &AppSettings) -> Vec<(String, Option<String>)> {
    let mut env = okena_workspace::claude_env::claude_pty_env_for_settings(settings);
    if settings.ai_proxy_enabled
        && let Some(root) = profile_root()
    {
        match ensure_endpoint(&root) {
            Ok(endpoint) => env.push(("ANTHROPIC_BASE_URL".into(), Some(endpoint.base_url()))),
            Err(error) => log::error!("[ai-proxy] no endpoint for new shells: {error:#}"),
        }
    }
    env
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
    pane_env: &Arc<PaneEnv>,
    runtime: &tokio::runtime::Handle,
) {
    if !settings.lock().ai_proxy_enabled {
        return;
    }
    let settings = settings.clone();
    let backend = backend.clone();
    let pane_env = pane_env.clone();
    runtime.spawn(async move {
        match connect().await {
            Ok(_) => backend.set_extra_env(pane_env.pty_env(&settings.lock())),
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
