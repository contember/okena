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
}
