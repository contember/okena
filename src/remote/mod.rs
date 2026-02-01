pub mod auth;
pub mod bridge;
pub mod pty_broadcaster;
pub mod routes;
pub mod server;
pub mod types;

use parking_lot::Mutex;
use std::sync::Arc;

/// Shared remote server status, readable from any thread/view.
#[derive(Clone)]
pub struct RemoteInfo {
    inner: Arc<Mutex<RemoteInfoInner>>,
}

struct RemoteInfoInner {
    pub port: Option<u16>,
    pub pairing_code: Option<String>,
}

impl RemoteInfo {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(RemoteInfoInner {
                port: None,
                pairing_code: None,
            })),
        }
    }

    pub fn set_active(&self, port: u16, code: String) {
        let mut inner = self.inner.lock();
        inner.port = Some(port);
        inner.pairing_code = Some(code);
    }

    pub fn set_inactive(&self) {
        let mut inner = self.inner.lock();
        inner.port = None;
        inner.pairing_code = None;
    }

    #[allow(dead_code)]
    pub fn update_code(&self, code: String) {
        self.inner.lock().pairing_code = Some(code);
    }

    /// Returns (port, pairing_code) if server is active.
    pub fn status(&self) -> Option<(u16, String)> {
        let inner = self.inner.lock();
        match (inner.port, inner.pairing_code.as_ref()) {
            (Some(port), Some(code)) => Some((port, code.clone())),
            _ => None,
        }
    }
}

/// GPUI global wrapper for RemoteInfo.
#[derive(Clone)]
pub struct GlobalRemoteInfo(pub RemoteInfo);

impl gpui::Global for GlobalRemoteInfo {}

