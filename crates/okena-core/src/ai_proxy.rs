//! Shared contract of the per-profile AI proxy (`okena-ai`).
//!
//! The proxy is a separate process so `claude` sessions kept alive by dtach/tmux
//! keep a working `ANTHROPIC_BASE_URL` while the workspace daemon restarts. Its
//! endpoint (port + secret path token) is persisted in the profile, so the URL a
//! shell was started with stays valid across proxy restarts too.

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Directory holding the proxy's endpoint, accounts, lock and log.
pub fn proxy_dir(profile_root: &Path) -> PathBuf {
    profile_root.join("ai-proxy")
}

fn endpoint_path(profile_root: &Path) -> PathBuf {
    proxy_dir(profile_root).join("endpoint.json")
}

/// Loopback address of the proxy. The token is the first path segment of every
/// inference request, so only processes that were handed the URL (Okena shells)
/// can send requests through the proxy. Managing it needs the separate
/// [management key](ensure_management_key).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyEndpoint {
    pub port: u16,
    pub token: String,
}

impl ProxyEndpoint {
    /// Value for `ANTHROPIC_BASE_URL`; the SDK appends `/v1/messages` to it.
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/{}", self.port, self.token)
    }
}

pub fn load_endpoint(profile_root: &Path) -> anyhow::Result<Option<ProxyEndpoint>> {
    match std::fs::read(endpoint_path(profile_root)) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes).context("invalid ai-proxy endpoint.json")?,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn save_endpoint(profile_root: &Path, endpoint: &ProxyEndpoint) -> anyhow::Result<()> {
    write_private(&endpoint_path(profile_root), &serde_json::to_vec(endpoint)?)
}

/// Load the endpoint, creating it (free loopback port + random token) on first use.
pub fn ensure_endpoint(profile_root: &Path) -> anyhow::Result<ProxyEndpoint> {
    if let Some(endpoint) = load_endpoint(profile_root)? {
        return Ok(endpoint);
    }
    let endpoint = ProxyEndpoint {
        port: free_loopback_port()?,
        token: random_token()?,
    };
    save_endpoint(profile_root, &endpoint)?;
    Ok(endpoint)
}

fn management_key_path(profile_root: &Path) -> PathBuf {
    proxy_dir(profile_root).join("management.key")
}

/// Load the secret that authorizes the proxy's management routes, creating it on
/// first use. Unlike the endpoint token it never enters a shell's environment.
pub fn ensure_management_key(profile_root: &Path) -> anyhow::Result<String> {
    let path = management_key_path(profile_root);
    if !path.exists() {
        // The daemon and a starting proxy may race to create it; the first one wins.
        let created =
            private_temp_file(&path, random_token()?.as_bytes())?.persist_noclobber(&path);
        if let Err(error) = created
            && error.error.kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(error.error.into());
        }
    }
    let key = std::fs::read_to_string(&path)?;
    anyhow::ensure!(!key.is_empty(), "{} is empty", path.display());
    Ok(key)
}

pub fn free_loopback_port() -> anyhow::Result<u16> {
    Ok(std::net::TcpListener::bind(("127.0.0.1", 0))?
        .local_addr()?
        .port())
}

pub fn random_token() -> anyhow::Result<String> {
    let mut bytes = [0u8; 24];
    getrandom::getrandom(&mut bytes).map_err(|error| anyhow::anyhow!("getrandom: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Atomically replace `path` with `bytes`, readable only by the owner.
pub fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    private_temp_file(path, bytes)?
        .persist(path)
        .map_err(|error| error.error)?;
    Ok(())
}

/// Owner-only temporary file next to `path`, holding `bytes` and ready to persist.
fn private_temp_file(path: &Path, bytes: &[u8]) -> anyhow::Result<tempfile::NamedTempFile> {
    use std::io::Write;
    let dir = path.parent().context("path has no parent directory")?;
    std::fs::create_dir_all(dir)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    Ok(file)
}

/// Management commands, sent by clients through the daemon (`ActionRequest::AiProxy`)
/// and forwarded verbatim to the proxy. Every command answers with [`AiProxyStatus`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum AiProxyCommand {
    Status,
    /// Begin an OAuth login; the authorization URL appears in `AiProxyStatus::logins`.
    StartLogin {
        label: String,
    },
    /// Finish a login with the code shown by the provider after authorization.
    FinishLogin {
        login_id: String,
        code: String,
    },
    CancelLogin {
        login_id: String,
    },
    RemoveAccount {
        account_id: String,
    },
    SetAccountEnabled {
        account_id: String,
        enabled: bool,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AiProxyStatus {
    pub version: String,
    /// Requests currently being relayed.
    pub inflight: usize,
    pub accounts: Vec<AiAccount>,
    pub logins: Vec<AiPendingLogin>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiAccount {
    pub id: String,
    pub label: String,
    pub email: Option<String>,
    /// Subscription type reported by the provider (e.g. "max").
    pub plan: Option<String>,
    pub enabled: bool,
    /// The refresh token was rejected; the account needs a new login.
    pub needs_login: bool,
    /// The account was rate limited and is skipped until this time.
    pub cooldown_until_ms: Option<i64>,
    pub five_hour: Option<AiUsage>,
    pub seven_day: Option<AiUsage>,
    pub requests: u64,
    pub last_used_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiUsage {
    /// Percent of the window used, 0–100.
    pub utilization: f64,
    pub resets_at_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiPendingLogin {
    pub id: String,
    pub label: String,
    pub authorize_url: String,
    pub error: Option<String>,
}

/// Daemon reply to `ActionRequest::AiProxy`. (`failure`, not `error`: a
/// top-level `error` key marks the whole action as failed on the wire.)
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AiProxyState {
    /// Proxy status, when the proxy is reachable.
    pub status: Option<AiProxyStatus>,
    /// Why the command failed or the proxy is unreachable.
    pub failure: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_is_created_once_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_endpoint(dir.path()).unwrap().is_none());
        let created = ensure_endpoint(dir.path()).unwrap();
        assert_eq!(created.token.len(), 48);
        assert_eq!(ensure_endpoint(dir.path()).unwrap(), created);
        assert_eq!(
            created.base_url(),
            format!("http://127.0.0.1:{}/{}", created.port, created.token)
        );
    }

    #[cfg(unix)]
    #[test]
    fn endpoint_and_management_key_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        ensure_endpoint(dir.path()).unwrap();
        ensure_management_key(dir.path()).unwrap();
        for path in [endpoint_path(dir.path()), management_key_path(dir.path())] {
            let mode = std::fs::metadata(path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn management_key_is_created_once_and_differs_from_the_token() {
        let dir = tempfile::tempdir().unwrap();
        let key = ensure_management_key(dir.path()).unwrap();
        assert_eq!(key.len(), 48);
        assert_eq!(ensure_management_key(dir.path()).unwrap(), key);
        assert_ne!(ensure_endpoint(dir.path()).unwrap().token, key);
    }

    #[test]
    fn empty_management_key_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        write_private(&management_key_path(dir.path()), b"").unwrap();
        assert!(ensure_management_key(dir.path()).is_err());
    }

    #[test]
    fn commands_use_a_command_tag() {
        let json = serde_json::to_value(AiProxyCommand::SetAccountEnabled {
            account_id: "a".into(),
            enabled: false,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"command": "set_account_enabled", "account_id": "a", "enabled": false})
        );
    }
}
