use serde::{Deserialize, Serialize};

/// Configuration for a single remote server connection.
/// Persisted in settings.json as part of `remote_connections`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteConnectionConfig {
    /// Stable UUID, unique across sessions
    pub id: String,
    /// User-friendly label (e.g. "Work Laptop")
    pub name: String,
    /// Hostname or IP address of the remote server
    pub host: String,
    /// Server port (typically 19100-19200)
    pub port: u16,
    /// Persisted bearer token (24h TTL on server side)
    #[serde(default)]
    pub saved_token: Option<String>,
    /// Unix timestamp when the token was obtained (for refresh scheduling)
    #[serde(default)]
    pub token_obtained_at: Option<i64>,
    /// Connect over TLS (https/wss). Default false for backward compatibility
    /// with existing plain-http connections.
    #[serde(default)]
    pub tls: bool,
    /// SHA-256 fingerprint (lowercase hex) of the server's TLS certificate,
    /// pinned on first connect (TOFU). When set, the client refuses any cert
    /// whose fingerprint differs — defeating an active MITM. `None` until the
    /// first successful TLS handshake captures it.
    #[serde(default)]
    pub pinned_cert_sha256: Option<String>,
}

impl RemoteConnectionConfig {
    /// `http://host:port` or `https://host:port` depending on `tls`.
    pub fn base_url(&self) -> String {
        let scheme = if self.tls { "https" } else { "http" };
        format!("{}://{}:{}", scheme, self.host, self.port)
    }

    /// `ws://host:port/v1/stream` or `wss://…` depending on `tls`.
    pub fn ws_url(&self) -> String {
        let scheme = if self.tls { "wss" } else { "ws" };
        format!("{}://{}:{}/v1/stream", scheme, self.host, self.port)
    }
}
