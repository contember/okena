pub mod auth;
pub mod bridge;
pub mod pty_broadcaster;
pub mod routes;
pub mod serve;
pub mod server;
pub mod tls;
pub mod types;

use crate::auth::AuthStore;
use parking_lot::Mutex;
use std::sync::Arc;

/// Shared remote server status, readable from any thread/view.
#[derive(Clone)]
pub struct RemoteInfo {
    inner: Arc<Mutex<RemoteInfoInner>>,
}

struct RemoteInfoInner {
    port: Option<u16>,
    auth_store: Option<Arc<AuthStore>>,
    /// SHA-256 fingerprint (lowercase hex) of the server's TLS cert when the
    /// server is running with TLS enabled. `None` when TLS is off (plain http).
    cert_fingerprint: Option<String>,
}

impl Default for RemoteInfo {
    fn default() -> Self {
        Self::new()
    }
}

impl RemoteInfo {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(RemoteInfoInner {
                port: None,
                auth_store: None,
                cert_fingerprint: None,
            })),
        }
    }

    pub fn set_active(
        &self,
        port: u16,
        auth_store: Arc<AuthStore>,
        cert_fingerprint: Option<String>,
    ) {
        let mut inner = self.inner.lock();
        inner.port = Some(port);
        inner.auth_store = Some(auth_store);
        inner.cert_fingerprint = cert_fingerprint;
    }

    pub fn set_inactive(&self) {
        let mut inner = self.inner.lock();
        inner.port = None;
        inner.auth_store = None;
        inner.cert_fingerprint = None;
    }

    /// Returns the port if the server is active.
    pub fn port(&self) -> Option<u16> {
        self.inner.lock().port
    }

    /// Returns the auth store if the server is active.
    pub fn auth_store(&self) -> Option<Arc<AuthStore>> {
        self.inner.lock().auth_store.clone()
    }

    /// Returns the TLS cert fingerprint (lowercase hex SHA-256) if the server is
    /// running with TLS enabled. The user reads this to verify it against the
    /// fingerprint the client pinned during pairing.
    pub fn cert_fingerprint(&self) -> Option<String> {
        self.inner.lock().cert_fingerprint.clone()
    }
}

/// GPUI global wrapper for RemoteInfo.
#[derive(Clone)]
pub struct GlobalRemoteInfo(pub RemoteInfo);

impl gpui::Global for GlobalRemoteInfo {}

