//! Daemon side of the AI proxy (`okena-ai`): environment for new shells,
//! starting the proxy process, and forwarding `ActionRequest::AiProxy`.

use okena_core::ai_proxy::{AiProxyCommand, AiProxyState, ensure_endpoint};
use okena_terminal::backend::{LaunchEnvironment, TerminalBackend};
use okena_workspace::persistence::AppSettings;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;

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
