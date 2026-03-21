use crate::remote::auth::AuthStore;
use crate::remote::bridge::BridgeSender;
use crate::remote::pty_broadcaster::PtyBroadcaster;
use crate::remote::routes;
use okena_core::api::ApiGitStatus;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::sync::watch;

/// Handle to a running remote control server.
/// Dropping this will trigger shutdown.
pub struct RemoteServer {
    shutdown_tx: watch::Sender<bool>,
    runtime: Option<tokio::runtime::Runtime>,
    port: u16,
}

impl RemoteServer {
    /// Start the remote control server on a background tokio runtime.
    ///
    /// Tries ports 19100-19200, falling back to OS-assigned (port 0).
    /// Writes `remote.json` with port + pid on success.
    pub fn start(
        bridge_tx: BridgeSender,
        auth_store: Arc<AuthStore>,
        broadcaster: Arc<PtyBroadcaster>,
        state_version: Arc<watch::Sender<u64>>,
        bind_addr: IpAddr,
        git_status: Arc<watch::Sender<HashMap<String, ApiGitStatus>>>,
    ) -> anyhow::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("okena-remote")
            .build()?;

        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        // Clean up stale remote.json from a previous crash
        cleanup_stale_remote_json();

        // Try to bind to a port
        let listener = runtime.block_on(async {
            // Try preferred range first
            for port in 19100..=19200 {
                let addr = SocketAddr::new(bind_addr, port);
                if let Ok(listener) = tokio::net::TcpListener::bind(addr).await {
                    return Ok(listener);
                }
            }
            // Fall back to OS-assigned port
            let addr = SocketAddr::new(bind_addr, 0);
            tokio::net::TcpListener::bind(addr).await
        })?;

        let port = listener.local_addr()?.port();
        log::info!("Remote control server listening on {}:{}", bind_addr, port);

        // Write remote.json
        if let Err(e) = write_remote_json(port) {
            log::warn!("Failed to write remote.json: {}", e);
        }

        let start_time = std::time::Instant::now();

        // Spawn the server task
        let shutdown_rx_clone = shutdown_rx.clone();
        runtime.spawn(async move {
            let app = routes::build_router(
                bridge_tx,
                auth_store,
                broadcaster,
                state_version,
                start_time,
                git_status,
            );

            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(shutdown_signal(shutdown_rx_clone))
            .await
            .ok();

            log::info!("Remote control server shut down");
        });

        Ok(Self {
            shutdown_tx,
            runtime: Some(runtime),
            port,
        })
    }

    /// Get the port the server is listening on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Stop the server gracefully.
    pub fn stop(&mut self) {
        // Signal shutdown
        let _ = self.shutdown_tx.send(true);

        // Shut down the tokio runtime
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(std::time::Duration::from_secs(5));
        }

        // Remove remote.json
        remove_remote_json();

        log::info!("Remote control server stopped");
    }
}

impl Drop for RemoteServer {
    fn drop(&mut self) {
        if self.runtime.is_some() {
            self.stop();
        }
    }
}

/// Wait until the shutdown signal is received.
async fn shutdown_signal(mut rx: watch::Receiver<bool>) {
    while !*rx.borrow_and_update() {
        if rx.changed().await.is_err() {
            break;
        }
    }
}

/// Path to remote.json in the config dir.
fn remote_json_path() -> std::path::PathBuf {
    crate::workspace::persistence::config_dir().join("remote.json")
}

/// Write remote.json atomically (temp file + rename).
fn write_remote_json(port: u16) -> anyhow::Result<()> {
    let path = remote_json_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let content = serde_json::json!({
        "port": port,
        "pid": std::process::id(),
    });

    let tmp_path = path.with_extension("tmp");
    std::fs::write(&tmp_path, serde_json::to_string_pretty(&content)?)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&tmp_path, perms)?;
    }

    std::fs::rename(&tmp_path, &path)?;

    Ok(())
}

/// Remove remote.json on shutdown.
fn remove_remote_json() {
    let path = remote_json_path();
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
}

/// Clean up stale remote.json left behind by a crashed process.
fn cleanup_stale_remote_json() {
    let path = remote_json_path();
    let data = match std::fs::read_to_string(&path) {
        Ok(data) => data,
        Err(_) => return,
    };

    let json: serde_json::Value = match serde_json::from_str(&data) {
        Ok(v) => v,
        Err(_) => {
            let _ = std::fs::remove_file(&path);
            return;
        }
    };

    if let Some(pid) = json.get("pid").and_then(|v| v.as_u64()) {
        if !is_process_alive(pid as u32) {
            log::info!("Removing stale remote.json (pid {} is dead)", pid);
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Check if a process with the given PID is still running.
fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // signal 0 checks if process exists without sending a signal
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        // On non-Unix platforms, assume the process may be alive to avoid
        // accidentally removing a valid remote.json. The stale file is
        // harmless — the port will simply fail to bind and we'll pick another.
        let _ = pid;
        true
    }
}
