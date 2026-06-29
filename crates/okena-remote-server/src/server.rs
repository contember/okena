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
        bind_addrs: Vec<IpAddr>,
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

        let bind_addrs = normalize_bind_addrs(bind_addrs);
        let listeners = runtime.block_on(bind_listeners(&bind_addrs))?;
        let port = listeners
            .first()
            .expect("bind_listeners returns at least one listener")
            .1
            .local_addr()?
            .port();

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
        let tls_server_config = match tls_material.as_ref() {
            Some(material) => Some(crate::tls::server_config(material)?),
            None => None,
        };

        let scheme = if tls_enabled {
            "http+https dual-stack"
        } else {
            "http/ws"
        };
        log::info!(
            "Remote control server listening on {}:{} ({})",
            bind_addrs
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(","),
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
        if bind_addrs.iter().any(|addr| !addr.is_loopback()) && !tls_enabled {
            log::warn!(
                "Remote control server is bound to a NON-LOOPBACK address ({}:{}) WITHOUT TLS.\n\
                 The connection is UNENCRYPTED (http/ws): the pairing token and all terminal\n\
                 I/O (including passwords, SSH keys, and any typed secrets) are sent in cleartext\n\
                 and visible to anyone on the network. Enable TLS in settings, only use this on a\n\
                 trusted network, or tunnel it over SSH/WireGuard.",
                bind_addrs
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
                port
            );
        }

        let mut local_endpoint = crate::local::default_local_endpoint();
        #[cfg(unix)]
        let local_listener = match &local_endpoint {
            Some(LocalEndpoint::UnixSocket { path }) => {
                let path_buf = std::path::PathBuf::from(path);
                // `UnixListener::bind` registers with the tokio reactor, so it
                // must run inside the runtime context. The TCP binds get this for
                // free via `block_on` above; here we're on the plain caller thread
                // (daemon main / GPUI thread), so enter the runtime explicitly.
                let bound = {
                    let _guard = runtime.enter();
                    crate::serve::bind_unix_socket(&path_buf)
                };
                match bound {
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
        if let Err(e) = write_remote_json(port, tls_enabled, &bind_addrs, local_endpoint.as_ref()) {
            log::warn!("Failed to write remote.json: {}", e);
        }

        let start_time = std::time::Instant::now();
        okena_ext_updater::installer::cleanup_old_binary();
        let update_info = okena_ext_updater::UpdateInfo::new(app_version.to_string());

        // Spawn the server task
        let mut shutdown_rx_clone = shutdown_rx.clone();
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

            let mut tcp_servers = Vec::new();
            for (addr, listener) in listeners {
                let app = app.clone();
                let shutdown_rx = shutdown_rx_clone.clone();
                let tls_config = tls_server_config.clone();
                tcp_servers.push(tokio::spawn(async move {
                    let result = if let Some(tls_config) = tls_config {
                        // TLS enabled → dual-stack: accept BOTH http and TLS on this one
                        // port so already-paired plain-http clients keep working while
                        // new/auto clients negotiate TLS.
                        crate::serve::serve_dual_stack(
                            listener,
                            app,
                            tls_config,
                            shutdown_signal(shutdown_rx),
                        )
                        .await
                    } else {
                        // TLS disabled → plain http only.
                        crate::serve::serve_plain(listener, app, shutdown_signal(shutdown_rx)).await
                    };
                    match result {
                        Ok(()) => log::info!("Remote control server on {addr} shut down"),
                        Err(e) => {
                            log::error!("Remote control server on {addr} exited with error: {e}")
                        }
                    }
                }));
            }
            let _ = shutdown_rx_clone.changed().await;
            for handle in tcp_servers {
                let _ = handle.await;
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

fn normalize_bind_addrs(addrs: Vec<IpAddr>) -> Vec<IpAddr> {
    let mut out = Vec::new();
    for addr in addrs {
        if out.contains(&addr) {
            continue;
        }
        out.push(addr);
    }
    if out.is_empty() {
        out.push(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    }
    out
}

async fn bind_listeners(
    bind_addrs: &[IpAddr],
) -> anyhow::Result<Vec<(IpAddr, tokio::net::TcpListener)>> {
    let primary = *bind_addrs
        .first()
        .unwrap_or(&IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));

    for port in 19100..=19200 {
        if let Ok(listeners) = bind_all_on_port(bind_addrs, port).await {
            return Ok(listeners);
        }
    }

    let first = tokio::net::TcpListener::bind(SocketAddr::new(primary, 0)).await?;
    let port = first.local_addr()?.port();
    let mut listeners = vec![(primary, first)];
    for addr in bind_addrs.iter().copied().skip(1) {
        let listener = tokio::net::TcpListener::bind(SocketAddr::new(addr, port)).await?;
        listeners.push((addr, listener));
    }
    Ok(listeners)
}

async fn bind_all_on_port(
    bind_addrs: &[IpAddr],
    port: u16,
) -> std::io::Result<Vec<(IpAddr, tokio::net::TcpListener)>> {
    let mut listeners = Vec::with_capacity(bind_addrs.len());
    for addr in bind_addrs {
        let listener = tokio::net::TcpListener::bind(SocketAddr::new(*addr, port)).await?;
        listeners.push((*addr, listener));
    }
    Ok(listeners)
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
    bind_addrs: &[IpAddr],
    local_endpoint: Option<&LocalEndpoint>,
) -> anyhow::Result<()> {
    let path = remote_json_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut content = serde_json::json!({
        "port": port,
        "local_host": local_tcp_host(bind_addrs),
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

fn local_tcp_host(bind_addrs: &[IpAddr]) -> &'static str {
    if bind_addrs.iter().any(|addr| match addr {
        IpAddr::V4(v4) => *v4 == std::net::Ipv4Addr::LOCALHOST || v4.is_unspecified(),
        IpAddr::V6(_) => false,
    }) {
        return crate::local::LOCAL_HOST;
    }

    if bind_addrs.iter().any(|addr| match addr {
        IpAddr::V4(_) => false,
        IpAddr::V6(v6) => *v6 == std::net::Ipv6Addr::LOCALHOST || v6.is_unspecified(),
    }) {
        return "::1";
    }

    crate::local::LOCAL_HOST
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_tcp_host_prefers_ipv4_loopback_when_available() {
        let addrs = [
            IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED),
            IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        ];
        assert_eq!(local_tcp_host(&addrs), crate::local::LOCAL_HOST);
    }

    #[test]
    fn local_tcp_host_uses_ipv6_loopback_for_ipv6_only_binds() {
        let addrs = [IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED)];
        assert_eq!(local_tcp_host(&addrs), "::1");
    }
}
