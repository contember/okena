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

    /// Returns the port if the server is active.
    pub fn port(&self) -> Option<u16> {
        self.inner.lock().port
    }

    /// Returns the auth store if the server is active.
    pub fn auth_store(&self) -> Option<Arc<AuthStore>> {
        self.inner.lock().auth_store.clone()
    }
}

/// GPUI global wrapper for RemoteInfo.
#[derive(Clone)]
pub struct GlobalRemoteInfo(pub RemoteInfo);

impl gpui::Global for GlobalRemoteInfo {}

