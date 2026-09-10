//! Dual-stack server: accepts BOTH plain http and TLS on the same port.
//!
//! The first byte of every accepted connection is peeked — a TLS ClientHello
//! starts with a handshake record (`0x16`), plain HTTP starts with an ASCII
//! method verb — so we can route each connection to either the rustls acceptor
//! or straight to the HTTP server without separate ports. This lets an
//! already-paired plain-http client keep working after the server enables TLS,
//! while new/auto clients negotiate TLS, so TLS can be on by default without a
//! flag-day migration.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use hyper::Request;
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tower_service::Service;

/// First byte of a TLS 1.x record layer handshake message.
const TLS_HANDSHAKE_BYTE: u8 = 0x16;

/// Serve `app` on `listener`, accepting both http and TLS, until `shutdown`
/// resolves. Each connection is handled on its own task.
pub async fn serve_dual_stack(
    listener: TcpListener,
    app: Router,
    tls: Arc<rustls::ServerConfig>,
    shutdown: impl std::future::Future<Output = ()>,
) -> std::io::Result<()> {
    let acceptor = TlsAcceptor::from(tls);
    tokio::pin!(shutdown);

    loop {
        let (stream, peer) = tokio::select! {
            res = listener.accept() => match res {
                Ok(v) => v,
                Err(e) => {
                    log::warn!("Remote server accept error: {e}");
                    continue;
                }
            },
            _ = &mut shutdown => return Ok(()),
        };

        let app = app.clone();
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            // Peek (don't consume) the first byte to detect TLS.
            let mut first = [0u8; 1];
            let is_tls = matches!(stream.peek(&mut first).await, Ok(n) if n > 0 && first[0] == TLS_HANDSHAKE_BYTE);

            // Per-connection hyper service: map the body, inject ConnectInfo
            // (the pairing route reads the peer IP for rate limiting), and call
            // the axum Router (always ready, infallible).
            let svc = hyper::service::service_fn(move |req: Request<Incoming>| {
                let mut app = app.clone();
                async move {
                    let mut req = req.map(Body::new);
                    req.extensions_mut().insert(ConnectInfo(peer));
                    req.extensions_mut()
                        .insert(crate::routes::PeerInfo::Tcp(peer));
                    app.call(req).await
                }
            });

            let builder = Builder::new(TokioExecutor::new());
            if is_tls {
                match acceptor.accept(stream).await {
                    Ok(tls_stream) => {
                        if let Err(e) = builder
                            .serve_connection_with_upgrades(TokioIo::new(tls_stream), svc)
                            .await
                        {
                            log::debug!("TLS connection from {peer} ended: {e}");
                        }
                    }
                    Err(e) => log::debug!("TLS handshake from {peer} failed: {e}"),
                }
            } else if let Err(e) = builder
                .serve_connection_with_upgrades(TokioIo::new(stream), svc)
                .await
            {
                log::debug!("HTTP connection from {peer} ended: {e}");
            }
        });
    }
}

/// Serve TLS exclusively. Plain HTTP is rejected during the rustls handshake.
pub async fn serve_tls(
    listener: TcpListener,
    app: Router,
    tls: Arc<rustls::ServerConfig>,
    shutdown: impl std::future::Future<Output = ()>,
) -> std::io::Result<()> {
    let acceptor = TlsAcceptor::from(tls);
    tokio::pin!(shutdown);

    loop {
        let (stream, peer) = tokio::select! {
            res = listener.accept() => match res {
                Ok(value) => value,
                Err(error) => {
                    log::warn!("Remote server accept error: {error}");
                    continue;
                }
            },
            _ = &mut shutdown => return Ok(()),
        };

        let app = app.clone();
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            let tls_stream = match acceptor.accept(stream).await {
                Ok(stream) => stream,
                Err(error) => {
                    log::debug!("Rejected non-TLS or invalid handshake from {peer}: {error}");
                    return;
                }
            };
            let svc = hyper::service::service_fn(move |req: Request<Incoming>| {
                let mut app = app.clone();
                async move {
                    let mut req = req.map(Body::new);
                    req.extensions_mut().insert(ConnectInfo(peer));
                    req.extensions_mut()
                        .insert(crate::routes::PeerInfo::Tcp(peer));
                    app.call(req).await
                }
            });
            if let Err(error) = Builder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(TokioIo::new(tls_stream), svc)
                .await
            {
                log::debug!("TLS connection from {peer} ended: {error}");
            }
        });
    }
}

pub async fn serve_plain(
    listener: TcpListener,
    app: Router,
    shutdown: impl std::future::Future<Output = ()>,
) -> std::io::Result<()> {
    tokio::pin!(shutdown);

    loop {
        let (stream, peer) = tokio::select! {
            res = listener.accept() => match res {
                Ok(v) => v,
                Err(e) => {
                    log::warn!("Remote server accept error: {e}");
                    continue;
                }
            },
            _ = &mut shutdown => return Ok(()),
        };

        let app = app.clone();
        tokio::spawn(async move {
            let svc = hyper::service::service_fn(move |req: Request<Incoming>| {
                let mut app = app.clone();
                async move {
                    let mut req = req.map(Body::new);
                    req.extensions_mut().insert(ConnectInfo(peer));
                    req.extensions_mut()
                        .insert(crate::routes::PeerInfo::Tcp(peer));
                    app.call(req).await
                }
            });

            if let Err(e) = Builder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(TokioIo::new(stream), svc)
                .await
            {
                log::debug!("HTTP connection from {peer} ended: {e}");
            }
        });
    }
}

