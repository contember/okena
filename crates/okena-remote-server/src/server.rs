use crate::auth::AuthStore;
use crate::bridge::BridgeSender;
use crate::pty_broadcaster::PtyBroadcaster;
use crate::routes;
use okena_core::api::{ApiGitStatus, ApiToast};
use okena_transport::client::LocalEndpoint;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, RwLock};
use tokio::sync::watch;

/// Handle to a running remote control server.
/// Dropping this will trigger shutdown.
pub struct RemoteServer {
    shutdown_tx: watch::Sender<bool>,
    runtime: Option<tokio::runtime::Runtime>,
    port: u16,
    /// SHA-256 fingerprint (lowercase hex) of the TLS cert, when TLS is enabled.
    cert_fingerprint: Option<String>,
}

impl RemoteServer {
    /// Start the remote control server on a background tokio runtime.
    ///
    /// Tries ports 19100-19200, falling back to OS-assigned (port 0).
    /// Writes `remote.json` with port + pid on success.
    // Each param is a distinct channel/state dependency wired into the server.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        bridge_tx: BridgeSender,
        auth_store: Arc<AuthStore>,
        broadcaster: Arc<PtyBroadcaster>,
        state_version: Arc<watch::Sender<u64>>,
        bind_addr: IpAddr,
        git_status: Arc<watch::Sender<HashMap<String, ApiGitStatus>>>,
        toast_tx: Arc<tokio::sync::broadcast::Sender<ApiToast>>,
        remote_subscribed_terminals: Arc<RwLock<HashMap<u64, HashSet<String>>>>,
        next_connection_id: Arc<AtomicU64>,
        tls_enabled: bool,
        app_version: &'static str,
    ) -> anyhow::Result<Self> {
        let _slow = okena_core::timing::SlowGuard::new("RemoteServer::start");
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

        // Load (or generate) the persisted self-signed cert when TLS is enabled.
        // If cert setup fails we deliberately do NOT silently fall back to plain
        // http — that would defeat the point — so the error propagates and the
        // server stays off until the user fixes it.
        let tls_material = if tls_enabled {
            let dir = okena_workspace::persistence::config_dir();
            Some(crate::tls::load_or_generate(&dir)?)
        } else {
            None
        };
        let cert_fingerprint = tls_material.as_ref().map(|m| m.fingerprint.clone());

        let scheme = if tls_enabled {
            "http+https dual-stack"
        } else {
            "http/ws"
        };
        log::info!(
            "Remote control server listening on {}:{} ({})",
            bind_addr,
            port,
            scheme
        );
        if let Some(fp) = &cert_fingerprint {
            log::info!(
                "Remote TLS certificate fingerprint (verify this on the client when pairing):\n  {}",
                fp
            );
        }

        // Warn loudly when bound to a non-loopback address WITHOUT TLS: the
        // pairing token and all terminal I/O would travel the network in cleartext.
        if !bind_addr.is_loopback() && !tls_enabled {
            log::warn!(
                "Remote control server is bound to a NON-LOOPBACK address ({}:{}) WITHOUT TLS.\n\
                 The connection is UNENCRYPTED (http/ws): the pairing token and all terminal\n\
                 I/O (including passwords, SSH keys, and any typed secrets) are sent in cleartext\n\
                 and visible to anyone on the network. Enable TLS in settings, only use this on a\n\
                 trusted network, or tunnel it over SSH/WireGuard.",
                bind_addr,
                port
            );
        }

        let mut local_endpoint = crate::local::default_local_endpoint();
        #[cfg(unix)]
        let local_listener = match &local_endpoint {
            Some(LocalEndpoint::UnixSocket { path }) => {
                let path_buf = std::path::PathBuf::from(path);
                match crate::serve::bind_unix_socket(&path_buf) {
                    Ok(listener) => Some((path_buf, listener)),
                    Err(e) => {
                        log::warn!("Failed to bind local daemon socket at {path}: {e}");
                        local_endpoint = None;
                        None
                    }
                }
            }
            _ => None,
        };

        // Write remote.json
        if let Err(e) = write_remote_json(port, tls_enabled, local_endpoint.as_ref()) {
            log::warn!("Failed to write remote.json: {}", e);
        }

        let start_time = std::time::Instant::now();
        okena_ext_updater::installer::cleanup_old_binary();
        let update_info = okena_ext_updater::UpdateInfo::new(app_version.to_string());

        // Spawn the server task
        let shutdown_rx_clone = shutdown_rx.clone();
        runtime.spawn(async move {
            routes::update::spawn_background_checker(update_info.clone());
            let app = routes::build_router(
                bridge_tx,
                auth_store,
                broadcaster,
                state_version,
                start_time,
                git_status,
                toast_tx,
                remote_subscribed_terminals,
                next_connection_id,
                update_info,
            );
            #[cfg(unix)]
            let local_server = if let Some((path, listener)) = local_listener {
                Some(tokio::spawn(crate::serve::serve_unix_listener(
                    path,
                    listener,
                    app.clone(),
                    shutdown_signal(shutdown_rx.clone()),
                )))
            } else {
                None
            };

            let serve_result = if let Some(material) = tls_material {
                // TLS enabled → dual-stack: accept BOTH http and TLS on this one
                // port so already-paired plain-http clients keep working while
                // new/auto clients negotiate TLS.
                match crate::tls::server_config(&material) {
                    Ok(tls_config) => {
                        crate::serve::serve_dual_stack(
                            listener,
                            app,
                            tls_config,
                            shutdown_signal(shutdown_rx_clone),
                        )
                        .await
                    }
                    Err(e) => {
                        log::error!("Failed to build TLS server config: {e:#}");
                        Err(std::io::Error::other(e.to_string()))
                    }
                }
            } else {
                // TLS disabled → plain http only.
                crate::serve::serve_plain(listener, app, shutdown_signal(shutdown_rx_clone)).await
            };

            match serve_result {
                Ok(()) => log::info!("Remote control server shut down"),
                Err(e) => log::error!("Remote control server exited with error: {e}"),
            }

            #[cfg(unix)]
            if let Some(handle) = local_server {
                handle.abort();
            }
        });

        Ok(Self {
            shutdown_tx,
            runtime: Some(runtime),
            port,
            cert_fingerprint,
        })
    }

    /// Get the port the server is listening on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// SHA-256 fingerprint (lowercase hex) of the TLS cert, when TLS is enabled.
    pub fn cert_fingerprint(&self) -> Option<String> {
        self.cert_fingerprint.clone()
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
    okena_workspace::persistence::config_dir().join("remote.json")
}

/// Write remote.json atomically (temp file + rename).
fn write_remote_json(
    port: u16,
    tls_enabled: bool,
    local_endpoint: Option<&LocalEndpoint>,
) -> anyhow::Result<()> {
    let path = remote_json_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut content = serde_json::json!({
        "port": port,
        "pid": std::process::id(),
        "tls": tls_enabled,
    });
    if let Some(local_endpoint) = local_endpoint {
        content["local_endpoint"] = serde_json::to_value(local_endpoint)?;
    }

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

    if let Some(pid) = json.get("pid").and_then(|v| v.as_u64())
        && !crate::local::is_process_alive(pid as u32)
    {
        log::info!("Removing stale remote.json (pid {} is dead)", pid);
        let _ = std::fs::remove_file(&path);
    }
}
