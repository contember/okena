pub mod auth;
pub mod bridge;
pub mod pty_broadcaster;
pub mod routes;
pub mod server;
pub mod types;

use crate::remote::auth::AuthStore;
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
}

impl RemoteInfo {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(RemoteInfoInner {
                port: None,
                auth_store: None,
            })),
        }
    }

    pub fn set_active(&self, port: u16, auth_store: Arc<AuthStore>) {
        let mut inner = self.inner.lock();
        inner.port = Some(port);
        inner.auth_store = Some(auth_store);
    }

    pub fn set_inactive(&self) {
        let mut inner = self.inner.lock();
        inner.port = None;
        inner.auth_store = None;
    }

    /// Returns (port, pairing_code) if server is active.
    /// The pairing code is always fresh (regenerated if expired).
    pub fn status(&self) -> Option<(u16, String)> {
        let inner = self.inner.lock();
        match (inner.port, inner.auth_store.as_ref()) {
            (Some(port), Some(auth_store)) => {
                let code = auth_store.get_or_create_code();
                Some((port, code))
            }
            _ => None,
        }
    }
}

/// GPUI global wrapper for RemoteInfo.
#[derive(Clone)]
pub struct GlobalRemoteInfo(pub RemoteInfo);

impl gpui::Global for GlobalRemoteInfo {}