/// Bind the local daemon socket inside a directory that is ours and private.
/// A directory another user owns or can write to would let them replace the
/// socket and impersonate the daemon, so binding fails rather than proceeds.
#[cfg(unix)]
pub fn bind_unix_socket(path: &std::path::Path) -> std::io::Result<tokio::net::UnixListener> {
    use std::os::unix::fs::PermissionsExt as _;

    if let Some(parent) = path.parent() {
        // `create_dir_all` and `set_permissions` both follow symlinks, so a
        // planted link would redirect the chmod and the bind together.
        if std::fs::symlink_metadata(parent).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("{} is a symlink", parent.display()),
            ));
        }
        std::fs::create_dir_all(parent)?;
        // chmod is owner-only, so this both proves the directory is ours and
        // repairs one an earlier umask left readable. That proof holds only for
        // a non-root daemon (root has CAP_FOWNER); the client's owner check is
        // what carries it otherwise.
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }

    let listener = tokio::net::UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Same-user check for a local socket peer. Credentials we cannot read are
/// refused rather than trusted.
#[cfg(unix)]
fn peer_is_socket_owner(stream: &tokio::net::UnixStream, owner_uid: u32) -> bool {
    match stream.peer_cred() {
        Ok(cred) => cred.uid() == owner_uid,
        Err(error) => {
            log::warn!("Refusing local socket peer with unreadable credentials: {error}");
            false
        }
    }
}

#[cfg(unix)]
pub async fn serve_unix_listener(
    path: std::path::PathBuf,
    listener: tokio::net::UnixListener,
    app: Router,
    shutdown: impl std::future::Future<Output = ()>,
) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt as _;

    log::info!("Local daemon socket listening on {}", path.display());
    // We created the socket, so its owner is this daemon's user — the only peer
    // whose requests may be treated as same-user local traffic.
    let owner_uid = std::fs::metadata(&path)?.uid();
    tokio::pin!(shutdown);

    loop {
        let (stream, _peer) = tokio::select! {
            res = listener.accept() => match res {
                Ok(v) => v,
                Err(e) => {
                    log::warn!("Local socket accept error: {e}");
                    continue;
                }
            },
            _ = &mut shutdown => {
                let _ = tokio::fs::remove_file(&path).await;
                return Ok(());
            },
        };

        if !peer_is_socket_owner(&stream, owner_uid) {
            log::warn!("Refusing local socket connection from another user");
            continue;
        }

        let app = app.clone();
        tokio::spawn(async move {
            let svc = hyper::service::service_fn(move |req: Request<Incoming>| {
                let mut app = app.clone();
                async move {
                    let mut req = req.map(Body::new);
                    req.extensions_mut().insert(crate::routes::PeerInfo::Local);
                    app.call(req).await
                }
            });

            if let Err(e) = Builder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(TokioIo::new(stream), svc)
                .await
            {
                log::debug!("Local socket connection ended: {e}");
            }
        });
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    #[tokio::test]
    async fn binding_makes_the_socket_and_its_directory_private() {
        let dir = tempfile::tempdir().expect("temp dir");
        let socket = dir.path().join("nested").join("daemon.sock");
        std::fs::create_dir_all(socket.parent().expect("parent")).expect("create parent");
        std::fs::set_permissions(
            socket.parent().expect("parent"),
            std::fs::Permissions::from_mode(0o755),
        )
        .expect("loosen parent");

        let _listener = bind_unix_socket(&socket).expect("bind");

        let parent_mode = std::fs::metadata(socket.parent().expect("parent"))
            .expect("stat parent")
            .permissions()
            .mode();
        assert_eq!(parent_mode & 0o777, 0o700);
        let socket_mode = std::fs::metadata(&socket)
            .expect("stat socket")
            .permissions()
            .mode();
        assert_eq!(socket_mode & 0o777, 0o600);
    }

    #[tokio::test]
    async fn a_symlinked_socket_directory_is_refused() {
        let dir = tempfile::tempdir().expect("temp dir");
        let real = dir.path().join("real");
        std::fs::create_dir(&real).expect("create the planted target");
        std::os::unix::fs::symlink(&real, dir.path().join("link")).expect("plant the symlink");

        let error = bind_unix_socket(&dir.path().join("link").join("daemon.sock"))
            .expect_err("a symlinked socket directory must be refused");

        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[tokio::test]
    async fn the_socket_owner_is_accepted_as_a_local_peer() {
        let dir = tempfile::tempdir().expect("temp dir");
        let socket = dir.path().join("daemon.sock");
        let listener = bind_unix_socket(&socket).expect("bind");
        let owner_uid = {
            use std::os::unix::fs::MetadataExt as _;
            std::fs::metadata(&socket).expect("stat socket").uid()
        };

        let client = tokio::net::UnixStream::connect(&socket)
            .await
            .expect("connect");
        let (server, _) = listener.accept().await.expect("accept");

        assert!(peer_is_socket_owner(&server, owner_uid));
        assert!(
            !peer_is_socket_owner(&server, owner_uid + 1),
            "a peer running as another user must be refused"
        );
        drop(client);
    }
}
