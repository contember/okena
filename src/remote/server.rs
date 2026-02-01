use crate::remote::auth::AuthStore;
use crate::remote::bridge::BridgeSender;
use crate::remote::pty_broadcaster::PtyBroadcaster;
use crate::remote::routes;
use std::net::SocketAddr;
use std::sync::atomic::AtomicU64;
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
        state_version: Arc<AtomicU64>,
    ) -> anyhow::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("muxy-remote")
            .build()?;

        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        // Try to bind to a port
        let listener = runtime.block_on(async {
            // Try preferred range first
            for port in 19100..=19200 {
                let addr = SocketAddr::from(([127, 0, 0, 1], port));
                if let Ok(listener) = tokio::net::TcpListener::bind(addr).await {
                    return Ok(listener);
                }
            }
            // Fall back to OS-assigned port
            let addr = SocketAddr::from(([127, 0, 0, 1], 0));
            tokio::net::TcpListener::bind(addr).await
        })?;

        let port = listener.local_addr()?.port();
        log::info!("Remote control server listening on 127.0.0.1:{}", port);

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
