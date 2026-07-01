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
                    req.extensions_mut().insert(crate::routes::PeerInfo::Tcp(peer));
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
                    req.extensions_mut().insert(crate::routes::PeerInfo::Tcp(peer));
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

#[cfg(unix)]
pub fn bind_unix_socket(path: &std::path::Path) -> std::io::Result<tokio::net::UnixListener> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }

    tokio::net::UnixListener::bind(path)
}

#[cfg(unix)]
pub async fn serve_unix_listener(
    path: std::path::PathBuf,
    listener: tokio::net::UnixListener,
    app: Router,
    shutdown: impl std::future::Future<Output = ()>,
) -> std::io::Result<()> {
    log::info!("Local daemon socket listening on {}", path.display());
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
